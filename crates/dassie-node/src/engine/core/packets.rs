//! Packets: from BTP clients and peers into the connector, to the STREAM
//! receivers, and back.

use std::collections::BTreeMap;

use dassie_connector::{EndpointId, Forwarded, Outcome, Responded, Upstream};
use dassie_identity::SeedPath;
use dassie_ilp::{Address, ErrorCode, Prepare, Timestamp};
use dassie_ledger::LedgerId;
use dassie_oer::Decode;
use dassie_stream::{AssetDetails, ConnectionId, Event, Policy, Server, ServerConfig};

use super::Core;
use super::downlinks::PrepareIn;
use super::events::{BtpEvent, PaymentEvent, PeerEvent};
use super::hubs::HubSent;
use super::payers::Answer;
use super::{EngineError, MAX_SENDS_PER_POLL, Outbound};

/// The type byte of an ILP Prepare.
const PREPARE_TYPE: u8 = 12;

/// The prefix of a receiver's address segment below the node's own; the
/// ledger's slug follows. BTP clients' segments have no `-`, so they never
/// collide with one.
pub(super) const RECEIVER_SEGMENT: &str = "stream-";

/// The prefix of a receiver's seed path: its server secret derives from the
/// node key, so credentials stay valid across restarts.
const RECEIVER_SECRET_PATH: &str = "node/stream-server/";

/// Most STREAM connections a receiver keeps.
const MAX_RECEIVER_CONNECTIONS: usize = 256;

/// The part of a ledger's ID before the `+`, its scheme's ID: its name in
/// its receiver's and its payer's addresses, and its receiver's seed path.
/// Its wallet address names it for people instead
/// (`LedgerId::wallet_name`).
#[must_use]
pub fn ledger_slug(ledger: LedgerId) -> &'static str {
    let id = ledger.as_str();
    id.split_once('+').map_or(id, |(slug, _)| slug)
}

/// A STREAM receiver: the local endpoint through which incoming payments
/// on one ledger are paid, into the owner's pool.
pub(super) struct Receiver {
    pub server: Server,
    pub ledger: LedgerId,
    pub endpoint: EndpointId,
    /// The receiver's own packets in flight: the connector's request ID for
    /// each, and the connection and request it is.
    requests: BTreeMap<u32, (ConnectionId, u64)>,
    next_request: u32,
    connections: Vec<ConnectionId>,
}

impl Receiver {
    pub fn new(
        key: &dassie_identity::NodeKey,
        own_address: &str,
        ledger: LedgerId,
        endpoint: EndpointId,
    ) -> Result<Self, EngineError> {
        let path = format!("{RECEIVER_SECRET_PATH}{}", ledger_slug(ledger));
        let path = SeedPath::parse(&path)
            .map_err(|_| EngineError::Internal("receiver seed path".into()))?;
        let secret = key
            .derive(&path)
            .map_err(|_| EngineError::Internal("receiver secret".into()))?;
        let address = receiver_address(own_address, ledger);
        let address = Address::parse(&address)
            .map_err(|_| EngineError::Internal("receiver address".into()))?;
        let asset = ledger.asset();
        let server = Server::new(&ServerConfig {
            secret: *secret.as_bytes(),
            address,
            asset: Some(AssetDetails {
                code: asset.code,
                scale: asset.scale,
            }),
            // Money on every stream, from its first packet: each incoming
            // payment limits its connection instead (see `admission`).
            policy: Policy {
                remote_stream_receive_max: u64::MAX,
                ..Policy::DEFAULT
            },
            max_connections: MAX_RECEIVER_CONNECTIONS,
        });
        Ok(Self {
            server,
            ledger,
            endpoint,
            requests: BTreeMap::new(),
            next_request: 0,
            connections: Vec::new(),
        })
    }

    pub fn deadline(&self) -> Option<u64> {
        self.server
            .connections()
            .filter_map(|(_, connection)| connection.poll_timeout())
            .min()
            .map(|at| at.unix_ms)
    }

    /// Drops the connections the server no longer has.
    fn forget_gone(&mut self) {
        let server = &self.server;
        self.connections
            .retain(|id| server.connection(*id).is_some());
    }
}

/// A ledger's receiver address.
pub(super) fn receiver_address(own_address: &str, ledger: LedgerId) -> String {
    format!("{own_address}.{RECEIVER_SEGMENT}{}", ledger_slug(ledger))
}

