//! The peer protocol in the core: what peering needs from the schemes and
//! the ledger, peers as connector endpoints, routes, and packets over the
//! peer protocol (spec/areas/network.md#peering).

use std::collections::BTreeMap;

use dassie_connector::{Connector, EndpointId};
use dassie_identity::NodeId;
use dassie_ilp::ErrorCode;
use dassie_ledger::{AccountName, AccountPath, Ledger, LedgerId, Name};
use dassie_peer::wire::{RefusalReason, SchemeId, TerminationReason};
use dassie_peer::{
    Agreed, BanError, Declined, Event, InboundId, Output as PeerOutput, PeeringHooks, Reply,
};

use super::events::{PeerEvent, SettlementEvent};
use super::ledgers::ActiveScheme;
use super::{Core, EngineError, Output};
use crate::store::PeerSettings;

/// What peering needs, over the core's schemes, ledger and connector.
pub(super) struct Hooks<'a> {
    pub schemes: &'a mut BTreeMap<Name, ActiveScheme>,
    pub ledger: &'a mut Ledger,
    /// The credit a junior extends to its senior at first (D20).
    pub credit: u64,
    /// The credit a senior extends to its junior at first.
    pub senior_credit: u64,
    pub connector: &'a Connector,
    pub endpoints: &'a BTreeMap<NodeId, u64>,
}

impl Hooks<'_> {
    fn scheme(&mut self, scheme: SchemeId) -> Option<&mut ActiveScheme> {
        let name = Name::new(scheme.as_str()).ok()?;
        self.schemes.get_mut(&name)
    }

    /// The credit we extend to `peer` on `ledger`: what was posted to its
    /// trust account, if it has one from an earlier peering, which is not
    /// posted again (ACC-5); otherwise `start`, what we would extend now.
    fn credit_for(&self, peer: NodeId, ledger: LedgerId, start: u64) -> u64 {
        Name::new(peer.as_str())
            .ok()
            .and_then(|name| {
                self.ledger
                    .find(&AccountPath::new(ledger, AccountName::ContraTrust(name)))
            })
            .and_then(|id| self.ledger.account(id))
            .map_or(start, |account| {
                dassie_settlement::policy::trust(account.balance())
            })
    }

    /// Raises the credit we extend to `peer` on `ledger` by `amount`.
    fn raise(
        &mut self,
        now_ms: u64,
        peer: NodeId,
        ledger: LedgerId,
        amount: u64,
    ) -> Result<(), ()> {
        let name = Name::new(peer.as_str()).map_err(|_| ())?;
        super::growth::raise_trust(self.ledger, now_ms, ledger, name, amount).map_err(|_| ())
    }

    /// Opens the peer's accounts and extends it `start`, once (ACC-5).
    fn open(&mut self, now_ms: u64, peer: NodeId, ledger: LedgerId, start: u64) -> Result<(), ()> {
        let name = Name::new(peer.as_str()).map_err(|_| ())?;
        self.ledger
            .open_peer(now_ms, ledger, name, start)
            .map(|_| ())
            .map_err(|_| ())
    }
}

