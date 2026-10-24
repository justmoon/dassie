//! Inbound requests (spec/areas/peer-protocol.md#message-set,
//! spec/areas/peer-protocol.md#statuses): decoding, authentication, and
//! what each kind gets, with HTTP statuses that mean something (NET-11,
//! NET-27, NET-28). Registrations and peering requests wait on a probe of
//! the requester's URL.

use alloc::boxed::Box;
use alloc::string::String;
use alloc::vec::Vec;

use dassie_identity::{NodeId, PublicKey};
use dassie_oer::Decode;

use super::gossip::{Processed, Retransmit};
use super::{PeerNode, Problem, Purpose};
use crate::auth::{Accepted, AuthError};
use crate::hooks::PeeringHooks;
use crate::io::{
    Event, Inbound, InboundId, Output, PacketAnswer, Reply, SettlementAnswer, VERDICT_ACCEPTED,
};
use crate::table::{Entry, Network, Registration, Sources};
use crate::wire::{
    Content, Envelope, EnvelopeError, LinkStateResponse, MAX_CLAIM_BYTES, SchemeId, SignedNodeInfo,
    encode_expiry, encode_link_states, encode_octets, encode_response, encode_settling,
};

/// The ILP packet type of a Prepare.
pub(super) const PREPARE_TYPE: u8 = 12;

