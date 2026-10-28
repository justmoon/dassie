//! Inbound requests (spec/areas/peer-protocol.md#message-set,
//! spec/areas/peer-protocol.md#statuses): decoding, and what each kind
//! gets from the node its connection proved, with statuses that tell a
//! refusal from a failure. Registrations and peering requests wait on a
//! probe of the requester's URL; name messages go to the registrar
//! (`names.rs`).

use alloc::boxed::Box;
use alloc::vec::Vec;

use dassie_identity::{NodeId, NodeIdentity, PublicKey};
use dassie_oer::Decode;

use super::gossip::{Processed, Retransmit};
use super::{PeerNode, Problem, Purpose};
use crate::hooks::PeeringHooks;
use crate::io::{Event, Inbound, InboundId, Output, PacketAnswer, Reply, SettlementAnswer};
use crate::table::{Entry, Network, Registration, Sources};
use crate::wire::{
    Content, Endpoint, LinkStateResponse, MAX_CLAIM_BYTES, SchemeId, SignedNodeInfo, encode_expiry,
    encode_link_states, encode_octets, encode_settling,
};

/// The ILP packet type of a Prepare.
pub(super) const PREPARE_TYPE: u8 = 12;

/// The ILP packet type of a Fulfill.
pub(super) const FULFILL_TYPE: u8 = 13;

/// A request taken in: from whom, by the key its QUIC connection proved
/// (spec/areas/peer-protocol.md#authentication).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Accepted {
    /// The sender: the node of the key.
    pub sender: NodeId,
    /// The key it proved.
    pub key: PublicKey,
}

/// What an inbound request waits on a probe for.
#[derive(Clone, Debug)]
pub(crate) enum ProbeFor {
    /// A registration, with the registrant's link state.
    Registration(Vec<u8>),
    /// A peering request, with its content.
    Peering(Vec<u8>),
}

/// An inbound request waiting on a probe of its sender's URL.
#[derive(Clone, Debug)]
pub(crate) struct Probe {
    pub inbound: InboundId,
    pub accepted: Accepted,
    /// The sequence the sender's link state must have at least.
    pub sequence: u64,
    /// The network the request came from, which pays for the probe if it
    /// fails.
    pub network: Option<Network>,
    pub purpose: ProbeFor,
}