impl Core {
    /// A Prepare from a BTP client. A revoked token's links stay until the
    /// step ends, so that what is queued for them goes out first; its cap
    /// is gone at once, so they take no new Prepares meanwhile.
    pub(super) fn btp_prepare(&mut self, link: u64, request_id: u32, packet: &[u8]) {
        let Some(token) = self.links.get(&link).map(|link| link.token) else {
            return;
        };
        let revoked = self
            .tokens
            .get(&token)
            .is_none_or(|token| token.row.revoked_at_ms.is_some());
        if revoked {
            self.refuse_revoked(link, request_id);
        } else {
            self.prepare_from(EndpointId::Btp(link), request_id, packet);
        }
    }

    /// Answers a Prepare on a revoked token's link with a final Reject: it
    /// would not pass again on that token.
    fn refuse_revoked(&mut self, link: u64, request_id: u32) {
        let reject = dassie_ilp::Reject {
            code: ErrorCode::F00_BAD_REQUEST,
            triggered_by: Address::parse(self.connector.own_address()).ok(),
            message: "the token is revoked",
            data: &[],
        };
        let length = dassie_oer::Encode::encode(&reject, &mut self.scratch);
        let packet = self.scratch.get(..length).unwrap_or_default().to_vec();
        self.outputs
            .push((link, Outbound::Response { request_id, packet }));
    }

    pub(super) fn btp_response(&mut self, link: u64, request_id: u32, packet: &[u8]) {
        if self.links.contains_key(&link) {
            self.response_from(EndpointId::Btp(link), request_id, packet);
        }
    }

    /// An ILP packet from a peer: a Prepare, or the response to one of
    /// ours, told apart by its type. A response from a node no longer
    /// peered can only be a resend, and is dropped.
    ///
    /// For a response, whether it fulfilled a packet of ours and the books
    /// posted it: what a claim over a hub counts.
    pub(super) fn peer_packet(
        &mut self,
        from: dassie_identity::NodeId,
        request_id: u32,
        packet: &[u8],
    ) -> bool {
        let is_prepare = packet.first() == Some(&PREPARE_TYPE);
        let Some(&endpoint) = self.peer_endpoints.get(&from) else {
            if is_prepare {
                self.hub_prepare_dropped(&from, request_id);
            }
            return false;
        };
        let source = EndpointId::Peer(endpoint);
        match packet.first() {
            Some(&PREPARE_TYPE) => {
                if !self.prepare_from(source, request_id, packet) {
                    self.hub_prepare_dropped(&from, request_id);
                }
                false
            }
            Some(_) => self.response_from(source, request_id, packet),
            None => false,
        }
    }

    /// A Prepare from an endpoint, through the connector.
    /// Whether the connector took it: forwarded, answered, or a duplicate
    /// of one it holds.
    fn prepare_from(&mut self, source: EndpointId, request_id: u32, packet: &[u8]) -> bool {
        let now = self.now_ms();
        let outcome = self.connect_prepare(now, (source, request_id), PrepareIn::Given(packet));
        match outcome {
            Outcome::Forward(forwarded) => self.forwarded(now, forwarded),
            Outcome::Respond(length) => {
                let upstream = Upstream {
                    to: source,
                    request_id,
                    length,
                };
                if let (Some(from), Some(to)) = (self.out.get(..length), self.up.get_mut(..length))
                {
                    to.copy_from_slice(from);
                }
                self.upstream(now, upstream);
            }
            Outcome::Duplicate => match source {
                EndpointId::Btp(link) => self.outputs.push((
                    link,
                    Outbound::Error {
                        request_id,
                        message: "a request with this ID is in flight",
                    },
                )),
                // A peer has no error channel: the duplicate is dropped.
                _ => self.report(PeerEvent::DuplicateRequest { source, request_id }),
            },
            Outcome::UnknownSource => return false,
        }
        true
    }

    /// A response from an endpoint, through the connector.
    fn response_from(&mut self, source: EndpointId, request_id: u32, packet: &[u8]) -> bool {
        let now = self.now_ms();
        let responded = self.connector.handle_response(
            now,
            source,
            request_id,
            packet,
            &mut self.ledger,
            &mut self.up,
        );
        let posted = matches!(
            responded,
            Responded::Resolved {
                posted: Some(_),
                ..
            }
        );
        self.responded(now, responded, None);
        posted
    }

    /// Carries out what the connector did with a response: counts what our
    /// receiver took once the packet is posted, and sends the response
    /// upstream. A Fulfill counts in our books whether or not the upstream
    /// takes it (spec/decisions/own-fulfills.md).
    pub(super) fn responded(
        &mut self,
        now: u64,
        responded: Responded,
        incoming: Option<crate::store::IncomingPaid>,
    ) {
        let Responded::Resolved { upstream, posted } = responded else {
            return;
        };
        if posted.is_some()
            && let Some(paid) = incoming
        {
            self.incoming_received(paid);
        }
        if let Some(upstream) = upstream {
            self.upstream(now, upstream);
        }
    }