/// What an inbound request gave, owned, for after the peer node is done.
enum Inbound {
    Reply(Reply),
    Deferred,
    Packet(NodeId, u32, Vec<u8>, Reply),
    Settlement(NodeId, SchemeId, u64, Vec<u8>, u64, Reply),
    SchemeMessage(NodeId, SchemeId, Vec<u8>, Reply),
    SettlementRequest(NodeId, SchemeId, dassie_peer::SettlementAnswer),
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
        data: &[u8],
        response: &mut Vec<u8>,
    ) -> Result<Agreed, RefusalReason> {
        let peer_name = name(peer).ok_or(RefusalReason::Unspecified)?;
        let active = self
            .scheme(scheme)
            .ok_or(RefusalReason::UnsupportedScheme)?;
        let ledger = active.scheme.ledger();
        // We accept, so we are the senior: the requester gets our credit
        // only once the peering earned it (D20).
        let start = self.senior_credit;
        let credit_out = self.credit_for(peer, ledger, start);
        let active = self
            .scheme(scheme)
            .ok_or(RefusalReason::UnsupportedScheme)?;
        let state = active
            .scheme
            .accept_peering_request(peer_name, data, response)
            .map_err(|_| RefusalReason::SchemeRefused)?;
        self.open(now_ms, peer, ledger, start)
            .map_err(|()| RefusalReason::Capacity)?;
        // Only now is the peering made: a request refused takes none of
        // the scheme's room.
        let active = self
            .scheme(scheme)
            .ok_or(RefusalReason::UnsupportedScheme)?;
        active.scheme.on_peered(peer_name, &state);
        Ok(Agreed { state, credit_out })
    }

    fn finalize(
        &mut self,
        now_ms: u64,
        peer: NodeId,
        scheme: SchemeId,
        _version: u16,
        info: &[u8],
        data: &[u8],
        ended_for_room: bool,
    ) -> Result<Agreed, Declined> {
        let peer_name = name(peer).ok_or(Declined)?;
        let active = self.scheme(scheme).ok_or(Declined)?;
        let ledger = active.scheme.ledger();
        // Our request was accepted, so we are the junior, and extend the
        // default credit line (D20).
        let start = self.credit;
        let credit_out = self.credit_for(peer, ledger, start);
        let active = self.scheme(scheme).ok_or(Declined)?;
        let state = active
            .scheme
            .finalize_peering_request(peer_name, info, data)
            .map_err(|_| Declined)?;
        self.open(now_ms, peer, ledger, start)
            .map_err(|()| Declined)?;
        // A trust account from an earlier peering may hold less, from a
        // time we were the senior: a junior extends at least the default,
        // unless we ended a peering with it for room, which keeps what it
        // had (D33).
        let credit_out = if credit_out < start && !ended_for_room {
            self.raise(now_ms, peer, ledger, start - credit_out)
                .map_err(|()| Declined)?;
            start
        } else {
            credit_out
        };
        let active = self.scheme(scheme).ok_or(Declined)?;
        active.scheme.on_peered(peer_name, &state);
        Ok(Agreed { state, credit_out })
    }

    fn drained(&self, peer: NodeId) -> bool {
        self.endpoints
            .get(&peer)
            .is_none_or(|id| self.connector.pending_for(EndpointId::Peer(*id)) == 0)
    }

    fn settling_to(&self, peer: NodeId) -> bool {
        self.ledger
            .pending_transfers()
            .any(|transfer| super::settlement::settles_to(self.ledger, transfer, &peer))
    }
}

/// A peering as the owner sees it.
#[derive(Clone, Debug)]
pub struct PeerView {
    /// The peer.
    pub id: NodeId,
    /// The peering.
    pub peering: dassie_peer::Peering,
    /// Its termination, while it is being ended.
    pub termination: Option<dassie_peer::Termination>,
    /// Whether it is listed as our neighbor.
    pub advertised: bool,
    /// Whether it was heard from recently enough to count as online.
    pub online: bool,
    /// What we owe the peer net; negative when it owes us.
    pub owed: Option<i128>,
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
    pub peers: Vec<PeerView>,
    /// Our standing with the bootstrap nodes.
    pub registration: Vec<dassie_peer::RegistrationStatus>,
    /// Settlement channels, with nodes peered with now or before.
    pub channels: Vec<(NodeId, dassie_peer::Channel)>,
    /// Banned nodes.
    pub bans: Vec<NodeId>,
    /// Routes to the node's own endpoints other than peers: address,
    /// endpoint and ledger.
    pub local_routes: Vec<(String, EndpointId, dassie_ledger::LedgerId)>,
    /// Whether we are on a majority of the bootstrap nodes' lists.
    pub member: bool,
}

impl Core {
    /// The owner's peer settings.
    pub const fn peer_settings(&self) -> PeerSettings {
        self.peer_settings
    }

    /// Changes the owner's peer settings.
    ///
    /// # Errors
    ///
    /// They are out of range.
    pub fn set_peer_settings(&mut self, settings: PeerSettings) -> Result<(), EngineError> {
        if !settings.is_valid() {
            return Err(EngineError::Invalid(
                "the fee and the spread must be at most 100 000 ppm".into(),
            ));
        }
        self.connector.set_charges(settings.charges());
        self.peer_settings = settings;
        self.batch.peer_settings = Some(settings);
        self.changes.network = true;
        Ok(())
    }