impl PeerNode {
    /// Handles an inbound request, `body` being the stream's body, from the
    /// node whose identity `from` its connection proved, at the address
    /// `source` where one counts (spec/areas/peer-protocol.md#authentication).
    pub fn handle_inbound<'a>(
        &mut self,
        now_ms: u64,
        hooks: &mut dyn PeeringHooks,
        id: InboundId,
        from: NodeIdentity,
        body: &'a [u8],
        source: Option<core::net::IpAddr>,
    ) -> Inbound<'a> {
        self.advance(now_ms);
        let accepted = Accepted {
            sender: from.node_id(),
            key: from.key(),
        };
        let inbound = self.decode_inbound(hooks, id, accepted, body, source);
        if let Inbound::Reply(reply) = &inbound
            && !reply.is_ok()
        {
            self.stats.inbound_refused += 1;
        }
        inbound
    }

    fn decode_inbound<'a>(
        &mut self,
        hooks: &mut dyn PeeringHooks,
        id: InboundId,
        accepted: Accepted,
        body: &'a [u8],
        source: Option<core::net::IpAddr>,
    ) -> Inbound<'a> {
        let Ok(content) = Content::decode_exact(body) else {
            return refuse(400, "malformed message");
        };
        if accepted.sender == self.own {
            return refuse(400, "the sender is the recipient");
        }
        let network = source.map(Network::of);
        self.saw(&accepted.sender);
        match content {
            Content::InterledgerPacket { request_id, packet } => {
                self.response_request(hooks, accepted, request_id, packet)
            }
            Content::MethodPacket {
                request_id,
                scheme,
                packet,
                commitment,
            } => self.prepare_request(hooks, accepted, request_id, scheme, packet, commitment),
            Content::MethodAdd { scheme, data } => {
                Inbound::Reply(self.method_add_request(hooks, &accepted, scheme, data))
            }
            Content::MethodDrop { scheme } => {
                Inbound::Reply(self.method_drop_request(&accepted, scheme))
            }
            Content::Registration(info) => self.registration_request(accepted, &info, id, network),
            Content::PeeringRequest(_) => self.peering_request(hooks, accepted, body, id, network),
            Content::Settlement {
                scheme,
                amount,
                data,
                sequence,
            } => self.settlement_request(hooks, &accepted, scheme, amount, data, sequence),
            Content::PeeringTermination(reason) => {
                Inbound::Reply(self.termination_request(&accepted, reason))
            }
            Content::SettlementRequest { scheme } => self.settle_on_request(accepted, scheme),
            Content::NameCommit(commitment) => {
                Inbound::Reply(self.name_commit(&accepted, commitment, network))
            }
            Content::NameUnseal { name, salt } => {
                Inbound::Reply(self.name_unseal(&accepted, name, &salt))
            }
            Content::NameRenew(name) => Inbound::Reply(self.name_renew(&accepted, name)),
            Content::Subscription { subscribe } => {
                Inbound::Reply(self.subscription_request(hooks, &accepted, subscribe))
            }
            Content::Snapshot(snapshot) => {
                Inbound::Reply(self.snapshot_request(hooks, &accepted, snapshot))
            }
            other => Inbound::Reply(self.answer(hooks, &accepted, other)),
        }
    }

    /// A peer's request that we settle on `scheme`
    /// (spec/decisions/own-fulfills.md).
    fn settle_on_request(&mut self, accepted: Accepted, scheme: SchemeId) -> Inbound<'static> {
        // Only an active peering with a method on that scheme is settled on
        // request; nothing is settled for one being ended
        // (spec/decisions/ending-a-peering.md).
        let peering = self
            .table
            .get(&accepted.sender)
            .and_then(Entry::active_peering)
            .map(|peering| (peering.has(scheme), peering.depeering.is_some()));
        let Some((has, ending)) = peering else {
            self.notice_one_sided(accepted.sender);
            return refuse(409, "not peered on that scheme");
        };
        if !has {
            // Its side of the method is left over, as when our drop of it
            // was lost with a restart: it is told, as for a Prepare on it
            // (spec/areas/network.md#one-sided-peerings).
            self.queue_drop(accepted.sender, scheme);
            return refuse(409, "not peered on that scheme");
        }
        // Nor for one we are ending, which the peer does not know yet: it
        // is answered as a request that settles nothing.
        if ending {
            return Inbound::Reply(ok(&encode_settling(0)));
        }
        Inbound::SettlementRequest {
            from: accepted.sender,
            scheme,
            answer: SettlementAnswer(accepted.sender),
        }
    }

    /// An ILP Fulfill or Reject, in an `interledgerPacket`: it needs a
    /// peering, ending or not, since one ends only once nothing is in flight
    /// with the peer, so a response from a node not peered answers nothing
    /// we wait for. A Prepare travels only in a `methodPacket`, so one here
    /// is malformed (spec/areas/peer-protocol.md#message-set).
    fn response_request<'a>(
        &mut self,
        hooks: &dyn PeeringHooks,
        accepted: Accepted,
        request_id: u32,
        packet: &'a [u8],
    ) -> Inbound<'a> {
        if packet.first() == Some(&PREPARE_TYPE) {
            return refuse(400, "a Prepare travels in a methodPacket");
        }
        let Some(peering) = self.peering_of(&accepted.sender) else {
            return refuse(409, "not peered");
        };
        // A Fulfill names no method: its reply may carry a claim on a
        // peering with one over a hub, or for a Prepare of ours that went
        // on one dropped since, and the node knows which its Prepare went
        // on.
        let hub = peering
            .schemes()
            .any(|scheme| hooks.uses_commitments(scheme))
            || hooks.forwarded_over_hub(accepted.sender, request_id);
        Inbound::Packet {
            from: accepted.sender,
            request_id,
            packet,
            scheme: None,
            commitment: None,
            answer: PacketAnswer {
                peer: accepted.sender,
                fulfill: packet.first() == Some(&FULFILL_TYPE),
                hub,
            },
        }
    }

    /// An ILP Prepare on a method of our peering, in a `methodPacket`: it
    /// needs a peering that is not ending, with a method on `scheme`, and
    /// comes with no commitment unless the method's scheme uses them
    /// (spec/decisions/settlement-methods.md,
    /// spec/designs/settlement-hubs.md#4-peering-over-a-hub). On a method
    /// over a hub it may come without one, as between a member and a peer
    /// with no account in the hub yet
    /// (spec/designs/settlement-hubs.md#5-joining): whether the two promise
    /// is the node's to judge, by its scheme, so the commitment is handed
    /// on only when there is one. Each refusal is a 409, as for a Prepare
    /// from a node not peered: the sender reads it as not acted upon, and
    /// rejects the packet at once.
    fn prepare_request<'a>(
        &mut self,
        hooks: &dyn PeeringHooks,
        accepted: Accepted,
        request_id: u32,
        scheme: SchemeId,
        packet: &'a [u8],
        commitment: &'a [u8],
    ) -> Inbound<'a> {
        if packet.first() != Some(&PREPARE_TYPE) {
            return refuse(400, "only a Prepare travels in a methodPacket");
        }
        let entry = self.table.get(&accepted.sender);
        let Some(peering) = entry.and_then(Entry::peering) else {
            self.notice_one_sided(accepted.sender);
            return refuse(409, "not peered");
        };
        if !peering.has(scheme) {
            // Its side of the method is left over, as when our drop of it
            // was lost with a restart: it is told, as a node not peered is
            // (spec/areas/network.md#one-sided-peerings).
            if entry.is_some_and(|entry| entry.active_peering().is_some()) {
                self.queue_drop(accepted.sender, scheme);
            }
            return refuse(409, "no method on that scheme");
        }
        let hub = hooks.uses_commitments(scheme);
        if !hub && !commitment.is_empty() {
            return refuse(409, "the method takes no commitments");
        }
        // A peering being ended takes no new Prepares: refused here, they
        // leave nothing to answer, so the drain can finish.
        if entry.is_some_and(|entry| entry.termination().is_some()) {
            return refuse(409, "the peering is ending");
        }
        Inbound::Packet {
            from: accepted.sender,
            request_id,
            packet,
            scheme: Some(scheme),
            commitment: (hub && !commitment.is_empty()).then_some(commitment),
            answer: PacketAnswer {
                peer: accepted.sender,
                fulfill: false,
                hub,
            },
        }
    }

    /// The kinds answered at once.
    fn answer(
        &mut self,
        hooks: &mut dyn PeeringHooks,
        accepted: &Accepted,
        content: Content<'_>,
    ) -> Reply {
        match content {
            Content::LinkStateUpdate(info) => {
                let sender = accepted.sender;
                match self.process_link_state(&info, Some(sender), Retransmit::Scheduled) {
                    Processed::Invalid(_) => return Reply::refuse(400, "invalid link state"),
                    _ if info.node_id == sender => {
                        self.heard_from(&sender);
                        // A core node also sends its own link state to the
                        // bootstrap nodes it is not peered with, even one
                        // it is asking for a peering: only one that lists
                        // us presumes a peering. A periphery node's lists
                        // nobody, and goes only to its uplinks, in
                        // heartbeats and when it issues a new one (to one
                        // it asks, it goes in the peering request), so it
                        // presumes one (spec/areas/network.md#one-sided-peerings).
                        if info.info.is_periphery()
                            || info.info.neighbors().any(|neighbor| neighbor == self.own)
                        {
                            self.notice_one_sided(sender);
                        }
                    }
                    // Another node's link state is forwarded only to peers.
                    _ => self.notice_one_sided(sender),
                }
                ok(&[])
            }
            Content::LinkStateRequest(ids) => {
                // In the order asked, each ID once: the answer lines up
                // with the request.
                let mut ids = ids;
                let mut seen = alloc::collections::BTreeSet::new();
                ids.retain(|id| seen.insert(*id));
                // A periphery node's link state is served by nobody but
                // itself: only its uplinks are to see it
                // (spec/areas/network.md#periphery-nodes).
                let own = self.own;
                let found: Vec<Option<&[u8]>> = ids
                    .iter()
                    .map(|id| {
                        let link = self.table.get(id)?.link.as_ref()?;
                        (*id == own || !link.periphery).then_some(link.bytes.as_slice())
                    })
                    .collect();
                let body = encode_link_states(&found);
                ok(&body)
            }
            Content::NodeListHashRequest => {
                self.rebuild_served();
                let (_, hash) = self.lists.served();
                ok(&hash)
            }
            Content::NodeListRequest => {
                self.rebuild_served();
                let (served, _) = self.lists.served();
                ok(served)
            }
            Content::NameListHashRequest => self.name_list(true),
            Content::NameListRequest => self.name_list(false),
            Content::PeeringInfoRequest { scheme } => {
                let mut info = Vec::new();
                if hooks.peering_info(scheme, &mut info).is_err() {
                    return Reply::refuse(409, "the scheme is not active");
                }
                ok(&encode_octets(&info))
            }
            Content::InterledgerPacket { .. }
            | Content::Registration(_)
            | Content::PeeringRequest(_)
            | Content::Settlement { .. }
            | Content::SettlementRequest { .. }
            | Content::PeeringTermination(_)
            | Content::NameCommit(_)
            | Content::NameUnseal { .. }
            | Content::NameRenew(_)
            | Content::MethodPacket { .. }
            | Content::MethodAdd { .. }
            | Content::MethodDrop { .. }
            | Content::Subscription { .. }
            | Content::Snapshot(_) => Reply::refuse(400, "unexpected"),
        }
    }

    /// A registration (spec/decisions/membership-economics.md): verified, and
    /// limited until it has a fee. A new one, or one at a new URL, waits on a
    /// probe of that URL.
    fn registration_request(
        &mut self,
        accepted: Accepted,
        info: &SignedNodeInfo<'_>,
        inbound: InboundId,
        network: Option<Network>,
    ) -> Inbound<'static> {
        let sender = accepted.sender;
        if !self
            .bootstrap
            .iter()
            .any(|bootstrap| bootstrap.id == self.own)
        {
            return refuse(409, "not a bootstrap node");
        }
        if info.node_id != sender {
            return refuse(400, "the link state is not the sender's");
        }
        let Ok(key) = info.verify() else {
            return refuse(400, "invalid link state");
        };
        if info.info.sequence > self.now_ms.saturating_add(self.config.max_future_ms) {
            return refuse(400, "the link state is from the future");
        }
        // A periphery node is in no node list
        // (spec/areas/network.md#periphery-nodes).
        if info.info.is_periphery() {
            return refuse(409, "a periphery node does not register");
        }
        // A link state that verifies, but a periphery node's, has a URL.
        let Some(endpoint) = Endpoint::of_url(info.info.url) else {
            return refuse(400, "invalid link state");
        };
        // A renewal from the endpoint that was probed: anything else is
        // probed again (spec/areas/network.md#registration).
        let renewal = self.table.get(&sender).is_some_and(|entry| {
            entry
                .registration
                .as_ref()
                .is_some_and(|registration| registration.endpoint == endpoint)
        });
        if renewal {
            return Inbound::Reply(self.register(&accepted, info, network, key.key()));
        }
        if let Some(refusal) = self.registration_refusal(&sender, &endpoint, network) {
            return Inbound::Reply(refusal);
        }
        let probe = Probe {
            inbound,
            accepted,
            sequence: info.info.sequence,
            network,
            purpose: ProbeFor::Registration(info.bytes.to_vec()),
        };
        self.start_probe(sender, endpoint, Box::new(probe))
    }

    /// Why a new registration is refused, if it is.
    fn registration_refusal(
        &mut self,
        sender: &NodeId,
        endpoint: &Endpoint,
        network: Option<Network>,
    ) -> Option<Reply> {
        let registered = self
            .table
            .values()
            .filter(|entry| entry.registration.is_some())
            .count();
        if registered >= self.config.max_registrations {
            return Some(Reply::refuse(503, "registrations are full"));
        }
        let others = self
            .table
            .iter()
            .filter(|(id, _)| *id != sender)
            .filter_map(|(_, entry)| entry.registration.as_ref());
        let mut from_network = 0;
        for registration in others {
            if registration.endpoint == *endpoint {
                return Some(Reply::refuse(
                    409,
                    "another node is registered at that host",
                ));
            }
            if network.is_some() && registration.network == network {
                from_network += 1;
            }
        }
        if from_network >= self.config.max_registrations_per_network {
            return Some(Reply::refuse(
                409,
                "too many registrations from that network",
            ));
        }
        // A token is spent when a registration succeeds, not on attempts
        // whose probe fails.
        if !self
            .registrations
            .has_token(self.config.registration_rate, self.now_ms)
        {
            return Some(Reply::refuse(429, "too many new registrations"));
        }
        None
    }

    /// Asks `node` at `endpoint` for its link state, on a connection pinned
    /// to its key, before answering `probe.inbound`: it proves the endpoint
    /// is the node's, and that the node can be reached
    /// (spec/decisions/membership-economics.md,
    /// spec/decisions/peering-lifecycle.md). Every probe spends a token of
    /// all the node's probes. The network the request came from also pays one
    /// of its own share while the probe is in flight, and gets it back if the
    /// probe succeeds, so failing probes from one network cannot spend them
    /// all (spec/areas/network.md#registration).
    pub(crate) fn start_probe(
        &mut self,
        node: NodeId,
        endpoint: Endpoint,
        probe: Box<Probe>,
    ) -> Inbound<'static> {
        if self.deferred >= self.config.max_deferred {
            return refuse(503, "busy");
        }
        if self
            .probing
            .iter()
            .any(|(id, probed)| *id == node || *probed == endpoint)
        {
            return refuse(429, "a probe of that node or host is in flight");
        }
        let share = self.config.probe_rate_per_network;
        let network = probe.network;
        if let Some(network) = network
            && !self.network_probes.has_token(network, share, self.now_ms)
        {
            return refuse(429, "too many probes from that network");
        }
        if !self.probes.take(self.config.probe_rate, self.now_ms) {
            return refuse(429, "too many probes");
        }
        if let Some(network) = network {
            let taken = self.network_probes.take(network, share, self.now_ms);
            assert!(taken, "the network's token was there");
        }
        self.probing.push((node, endpoint.clone()));
        let content = Content::LinkStateRequest(alloc::vec![node]);
        let timeout = self.config.poll_timeout_ms;
        let sent = self.send_via(
            node,
            Some(endpoint),
            &content,
            Purpose::Probe(probe),
            timeout,
        );
        if sent.is_none() {
            // Nothing went out: the probe is not in flight, and spent nothing.
            let popped = self.probing.pop();
            assert!(
                popped.is_some_and(|(id, _)| id == node),
                "the entry just pushed"
            );
            self.probes.give_back(self.config.probe_rate);
            if let Some(network) = network {
                self.network_probes.give_back(network, share, self.now_ms);
            }
            return refuse(503, "busy");
        }
        self.deferred += 1;
        Inbound::Deferred
    }

    /// The answer to a probe: the node's link state, at least as new as the
    /// one it sent.
    pub(super) fn on_probe(
        &mut self,
        hooks: &mut dyn PeeringHooks,
        node: NodeId,
        probe: Box<Probe>,
        result: Result<Vec<u8>, Problem>,
    ) {
        let probe = *probe;
        self.deferred = self.deferred.saturating_sub(1);
        self.probing.retain(|(id, _)| *id != node);
        let reachable = result.ok().is_some_and(|body| {
            LinkStateResponse::decode_exact(&body).is_ok_and(|response| {
                response
                    .0
                    .first()
                    .and_then(Option::as_ref)
                    .is_some_and(|signed| {
                        signed.node_id == node
                            && signed.info.sequence >= probe.sequence
                            && signed.verify().is_ok()
                    })
            })
        });
        if reachable && let Some(network) = probe.network {
            let share = self.config.probe_rate_per_network;
            self.network_probes.give_back(network, share, self.now_ms);
        }
        let reply = match probe.purpose {
            ProbeFor::Registration(bytes) => match SignedNodeInfo::decode_exact(&bytes) {
                Ok(_)
                    if reachable
                        && !self
                            .registrations
                            .take(self.config.registration_rate, self.now_ms) =>
                {
                    Reply::refuse(429, "too many new registrations")
                }
                Ok(info) if reachable => match info.verify() {
                    Ok(key) => self.register(&probe.accepted, &info, probe.network, key.key()),
                    Err(_) => Reply::refuse(400, "invalid link state"),
                },
                _ => Reply::refuse(409, "not reachable at its URL"),
            },
            ProbeFor::Peering(content) => {
                self.complete_peering(hooks, &probe.accepted, &content, reachable)
            }
        };
        self.outputs.push_back(Output::Reply {
            inbound: probe.inbound,
            reply,
        });
    }

    /// Registers the sender, or renews its registration.
    fn register(
        &mut self,
        accepted: &Accepted,
        info: &SignedNodeInfo<'_>,
        network: Option<Network>,
        key: PublicKey,
    ) -> Reply {
        let sender = accepted.sender;
        let Some(endpoint) = Endpoint::of_url(info.info.url) else {
            return Reply::refuse(400, "invalid link state");
        };
        if !self.ensure(sender, Sources::REGISTERED) {
            return Reply::refuse(503, "the table is full");
        }
        if let Processed::Invalid(_) = self.process_link_state(info, None, Retransmit::Never) {
            return Reply::refuse(400, "invalid link state");
        }
        let now = self.now_ms;
        let expires_ms = now.saturating_add(self.config.registration_ttl_ms);
        if let Some(entry) = self.table.get_mut(&sender) {
            entry.key.get_or_insert(key);
            let registered_ms = entry.registration.as_ref().map_or(now, |r| r.registered_ms);
            entry.registration = Some(Registration {
                registered_ms,
                renewed_ms: now,
                expires_ms,
                endpoint,
                network,
            });
        }
        self.dirty.insert(sender);
        self.lists.served_changed = true;
        self.event(Event::Registered(sender));
        ok(&encode_expiry(expires_ms))
    }
}