    /// Sends an ILP packet to a peer. A Prepare that cannot be sent fails
    /// at once; a response that cannot leaves the peer to time out.
    fn send_to_peer(
        &mut self,
        now: u64,
        endpoint: u64,
        request_id: u32,
        prepare: Option<usize>,
        response: Option<usize>,
    ) {
        let Some(&node) = self.endpoint_peers.get(&endpoint) else {
            return;
        };
        let (packet, timeout_ms) = match (prepare, response) {
            (Some(length), _) => {
                let packet = self.out.get(..length).unwrap_or_default();
                let expiry = Prepare::decode_exact(packet).map_or(now, |p| p.expires_at.unix_ms);
                (packet, expiry.saturating_sub(now))
            }
            (None, Some(length)) => (self.up.get(..length).unwrap_or_default(), u64::MAX),
            (None, None) => return,
        };
        let packet = packet.to_vec();
        if prepare.is_none() {
            self.answered_over_hub(now, &node, request_id, &packet);
            // A response is retried by the peer node until it arrives.
            self.peer.send_response(now, &node, request_id, &packet);
            self.after_peer();
            return;
        }
        let sent = match self.send_over_hub(now, &node, request_id, &packet, timeout_ms) {
            Some(sent) => sent,
            None => match self
                .peer
                .send_packet(now, &node, request_id, &packet, timeout_ms)
            {
                Ok(_) => HubSent::Sent,
                Err(_) => HubSent::Unreachable,
            },
        };
        let failure = match sent {
            HubSent::Sent => None,
            HubSent::Refused => Some((
                ErrorCode::T04_INSUFFICIENT_LIQUIDITY,
                "the hold does not cover the packet",
            )),
            HubSent::Unreachable => Some((
                ErrorCode::T01_PEER_UNREACHABLE,
                "the peer could not be reached",
            )),
        };
        if let Some((code, message)) = failure {
            self.connector.fail_forwarded(
                now,
                EndpointId::Peer(endpoint),
                request_id,
                code,
                message,
            );
        }
        self.after_peer();
    }

    /// Sends a Prepare the connector forwarded: to the node's own receiver,
    /// or out.
    fn forwarded(&mut self, now: u64, forwarded: Forwarded) {
        match forwarded.to {
            EndpointId::Local(_) => self.deliver(now, forwarded),
            EndpointId::Btp(_) | EndpointId::Peer(_) => self.forward_out(now, forwarded),
        }
    }

    /// Sends a Prepare the connector forwarded to a BTP client or a peer.
    /// What is for the node's own endpoints, its callers deliver.
    // Panics only on a broken invariant: callers send out only what goes
    // out.
    #[allow(clippy::panic)]
    pub(super) fn forward_out(&mut self, now: u64, forwarded: Forwarded) {
        match forwarded.to {
            EndpointId::Btp(link) => {
                let packet = self
                    .out
                    .get(..forwarded.length)
                    .unwrap_or_default()
                    .to_vec();
                self.outputs.push((
                    link,
                    Outbound::Prepare {
                        request_id: forwarded.request_id,
                        packet,
                    },
                ));
            }
            EndpointId::Local(_) => panic!("a packet for a local endpoint sent out"),
            EndpointId::Peer(endpoint) => {
                self.send_to_peer(
                    now,
                    endpoint,
                    forwarded.request_id,
                    Some(forwarded.length),
                    None,
                );
            }
        }
    }

    /// The ledger whose receiver is `endpoint`.
    fn receiver_ledger(&self, endpoint: EndpointId) -> Option<LedgerId> {
        self.receivers
            .values()
            .find(|receiver| receiver.endpoint == endpoint)
            .map(|receiver| receiver.ledger)
    }

