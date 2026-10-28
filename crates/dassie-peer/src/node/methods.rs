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
use crate::table::{Entry, Method, PeerState, Peering, Winding, backoff};
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
            // Nor on a scheme that takes no newcomer of the peer's kind now
            // (spec/decisions/slots-by-liquidity.md): asked again once it
            // does.
            let downlink = entry.is_downlink();
            let missing: Vec<SchemeId> = self
                .methods_with(hooks, link)
                .into_iter()
                .filter(|scheme| {
                    !peering.has(*scheme)
                        && !self.dropping.contains_key(&(*id, *scheme))
                        && hooks.takes_newcomer(*scheme, downlink)
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
            dropped: false,
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
    /// way, the peer dropped no method on it since, and the peering still
    /// takes one.
    fn method_asking(&self, node: &NodeId) -> Option<SchemeId> {
        let attempt = self.peering.method_attempt.as_ref()?;
        let peering = self.table.get(node).and_then(Entry::active_peering)?;
        (attempt.node == *node && !attempt.dropped && !peering.has(attempt.scheme))
            .then_some(attempt.scheme)
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
        let agreed = if room && hooks.speaks(scheme) {
            hooks.finalize(self.now_ms, node, scheme, &info).ok()
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
        } else if peering.methods.len() >= MAX_METHODS
            || !hooks.takes_newcomer(scheme, self.is_downlink(&sender))
        {
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
    /// dropped already; if we are asking the peer for one on that scheme,
    /// the peer took it and dropped it before its acceptance reached us,
    /// so the acceptance, when it arrives, adds nothing. Our request still
    /// waits for it, or for its timeout, as the answer is matched to the
    /// request by its peer alone: one asked for after it could take it for
    /// its own. A peering it leaves with none ends; one being ended keeps
    /// its last until it is over.
    pub(super) fn method_drop_request(&mut self, accepted: &Accepted, scheme: SchemeId) -> Reply {
        let sender = accepted.sender;
        let Some(peering) = self.peering_of(&sender) else {
            self.notice_one_sided(sender);
            return Reply::refuse(409, "not peered");
        };
        if !peering.has(scheme) {
            if self.method_asking(&sender) == Some(scheme)
                && let Some(attempt) = self.peering.method_attempt.as_mut()
            {
                attempt.dropped = true;
            }
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

    /// Winds down our method on `scheme` with `peer`: collects on it, then
    /// drops it (spec/decisions/settlement-methods.md). Until then it takes
    /// no new packets from the peer and counts as unavailable; the peer
    /// sees it as before. Once our own settlements on it are done, or we
    /// waited [`depeer_wait_ms`](crate::Config::depeer_wait_ms), the peer is
    /// asked to settle on it once; once what it said is on its way is
    /// booked, or we waited as long again, the method is dropped and the
    /// peer told ([`drop_method`](Self::drop_method)). What is left stays on
    /// the method's accounts. A peering that winds down its last method
    /// ends once it is collected. Not while we end the peering, which
    /// collects as it does. Returns whether the method is being wound down.
    pub fn wind_down(&mut self, now_ms: u64, peer: &NodeId, scheme: SchemeId) -> bool {
        self.advance(now_ms);
        let now = self.now_ms;
        let Some(entry) = self.table.get_mut(peer) else {
            return false;
        };
        let PeerState::Peered(peering) = &mut entry.peer else {
            return false;
        };
        if peering.depeering.is_some() {
            return false;
        }
        let Some(method) = peering.method_mut(scheme) else {
            return false;
        };
        if method.winding.is_some() {
            return true;
        }
        method.winding = Some(Winding::new(now));
        self.dirty.insert(*peer);
        self.event(Event::MethodWinding {
            peer: *peer,
            scheme,
        });
        // Unavailable now: a peer whose methods all are is withdrawn at the
        // review, which is then due at once.
        true
    }

    /// Whether we wind down our method on `scheme` with `peer`
    /// ([`Method::winding_down`]): it takes no new packets from the peer.
    #[must_use]
    pub fn method_winding(&self, peer: &NodeId, scheme: SchemeId) -> bool {
        self.peering_of(peer)
            .and_then(|peering| peering.method(scheme))
            .is_some_and(Method::winding_down)
    }

    /// A method we wind down, to change its steps.
    pub(super) fn winding_mut(&mut self, peer: &NodeId, scheme: SchemeId) -> Option<&mut Winding> {
        match &mut self.table.get_mut(peer)?.peer {
            PeerState::Peered(peering) if peering.depeering.is_none() => {
                peering.method_mut(scheme)?.winding.as_mut()
            }
            _ => None,
        }
    }

    /// Takes the next step of each method we wind down, as
    /// [`advance_depeerings`](Self::advance_depeerings) does of a peering we
    /// end: once our own settlements on it are done, or we waited long
    /// enough, ask the peer to settle on it once; once what it said is on
    /// its way is booked, or we waited long enough, drop it and tell the
    /// peer. At most [`MAX_METHODS`] per peering.
    pub(super) fn advance_windings(&mut self, hooks: &dyn PeeringHooks) {
        let (now, wait) = (self.now_ms, self.config.depeer_wait_ms);
        let winding: Vec<(NodeId, SchemeId, Winding)> = self
            .table
            .iter()
            .filter_map(|(id, entry)| entry.active_peering().map(|peering| (id, peering)))
            .filter(|(_, peering)| peering.depeering.is_none())
            .flat_map(|(id, peering)| {
                peering
                    .methods
                    .iter()
                    .filter_map(move |method| Some((*id, method.scheme, method.winding?)))
            })
            .collect();
        for (peer, scheme, winding) in winding {
            match winding.asked_ms {
                None => {
                    let ours = hooks.settling_on(peer, scheme) || self.settling_on(&peer, scheme);
                    if !ours || winding.since_ms.saturating_add(wait) <= now {
                        self.ask_wind(peer, scheme);
                    }
                }
                Some(asked) => {
                    if winding.collected() || asked.saturating_add(wait) <= now {
                        self.drop_method_now(peer, scheme);
                    }
                }
            }
        }
    }

    /// When the next step of a method `peering` winds down is due: at the
    /// end of a wait, or at once when collected; whether our own
    /// settlements on it are done only the hooks tell, so that is looked at
    /// with each peering check.
    pub(super) fn next_wind(&self, peering: &Peering) -> u64 {
        if peering.depeering.is_some() {
            return u64::MAX;
        }
        let (now, wait) = (self.now_ms, self.config.depeer_wait_ms);
        peering
            .methods
            .iter()
            .filter_map(|method| method.winding)
            .map(|winding| match winding.asked_ms {
                None => winding
                    .since_ms
                    .saturating_add(wait)
                    .min(self.peering.check_at_ms()),
                Some(_) if winding.collected() => now,
                Some(asked) => asked.saturating_add(wait),
            })
            .min()
            .unwrap_or(u64::MAX)
    }

    /// Whether a settlement of ours to `node` on `scheme` is on its way
    /// over the channel.
    fn settling_on(&self, node: &NodeId, scheme: SchemeId) -> bool {
        self.channels
            .get(node)
            .and_then(|channel| channel.outbox.as_ref())
            .is_some_and(|outbox| outbox.scheme == scheme)
    }

    /// Asks `peer` to settle on `scheme` once, before we drop the method. A
    /// request that cannot be sent counts as asked, and is waited out.
    fn ask_wind(&mut self, peer: NodeId, scheme: SchemeId) {
        let now = self.now_ms;
        if let Some(winding) = self.winding_mut(&peer, scheme) {
            winding.asked_ms = Some(now);
        }
        let content = Content::SettlementRequest { scheme };
        let timeout = self.config.request_timeout_ms;
        if self
            .send(peer, &content, Purpose::WindRequest(scheme), timeout)
            .is_some()
        {
            self.stats.settlements_requested += 1;
        }
    }

    /// The answer to our request to settle on a method we wind down: what
    /// the peer has on its way on it. A 409, from a peer that holds no
    /// peering, means nothing is; an answer it cannot read, or none, is
    /// waited out. One that comes while the method has not asked, as the
    /// answer to a request of an earlier winding down, the method dropped
    /// and added again since, is no answer to it, and counts for nothing.
    pub(super) fn on_wind_request(
        &mut self,
        peer: NodeId,
        scheme: SchemeId,
        result: &Result<Vec<u8>, Problem>,
    ) {
        let answered = match result {
            Ok(body) => crate::wire::decode_settling(body).ok(),
            Err(_) => None,
        };
        if let Some(settling) = answered {
            self.event(Event::SettlementRequestAnswered { peer, settling });
        }
        let promised = match result {
            Err(Problem::Status(409)) => Some(0),
            _ => answered,
        };
        if let Some(winding) = self.winding_mut(&peer, scheme)
            && winding.asked_ms.is_some()
        {
            winding.promised = promised;
        }
    }

    /// Drops at once the methods on a scheme that has not been able to
    /// receive a peer's settlement for as long as a silent peer is kept,
    /// counted from the node's start at the earliest: no settlement on it
    /// can arrive, so nothing is collected first, and a peering left with
    /// none ends, told at once (spec/decisions/settlement-methods.md,
    /// spec/areas/settlement.md#when-peers-cannot-pay-us). Not while we end
    /// the peering, which collects as it does.
    pub(super) fn drop_unpayable(&mut self, hooks: &dyn PeeringHooks) {
        let (now, dead, start) = (self.now_ms, self.config.peer_dead_ms, self.started_ms);
        let mut dropped: Vec<(NodeId, SchemeId)> = Vec::new();
        for (id, entry) in &mut self.table {
            let PeerState::Peered(peering) = &mut entry.peer else {
                continue;
            };
            if peering.depeering.is_some() {
                continue;
            }
            for method in &mut peering.methods {
                if hooks.receives(method.scheme) {
                    method.unpayable_since_ms = None;
                    continue;
                }
                let since = *method.unpayable_since_ms.get_or_insert(now.max(start));
                if since.saturating_add(dead) <= now {
                    dropped.push((*id, method.scheme));
                }
            }
        }
        for (peer, scheme) in dropped {
            self.drop_method_now(peer, scheme);
        }
    }

    /// When a method of `peering` will have been unable to receive for as
    /// long as a silent peer is kept.
    pub(super) fn next_unpayable(&self, peering: &Peering) -> u64 {
        if peering.depeering.is_some() {
            return u64::MAX;
        }
        peering
            .methods
            .iter()
            .filter_map(|method| method.unpayable_since_ms)
            .map(|since| since.saturating_add(self.config.peer_dead_ms))
            .min()
            .unwrap_or(u64::MAX)
    }

    /// Drops our method on `scheme` with `peer` at once, without collecting,
    /// and tells the peer, which drops it at once
    /// (spec/decisions/settlement-methods.md): as for a method whose scheme
    /// the node no longer runs or that cannot receive, and once one wound
    /// down is collected ([`wind_down`](Self::wind_down)). What is left
    /// stays on its accounts. A peering left with none ends, told at once.
    /// Returns whether the peering had the method.
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

#[cfg(all(test, feature = "sim"))]
mod tests {
    #![allow(clippy::unwrap_used)]

    use crate::Config;
    use crate::sim::{Network, SECOND, scheme};
    use crate::wire::encode_settling;

    // An answer to a request to settle that comes while the method being
    // wound down has not asked, as one of an earlier winding down of it,
    // since dropped and added again, is no answer to it: it promises
    // nothing, so the method still asks, and waits for what the peer then
    // says.
    #[test]
    fn an_answer_before_the_method_asked_promises_nothing() {
        let mut network = Network::new(82, 2, 1, Config::DEV);
        for index in 0..2 {
            network.set_running(index, scheme(SECOND), true);
        }
        let b = *network.node(1).own();
        let both = |network: &Network| {
            network
                .node(0)
                .peering_of(&b)
                .is_some_and(|peering| peering.methods.len() == 2)
        };
        for _ in 0..60 {
            if both(&network) {
                break;
            }
            network.run_until(network.now + 3_000);
        }
        assert!(both(&network), "never peered on both");
        // Our own settlement on it is on its way, so it does not ask yet.
        network.nodes[0].hooks.settling.insert(b);
        network.act(0, |node, now| {
            assert!(node.wind_down(now, &b, scheme(SECOND)));
            node.on_wind_request(b, scheme(SECOND), &Ok(encode_settling(5)));
            let method = node.peering_of(&b).unwrap().method(scheme(SECOND)).unwrap();
            let winding = method.winding.unwrap();
            assert_eq!(winding.asked_ms, None);
            assert_eq!(winding.promised, None, "a stale answer promised");
            node.check_invariants();
        });
    }
}