/// The ILP packet type of a Fulfill.
pub(super) const FULFILL_TYPE: u8 = 13;

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
    /// Handles an inbound request, `body` being the HTTP body.
    pub fn handle_inbound<'a>(
        &mut self,
        now_ms: u64,
        hooks: &mut dyn PeeringHooks,
        id: InboundId,
        body: &'a [u8],
        source: Option<core::net::IpAddr>,
    ) -> Inbound<'a> {
        self.advance(now_ms);
        let inbound = self.decode_inbound(hooks, id, body, source);
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
        body: &'a [u8],
        source: Option<core::net::IpAddr>,
    ) -> Inbound<'a> {
        let envelope = match Envelope::decode(body) {
            Ok(envelope) => envelope,
            Err(EnvelopeError::Version(_)) => return refuse(400, "unsupported version"),
            Err(EnvelopeError::Malformed(_)) => return refuse(400, "malformed message"),
        };
        let Ok(content) = Content::decode_exact(envelope.content) else {
            return refuse(400, "malformed message");
        };
        if envelope.sender == self.own {
            return refuse(400, "the sender is the recipient");
        }
        let network = source.map(Network::of);
        let accepted = match self
            .auth
            .accept(self.now_ms, &envelope, content.kind(), network)
        {
            Ok(accepted) => accepted,
            Err(AuthError::Busy) => return refuse(503, "busy"),
            Err(error) => return refuse(401, auth_reason(error)),
        };
        if accepted.is_authenticated() {
            self.saw(&accepted.sender);
        }
        match content {
            Content::InterledgerPacket { request_id, packet } => {
                self.packet_request(hooks, accepted, request_id, packet, None)
            }
            Content::HubPacket {
                request_id,
                packet,
                commitment,
            } => self.packet_request(hooks, accepted, request_id, packet, Some(commitment)),
            Content::Registration(info) => self.registration_request(accepted, &info, id, network),
            Content::PeeringRequest(_) => {
                self.peering_request(hooks, accepted, envelope.content, id, network)
            }
            Content::Settlement {
                scheme,
                amount,
                data,
                sequence,
            } => self.settlement_request(hooks, &accepted, scheme, amount, data, sequence),
            Content::SettlementMessage { scheme, message } => {
                if self.peering_of(&accepted.sender).is_none() {
                    self.notice_one_sided(accepted.sender);
                    return refuse(409, "not peered");
                }
                Inbound::SchemeMessage {
                    from: accepted.sender,
                    scheme,
                    message,
                    reply: ok(&accepted, &[]),
                }
            }
            Content::PeeringTermination(reason) => {
                Inbound::Reply(self.termination_request(&accepted, reason))
            }
            Content::SettlementRequest { scheme } => self.settle_on_request(accepted, scheme),
            // Names are not taken yet (spec/roadmap.md#names): refused as
            // not acted upon, so that a registrant takes nothing as done.
            Content::NameCommit(_)
            | Content::NameUnseal { .. }
            | Content::NameRenew(_)
            | Content::NameListHashRequest
            | Content::NameListRequest => refuse(409, "names are not built"),
            other => Inbound::Reply(self.answer(hooks, &accepted, other)),
        }
    }

    /// A peer's request that we settle on `scheme`
    /// (spec/decisions/own-fulfills.md).
    fn settle_on_request(&mut self, accepted: Accepted, scheme: SchemeId) -> Inbound<'static> {
        // Only an active peering on that scheme is settled on request;
        // nothing is settled for one being ended
        // (spec/decisions/ending-a-peering.md).
        let peering = self
            .table
            .get(&accepted.sender)
            .and_then(Entry::active_peering);
        if peering.is_none_or(|peering| peering.scheme != scheme) {
            self.notice_one_sided(accepted.sender);
            return refuse(409, "not peered on that scheme");
        }
        // Nor for one we are ending, which the peer does not know yet: it
        // is answered as a request that settles nothing.
        if peering.is_some_and(|peering| peering.depeering.is_some()) {
            return Inbound::Reply(ok(&accepted, &encode_settling(0)));
        }
        Inbound::SettlementRequest {
            from: accepted.sender,
            scheme,
            answer: SettlementAnswer(Box::new(accepted)),
        }
    }

    /// An ILP packet. A Prepare needs a peering that is not ending, and
    /// comes with a commitment exactly when the peering's scheme uses them
    /// (spec/designs/settlement-hubs.md#4-peering-over-a-hub). A response
    /// also comes from a node we hold a settlement channel with, peered or
    /// not: a node of 0.1 asks for verdicts after the peering ended, and is
    /// told it was accepted (spec/decisions/own-fulfills.md).
    fn packet_request<'a>(
        &mut self,
        hooks: &dyn PeeringHooks,
        accepted: Accepted,
        request_id: u32,
        packet: &'a [u8],
        commitment: Option<&'a [u8]>,
    ) -> Inbound<'a> {
        let prepare = packet.first() == Some(&PREPARE_TYPE);
        if commitment.is_some() && !prepare {
            return refuse(400, "only a Prepare comes with a commitment");
        }
        let entry = self.table.get(&accepted.sender);
        let peering = entry.and_then(Entry::peering);
        let hub = peering.is_some_and(|peering| hooks.uses_commitments(peering.scheme));
        if prepare {
            if peering.is_none() {
                self.notice_one_sided(accepted.sender);
                return refuse(409, "not peered");
            }
            // Refused as unpeered Prepares are: the sender reads 409 as
            // not acted upon, and rejects the packet at once.
            if commitment.is_some() != hub {
                return refuse(409, commitment_refusal(hub));
            }
            // A peering being ended takes no new Prepares: refused here,
            // they leave nothing to answer, so the drain can finish.
            if entry.is_some_and(|entry| entry.termination().is_some()) {
                return refuse(409, "the peering is ending");
            }
        } else if peering.is_none() && !self.channels.contains_key(&accepted.sender) {
            return refuse(409, "not peered");
        }
        Inbound::Packet {
            from: accepted.sender,
            request_id,
            packet,
            commitment,
            answer: PacketAnswer {
                accepted: Box::new(accepted),
                fulfill: packet.first() == Some(&FULFILL_TYPE),
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
                    _ if accepted.is_authenticated() && info.info.node_id == sender => {
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
                    _ if accepted.is_authenticated() => self.notice_one_sided(sender),
                    _ => {}
                }
                ok(accepted, &[])
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
                ok(accepted, &body)
            }
            Content::NodeListHashRequest => {
                self.rebuild_served();
                let (_, hash) = self.lists.served();
                ok(accepted, &hash)
            }
            Content::NodeListRequest => {
                self.rebuild_served();
                let (served, _) = self.lists.served();
                ok(accepted, served)
            }
            Content::PeeringInfoRequest { scheme } => {
                let mut info = Vec::new();
                if hooks.peering_info(scheme, &mut info).is_err() {
                    return Reply::refuse(409, "the scheme is not active");
                }
                ok(accepted, &encode_octets(&info))
            }
            Content::InterledgerPacket { .. }
            | Content::HubPacket { .. }
            | Content::Registration(_)
            | Content::PeeringRequest(_)
            | Content::Settlement { .. }
            | Content::SettlementMessage { .. }
            | Content::SettlementRequest { .. }
            | Content::PeeringTermination(_)
            | Content::NameCommit(_)
            | Content::NameUnseal { .. }
            | Content::NameRenew(_)
            | Content::NameListHashRequest
            | Content::NameListRequest => Reply::refuse(400, "unexpected"),
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
        if info.info.node_id != sender {
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
        // A renewal from the URL that was probed: anything else is probed
        // again (spec/areas/network.md#registration).
        let renewal = self.table.get(&sender).is_some_and(|entry| {
            entry
                .registration
                .as_ref()
                .is_some_and(|registration| registration.url == info.info.url)
        });
        if renewal {
            return Inbound::Reply(self.register(&accepted, info, network, key));
        }
        if let Some(refusal) = self.registration_refusal(&sender, info.info.url, network) {
            return Inbound::Reply(refusal);
        }
        let probe = Probe {
            inbound,
            accepted,
            sequence: info.info.sequence,
            network,
            purpose: ProbeFor::Registration(info.bytes.to_vec()),
        };
        self.start_probe(sender, key, info.info.url, Box::new(probe))
    }

    /// Why a new registration is refused, if it is.
    fn registration_refusal(
        &mut self,
        sender: &NodeId,
        url: &str,
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
        let host = url_host(url);
        let others = self
            .table
            .iter()
            .filter(|(id, _)| *id != sender)
            .filter_map(|(_, entry)| entry.registration.as_ref());
        let mut from_network = 0;
        for registration in others {
            if url_host(&registration.url) == host {
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

    /// Asks `node` at `url` for its link state, authenticated, before
    /// answering `probe.inbound`: it proves the URL is the node's, and
    /// that the node can be reached (spec/decisions/membership-economics.md,
    /// spec/decisions/peering-lifecycle.md). Every probe spends a token of
    /// all the node's probes. The network the request came from also pays one
    /// of its own share while the probe is in flight, and gets it back if the
    /// probe succeeds, so failing probes from one network cannot spend them
    /// all (spec/areas/network.md#registration).
    pub(crate) fn start_probe(
        &mut self,
        node: NodeId,
        key: PublicKey,
        url: &str,
        probe: Box<Probe>,
    ) -> Inbound<'static> {
        if self.deferred >= self.config.max_deferred {
            return refuse(503, "busy");
        }
        let host = String::from(url_host(url));
        if self
            .probing
            .iter()
            .any(|(id, probed)| *id == node || *probed == host)
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
        self.probing.push((node, host));
        let content = Content::LinkStateRequest(alloc::vec![node]);
        let timeout = self.config.poll_timeout_ms;
        let sent = self.send_to(
            node,
            key,
            String::from(url),
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
                        signed.info.node_id == node
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
                    Ok(key) => self.register(&probe.accepted, &info, probe.network, key),
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
                url: String::from(info.info.url),
                network,
            });
        }
        // Its renewals must get through however many strangers write to us
        // (spec/areas/peer-protocol.md#replay-protection).
        self.auth.pin(sender);
        self.dirty.insert(sender);
        self.lists.served_changed = true;
        self.event(Event::Registered(sender));
        ok(accepted, &encode_expiry(expires_ms))
    }

    /// Sends to a node not (yet) reachable through the table.
    pub(crate) fn send_to(
        &mut self,
        to: NodeId,
        key: PublicKey,
        url: String,
        content: &Content<'_>,
        purpose: Purpose,
        timeout_ms: u64,
    ) -> Option<crate::io::RequestId> {
        self.send_via(to, key, url, content, purpose, timeout_ms)
    }
}

