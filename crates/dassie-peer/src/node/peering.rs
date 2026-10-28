//! Peering (spec/areas/network.md#peering,
//! spec/decisions/peering-lifecycle.md): looking for peers, asking, and
//! answering. Termination, bans and the advertise flag are in
//! [`lifecycle`](super::lifecycle).

use alloc::vec::Vec;

use dassie_identity::NodeId;
use dassie_oer::{Decode, Encode};

use super::gossip::Retransmit;
use super::inbound::Accepted;
use super::inbound::{Probe, ProbeFor, ok};
use super::{PeerNode, Problem, Purpose};
use crate::hooks::PeeringHooks;
use crate::io::{Event, Inbound, InboundId, Reply};
use crate::table::{
    Advertise, Entry, LinkState, Method, Network, PeerState, Peering as PeeringState, Sources,
    backoff,
};
use crate::wire::{
    Content, Endpoint, HubDetails, PeeringRequest, PeeringResponse, RefusalReason, SchemeId,
    SignedNodeInfo, decode_octets,
};

/// The peering side of the node's state.
pub(crate) struct Peering {
    check_at_ms: u64,
    bridge_at_ms: u64,
    /// The node we are asking, if any: one attempt at a time.
    attempt: Option<NodeId>,
    /// The method we are asking a peer for, if any: one at a time.
    pub(crate) method_attempt: Option<MethodAttempt>,
}

/// Our request for a further method of a peering
/// (spec/decisions/settlement-methods.md).
pub(crate) struct MethodAttempt {
    /// The peer asked.
    pub node: NodeId,
    /// The method's scheme.
    pub scheme: SchemeId,
    /// The peer's peering info on the scheme, once it gave it.
    pub info: Option<Vec<u8>>,
    /// Whether the peer dropped a method on the scheme while we asked: its
    /// answer, when it comes, ends the request and adds nothing.
    pub dropped: bool,
}

impl Peering {
    pub(crate) const fn new(now_ms: u64) -> Self {
        Self {
            check_at_ms: now_ms,
            bridge_at_ms: now_ms,
            attempt: None,
            method_attempt: None,
        }
    }

    /// Whether we are asking `node` to peer.
    pub(crate) fn is_asking(&self, node: &NodeId) -> bool {
        self.attempt == Some(*node)
    }

    /// When the next check for peers is due, whether or not one is wanted.
    pub(crate) const fn check_at_ms(&self) -> u64 {
        self.check_at_ms
    }

    /// The next check, given when a candidate may first be asked, if the
    /// node wants a peer and can start a peering; and the next bridge check.
    pub(crate) fn next_due(&self, candidate_at_ms: Option<u64>) -> Option<u64> {
        let check = candidate_at_ms.map(|at| at.max(self.check_at_ms));
        Some(check.map_or(self.bridge_at_ms, |check| check.min(self.bridge_at_ms)))
    }

    /// Holds off looking for peers until `at_ms`: after a restart, the
    /// peers we have get a heartbeat's time to confirm themselves first.
    pub(crate) const fn hold_until(&mut self, at_ms: u64) {
        if self.check_at_ms < at_ms {
            self.check_at_ms = at_ms;
        }
        if self.bridge_at_ms < at_ms {
            self.bridge_at_ms = at_ms;
        }
    }
}

impl PeerNode {
    /// Looks for peers until enough are confirmed, and for a bridge when a
    /// majority node stays unreachable (spec/decisions/peering-lifecycle.md).
    pub(super) fn seek_peers(&mut self, hooks: &mut dyn PeeringHooks) {
        let now = self.now_ms;
        if self.peering.check_at_ms <= now {
            self.peering.check_at_ms = now.saturating_add(self.config.peering_check_ms);
            let wanted = self.confirmed_peers() < self.peer_target();
            if wanted
                && self.can_start()
                && let Some((node, scheme)) = self.pick_candidate(hooks, false)
            {
                self.start_peering(hooks, node, scheme);
            }
        }
        if self.peering.bridge_at_ms <= now {
            self.peering.bridge_at_ms = now.saturating_add(self.config.bridge_check_ms);
            // A periphery node joins no islands: it routes through its
            // uplinks only.
            if !self.is_periphery()
                && self.can_start()
                && let Some((node, scheme)) = self.pick_candidate(hooks, true)
            {
                self.start_peering(hooks, node, scheme);
            }
        }
    }

