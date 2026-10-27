//! Settlement methods (spec/decisions/settlement-methods.md,
//! spec/areas/network.md#settlement-methods): the further methods of a
//! peering, which the node that made it asks for, one at a time, as it
//! asked for the peering; and methods dropped, which the node that drops
//! one tells the peer of until it hears back.

use alloc::vec::Vec;

use dassie_identity::NodeId;
use dassie_oer::{Decode, Encode};

use super::inbound::{Accepted, ok};
use super::peering::MethodAttempt;
use super::{MAX_METHODS, PeerNode, Problem, Purpose};
use crate::hooks::PeeringHooks;
use crate::io::{Event, Reply};
use crate::table::{Entry, Method, PeerState, backoff};
use crate::wire::{
    Content, MethodResponse, RefusalReason, SchemeId, TerminationReason, decode_octets,
};

impl PeerNode {
    /// Asks a peer for a further method, if a peering we made lacks one:
    /// a scheme a peering with it settles on ([`Self::methods_with`]) that
    /// it has no method on, and the node wants one on. One request at a
    /// time; a peer that refused, or that the node does not want a method
    /// with, is asked again only after a backoff, and then for the next
    /// scheme it lacks, so one refused holds up no other. Looked at
    /// whenever the node does what is due, which a node with peers does at
    /// least once a heartbeat, so nothing waits on a timer of its own.
    pub(super) fn seek_methods(&mut self, hooks: &mut dyn PeeringHooks) {
        if self.peering.method_attempt.is_some() {
            return;
        }
        let now = self.now_ms;
        let mut wanted = None;
        for (id, entry) in &self.table {
            let (Some(peering), Some(link)) = (entry.active_peering(), entry.link.as_ref()) else {
                continue;
            };
            // Only the node that made the peering asks, so the two never
            // ask for one at once.
            if peering.inbound
                || peering.depeering.is_some()
                || peering.methods.len() >= MAX_METHODS
                || entry.method_at_ms > now
                || self.bans.contains(id)
            {
                continue;
            }
            // Not while we are telling the peer we dropped it: the drop
            // could arrive after the request.
            let missing: Vec<SchemeId> = self
                .methods_with(hooks, link)
                .into_iter()
                .filter(|scheme| {
                    !peering.has(*scheme) && !self.dropping.contains_key(&(*id, *scheme))
                })
                .collect();
            // The one after the scheme asked for last, round the list.
            let after = entry
                .method_tried
                .and_then(|tried| missing.iter().position(|scheme| *scheme == tried))
                .map_or(0, |at| at + 1);
            if let Some(scheme) = missing.iter().cycle().nth(after) {
                wanted = Some((*id, *scheme));
                break;
            }
        }
        let Some((node, scheme)) = wanted else {
            return;
        };
        if let Some(entry) = self.table.get_mut(&node) {
            entry.method_tried = Some(scheme);
        }
        let downlink = self.is_downlink(&node);
        if !hooks.speaks(scheme) || !hooks.wants_method(node, downlink, scheme) {
            self.method_failed(node);
            return;
        }
        self.peering.method_attempt = Some(MethodAttempt {
            node,
            scheme,
            info: None,
        });
        let content = Content::PeeringInfoRequest { scheme };
        let timeout = self.config.request_timeout_ms;
        if self
            .send(node, &content, Purpose::MethodInfo, timeout)
            .is_none()
        {
            self.method_failed(node);
        }
    }

    /// The scheme of our request for a method from `node`, if one is under
    /// way and the peering still takes one.
    fn method_asking(&self, node: &NodeId) -> Option<SchemeId> {
        let attempt = self.peering.method_attempt.as_ref()?;
        let peering = self.table.get(node).and_then(Entry::active_peering)?;
        (attempt.node == *node && !peering.has(attempt.scheme)).then_some(attempt.scheme)
    }

