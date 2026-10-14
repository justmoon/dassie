//! The peer protocol in the core: what peering needs from the schemes and
//! the ledger, peers as connector endpoints, routes, and packets over the
//! peer protocol (spec 7 §4.7).

use std::collections::BTreeMap;

use dassie_connector::EndpointId;
use dassie_identity::NodeId;
use dassie_ilp::ErrorCode;
use dassie_ledger::{Ledger, LedgerId, MAX_TOTAL, Name};
use dassie_peer::wire::{RefusalReason, SchemeId};
use dassie_peer::{Agreed, Declined, Event, InboundId, Output as PeerOutput, PeeringHooks};

use super::ledgers::ActiveScheme;
use super::{Core, Output};
use crate::log::target;

/// What peering needs, over the core's schemes and ledger.
pub(super) struct Hooks<'a> {
    pub schemes: &'a mut BTreeMap<Name, ActiveScheme>,
    pub ledger: &'a mut Ledger,
    pub credit: u64,
}

impl Hooks<'_> {
    fn scheme(&mut self, scheme: SchemeId) -> Option<&mut ActiveScheme> {
        let name = Name::new(scheme.as_str()).ok()?;
        self.schemes.get_mut(&name)
    }

    /// Opens the peer's accounts and extends it our credit, once (ACC-5).
    fn open(&mut self, now_ms: u64, peer: NodeId, ledger: LedgerId) -> Result<(), ()> {
        let name = Name::new(peer.as_str()).map_err(|_| ())?;
        self.ledger
            .open_peer(now_ms, ledger, name, self.credit)
            .map(|_| ())
            .map_err(|_| ())
    }
}

/// A node's name in the ledger.
fn name(peer: NodeId) -> Option<Name> {
    Name::new(peer.as_str()).ok()
}

impl PeeringHooks for Hooks<'_> {
    fn versions(&self, scheme: SchemeId) -> &'static [u16] {
        Name::new(scheme.as_str())
            .ok()
            .and_then(|name| self.schemes.get(&name))
            .map_or(&[], |active| active.scheme.supported_versions())
    }

    fn credit(&self, _: SchemeId) -> u64 {
        self.credit
    }

    fn peering_info(&mut self, scheme: SchemeId, out: &mut Vec<u8>) -> Result<(), Declined> {
        let active = self.scheme(scheme).ok_or(Declined)?;
        active.scheme.peering_info(out);
        Ok(())
    }

    fn create_request(
        &mut self,
        peer: NodeId,
        scheme: SchemeId,
        info: &[u8],
        out: &mut Vec<u8>,
    ) -> Result<(), Declined> {
        let peer = name(peer).ok_or(Declined)?;
        let active = self.scheme(scheme).ok_or(Declined)?;
        active
            .scheme
            .create_peering_request(peer, info, out)
            .map_err(|_| Declined)
    }

    fn accept(
        &mut self,
        now_ms: u64,
        peer: NodeId,
        scheme: SchemeId,
        _version: u16,
        credit_in: u64,
        data: &[u8],
        response: &mut Vec<u8>,
    ) -> Result<Agreed, RefusalReason> {
        if credit_in > MAX_TOTAL {
            return Err(RefusalReason::SchemeRefused);
        }
        let peer_name = name(peer).ok_or(RefusalReason::Unspecified)?;
        let credit_out = self.credit;
        let active = self
            .scheme(scheme)
            .ok_or(RefusalReason::UnsupportedScheme)?;
        let ledger = active.scheme.ledger();
        let state = active
            .scheme
            .accept_peering_request(peer_name, data, response)
            .map_err(|_| RefusalReason::SchemeRefused)?;
        self.open(now_ms, peer, ledger)
            .map_err(|()| RefusalReason::Capacity)?;
        Ok(Agreed { state, credit_out })
    }

    fn finalize(
        &mut self,
        now_ms: u64,
        peer: NodeId,
        scheme: SchemeId,
        _version: u16,
        credit_in: u64,
        info: &[u8],
        data: &[u8],
    ) -> Result<Agreed, Declined> {
        if credit_in > MAX_TOTAL {
            return Err(Declined);
        }
        let peer_name = name(peer).ok_or(Declined)?;
        let credit_out = self.credit;
        let active = self.scheme(scheme).ok_or(Declined)?;
        let ledger = active.scheme.ledger();
        let state = active
            .scheme
            .finalize_peering_request(peer_name, info, data)
            .map_err(|_| Declined)?;
        self.open(now_ms, peer, ledger).map_err(|()| Declined)?;
        Ok(Agreed { state, credit_out })
    }
}