    /// The network as the owner sees it.
    pub fn network(&self) -> NetworkView {
        NetworkView {
            own: *self.peer.own(),
            allocation_scheme: self.allocation_scheme,
            nodes: self.peer.nodes(),
            peers: self
                .peer
                .nodes()
                .into_iter()
                .filter_map(|view| {
                    let mut peering = view.peering?;
                    // The credit we extend now, which grows (D20); the
                    // peering keeps what it started with.
                    if let Some(credit) = self.credit_out(view.id, peering.scheme) {
                        peering.credit_out = credit;
                    }
                    Some(PeerView {
                        id: view.id,
                        owed: self.owed(view.id, peering.scheme),
                        peering,
                        termination: view.termination,
                        advertised: view.advertised,
                        online: view.online,
                    })
                })
                .collect(),
            registration: self.peer.registration_status(),
            channels: self
                .peer
                .channels()
                .map(|(id, channel)| (*id, channel.clone()))
                .collect(),
            bans: self.peer.bans().copied().collect(),
            local_routes: self
                .connector
                .endpoints()
                .filter(|(id, _, _)| !id.is_peer())
                .map(|(id, address, ledger)| (address.to_string(), id, ledger))
                .collect(),
            member: self.peer.majority().contains(self.peer.own()),
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
            senior_credit: self.senior_credit,
            connector: &self.connector,
            endpoints: &self.peer_endpoints,
        };
        f(&mut self.peer, &mut hooks, now)
    }

    /// An inbound peer request: its reply goes out once the batch is on
    /// disk, as a peer accepting a peering must have recorded it first.
    pub fn peer_inbound(
        &mut self,
        inbound: InboundId,
        body: &[u8],
        source: Option<std::net::IpAddr>,
    ) {
        let outcome = self.with_peer(|peer, hooks, now| {
            match peer.handle_inbound(now, hooks, inbound, body, source) {
                dassie_peer::Inbound::Reply(reply) => Inbound::Reply(reply),
                dassie_peer::Inbound::Deferred => Inbound::Deferred,
                dassie_peer::Inbound::Packet {
                    from,
                    request_id,
                    packet,
                    reply,
                } => Inbound::Packet(from, request_id, packet.to_vec(), reply),
                dassie_peer::Inbound::Settlement {
                    from,
                    scheme,
                    amount,
                    data,
                    sequence,
                    reply,
                } => Inbound::Settlement(from, scheme, amount, data.to_vec(), sequence, reply),
                dassie_peer::Inbound::SchemeMessage {
                    from,
                    scheme,
                    message,
                    reply,
                } => Inbound::SchemeMessage(from, scheme, message.to_vec(), reply),
                dassie_peer::Inbound::SettlementRequest {
                    from,
                    scheme,
                    answer,
                } => Inbound::SettlementRequest(from, scheme, answer),
            }
        });
        let reply = match outcome {
            Inbound::Reply(reply) => Some(reply),
            Inbound::Deferred => None,
            Inbound::Packet(from, request_id, packet, reply) => {
                self.peer_packet(from, request_id, &packet);
                Some(reply)
            }
            Inbound::Settlement(from, scheme, amount, data, sequence, reply) => {
                Some(self.settlement_from(from, scheme, amount, &data, sequence, reply))
            }
            Inbound::SchemeMessage(from, scheme, message, reply) => {
                self.scheme_message(from, scheme, &message);
                Some(reply)
            }
            Inbound::SettlementRequest(from, scheme, answer) => {
                let settling = self.settlement_requested(from, scheme);
                Some(answer.reply(settling))
            }
        };
        if let Some(reply) = reply {
            self.outputs_peer.push(Output::PeerReply { inbound, reply });
        }
        self.after_peer();
    }

    /// Applies a peer's settlement, once: refused with 400 if the books
    /// cannot take it, which changes nothing.
    fn settlement_from(
        &mut self,
        from: NodeId,
        scheme: SchemeId,
        amount: u64,
        data: &[u8],
        sequence: u64,
        reply: Reply,
    ) -> Reply {
        match self.incoming_settlement(from, scheme, amount, data) {
            Ok(()) => {
                self.peer.settlement_applied(self.now_ms, &from, sequence);
                reply
            }
            Err(error) => {
                self.report(SettlementEvent::PeerSettlementRefused {
                    from,
                    error: error.clone(),
                });
                Reply::refuse(400, "the settlement cannot be booked")
            }
        }
    }