    /// When a check for peers may next find something to do: never while
    /// the node has enough peers or cannot start a peering, otherwise when
    /// the first candidate's backoff ends. A new candidate, a peer lost or
    /// an attempt answered come with an input, after which the node is
    /// polled again; nothing needs a check every second.
    pub(super) fn next_peer_check(&self) -> Option<u64> {
        if self.confirmed_peers() >= self.peer_target() || !self.can_start() {
            return None;
        }
        let (now, stale, future) = (self.now_ms, self.config.stale_ms, self.config.max_future_ms);
        self.table
            .iter()
            .filter(|(id, entry)| {
                **id != self.own
                    && entry.peer == PeerState::None
                    && !self.bans.contains(id)
                    && self.channel_room(id)
                    && entry.link.as_ref().is_some_and(|link| {
                        link.is_fresh(now, stale, future)
                            && self.may_ask(id, link)
                            && link.schemes().any(|s| self.offer_on(s).is_some())
                    })
            })
            .map(|(_, entry)| entry.peering_at_ms)
            .min()
    }

    /// Whether the node may start a peering now. A node pinned to its
    /// uplinks has slots of its own, as many as it pins, and counts only
    /// peerings with them among those it started, so that ones made before
    /// the pin never keep it from asking them
    /// (spec/areas/network.md#pinned-uplinks); [`Self::max_core_peers`]
    /// still bounds them all.
    fn can_start(&self) -> bool {
        let pinned = self.pinned_uplinks();
        let outbound = self
            .core_peers()
            .filter(|(id, peering)| {
                !peering.inbound && pinned.is_none_or(|pinned| pinned.binary_search(id).is_ok())
            })
            .count();
        self.peering.attempt.is_none()
            && !self.offers.is_empty()
            && self.own_link_bytes().is_some()
            && outbound < self.max_outbound()
            && self.core_peers().count() < self.max_core_peers()
    }

    /// Most peerings the node starts: as many as it is pinned to, if it
    /// is, else the configured cap.
    fn max_outbound(&self) -> usize {
        self.pinned_uplinks()
            .map_or(self.config.max_outbound(), <[NodeId]>::len)
    }

    /// Most peerings with core nodes the node holds: a pinned node's
    /// slots are its own, bounded by [`crate::MAX_UPLINKS`] rather than
    /// the configured `max_peers` ([`crate::Config::max_core_peers`]).
    pub(super) fn max_core_peers(&self) -> usize {
        if self.pinned_uplinks().is_some() {
            self.config.max_core_peers()
        } else {
            self.config.max_peers
        }
    }

    /// Whether `node`, with `link`, may be asked to peer: never a
    /// periphery node; and by a periphery node, only one it is pinned to if
    /// it is pinned to any (spec/areas/network.md#pinned-uplinks).
    fn may_ask(&self, node: &NodeId, link: &LinkState) -> bool {
        self.is_core_node(node, !link.periphery)
            && self
                .pinned_uplinks()
                .is_none_or(|pinned| pinned.binary_search(node).is_ok())
    }

    /// How many peers the node looks for: as many as it is pinned to, if
    /// it is, else the configured target.
    fn peer_target(&self) -> usize {
        self.pinned_uplinks()
            .map_or(self.config.peer_target, <[NodeId]>::len)
    }