    /// The peer's peering info on the scheme we ask a method on: the
    /// scheme builds its request from it.
    pub(super) fn on_method_info(
        &mut self,
        hooks: &mut dyn PeeringHooks,
        node: NodeId,
        result: Result<Vec<u8>, Problem>,
    ) {
        let Some(scheme) = self.method_asking(&node) else {
            return self.method_failed(node);
        };
        let Ok(info) = result.and_then(|body| {
            decode_octets(&body)
                .map(<[u8]>::to_vec)
                .map_err(|_| Problem::Invalid)
        }) else {
            return self.method_failed(node);
        };
        let mut data = Vec::new();
        if hooks
            .create_request(node, scheme, &info, &mut data)
            .is_err()
        {
            return self.method_failed(node);
        }
        if let Some(attempt) = self.peering.method_attempt.as_mut() {
            attempt.info = Some(info);
        }
        let content = Content::MethodAdd {
            scheme,
            data: &data,
        };
        let timeout = self.config.request_timeout_ms;
        if self
            .send(node, &content, Purpose::MethodAdd, timeout)
            .is_none()
        {
            self.method_failed(node);
        }
    }

    /// The peer's answer to our request for a method. Once it accepted,
    /// the scheme finalizes the method and opens its accounts, as for a
    /// peering's first; a method the peer took but we cannot, the peer is
    /// told we dropped, so the two keep the same methods.
    pub(super) fn on_method_response(
        &mut self,
        hooks: &mut dyn PeeringHooks,
        node: NodeId,
        result: Result<Vec<u8>, Problem>,
    ) {
        let scheme = self.method_asking(&node);
        let info = match self.peering.method_attempt.take() {
            Some(attempt) if attempt.node == node => attempt.info.unwrap_or_default(),
            other => {
                self.peering.method_attempt = other;
                return;
            }
        };
        let Some(scheme) = scheme else {
            return self.method_failed(node);
        };
        let accepted = result.is_ok_and(|body| {
            matches!(
                MethodResponse::decode_exact(&body),
                Ok(MethodResponse::Accepted)
            )
        });
        if !accepted {
            return self.method_failed(node);
        }
        // A Prepare of the peer's on the method reached us before its
        // acceptance did, and we told the peer we have no such method: it
        // drops it, so we add none, and ask again once it heard.
        if self.dropping.contains_key(&(node, scheme)) {
            return self.method_failed(node);
        }
        let room = self
            .peering_of(&node)
            .is_some_and(|peering| peering.methods.len() < MAX_METHODS);
        let ended_for_room = self.ended_for_room(&node, hooks);
        let agreed = if room && hooks.speaks(scheme) {
            hooks
                .finalize(self.now_ms, node, scheme, &info, ended_for_room)
                .ok()
        } else {
            None
        };
        match agreed {
            Some(agreed) => {
                let method = Method::new(scheme, agreed.state, agreed.credit_out);
                self.add_method(node, method, false);
            }
            None => {
                self.queue_drop(node, scheme);
                self.method_failed(node);
            }
        }
    }

    /// Ends our request for a method from `node`, if it is under way, and
    /// backs off from asking it again.
    fn method_failed(&mut self, node: NodeId) {
        if self
            .peering
            .method_attempt
            .as_ref()
            .is_some_and(|attempt| attempt.node == node)
        {
            self.peering.method_attempt = None;
        }
        let (now, min, max) = (
            self.now_ms,
            self.config.min_backoff_ms,
            self.config.max_backoff_ms,
        );
        if let Some(entry) = self.table.get_mut(&node) {
            entry.method_failures = entry.method_failures.saturating_add(1);
            entry.method_at_ms = now.saturating_add(backoff(min, max, entry.method_failures));
        }
    }

    /// Adds `method` to our peering with `node`, which has room for it and
    /// none on its scheme.
    fn add_method(&mut self, node: NodeId, method: Method, inbound: bool) {
        let scheme = method.scheme;
        let Some(entry) = self.table.get_mut(&node) else {
            return;
        };
        let PeerState::Peered(peering) = &mut entry.peer else {
            return;
        };
        assert!(
            !peering.has(scheme) && peering.methods.len() < MAX_METHODS,
            "a method the peering has room for"
        );
        peering.methods.push(method);
        entry.method_failures = 0;
        self.dirty.insert(node);
        self.stats.methods_added += 1;
        self.event(Event::MethodAdded {
            peer: node,
            scheme,
            inbound,
        });
    }

