//! Settlement methods (spec/decisions/settlement-methods.md,
//! spec/areas/network.md#settlement-methods): adding them, which offers
//! decide ([`offers`](super::offers)), winding them down and dropping
//! them. A peer of the release before, which takes no offers, asks for a
//! method (`methodAdd`) and is told of one dropped (`methodDrop`), again
//! until it hears back (spec/decisions/method-offers.md).

use alloc::vec::Vec;

use dassie_identity::NodeId;
use dassie_oer::Encode;

use super::inbound::{Accepted, ok};
use super::{MAX_METHODS, PeerNode, Problem, Purpose};
use crate::hooks::PeeringHooks;
use crate::io::{Event, Reply};
use crate::table::{Entry, Method, PeerState, Peering, Winding};
use crate::wire::{Content, MethodResponse, RefusalReason, SchemeId, TerminationReason};

impl PeerNode {
    /// Adds `method` to our peering with `node`, which has room for it and
    /// none on its scheme.
    pub(super) fn add_method(&mut self, node: NodeId, method: Method) {
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
        let confirmed = peering.confirms(&method);
        peering.methods.push(method);
        self.dirty.insert(node);
        self.stats.methods_added += 1;
        self.event(Event::MethodAdded { peer: node, scheme });
        if confirmed {
            self.event(Event::MethodConfirmed { peer: node, scheme });
        }
    }

    /// A peer of the release before asks to add a method to our peering
    /// (`methodAdd`), as the node that asked for the peering did: answered
    /// as a peering request is, with the method recorded before the
    /// acceptance goes out. A repeat, after an answer that got lost, is
    /// accepted again, as it is. Kept for one release, while such nodes
    /// run (spec/decisions/method-offers.md).
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
            self.older_release(sender);
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
        } else {
            None
        };
        if let Some(reason) = reason {
            return refused(reason);
        }
        if self.legacy_method_add(hooks, sender, scheme, data) {
            ok(&MethodResponse::Accepted.to_vec())
        } else {
            refused(RefusalReason::SchemeRefused)
        }
    }

    /// A peer dropped a method of our peering (`methodDrop`), after
    /// collecting on it: we drop it at once, and settle nothing more on it
    /// (spec/decisions/settlement-methods.md). A peer of the release before
    /// says so with it; a peer that takes offers sends one too while it
    /// does not know whether we take them. A method we do not have is
    /// dropped already. A peering it leaves with none ends; one being ended
    /// keeps its last until it is over.
    pub(super) fn method_drop_request(&mut self, accepted: &Accepted, scheme: SchemeId) -> Reply {
        let sender = accepted.sender;
        if self.peering_of(&sender).is_none() {
            self.notice_one_sided(sender);
            return Reply::refuse(409, "not peered");
        }
        self.forget_theirs(&sender, scheme);
        self.peer_dropped(sender, scheme);
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
        // Our offer leaves it out from now; a peer that may not take offers
        // is told with a drop too. Offered again only after a while, so a
        // drop is never undone at once.
        self.withdraw_from_offer(peer, scheme);
        self.exclude(peer, scheme);
        if self.speaks_offers(&peer) != Some(true) {
            self.queue_drop(peer, scheme);
        }
        true
    }

    /// Takes the method on `scheme` out of our peering with `node`, which
    /// keeps another.
    pub(super) fn remove_method(&mut self, node: NodeId, scheme: SchemeId, by_us: bool) {
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