    /// Peers that count toward the target: advertised ones, confirmed by
    /// a recent message and not held down. A node whose peers went silent,
    /// or flap, looks for others (spec/decisions/peering-lifecycle.md).
    /// Downlinks do not count: they are apart from the core peering slots.
    /// A node pinned to its uplinks counts only those. A periphery node
    /// counts only uplinks that are core nodes: one that left the core
    /// with it forwards nothing for it, and it looks for others.
    pub(crate) fn confirmed_peers(&self) -> usize {
        let pinned = self.pinned_uplinks();
        let periphery = !self.forwards();
        self.table
            .iter()
            .filter(|(id, entry)| {
                !entry.is_downlink()
                    && entry.active_peering().is_some()
                    && entry.advertise.on
                    && pinned.is_none_or(|pinned| pinned.binary_search(id).is_ok())
                    && (!periphery
                        || entry
                            .link
                            .as_ref()
                            .is_some_and(|link| self.is_core_node(id, !link.periphery)))
            })
            .count()
    }

    /// A node to ask: fresh, with a scheme in common, not in backoff.
    /// Nodes our routes do not reach come first, then majority nodes; the
    /// RNG picks among the best. A node with a URL that is not core yet
    /// asks those with a core relationship free first, which can promote it
    /// (spec/decisions/promotion-to-core.md). A bridge only considers
    /// majority nodes unreachable for the grace period.
    fn pick_candidate(
        &mut self,
        hooks: &dyn PeeringHooks,
        bridge: bool,
    ) -> Option<(NodeId, SchemeId)> {
        let (now, stale, future) = (self.now_ms, self.config.stale_ms, self.config.max_future_ms);
        let grace = self.config.unreachable_grace_ms;
        let seeking_promotion = self.has_url() && !self.is_core();
        let mut best: Vec<(u8, NodeId, SchemeId)> = Vec::new();
        for (id, entry) in &self.table {
            let Some(link) = entry.link.as_ref() else {
                continue;
            };
            if *id == self.own
                || entry.peer != PeerState::None
                || entry.peering_at_ms > now
                || !link.is_fresh(now, stale, future)
                || !self.may_ask(id, link)
                || self.bans.contains(id)
                || !self.channel_room(id)
            {
                continue;
            }
            let Some(&scheme) = self.methods_with(hooks, link).first() else {
                continue;
            };
            let unreachable = !self.routes.contains_key(id);
            let majority = entry.sources.contains(Sources::MAJORITY);
            if bridge
                && !(majority
                    && entry
                        .unreachable_ms
                        .is_some_and(|since| since.saturating_add(grace) <= now))
            {
                continue;
            }
            let full = seeking_promotion && link.free_core.is_none_or(|free| free == 0);
            let score = u8::from(full) * 4 + u8::from(!unreachable) * 2 + u8::from(!majority);
            best.push((score, *id, scheme));
        }
        let top = best.iter().map(|(score, _, _)| *score).min()?;
        best.retain(|(score, _, _)| *score == top);
        let index = usize::try_from(self.rng.next_u64() % (best.len() as u64)).unwrap_or(0);
        best.get(index).map(|(_, id, scheme)| (*id, *scheme))
    }