    /// A peer asks to add a method to our peering (`methodAdd`): answered
    /// as a peering request is, with the method recorded before the
    /// acceptance goes out. A repeat, after an answer that got lost, is
    /// accepted again, as it is.
    pub(super) fn method_add_request(
        &mut self,
        hooks: &mut dyn PeeringHooks,
        accepted: &Accepted,
        scheme: SchemeId,
        data: &[u8],
    ) -> Reply {
        let sender = accepted.sender;
        let refused = |reason| ok(&MethodResponse::Refused(reason).to_vec());
        let Some(entry) = self.table.get(&sender) else {
            self.notice_one_sided(sender);
            return Reply::refuse(409, "not peered");
        };
        let peering = match &entry.peer {
            PeerState::Peered(peering) => peering,
            PeerState::Terminating(..) => return refused(RefusalReason::Terminating),
            PeerState::None | PeerState::Requesting { .. } => {
                self.notice_one_sided(sender);
                return Reply::refuse(409, "not peered");
            }
        };
        if peering.has(scheme) {
            return ok(&MethodResponse::Accepted.to_vec());
        }
        let reason = if peering.depeering.is_some() {
            Some(RefusalReason::Terminating)
        } else if self.bans.contains(&sender) {
            Some(RefusalReason::Banned)
        } else if peering.methods.len() >= MAX_METHODS {
            Some(RefusalReason::Capacity)
        } else if self.dropping.contains_key(&(sender, scheme)) {
            // Our drop of it is still on its way: asked again after it.
            Some(RefusalReason::Concurrent)
        } else if !hooks.speaks(scheme)
            || !entry
                .link
                .as_ref()
                .is_some_and(|link| self.methods_with(hooks, link).contains(&scheme))
        {
            Some(RefusalReason::UnsupportedScheme)
        } else if !hooks.wants_method(sender, self.is_downlink(&sender), scheme) {
            Some(RefusalReason::SchemeRefused)
        } else {
            None
        };
        if let Some(reason) = reason {
            return refused(reason);
        }
        let downlink = self.is_downlink(&sender);
        match hooks.accept(self.now_ms, sender, downlink, scheme, data) {
            Ok(agreed) => {
                let method = Method::new(scheme, agreed.state, agreed.credit_out);
                self.add_method(sender, method, true);
                ok(&MethodResponse::Accepted.to_vec())
            }
            Err(reason) => refused(reason),
        }
    }

    /// A peer dropped a method of our peering (`methodDrop`), after
    /// collecting on it: we drop it at once, and settle nothing more on it
    /// (spec/decisions/settlement-methods.md). A method we do not have is
    /// dropped already. A peering it leaves with none ends; one being ended
    /// keeps its last until it is over.
    pub(super) fn method_drop_request(&mut self, accepted: &Accepted, scheme: SchemeId) -> Reply {
        let sender = accepted.sender;
        let Some(peering) = self.peering_of(&sender) else {
            self.notice_one_sided(sender);
            return Reply::refuse(409, "not peered");
        };
        if !peering.has(scheme) {
            return ok(&[]);
        }
        if peering.methods.len() > 1 && !peering.collecting_on(scheme) {
            self.remove_method(sender, scheme, false);
        } else if self
            .table
            .get(&sender)
            .and_then(Entry::active_peering)
            .is_some()
        {
            self.begin_termination(sender, TerminationReason::Unspecified, true);
        }
        ok(&[])
    }

    /// Drops our method on `scheme` with `peer`, and tells the peer, which
    /// drops it at once (spec/decisions/settlement-methods.md). The node
    /// collects on the method first, as at a peering's end; what is left
    /// stays on its accounts. A peering left with none ends, told at once,
    /// as one on a scheme the node does not run is. Returns whether the
    /// peering had the method.
    pub fn drop_method(&mut self, now_ms: u64, peer: &NodeId, scheme: SchemeId) -> bool {
        self.advance(now_ms);
        self.drop_method_now(*peer, scheme)
    }