    /// Ends the peering with `node`, as its owner.
    ///
    /// # Errors
    ///
    /// There is no peering with it.
    pub fn terminate_peering(&mut self, node: NodeId) -> Result<(), EngineError> {
        let now = self.now_ms;
        if !self.peer.terminate(now, &node, TerminationReason::Owner) {
            return Err(EngineError::NotFound("no peering with that node".into()));
        }
        self.after_peer();
        Ok(())
    }

    /// Registers with the bootstrap node `node` now.
    ///
    /// # Errors
    ///
    /// It is not one of our bootstrap nodes, we have no link state yet, or
    /// a registration with it is on its way.
    pub fn register_with_bootstrap(&mut self, node: NodeId) -> Result<(), EngineError> {
        let now = self.now_ms;
        self.peer
            .register_now(now, &node)
            .map_err(|error| match error {
                dassie_peer::RegisterError::NotBootstrap => {
                    EngineError::NotFound("not a bootstrap node".into())
                }
                dassie_peer::RegisterError::NoLinkState => EngineError::Precondition(
                    "the node has no link state to register yet: set its hostname".into(),
                ),
                dassie_peer::RegisterError::InFlight => {
                    EngineError::Precondition("a registration with it is on its way".into())
                }
            })?;
        self.changes.network = true;
        self.after_peer();
        Ok(())
    }

    /// Bans `node`, ending a peering with it.
    ///
    /// # Errors
    ///
    /// It is us, or the list is full.
    pub fn ban_node(&mut self, node: NodeId) -> Result<(), EngineError> {
        let now = self.now_ms;
        self.peer.ban(now, node).map_err(|error| match error {
            BanError::Ourselves => EngineError::Invalid("a node cannot ban itself".into()),
            BanError::Full => EngineError::Exhausted("too many banned nodes".into()),
        })?;
        self.after_peer();
        Ok(())
    }

    /// Lifts a ban.
    ///
    /// # Errors
    ///
    /// The node is not banned.
    pub fn unban_node(&mut self, node: &NodeId) -> Result<(), EngineError> {
        let now = self.now_ms;
        if !self.peer.unban(now, node) {
            return Err(EngineError::NotFound("the node is not banned".into()));
        }
        self.after_peer();
        Ok(())
    }

