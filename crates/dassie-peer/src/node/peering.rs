//! Peering (spec 3 §12, D12, spec 7 §4.7): looking for peers, asking, and
//! answering. Termination, bans and the advertise flag are in
//! [`lifecycle`](super::lifecycle).

use alloc::vec::Vec;

use dassie_identity::NodeId;
use dassie_oer::{Decode, Encode};

use super::gossip::Retransmit;
use super::inbound::{Probe, ProbeFor, ok};
use super::{PeerNode, Problem, Purpose};
use crate::auth::Accepted;
use crate::hooks::PeeringHooks;
use crate::io::{Event, Inbound, InboundId, Reply};
use crate::table::{Advertise, Entry, PeerState, Peering as PeeringState, Sources, backoff};
use crate::wire::{
    Content, PeeringRequest, PeeringResponse, RefusalReason, SchemeId, SignedNodeInfo,
    decode_octets,
};

/// The peering side of the node's state.
pub(crate) struct Peering {
    check_at_ms: u64,
    bridge_at_ms: u64,
    /// The node we are asking, if any: one attempt at a time.
    attempt: Option<NodeId>,
}

impl Peering {
    pub(crate) const fn new(now_ms: u64) -> Self {
        Self {
            check_at_ms: now_ms,
            bridge_at_ms: now_ms,
            attempt: None,
        }
    }