/// The network as the owner sees it.
#[derive(Clone, Debug)]
pub struct NetworkView {
    /// Our node ID.
    pub own: NodeId,
    /// `test` or `g`.
    pub allocation_scheme: &'static str,
    /// The node table.
    pub nodes: Vec<dassie_peer::NodeView>,
    /// Our peerings.
    pub peers: Vec<(NodeId, dassie_peer::Peering)>,
    /// Our standing with the bootstrap nodes.
    pub registration: Vec<dassie_peer::RegistrationStatus>,
}

impl Core {
    /// The network as the owner sees it.
    pub fn network(&self) -> NetworkView {
        NetworkView {
            own: *self.peer.own(),
            allocation_scheme: self.allocation_scheme,
            nodes: self.peer.nodes(),
            peers: self
                .peer
                .peers()
                .map(|(id, peering)| (*id, peering.clone()))
                .collect(),
            registration: self.peer.registration_status(),
        }
    }

    /// Runs `f` on the peer node with the hooks over the core.
    pub(super) fn with_peer<T>(
        &mut self,
        f: impl FnOnce(&mut dassie_peer::PeerNode, &mut Hooks<'_>, u64) -> T,
    ) -> T {
        let now = self.now_ms;
        let mut hooks = Hooks {
            schemes: &mut self.schemes,
            ledger: &mut self.ledger,
            credit: self.peer_credit,
        };
        f(&mut self.peer, &mut hooks, now)
    }

    /// An inbound peer request: its reply goes out once the batch is on
    /// disk, as a peer accepting a peering must have recorded it first.
    pub fn peer_inbound(&mut self, inbound: InboundId, body: &[u8]) {
        let outcome = self.with_peer(|peer, hooks, now| {
            match peer.handle_inbound(now, hooks, inbound, body) {
                dassie_peer::Inbound::Reply(reply) => (Some(reply), None),
                dassie_peer::Inbound::Deferred => (None, None),
                dassie_peer::Inbound::Packet {
                    from,
                    request_id,
                    packet,
                    reply,
                } => (Some(reply), Some((from, request_id, packet.to_vec()))),
            }
        });
        let (reply, packet) = outcome;
        if let Some(reply) = reply {
            self.outputs_peer.push(Output::PeerReply { inbound, reply });
        }
        if let Some((from, request_id, packet)) = packet {
            self.peer_packet(from, request_id, &packet);
        }
        self.after_peer();
    }

    /// The result of one of our peer requests.
    pub fn peer_response(
        &mut self,
        id: dassie_peer::RequestId,
        result: Result<dassie_peer::Response, dassie_peer::Failure>,
    ) {
        self.with_peer(|peer, hooks, now| peer.handle_response(now, hooks, id, result));
        self.after_peer();
    }

    /// Changes what our link state says: a new one follows (NET-6).
    pub fn set_own_info(&mut self, info: dassie_peer::OwnInfo) {
        let now = self.now_ms;
        self.peer.set_own_info(now, info);
        self.after_peer();
    }

    /// Runs the peer node's timers, if due.
    pub(super) fn peer_timers(&mut self) {
        let now = self.now_ms;
        if self.peer.poll_timeout().is_some_and(|at| at <= now) {
            self.with_peer(|peer, hooks, now| peer.handle_timeout(now, hooks));
            self.after_peer();
        }
    }

    /// Tells the peer node which schemes are active, in a fixed order.
    pub(super) fn update_peer_schemes(&mut self) {
        let schemes: Vec<SchemeId> = self
            .schemes
            .keys()
            .filter_map(|name| SchemeId::parse(name.as_str()))
            .collect();
        let now = self.now_ms;
        self.peer.set_schemes(now, schemes);
    }

    /// Takes in what the peer node did: its requests and replies to hand
    /// over, and its events.
    pub(super) fn after_peer(&mut self) {
        while let Some(output) = self.peer.poll_output() {
            self.outputs_peer.push(match output {
                PeerOutput::Request(request) => Output::PeerRequest(request),
                PeerOutput::Reply { inbound, reply } => Output::PeerReply { inbound, reply },
            });
        }
        while let Some(event) = self.peer.poll_event() {
            self.peer_event(event);
        }
        self.take_journal();
    }

    fn peer_event(&mut self, event: Event) {
        match event {
            Event::Peered {
                node,
                scheme,
                inbound,
            } => {
                tracing::info!(target: target::PEER, %node, %scheme, inbound, "peered");
                if let Err(error) = self.add_peer_endpoint(node) {
                    tracing::error!(target: target::PEER, %node, %error, "the peer has no endpoint");
                }
                self.update_routes();
            }
            Event::RoutesChanged => self.update_routes(),
            Event::PacketFailed {
                peer,
                request_id,
                definite,
            } => {
                // Only a packet that surely did not arrive is failed now:
                // one that may have is left to expire, so both sides' books
                // agree.
                if definite && let Some(&endpoint) = self.peer_endpoints.get(&peer) {
                    self.connector.fail_forwarded(
                        self.now_ms,
                        EndpointId::Peer(endpoint),
                        request_id,
                        ErrorCode::T01_PEER_UNREACHABLE,
                        "the peer could not be reached",
                    );
                }
            }
            Event::NodeAdded(node) => tracing::debug!(target: target::PEER, %node, "node added"),
            Event::NodeRemoved(node) => {
                tracing::debug!(target: target::PEER, %node, "node removed")
            }
            Event::Registered(node) => tracing::debug!(target: target::PEER, %node, "registration"),
            Event::RegisteredWith(node) => {
                tracing::debug!(target: target::PEER, %node, "registered with a bootstrap node");
            }
            Event::PeeringFailed { node, reason } => {
                tracing::debug!(target: target::PEER, %node, reason, "peering failed");
            }
        }
    }

    /// Makes a peer a connector endpoint, on its scheme's ledger, at the
    /// address ILDCP will tell it.
    pub(super) fn add_peer_endpoint(&mut self, node: NodeId) -> Result<(), String> {
        if self.peer_endpoints.contains_key(&node) {
            return Ok(());
        }
        let peering = self.peer.peering_of(&node).ok_or("not peered")?;
        let scheme = Name::new(peering.scheme.as_str()).map_err(|e| e.to_string())?;
        let ledger = self
            .schemes
            .get(&scheme)
            .map(|active| active.scheme.ledger())
            .ok_or("the peer's scheme is not active")?;
        let name = Name::new(node.as_str()).map_err(|e| e.to_string())?;
        let account = self
            .ledger
            .find(&dassie_ledger::AccountPath::new(
                ledger,
                dassie_ledger::AccountName::AssetsInterledger(name),
            ))
            .ok_or("the peer's account is missing")?;
        let id = self.next_peer_endpoint;
        self.next_peer_endpoint += 1;
        let address = format!("{}.das.{node}", self.allocation_scheme);
        let first = self.rng.next_u32();
        self.connector
            .add_peer(EndpointId::Peer(id), &address, account, ledger, first)
            .map_err(|e| e.to_string())?;
        self.peer_endpoints.insert(node, id);
        self.endpoint_peers.insert(id, node);
        Ok(())
    }

    /// Gives the connector the peer node's routes, as prefixes and peer
    /// endpoints.
    pub(super) fn update_routes(&mut self) {
        let scheme = self.allocation_scheme;
        let routes: Vec<(String, Vec<EndpointId>)> = self
            .peer
            .routes()
            .iter()
            .map(|(destination, hops)| {
                let endpoints = hops
                    .iter()
                    .filter_map(|hop| self.peer_endpoints.get(hop))
                    .map(|id| EndpointId::Peer(*id))
                    .collect();
                (format!("{scheme}.das.{destination}"), endpoints)
            })
            .filter(|(_, hops): &(String, Vec<EndpointId>)| !hops.is_empty())
            .collect();
        let routes = routes
            .iter()
            .map(|(prefix, hops)| (prefix.as_str(), hops.as_slice()));
        if let Err(error) = self.connector.set_routes(routes) {
            tracing::error!(target: target::ROUTING, ?error, "routes not taken");
        }
    }
}