impl SettlementAnswer {
    /// The answer: `settling` on its way to the peer, in the scheme
    /// ledger's units.
    #[must_use]
    pub fn reply(self, settling: u64) -> Reply {
        ok(&self.0, &encode_settling(settling))
    }
}

impl PacketAnswer {
    /// The acknowledgement: for a Fulfill, the verdict that accepts it,
    /// then `extra`; for a Prepare or Reject, an empty body.
    ///
    /// # Panics
    ///
    /// If `extra` is not empty where it cannot go, on a packet other than a
    /// Fulfill on a peering over a hub
    /// ([`takes_extra`](Self::takes_extra)), or is longer than
    /// [`MAX_CLAIM_BYTES`]: what follows the verdict is the node's own.
    #[must_use]
    pub fn reply(self, extra: &[u8]) -> Reply {
        assert!(
            extra.is_empty() || self.takes_extra(),
            "bytes after the verdict only on a Fulfill over a hub"
        );
        assert!(extra.len() <= MAX_CLAIM_BYTES, "a claim too long");
        if !self.fulfill {
            return ok(&self.accepted, &[]);
        }
        let mut body = Vec::with_capacity(1 + extra.len());
        body.push(VERDICT_ACCEPTED);
        body.extend_from_slice(extra);
        ok(&self.accepted, &body)
    }
}

/// Why a Prepare is refused for its commitment: missing where the
/// peering's scheme uses them, or there where it does not.
const fn commitment_refusal(hub: bool) -> &'static str {
    if hub {
        "a Prepare over a hub needs its commitment"
    } else {
        "the peering takes no commitments"
    }
}

/// A 200 with `body`, MACed if the request was authenticated.
pub(crate) fn ok(accepted: &Accepted, body: &[u8]) -> Reply {
    Reply {
        status: 200,
        body: encode_response(accepted.response_mac(body).as_ref(), body),
    }
}

fn refuse(status: u16, reason: &str) -> Inbound<'static> {
    Inbound::Reply(Reply::refuse(status, reason))
}

/// A URL's host and port: everything after `https://`, since valid node
/// URLs have nothing else, with the default port left out, so one host is
/// one host.
fn url_host(url: &str) -> &str {
    let authority = url.strip_prefix("https://").unwrap_or(url);
    authority.strip_suffix(":443").unwrap_or(authority)
}

const fn auth_reason(error: AuthError) -> &'static str {
    match error {
        AuthError::Missing => "authentication required",
        AuthError::Unsupported => "unsupported authentication",
        AuthError::BadKey | AuthError::WrongSender => "the key is not the sender's",
        AuthError::Stale => "the timestamp is too far from now",
        AuthError::Replayed => "replayed",
        AuthError::BadMac => "authentication failed",
        AuthError::Busy => "busy",
    }
}