    /// The schemes a peering with the node of `link` settles on, in the
    /// order of our offers (spec/decisions/settlement-methods.md): each
    /// scheme both offer, as we read its offer
    /// ([`Self::their_offers`]), over a hub both are in, one of them at
    /// least with an account in it, or, on a scheme not over a hub, one
    /// whose peers trust each other without limit, the test stub's. A scheme
    /// that is neither over a hub nor of unlimited trust would settle
    /// directly, so it is never a method (spec/decisions/hubs-only.md). The
    /// first is the one to ask for a peering on, and each of the others a
    /// further method of it. A hub scheme without a shared hub would leave
    /// each side without the other's account
    /// (spec/designs/settlement-hubs.md#11-choosing-a-hub). At most one per
    /// scheme we offer, so at most [`MAX_METHODS`](crate::MAX_METHODS).
    pub(crate) fn methods_with(&self, hooks: &dyn PeeringHooks, link: &LinkState) -> Vec<SchemeId> {
        let theirs = self.their_offers(link);
        let methods: Vec<SchemeId> = self
            .offers
            .iter()
            .filter(|own| {
                let Some((_, their_hub)) = theirs.iter().find(|(scheme, _)| *scheme == own.scheme)
                else {
                    return false;
                };
                match (own.hub, their_hub) {
                    (Some(ours), Some(theirs)) => {
                        hooks.uses_commitments(own.scheme) && shares_hub(&ours, theirs)
                    }
                    (None, None) => {
                        !hooks.uses_commitments(own.scheme) && hooks.trusts_unlimited(own.scheme)
                    }
                    _ => false,
                }
            })
            .map(|own| own.scheme)
            .collect();
        assert!(methods.len() <= crate::MAX_METHODS, "a method per scheme");
        methods
    }

    /// The offers of the node of `link` on schemes we offer, as we read
    /// them (spec/areas/peer-protocol.md#settlement-offers), in its order:
    /// each scheme's details read as our offer on it says, the hub's on a
    /// scheme we offer over a hub and none on any other. An offer whose
    /// details are not that, or whose hub an offer we took before names, is
    /// taken as absent, and an offer on a scheme we do not offer is passed
    /// over, unread: we would not peer on it.
    pub(super) fn their_offers(&self, link: &LinkState) -> Vec<(SchemeId, Option<HubDetails>)> {
        let mut taken: Vec<(SchemeId, Option<HubDetails>)> = Vec::with_capacity(self.offers.len());
        for offered in &link.offers {
            let Some(own) = self.offer_on(offered.scheme) else {
                continue;
            };
            let read = if own.hub.is_some() {
                match HubDetails::decode_exact(&offered.details) {
                    Ok(hub)
                        if !taken
                            .iter()
                            .any(|(_, other)| other.is_some_and(|o| o.hub_id == hub.hub_id)) =>
                    {
                        Some(Some(hub))
                    }
                    _ => None,
                }
            } else {
                offered.details.is_empty().then_some(None)
            };
            if let Some(hub) = read {
                taken.push((offered.scheme, hub));
            }
        }
        taken
    }

    fn start_peering(&mut self, hooks: &mut dyn PeeringHooks, node: NodeId, scheme: SchemeId) {
        if !hooks.speaks(scheme) {
            return;
        }
        if let Some(entry) = self.table.get_mut(&node) {
            entry.peer = PeerState::Requesting { scheme, info: None };
        }
        self.ensure(node, Sources::PEER);
        self.peering.attempt = Some(node);
        let timeout = self.config.request_timeout_ms;
        let content = Content::PeeringInfoRequest { scheme };
        if self
            .send(node, &content, Purpose::PeeringInfo, timeout)
            .is_none()
        {
            self.fail_attempt(node, "could not send");
        }
    }

    pub(super) fn on_peering_info(
        &mut self,
        hooks: &mut dyn PeeringHooks,
        node: NodeId,
        result: Result<Vec<u8>, Problem>,
    ) {
        let Some(scheme) = self.requesting(&node) else {
            return;
        };
        let info = match result.and_then(|body| {
            decode_octets(&body)
                .map(<[u8]>::to_vec)
                .map_err(|_| Problem::Invalid)
        }) {
            Ok(info) => info,
            Err(_) => return self.fail_attempt(node, "no peering info"),
        };
        let mut data = Vec::new();
        if hooks
            .create_request(node, scheme, &info, &mut data)
            .is_err()
        {
            return self.fail_attempt(node, "the scheme would not ask");
        }
        let Some(own) = self.own_link_bytes() else {
            return self.fail_attempt(node, "no link state of ours");
        };
        let Ok(node_info) = SignedNodeInfo::decode_exact(&own) else {
            return self.fail_attempt(node, "no link state of ours");
        };
        let content = Content::PeeringRequest(PeeringRequest {
            scheme,
            node_info,
            data: &data,
        });
        if let Some(entry) = self.table.get_mut(&node) {
            entry.peer = PeerState::Requesting {
                scheme,
                info: Some(info),
            };
        }
        let timeout = self.config.request_timeout_ms;
        if self
            .send(node, &content, Purpose::PeeringRequest, timeout)
            .is_none()
        {
            self.fail_attempt(node, "could not send");
        }
    }