    /// The same, at the node's time.
    pub(crate) fn drop_method_now(&mut self, peer: NodeId, scheme: SchemeId) -> bool {
        let Some(peering) = self.table.get(&peer).and_then(Entry::active_peering) else {
            return false;
        };
        if !peering.has(scheme) {
            return false;
        }
        // A peering being ended collects on its first method: one that
        // loses it ends at once, as one that loses its last.
        if peering.methods.len() == 1 || peering.collecting_on(scheme) {
            let reason = peering
                .depeering
                .map_or(TerminationReason::Unspecified, |depeering| depeering.reason);
            return self.begin_termination(peer, reason, true);
        }
        self.remove_method(peer, scheme, true);
        self.queue_drop(peer, scheme);
        true
    }

    /// Takes the method on `scheme` out of our peering with `node`, which
    /// keeps another.
    fn remove_method(&mut self, node: NodeId, scheme: SchemeId, by_us: bool) {
        let Some(entry) = self.table.get_mut(&node) else {
            return;
        };
        let (PeerState::Peered(peering) | PeerState::Terminating(peering, _)) = &mut entry.peer
        else {
            return;
        };
        assert!(peering.methods.len() > 1, "a peering keeps a method");
        peering.remove(scheme);
        self.dirty.insert(node);
        self.stats.methods_dropped += 1;
        self.event(Event::MethodDropped {
            peer: node,
            scheme,
            by_us,
        });
    }

    /// Tells `peer` we have no method on `scheme` (any more), now and again
    /// until it hears. At most [`MAX_METHODS`] drops per peer are told at
    /// once: a peer that sends Prepares on schemes of its own making gets no
    /// more.
    pub(super) fn queue_drop(&mut self, peer: NodeId, scheme: SchemeId) {
        let telling = self
            .dropping
            .keys()
            .filter(|(node, _)| *node == peer)
            .count();
        if self.peering_of(&peer).is_none()
            || self.dropping.contains_key(&(peer, scheme))
            || telling >= MAX_METHODS
        {
            return;
        }
        self.dropping.insert((peer, scheme), self.now_ms);
        assert!(
            self.dropping.len()
                <= (self.config.max_core_peers() + self.config.max_downlinks) * MAX_METHODS,
            "more drops than methods"
        );
        self.tell_drops();
    }

    /// Sends the drops whose try is due. One whose peering is over is
    /// forgotten: the peer's ended with it.
    pub(super) fn tell_drops(&mut self) {
        let now = self.now_ms;
        let due: Vec<(NodeId, SchemeId)> = self
            .dropping
            .iter()
            .filter(|(_, at)| **at <= now)
            .map(|(key, _)| *key)
            .collect();
        for (peer, scheme) in due {
            if self.peering_of(&peer).is_none() {
                self.dropping.remove(&(peer, scheme));
                continue;
            }
            // Not again until this one is answered.
            self.dropping.insert((peer, scheme), u64::MAX);
            let content = Content::MethodDrop { scheme };
            let timeout = self.config.request_timeout_ms;
            if self
                .send(peer, &content, Purpose::MethodDrop(scheme), timeout)
                .is_none()
            {
                let at = now.saturating_add(self.config.min_backoff_ms);
                self.dropping.insert((peer, scheme), at);
            }
        }
    }

    /// The answer to our drop: a 200, or a 400 or 409, which say it will
    /// never be taken, ends it; anything else is tried again later.
    pub(super) fn on_method_drop(
        &mut self,
        peer: NodeId,
        scheme: SchemeId,
        result: &Result<Vec<u8>, Problem>,
    ) {
        if !self.dropping.contains_key(&(peer, scheme)) {
            return;
        }
        if matches!(result, Ok(_) | Err(Problem::Status(400 | 409))) {
            self.dropping.remove(&(peer, scheme));
        } else {
            let at = self.now_ms.saturating_add(self.config.min_backoff_ms);
            self.dropping.insert((peer, scheme), at);
        }
    }

    /// When a drop is next to be told.
    pub(super) fn next_drop(&self) -> Option<u64> {
        self.dropping
            .values()
            .copied()
            .filter(|at| *at != u64::MAX)
            .min()
    }
}