    /// The banned nodes.
    pub fn bans(&self) -> Vec<NodeId> {
        self.peer.bans().copied().collect()
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
        // Becoming a periphery node, or a core node again, changes how
        // packets from peers are routed.
        self.update_routes();
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

    /// Tells the peer node which schemes it may peer on, in a fixed order:
    /// the active ones that have seen their ledger (D31) and have not
    /// stopped. Before a scheme first synced, a peer's settlement would be
    /// booked as the owner's deposit; once it stopped, it would be refused.
    /// So the node neither offers, asks for nor accepts a peering on it;
    /// peerings already made stay. The list is made from the schemes
    /// active now, each time: the original's started from those active at
    /// its start, so of two changes the second undid the first (ACC-11).
    pub(super) fn update_peer_schemes(&mut self) {
        let schemes: Vec<SchemeId> = self
            .schemes
            .iter()
            .filter(|(_, active)| active.failed.is_none() && active.scheme.has_synced())
            .filter_map(|(name, _)| SchemeId::parse(name.as_str()))
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
        self.changes.network = true;
        self.report(event.clone());
        match event {
            Event::Peered { node, .. } => {
                if let Err(error) = self.add_peer_endpoint(node) {
                    self.report(PeerEvent::NoEndpoint { node, error });
                }
                self.update_routes();
            }
            Event::RoutesChanged => self.update_routes(),
            // Asked again at the next check, not a minute later: a junior
            // that had not yet taken in our acceptance pays in advance at
            // once (D23), and a request that never arrived does not use up
            // the grace before a peer short of room is withdrawn (D12).
            Event::SettlementRequestRefused { peer }
            | Event::SettlementRequestNotDelivered { peer } => {
                self.settle.asked.remove(&peer);
            }
            Event::PacketFailed {
                peer,
                request_id,
                definite,
            } => {
                // Only a packet that surely did not arrive is failed now:
                // one that may have is left to expire, so a peer that
                // fulfilled it does not bear its loss.
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
            other => self.lifecycle_event(other),
        }
    }

    /// What a peering's lifecycle and its settlements bring about.
    fn lifecycle_event(&mut self, event: Event) {
        match event {
            // Its new Prepares are refused while we collect, ours still go
            // (D28).
            Event::Depeering { peer, .. } => {
                if let Some(&endpoint) = self.peer_endpoints.get(&peer) {
                    self.connector
                        .set_refusing(EndpointId::Peer(endpoint), true);
                }
            }
            Event::Terminating { peer, .. } => {
                if let Some(&endpoint) = self.peer_endpoints.get(&peer) {
                    self.connector
                        .set_draining(EndpointId::Peer(endpoint), true);
                }
                self.update_routes();
            }
            Event::PeeringEnded { peer } => {
                self.remove_peer_endpoint(&peer);
                self.update_routes();
            }
            _ => {}
        }
    }

    /// Takes a peer's endpoint away. Nothing is in flight with it by now,
    /// since the peering ended only once drained.
    fn remove_peer_endpoint(&mut self, peer: &NodeId) {
        if let Some(endpoint) = self.peer_endpoints.remove(peer) {
            self.endpoint_peers.remove(&endpoint);
            let now = self.now_ms;
            // What was in flight to it is due now: the step expires it as
            // it finishes, rather than here, which the expiry could reach
            // again through the peer node (D16: no recursion).
            self.connector
                .remove_endpoint(now, EndpointId::Peer(endpoint));
        }
    }

    /// Makes a peer a connector endpoint, on its scheme's ledger, at the
    /// address ILDCP will tell it: its own `<scheme>.das.<id>`, which
    /// routes reach as they reach any node's; or, for a downlink, its child
    /// address beneath ours, `<our address>.<id>`, which reaches it through
    /// us (spec/areas/connector.md#addresses-and-endpoints). The original
    /// gave a peer that was not a member an address beneath its own that
    /// nothing routed to (NET-21), and stored the addresses its peers gave
    /// it without reading them (NET-23).
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
        let address = if self.peer.is_downlink(&node) {
            format!("{}.{node}", self.connector.own_address())
        } else {
            format!("{}.das.{node}", self.allocation_scheme)
        };
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
            .map(|(destination, route)| {
                let endpoints = route
                    .first_hops
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
        // The table holds a route per node the peer node knows
        // (`check_bounds`), none of them below our own address.
        let taken = self.connector.set_routes(routes);
        assert!(taken.is_ok(), "routes not taken: {taken:?}");
        let children = self
            .peer
            .is_periphery()
            .then(|| self.child_addresses(false));
        // One per uplink, each beneath another node's address.
        let taken = self.connector.set_periphery(children.as_deref());
        assert!(taken.is_ok(), "child addresses not taken: {taken:?}");
    }

    /// A periphery node's addresses beneath its uplinks, each
    /// `<uplink's address>.<our ID>`, which ILDCP tells it there
    /// (spec/areas/connector.md#a-periphery-node): of every peering, or,
    /// if `connected`, of those whose uplink is confirmed now, best first.
    pub(super) fn child_addresses(&self, connected: bool) -> Vec<String> {
        let own = self.peer.own();
        self.peer
            .core_peers()
            .filter(|(id, _)| !connected || self.peer.is_advertised(id))
            .map(|(id, _)| format!("{}.das.{id}.{own}", self.allocation_scheme))
            .collect()
    }

    /// Where a periphery node is paid now: beneath an uplink it is
    /// connected to, if any. `None` on a core node, which is paid at its
    /// own address.
    pub(super) fn receiving_address(&self) -> Option<String> {
        if !self.peer.is_periphery() {
            return None;
        }
        let connected = self.child_addresses(true).into_iter().next();
        connected.or_else(|| self.child_addresses(false).into_iter().next())
    }
}