    pub(super) fn on_peering_response(
        &mut self,
        hooks: &mut dyn PeeringHooks,
        node: NodeId,
        result: Result<Vec<u8>, Problem>,
    ) {
        let Some(scheme) = self.requesting(&node) else {
            return;
        };
        let Ok(body) = result else {
            return self.fail_attempt(node, "no answer");
        };
        match PeeringResponse::decode_exact(&body) {
            Ok(PeeringResponse::Accepted)
                if self.core_peers().count() >= self.max_core_peers()
                    || !self.channel_room(&node) =>
            {
                self.fail_attempt(node, "no room left");
            }
            Ok(PeeringResponse::Accepted) if hooks.speaks(scheme) => {
                let info = match self.table.get(&node).map(|e| &e.peer) {
                    Some(PeerState::Requesting {
                        info: Some(info), ..
                    }) => info.clone(),
                    _ => Vec::new(),
                };
                let now = self.now_ms;
                match hooks.finalize(now, node, scheme, &info) {
                    Ok(agreed) => {
                        if !self.become_peered(hooks, node, scheme, agreed, false) {
                            self.fail_attempt(node, "no room left");
                        }
                    }
                    Err(_) => self.fail_attempt(node, "the scheme would not finalize"),
                }
            }
            Ok(PeeringResponse::Refused(reason)) => self.refused(node, reason),
            _ => self.fail_attempt(node, "an invalid answer"),
        }
    }

    fn refused(&mut self, node: NodeId, reason: RefusalReason) {
        let wait = match reason {
            // It has not heard of us yet: try again once lists moved on.
            RefusalReason::UnknownNode => Some(self.config.list_poll_interval_ms),
            // Its own request to us won.
            RefusalReason::Concurrent => Some(self.config.min_backoff_ms),
            _ => None,
        };
        self.fail_with(node, "refused", wait);
    }

    fn requesting(&self, node: &NodeId) -> Option<SchemeId> {
        if self.peering.attempt != Some(*node) {
            return None;
        }
        match self.table.get(node).map(|entry| &entry.peer) {
            Some(PeerState::Requesting { scheme, .. }) => Some(*scheme),
            _ => None,
        }
    }

    fn fail_attempt(&mut self, node: NodeId, reason: &'static str) {
        self.fail_with(node, reason, None);
    }

    /// Ends an attempt, backing off from the node.
    fn fail_with(&mut self, node: NodeId, reason: &'static str, wait: Option<u64>) {
        self.peering.attempt = None;
        let (now, min, max) = (
            self.now_ms,
            self.config.min_backoff_ms,
            self.config.max_backoff_ms,
        );
        if let Some(entry) = self.table.get_mut(&node)
            && matches!(entry.peer, PeerState::Requesting { .. })
        {
            entry.peer = PeerState::None;
            entry.peering_failures = entry.peering_failures.saturating_add(1);
            let delay = wait.unwrap_or_else(|| backoff(min, max, entry.peering_failures));
            entry.peering_at_ms = now.saturating_add(delay);
            self.drop_peer_source(&node);
        }
        self.event(Event::PeeringFailed { node, reason });
    }