    /// Hands a Prepare to its receiver, which answers at once: through the
    /// admission of the incoming payment it pays, which bounds what it may
    /// take.
    pub(super) fn deliver(&mut self, now: u64, forwarded: Forwarded) {
        let Some(ledger) = self.receiver_ledger(forwarded.to) else {
            // A payer takes no money: refused at once, so nothing waits on
            // it until its expiry.
            let reject = dassie_ilp::Reject {
                code: ErrorCode::F02_UNREACHABLE,
                triggered_by: Address::parse(self.connector.own_address()).ok(),
                message: "a payer takes no packets",
                data: &[],
            };
            let length = dassie_oer::Encode::encode(&reject, &mut self.scratch);
            let responded = self.connector.handle_response(
                now,
                forwarded.to,
                forwarded.request_id,
                self.scratch.get(..length).unwrap_or_default(),
                &mut self.ledger,
                &mut self.up,
            );
            self.responded(now, responded, None);
            return;
        };
        let Self {
            rng,
            receivers,
            incoming,
            finished_incoming,
            out,
            scratch,
            ..
        } = self;
        let Some(receiver) = receivers.get_mut(&ledger) else {
            return;
        };
        let Ok(prepare) = Prepare::decode_exact(out.get(..forwarded.length).unwrap_or_default())
        else {
            return;
        };
        let mut paid = None;
        let admit = |tag: &[u8]| {
            let (admission, id) =
                super::incoming::admission(incoming, finished_incoming, ledger, now, tag);
            paid = id;
            admission
        };
        let handled = receiver.server.handle_prepare_admitted(
            Timestamp::from_unix_ms(now),
            rng.as_mut(),
            &prepare,
            admit,
            scratch,
        );
        let incoming = match (paid, handled.received) {
            (Some((payment, connection)), amount @ 1..) => Some(crate::store::IncomingPaid {
                payment,
                connection,
                amount,
            }),
            _ => None,
        };
        let responded = self.connector.handle_response(
            now,
            forwarded.to,
            forwarded.request_id,
            self.scratch.get(..handled.length).unwrap_or_default(),
            &mut self.ledger,
            &mut self.up,
        );
        // What the receiver took counts once the packet is posted.
        self.responded(now, responded, incoming);
        self.pump(now);
    }

    /// Sends a response back to where its Prepare came from.
    pub(super) fn upstream(&mut self, now: u64, upstream: Upstream) {
        let packet = self.up.get(..upstream.length).unwrap_or_default();
        match upstream.to {
            EndpointId::Btp(link) => {
                let packet = packet.to_vec();
                self.outputs.push((
                    link,
                    Outbound::Response {
                        request_id: upstream.request_id,
                        packet,
                    },
                ));
            }
            EndpointId::Local(_) => {
                let Some(ledger) = self.receiver_ledger(upstream.to) else {
                    if self.payer_ledger(upstream.to).is_some() {
                        let answer = Answer::Up(upstream.length);
                        self.payer_response(now, upstream.request_id, answer);
                    }
                    return;
                };
                let Self {
                    receivers,
                    up,
                    work,
                    ..
                } = self;
                let Some(receiver) = receivers.get_mut(&ledger) else {
                    return;
                };
                let Some((id, request)) = receiver.requests.remove(&upstream.request_id) else {
                    return;
                };
                if let Some(connection) = receiver.server.connection_mut(id) {
                    let response = up.get(..upstream.length).unwrap_or_default();
                    connection.handle_response(
                        Timestamp::from_unix_ms(now),
                        request,
                        response,
                        work,
                    );
                }
                self.pump(now);
            }
            EndpointId::Peer(endpoint) => {
                self.send_to_peer(
                    now,
                    endpoint,
                    upstream.request_id,
                    None,
                    Some(upstream.length),
                );
            }
        }
    }

    /// Lets the receivers' connections do what they have to: drain their
    /// events and data, and send their own packets (closes, mostly).
    ///
    /// Nothing it calls comes back here, so the receivers and their lists
    /// stay put while it walks them, by position rather than a copy.
    pub(super) fn pump(&mut self, now: u64) {
        for nth in 0..self.receivers.len() {
            let Some((&ledger, receiver)) = self.receivers.iter_mut().nth(nth) else {
                return;
            };
            while let Some(id) = receiver.server.poll_opened() {
                receiver.connections.push(id);
            }
            receiver.forget_gone();
            for index in 0..MAX_RECEIVER_CONNECTIONS {
                let receiver = self.receivers.get(&ledger);
                let Some(&id) = receiver.and_then(|r| r.connections.get(index)) else {
                    break;
                };
                self.drain_events(ledger, id);
                self.transmit(now, ledger, id);
            }
            if let Some(receiver) = self.receivers.get_mut(&ledger) {
                receiver.server.remove_closed();
                receiver.forget_gone();
            }
        }
    }

    fn drain_events(&mut self, ledger: LedgerId, id: ConnectionId) {
        let Self {
            receivers,
            work,
            events,
            ..
        } = self;
        let Some(connection) = receivers
            .get_mut(&ledger)
            .and_then(|receiver| receiver.server.connection_mut(id))
        else {
            return;
        };
        while let Some(event) = connection.poll_event() {
            match event {
                Event::DataReadable { stream_id } => {
                    // Nobody reads stream data: drop it, which reopens the
                    // window.
                    while connection.read(stream_id, work).is_ok_and(|read| read > 0) {}
                }
                Event::ConnectionClosed { code, by_remote } => {
                    let connection = id.0;
                    let closed = PaymentEvent::StreamClosed {
                        connection,
                        code,
                        by_remote,
                    };
                    events.push(closed.into());
                }
                _ => {}
            }
        }
    }

