//! Inbound requests (spec 3 §8, spec 7 §4.7): decoding, authentication,
//! and what each kind gets, with HTTP statuses that mean something
//! (NET-11, NET-27, NET-28). Registrations and peering requests wait on a
//! probe of the requester's URL.

use alloc::boxed::Box;
use alloc::string::String;
use alloc::vec::Vec;

use dassie_identity::{NodeId, PublicKey};
use dassie_oer::Decode;

use super::gossip::{Processed, Retransmit};
use super::{PeerNode, Problem, Purpose};
use crate::auth::{Accepted, AuthError};
use crate::hooks::PeeringHooks;
use crate::io::{Event, Inbound, InboundId, Output, Reply};
use crate::table::{Registration, Sources};
use crate::wire::{
    Content, Envelope, EnvelopeError, LinkStateResponse, SignedNodeInfo, encode_expiry,
    encode_link_states, encode_octets, encode_response,
};

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
    ) -> Inbound<'a> {
        self.advance(now_ms);
        let inbound = self.decode_inbound(hooks, id, body);
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
    ) -> Inbound<'a> {
        let envelope = match Envelope::decode(body) {
            Ok(envelope) => envelope,
            Err(EnvelopeError::Version(_)) => return refuse(400, "unsupported version"),
            Err(EnvelopeError::Malformed(_)) => return refuse(400, "malformed message"),
        };
        let Ok(content) = Content::decode_exact(envelope.content) else {
            return refuse(400, "malformed message");
        };
        let required = content.kind().requires_authentication();
        let accepted = match self.auth.accept(self.now_ms, &envelope, required) {
            Ok(accepted) => accepted,
            Err(AuthError::Busy) => return refuse(503, "busy"),
            Err(error) => return refuse(401, auth_reason(error)),
        };
        if accepted.is_authenticated() {
            self.saw(&accepted.sender);
        }
        match content {
            Content::InterledgerPacket { request_id, packet } => {
                if self.peering_of(&accepted.sender).is_none() {
                    return refuse(409, "not peered");
                }
                Inbound::Packet {
                    from: accepted.sender,
                    request_id,
                    packet,
                    reply: ok(&accepted, &[]),
                }
            }
            Content::Registration(info) => self.registration_request(accepted, &info, id),
            Content::PeeringRequest(_) => {
                self.peering_request(hooks, accepted, envelope.content, id)
            }
            other => Inbound::Reply(self.answer(hooks, &accepted, other)),
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
                    }
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
                let found: Vec<Option<&[u8]>> = ids
                    .iter()
                    .map(|id| {
                        let link = self.table.get(id)?.link.as_ref()?;
                        Some(link.bytes.as_slice())
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
            Content::Settlement { .. } | Content::SettlementMessage { .. } => {
                if self.peering_of(&accepted.sender).is_none() {
                    return Reply::refuse(409, "not peered");
                }
                Reply::refuse(503, "settlement between peers comes later")
            }
            Content::InterledgerPacket { .. }
            | Content::Registration(_)
            | Content::PeeringRequest(_) => Reply::refuse(400, "unexpected"),
        }
    }

    /// A registration (D4): verified, and limited until it has a fee. A new
    /// one, or one at a new URL, waits on a probe of that URL.
    fn registration_request(
        &mut self,
        accepted: Accepted,
        info: &SignedNodeInfo<'_>,
        inbound: InboundId,
    ) -> Inbound<'static> {
        let sender = accepted.sender;
        if info.info.node_id != sender {
            return refuse(400, "the link state is not the sender's");
        }
        let Ok(key) = info.verify() else {
            return refuse(400, "invalid link state");
        };
        let renewal = self.table.get(&sender).is_some_and(|entry| {
            entry.registration.is_some()
                && entry
                    .link
                    .as_ref()
                    .is_some_and(|link| link.url == info.info.url)
        });
        if renewal {
            return Inbound::Reply(self.register(&accepted, info));
        }
        if let Some(refusal) = self.registration_refusal(&sender, info.info.url) {
            return Inbound::Reply(refusal);
        }
        let probe = Probe {
            inbound,
            accepted,
            sequence: info.info.sequence,
            purpose: ProbeFor::Registration(info.bytes.to_vec()),
        };
        self.start_probe(sender, key, info.info.url, Box::new(probe))
    }

    /// Why a new registration is refused, if it is.
    fn registration_refusal(&mut self, sender: &NodeId, url: &str) -> Option<Reply> {
        let registered = self
            .table
            .values()
            .filter(|entry| entry.registration.is_some())
            .count();
        if registered >= self.config.max_registrations {
            return Some(Reply::refuse(503, "registrations are full"));
        }
        let host = url_host(url);
        let taken = self.table.iter().any(|(id, entry)| {
            id != sender
                && entry.registration.is_some()
                && entry
                    .link
                    .as_ref()
                    .is_some_and(|link| url_host(&link.url) == host)
        });
        if taken {
            return Some(Reply::refuse(
                409,
                "another node is registered at that host",
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
    /// that the node can be reached (D4, D12).
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
                Ok(info) if reachable => self.register(&probe.accepted, &info),
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
    fn register(&mut self, accepted: &Accepted, info: &SignedNodeInfo<'_>) -> Reply {
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
            let registered_ms = entry.registration.map_or(now, |r| r.registered_ms);
            entry.registration = Some(Registration {
                registered_ms,
                renewed_ms: now,
                expires_ms,
            });
        }
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
/// URLs have nothing else.
fn url_host(url: &str) -> &str {
    url.strip_prefix("https://").unwrap_or(url)
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