    /// Records the peering with `node`, with its channel. Returns whether
    /// it is peered: not when its channel does not fit, since a peer we can
    /// reach always has one.
    fn become_peered(
        &mut self,
        hooks: &dyn PeeringHooks,
        node: NodeId,
        scheme: SchemeId,
        agreed: crate::hooks::Agreed,
        inbound: bool,
    ) -> bool {
        let now = self.now_ms;
        if self.peering.attempt == Some(node) {
            self.peering.attempt = None;
        }
        self.ensure(node, self.peer_source(&node));
        let Some(entry) = self.table.get(&node) else {
            return false;
        };
        if entry.peering().is_some() {
            // A peering stays as it was made
            // (spec/decisions/peering-lifecycle.md).
            return true;
        }
        if !self.open_channel(node, hooks) {
            return false;
        }
        let Some(entry) = self.table.get_mut(&node) else {
            return false;
        };
        let fresh = true;
        entry.advertise = Advertise {
            on: true,
            on_since_ms: now,
            ..Advertise::default()
        };
        let first = Method::new(scheme, agreed.state, agreed.credit_out);
        entry.peer = PeerState::Peered(PeeringState::new(first, now, inbound));
        entry.standing = crate::table::Standing {
            skew_ms: entry.standing.skew_ms,
            ..crate::table::Standing::NEW
        };
        entry.peering_failures = 0;
        // Its further methods are asked for at once.
        entry.method_at_ms = now;
        entry.method_failures = 0;
        entry.method_tried = None;
        self.dirty.insert(node);
        if fresh {
            // It just answered us, or we it after probing its URL: what an
            // earlier peering left of our reach to it is gone
            // (spec/decisions/unreachable-peers.md).
            self.unreached.remove(&node);
            self.stats.peerings += 1;
            self.gossip.heartbeat_soon(now);
            self.advertised_changed();
            self.event(Event::Peered {
                node,
                scheme,
                inbound,
            });
        }
        true
    }