impl SettlementAnswer {
    /// The answer: `settling` on its way to the peer, in the scheme
    /// ledger's units.
    #[must_use]
    pub fn reply(self, settling: u64) -> Reply {
        ok(&encode_settling(settling))
    }
}

impl PacketAnswer {
    /// The acknowledgement: empty, but for a Fulfill on a peering with a
    /// method over a hub, whose carries `claim`, the payer's.
    ///
    /// # Panics
    ///
    /// If `claim` is not empty where it cannot go, on a packet other than a
    /// Fulfill on a peering with a method over a hub
    /// ([`takes_claim`](Self::takes_claim)),
    /// or is longer than [`MAX_CLAIM_BYTES`]: the claim is the node's own.
    #[must_use]
    pub fn reply(self, claim: &[u8]) -> Reply {
        assert!(
            claim.is_empty() || self.takes_claim(),
            "a claim only on a Fulfill over a hub"
        );
        assert!(claim.len() <= MAX_CLAIM_BYTES, "a claim too long");
        ok(claim)
    }
}

/// A 200 with `body`.
pub(crate) fn ok(body: &[u8]) -> Reply {
    Reply {
        status: 200,
        body: body.to_vec(),
    }
}

fn refuse(status: u16, reason: &str) -> Inbound<'static> {
    Inbound::Reply(Reply::refuse(status, reason))
}