    pub(crate) fn next_due(&self) -> Option<u64> {
        Some(self.check_at_ms.min(self.bridge_at_ms))
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
    /// majority node stays unreachable (D12).
    pub(super) fn seek_peers(&mut self, hooks: &mut dyn PeeringHooks) {
        let now = self.now_ms;
        if self.peering.check_at_ms <= now {
            self.peering.check_at_ms = now.saturating_add(self.config.peering_check_ms);
            let wanted = self.confirmed_peers() < self.config.peer_target;
            if wanted
                && self.can_start()
                && let Some((node, scheme)) = self.pick_candidate(false)
            {
                self.start_peering(hooks, node, scheme);
            }
        }
        if self.peering.bridge_at_ms <= now {
            self.peering.bridge_at_ms = now.saturating_add(self.config.bridge_check_ms);
            if self.can_start()
                && let Some((node, scheme)) = self.pick_candidate(true)
            {
                self.start_peering(hooks, node, scheme);
            }
        }
    }

    fn can_start(&self) -> bool {
        let outbound = self.peers().filter(|(_, peering)| !peering.inbound).count();
        self.peering.attempt.is_none()
            && !self.schemes.is_empty()
            && self.own_link_bytes().is_some()
            && outbound < self.config.max_outbound()
            && self.peers().count() < self.config.max_peers
    }

    /// Peers that count toward the target: advertised ones, confirmed by
    /// a recent message and not held down. A node whose peers went silent,
    /// or flap, looks for others (D12).
    pub(crate) fn confirmed_peers(&self) -> usize {
        self.table
            .values()
            .filter(|entry| entry.active_peering().is_some() && entry.advertise.on)
            .count()
    }

    /// A node to ask: fresh, with a scheme in common, not in backoff.
    /// Nodes our routes do not reach come first, then majority nodes; the
    /// RNG picks among the best. A bridge only considers majority nodes
    /// unreachable for the grace period.
    fn pick_candidate(&mut self, bridge: bool) -> Option<(NodeId, SchemeId)> {
        let (now, stale, future) = (self.now_ms, self.config.stale_ms, self.config.max_future_ms);
        let grace = self.config.unreachable_grace_ms;
        let mut best: Vec<(u8, NodeId, SchemeId)> = Vec::new();
        for (id, entry) in &self.table {
            let Some(link) = entry.link.as_ref() else {
                continue;
            };
            if *id == self.own
                || entry.peer != PeerState::None
                || entry.peering_at_ms > now
                || !link.is_fresh(now, stale, future)
                || self.bans.contains(id)
                || !self.channel_room(id)
            {
                continue;
            }
            let Some(scheme) = link.schemes.iter().find(|s| self.schemes.contains(s)) else {
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
            let score = u8::from(!unreachable) * 2 + u8::from(!majority);
            best.push((score, *id, *scheme));
        }
        let top = best.iter().map(|(score, _, _)| *score).min()?;
        best.retain(|(score, _, _)| *score == top);
        let index = usize::try_from(self.rng.next_u64() % (best.len() as u64)).unwrap_or(0);
        best.get(index).map(|(_, id, scheme)| (*id, *scheme))
    }

    fn start_peering(&mut self, hooks: &mut dyn PeeringHooks, node: NodeId, scheme: SchemeId) {
        if hooks.versions(scheme).is_empty() {
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
            versions: hooks.versions(scheme).to_vec(),
            // A node knows only the credit it extends, and says nothing of
            // it (D23); nodes of 0.1 read 0.
            credit: 0,
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
            Ok(PeeringResponse::Accepted { .. })
                if self.peers().count() >= self.config.max_peers || !self.channel_room(&node) =>
            {
                self.fail_attempt(node, "no room left");
            }
            Ok(PeeringResponse::Accepted { version, data, .. })
                if hooks.versions(scheme).contains(&version) =>
            {
                let info = match self.table.get(&node).map(|e| &e.peer) {
                    Some(PeerState::Requesting {
                        info: Some(info), ..
                    }) => info.clone(),
                    _ => Vec::new(),
                };
                let now = self.now_ms;
                match hooks.finalize(now, node, scheme, version, &info, data) {
                    Ok(agreed) => self.become_peered(node, scheme, version, agreed, false),
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

    /// Ends an attempt, backing off from the node (NET-22).
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
            self.drop_source(&node, Sources::PEER);
        }
        self.event(Event::PeeringFailed { node, reason });
    }

    fn become_peered(
        &mut self,
        node: NodeId,
        scheme: SchemeId,
        version: u16,
        agreed: crate::hooks::Agreed,
        inbound: bool,
    ) {
        let now = self.now_ms;
        if self.peering.attempt == Some(node) {
            self.peering.attempt = None;
        }
        self.ensure(node, Sources::PEER);
        let Some(entry) = self.table.get_mut(&node) else {
            return;
        };
        if entry.peering().is_some() {
            // A peering stays as it was made (D12).
            return;
        }
        let since_ms = now;
        let fresh = true;
        entry.advertise = Advertise {
            on: true,
            on_since_ms: now,
            hold_until_ms: 0,
            flaps: 0,
        };
        entry.peer = PeerState::Peered(PeeringState {
            scheme,
            version,
            state: agreed.state,
            credit_out: agreed.credit_out,
            since_ms,
            inbound,
            last_seen_ms: now,
        });
        entry.peering_failures = 0;
        self.dirty.insert(node);
        self.open_channel(node);
        if fresh {
            self.auth.pin(node);
            self.stats.peerings += 1;
            self.gossip.heartbeat_soon(now);
            self.gossip.own_changed = true;
            self.gossip.routes_dirty = true;
            self.event(Event::Peered {
                node,
                scheme,
                inbound,
            });
        }
    }

    /// A peering request (NET-3, NET-4): the requester's link state must
    /// verify and be the sender's; it must be known, speak the scheme, and
    /// there must be room. Then its URL is probed before the answer.
    pub(super) fn peering_request(
        &mut self,
        hooks: &mut dyn PeeringHooks,
        accepted: Accepted,
        content: &[u8],
        inbound: InboundId,
    ) -> Inbound<'static> {
        let Ok(Content::PeeringRequest(request)) = Content::decode_exact(content) else {
            return Inbound::Reply(Reply::refuse(400, "malformed message"));
        };
        let sender = accepted.sender;
        if request.node_info.info.node_id != sender {
            return Inbound::Reply(Reply::refuse(400, "the link state is not the sender's"));
        }
        let Ok(key) = request.node_info.verify() else {
            return Inbound::Reply(Reply::refuse(400, "invalid link state"));
        };
        if request.node_info.info.sequence > self.now_ms.saturating_add(self.config.max_future_ms) {
            return Inbound::Reply(Reply::refuse(400, "the link state is from the future"));
        }
        if let Err(reason) = self.check_request(hooks, &sender, &request) {
            return Inbound::Reply(refused(&accepted, reason));
        }
        self.process_link_state(&request.node_info, None, Retransmit::Never);
        if let Some(entry) = self.table.get_mut(&sender) {
            entry.key.get_or_insert(key);
        }
        // A repeat, after an answer that got lost: the stored peering, as
        // it is (D12).
        if let Some(peering) = self
            .table
            .get(&sender)
            .and_then(Entry::active_peering)
            .filter(|peering| peering.scheme == request.scheme)
        {
            let answer = PeeringResponse::Accepted {
                version: peering.version,
                credit: 0,
                data: &[],
            };
            return Inbound::Reply(ok(&accepted, &answer.to_vec()));
        }
        let probe = Probe {
            inbound,
            accepted,
            sequence: request.node_info.info.sequence,
            purpose: ProbeFor::Peering(content.to_vec()),
        };
        self.start_probe(
            sender,
            key,
            request.node_info.info.url,
            alloc::boxed::Box::new(probe),
        )
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
        if !self.schemes.contains(&request.scheme) || hooks.versions(request.scheme).is_empty() {
            return Err(RefusalReason::UnsupportedScheme);
        }
        let Some(entry) = self.table.get(sender) else {
            return Err(RefusalReason::UnknownNode);
        };
        if entry.termination().is_some() || entry.refuse_until_ms > self.now_ms {
            return Err(RefusalReason::Terminating);
        }
        if !self.channel_room(sender) {
            return Err(RefusalReason::Capacity);
        }
        if let Some(peering) = entry.peering() {
            return if peering.scheme == request.scheme {
                Ok(())
            } else {
                Err(RefusalReason::Unspecified)
            };
        }
        common_version(hooks.versions(request.scheme), &request.versions)
            .ok_or(RefusalReason::NoCommonVersion)?;
        if matches!(entry.peer, PeerState::Requesting { .. }) {
            // Both asked at once: the lower node ID's request wins.
            if self.own < *sender {
                return Err(RefusalReason::Concurrent);
            }
            self.abandon_attempt(*sender);
        }
        let inbound = self.peers().filter(|(_, p)| p.inbound).count();
        if inbound >= self.config.max_inbound() || self.peers().count() >= self.config.max_peers {
            return Err(RefusalReason::Capacity);
        }
        Ok(())
    }

    pub(crate) fn abandon_attempt(&mut self, node: NodeId) {
        if self.peering.attempt == Some(node) {
            self.peering.attempt = None;
        }
        if let Some(entry) = self.table.get_mut(&node)
            && matches!(entry.peer, PeerState::Requesting { .. })
        {
            entry.peer = PeerState::None;
            self.drop_source(&node, Sources::PEER);
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
            return refused(accepted, RefusalReason::Unreachable);
        }
        let sender = accepted.sender;
        if let Err(reason) = self.check_request(hooks, &sender, &request) {
            return refused(accepted, reason);
        }
        let Some(version) = common_version(hooks.versions(request.scheme), &request.versions)
        else {
            return refused(accepted, RefusalReason::NoCommonVersion);
        };
        let mut response = Vec::new();
        let now = self.now_ms;
        let agreed = match hooks.accept(
            now,
            sender,
            request.scheme,
            version,
            request.data,
            &mut response,
        ) {
            Ok(agreed) => agreed,
            Err(reason) => return refused(accepted, reason),
        };
        self.become_peered(sender, request.scheme, version, agreed, true);
        let answer = PeeringResponse::Accepted {
            version,
            credit: 0,
            data: &response,
        };
        ok(accepted, &answer.to_vec())
    }
}

/// The highest version both speak.
fn common_version(ours: &[u16], theirs: &[u16]) -> Option<u16> {
    ours.iter().filter(|v| theirs.contains(v)).max().copied()
}

fn refused(accepted: &Accepted, reason: RefusalReason) -> Reply {
    ok(accepted, &PeeringResponse::Refused(reason).to_vec())
}