    /// A peering request: the requester's link state must
    /// verify and be the sender's; it must be known, speak the scheme, and
    /// there must be room. Then its URL is probed before the answer.
    pub(super) fn peering_request(
        &mut self,
        hooks: &mut dyn PeeringHooks,
        accepted: Accepted,
        content: &[u8],
        inbound: InboundId,
        network: Option<Network>,
    ) -> Inbound<'static> {
        let Ok(Content::PeeringRequest(request)) = Content::decode_exact(content) else {
            return Inbound::Reply(Reply::refuse(400, "malformed message"));
        };
        let sender = accepted.sender;
        if request.node_info.node_id != sender {
            return Inbound::Reply(Reply::refuse(400, "the link state is not the sender's"));
        }
        let Ok(key) = request.node_info.verify() else {
            return Inbound::Reply(Reply::refuse(400, "invalid link state"));
        };
        if request.node_info.info.sequence > self.now_ms.saturating_add(self.config.max_future_ms) {
            return Inbound::Reply(Reply::refuse(400, "the link state is from the future"));
        }
        let downlink = !self.is_core_node(&sender, !request.node_info.info.is_periphery());
        if let Err(reason) = self.check_request(hooks, &sender, &request) {
            return Inbound::Reply(refused(reason));
        }
        // A repeat keeps the peering as it was made, core or downlink.
        let repeat = self.peering_of(&sender).is_some();
        if downlink && !repeat && !self.ensure(sender, Sources::DOWNLINK) {
            return Inbound::Reply(refused(RefusalReason::Capacity));
        }
        // From the sender itself, whose clock its link state shows.
        self.process_link_state(&request.node_info, Some(sender), Retransmit::Never);
        if let Some(entry) = self.table.get_mut(&sender) {
            entry.key.get_or_insert(key.key());
        }
        // A repeat, after an answer that got lost: the stored peering, as
        // it is (spec/decisions/peering-lifecycle.md).
        if self
            .table
            .get(&sender)
            .and_then(Entry::active_peering)
            .is_some_and(|peering| peering.has(request.scheme))
        {
            return Inbound::Reply(ok(&PeeringResponse::Accepted.to_vec()));
        }
        // A periphery node is reached only over the connection its request
        // came on, which shows it is there: no URL of its is probed
        // (spec/areas/network.md#asking-and-answering).
        if downlink {
            let reply = self.complete_peering(hooks, &accepted, content, true);
            // A refused downlink takes no place in the table.
            if self.peering_of(&sender).is_none() {
                self.drop_downlink(&sender);
            }
            return Inbound::Reply(reply);
        }
        let probe = Probe {
            inbound,
            accepted,
            sequence: request.node_info.info.sequence,
            network,
            purpose: ProbeFor::Peering(content.to_vec()),
        };
        let Some(endpoint) = Endpoint::of_url(request.node_info.info.url) else {
            return Inbound::Reply(refused(RefusalReason::Unreachable));
        };
        self.start_probe(sender, endpoint, alloc::boxed::Box::new(probe))
    }

    /// Why a request is refused before probing, if it is.
    fn check_request(
        &mut self,
        hooks: &mut dyn PeeringHooks,
        sender: &NodeId,
        request: &PeeringRequest<'_>,
    ) -> Result<(), RefusalReason> {
        if self.bans.contains(sender) {
            return Err(RefusalReason::Banned);
        }
        // A periphery node peers only as a downlink, and only a core node
        // with room for them takes downlinks.
        let downlink = !self.is_core_node(sender, !request.node_info.info.is_periphery());
        if self.is_periphery() || (downlink && !self.takes_downlinks()) {
            return Err(RefusalReason::Unspecified);
        }
        // A repeat from a node peered with us with a method on the scheme
        // is answered with the stored peering, also on a scheme no longer
        // peered on anew, such as one that stopped: the methods on it stay.
        let repeat = self
            .table
            .get(sender)
            .and_then(Entry::peering)
            .is_some_and(|peering| peering.has(request.scheme));
        if !repeat && (self.offer_on(request.scheme).is_none() || !hooks.speaks(request.scheme)) {
            return Err(RefusalReason::UnsupportedScheme);
        }
        // A scheme neither over a hub nor of unlimited trust would settle
        // directly, so a peering is never made on it, whoever asks
        // (spec/decisions/hubs-only.md), as it is never one we ask on
        // ([`Self::methods_with`]).
        if !repeat && hooks.settles_directly(request.scheme) {
            return Err(RefusalReason::UnsupportedScheme);
        }
        // A downlink is in no node list: it enters the table by asking.
        let Some(entry) = self.table.get(sender) else {
            return if downlink {
                self.downlink_room(hooks, sender)
            } else {
                Err(RefusalReason::UnknownNode)
            };
        };
        if entry.termination().is_some() || entry.refuse_until_ms > self.now_ms {
            return Err(RefusalReason::Terminating);
        }
        if !self.channel_room_for(sender, downlink) {
            return Err(RefusalReason::Capacity);
        }
        if let Some(peering) = entry.peering() {
            return if peering.has(request.scheme) {
                Ok(())
            } else {
                Err(RefusalReason::Unspecified)
            };
        }
        if matches!(entry.peer, PeerState::Requesting { .. }) {
            // Both asked at once: the lower node ID's request wins.
            if self.own < *sender {
                return Err(RefusalReason::Concurrent);
            }
            self.abandon_attempt(*sender);
        }
        if downlink {
            return self.downlink_room(hooks, sender);
        }
        let inbound = self.core_peers().filter(|(_, p)| p.inbound).count();
        if inbound >= self.config.max_inbound()
            || self.core_peers().count() >= self.config.max_peers
        {
            return Err(RefusalReason::Capacity);
        }
        Ok(())
    }

    /// Whether a downlink `sender` fits: its place in the table, its
    /// peering and its channel, under their own bound, apart from the core
    /// peering slots (spec/areas/network.md#downlinks); and, if it never
    /// paid us, among the free receivers.
    fn downlink_room(
        &self,
        hooks: &dyn PeeringHooks,
        sender: &NodeId,
    ) -> Result<(), RefusalReason> {
        let peered = self.peers().filter(|(id, _)| self.is_downlink(id)).count();
        let placed = self.is_downlink(sender) || self.downlinks() < self.config.max_downlinks;
        if peered >= self.config.max_downlinks || !placed || !self.channel_room_for(sender, true) {
            return Err(RefusalReason::Capacity);
        }
        if self.free_room_for(hooks, sender) {
            Ok(())
        } else {
            Err(RefusalReason::Capacity)
        }
    }

    /// Whether a downlink `sender` fits among the free receivers: at most
    /// half of the downlinks' places are theirs, so keys made for nothing
    /// never fill the places of downlinks that paid us. One that paid us
    /// once is bounded by the downlinks' bound alone, and one whose channel
    /// is open as a downlink's holds its place already. Nothing is ever
    /// ended or displaced for this (spec/decisions/free-receivers-half.md).
    fn free_room_for(&self, hooks: &dyn PeeringHooks, sender: &NodeId) -> bool {
        if self.free_downlinks() < self.config.max_free_downlinks() {
            return true;
        }
        let held = self
            .channels
            .get(sender)
            .is_some_and(|channel| channel.endpoint.is_none());
        held || self.paid_us(sender, hooks)
    }

    pub(crate) fn abandon_attempt(&mut self, node: NodeId) {
        if self.peering.attempt == Some(node) {
            self.peering.attempt = None;
        }
        if let Some(entry) = self.table.get_mut(&node)
            && matches!(entry.peer, PeerState::Requesting { .. })
        {
            entry.peer = PeerState::None;
            self.drop_peer_source(&node);
        }
    }

    /// Accepts a request once its sender proved reachable: the scheme
    /// opens its accounts, and the peering is recorded before the answer
    /// goes out.
    pub(super) fn complete_peering(
        &mut self,
        hooks: &mut dyn PeeringHooks,
        accepted: &Accepted,
        content: &[u8],
        reachable: bool,
    ) -> Reply {
        let Ok(Content::PeeringRequest(request)) = Content::decode_exact(content) else {
            return Reply::refuse(400, "malformed message");
        };
        if !reachable {
            return refused(RefusalReason::Unreachable);
        }
        let sender = accepted.sender;
        if let Err(reason) = self.check_request(hooks, &sender, &request) {
            return refused(reason);
        }
        // Its channel opens as the kind its room was checked for: the link
        // state we hold for it says which kind that is.
        let downlink = !self.is_core_node(&sender, !request.node_info.info.is_periphery());
        let kind = self
            .contact(&sender)
            .map(|(_, endpoint)| endpoint.is_none());
        if !self.channels.contains_key(&sender) && kind != Some(downlink) {
            return refused(RefusalReason::Unspecified);
        }
        let now = self.now_ms;
        let agreed = match hooks.accept(now, sender, downlink, request.scheme, request.data) {
            Ok(agreed) => agreed,
            Err(reason) => return refused(reason),
        };
        if !self.become_peered(hooks, sender, request.scheme, agreed, true) {
            return refused(RefusalReason::Capacity);
        }
        ok(&PeeringResponse::Accepted.to_vec())
    }
}

fn refused(reason: RefusalReason) -> Reply {
    ok(&PeeringResponse::Refused(reason).to_vec())
}

/// Whether a peering may settle over the hubs `own` and `theirs` name: the
/// same hub, and one at least with an account in it, so that it can pay
/// the other in (spec/designs/settlement-hubs.md#5-joining).
fn shares_hub(own: &HubDetails, theirs: &HubDetails) -> bool {
    own.hub_id == theirs.hub_id
        && (own.account != HubDetails::NO_ACCOUNT || theirs.account != HubDetails::NO_ACCOUNT)
}