    /// Sends a receiver connection's packets through the connector.
    fn transmit(&mut self, now: u64, ledger: LedgerId, id: ConnectionId) {
        let at = Timestamp::from_unix_ms(now);
        for _ in 0..MAX_SENDS_PER_POLL {
            let Self {
                rng,
                receivers,
                scratch,
                ..
            } = self;
            let Some(receiver) = receivers.get_mut(&ledger) else {
                return;
            };
            let endpoint = receiver.endpoint;
            let Some(connection) = receiver.server.connection_mut(id) else {
                return;
            };
            let Some(transmit) = connection.poll_transmit(at, rng.as_mut(), scratch) else {
                return;
            };
            let local = receiver.next_request;
            receiver.next_request = local.wrapping_add(1);
            receiver.requests.insert(local, (id, transmit.request_id));
            let outcome =
                self.connect_prepare(now, (endpoint, local), PrepareIn::Scratch(transmit.length));
            self.transmitted(now, ledger, outcome, local, transmit.request_id, id);
        }
        self.sends_left();
    }

    /// What the connector did with a receiver's own packet.
    fn transmitted(
        &mut self,
        now: u64,
        ledger: LedgerId,
        outcome: Outcome,
        local: u32,
        request_id: u64,
        id: ConnectionId,
    ) {
        let Some(receiver) = self.receivers.get_mut(&ledger) else {
            return;
        };
        match outcome {
            // Not to a receiver, which cannot pay one: such a packet expires.
            Outcome::Forward(forwarded) if !matches!(forwarded.to, EndpointId::Local(_)) => {
                self.forward_out(now, forwarded);
            }
            Outcome::Forward(_) => {}
            Outcome::Respond(length) => {
                receiver.requests.remove(&local);
                if let Some(connection) = receiver.server.connection_mut(id) {
                    let response = self.out.get(..length).unwrap_or_default();
                    connection.handle_response(
                        Timestamp::from_unix_ms(now),
                        request_id,
                        response,
                        &mut self.work,
                    );
                }
            }
            Outcome::Duplicate | Outcome::UnknownSource => {
                receiver.requests.remove(&local);
            }
        }
    }

    /// Expires what is due: packets in flight, and the receivers' timers.
    pub(super) fn expire(&mut self, now: u64) {
        while let Some(upstream) =
            self.connector
                .handle_timeout(now, &mut self.ledger, &mut self.up)
        {
            self.upstream(now, upstream);
        }
        self.outgoing_timers(now);
        self.probe_timers(now);
        let at = Timestamp::from_unix_ms(now);
        for receiver in self.receivers.values_mut() {
            let Receiver {
                server,
                connections,
                ..
            } = receiver;
            for &id in connections.iter() {
                if let Some(connection) = server.connection_mut(id)
                    && connection.poll_timeout().is_some_and(|due| due <= at)
                {
                    connection.handle_timeout(at);
                }
            }
        }
        self.pump(now);
        self.take_journal();
    }

    /// A BTP client went away: its packets in flight fail upstream.
    pub fn btp_closed(&mut self, link: u64) {
        let Some(removed) = self.links.remove(&link) else {
            return;
        };
        if let Some(token) = self.tokens.get_mut(&removed.token) {
            token.connections.remove(&link);
        }
        let now = self.now_ms();
        self.connector.remove_endpoint(now, EndpointId::Btp(link));
        self.expire(now);
        self.report(BtpEvent::ClientGone { link });
    }

    /// Ends a BTP connection from this side: its client is told, after
    /// the messages already queued for it.
    pub fn close_link(&mut self, link: u64) {
        if self.links.contains_key(&link) {
            self.outputs.push((link, Outbound::Close));
        }
        self.btp_closed(link);
    }
}

impl Core {
    /// The STREAM receivers: each server's invariants, and its
    /// connections, in the order they opened.
    pub(super) fn check_receivers(&self) {
        for receiver in self.receivers.values() {
            receiver.server.check_invariants();
            assert!(receiver.connections.len() <= MAX_RECEIVER_CONNECTIONS);
            let in_order = receiver
                .connections
                .windows(2)
                .all(|pair| matches!(pair, [a, b] if a < b));
            assert!(
                in_order,
                "a receiver's connections in the order they opened"
            );
        }
    }
}
