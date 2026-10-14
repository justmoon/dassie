//! Packets: from BTP clients into the connector, to the STREAM receiver,
//! and back.

use std::collections::BTreeMap;

use dassie_connector::{EndpointId, Forwarded, Outcome, Upstream};
use dassie_identity::SeedPath;
use dassie_ilp::{Address, Prepare, Timestamp};
use dassie_ledger::LedgerId;
use dassie_oer::Decode;
use dassie_stream::{AssetDetails, ConnectionId, Event, Policy, Server, ServerConfig};

use super::state::Core;
use super::{EngineError, Outbound};
use crate::log::target;

/// The STREAM receiver's endpoint.
pub(super) const RECEIVER: EndpointId = EndpointId::Local(0);

/// The receiver's address segment below the node's own.
pub(super) const RECEIVER_SEGMENT: &str = "stream";

/// The seed path of the receiver's server secret: credentials derive from
/// the node key, so they stay valid across restarts (ILP-11).
const RECEIVER_SECRET_PATH: &str = "node/stream-server";

/// Most STREAM connections the receiver keeps.
const MAX_RECEIVER_CONNECTIONS: usize = 256;

/// The node's STREAM receiver, a local endpoint paying into the owner's
/// pool on its ledger.
pub(super) struct Receiver {
    pub server: Server,
    pub ledger: LedgerId,
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
    ) -> Result<Self, EngineError> {
        let path = SeedPath::parse(RECEIVER_SECRET_PATH)
            .map_err(|_| EngineError::Internal("receiver seed path".into()))?;
        let secret = key
            .derive(&path)
            .map_err(|_| EngineError::Internal("receiver secret".into()))?;
        let address = format!("{own_address}.{RECEIVER_SEGMENT}");
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
            // Money on every stream, from its first packet: the receiver
            // takes whatever it is paid.
            policy: Policy {
                remote_stream_receive_max: u64::MAX,
                ..Policy::DEFAULT
            },
            max_connections: MAX_RECEIVER_CONNECTIONS,
        });
        Ok(Self {
            server,
            ledger,
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
}

impl Core {
    pub(super) fn btp_prepare(&mut self, link: u64, request_id: u32, packet: &[u8]) {
        if !self.links.contains_key(&link) {
            return;
        }
        let now = self.now_ms();
        let outcome = self.connector.handle_prepare(
            now,
            EndpointId::Btp(link),
            request_id,
            packet,
            &mut self.ledger,
            &self.rates,
            &mut self.out,
        );
        match outcome {
            Outcome::Forward(forwarded) => self.forwarded(now, forwarded),
            Outcome::Respond(length) => {
                let packet = self.out.get(..length).unwrap_or_default().to_vec();
                self.outputs
                    .push((link, Outbound::Response { request_id, packet }));
            }
            Outcome::Duplicate => self.outputs.push((
                link,
                Outbound::Error {
                    request_id,
                    message: "a request with this ID is in flight",
                },
            )),
            Outcome::UnknownSource => {}
        }
    }

    pub(super) fn btp_response(&mut self, link: u64, request_id: u32, packet: &[u8]) {
        if !self.links.contains_key(&link) {
            return;
        }
        let now = self.now_ms();
        let upstream = self.connector.handle_response(
            now,
            EndpointId::Btp(link),
            request_id,
            packet,
            &mut self.ledger,
            &mut self.up,
        );
        if let Some(upstream) = upstream {
            self.upstream(now, upstream);
        }
    }

    /// Sends a Prepare the connector forwarded.
    fn forwarded(&mut self, now: u64, forwarded: Forwarded) {
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
            RECEIVER => self.deliver(now, forwarded),
            EndpointId::Local(_) => {}
        }
    }

    /// Hands a Prepare to the receiver, which answers at once.
    fn deliver(&mut self, now: u64, forwarded: Forwarded) {
        let Self {
            node,
            receiver,
            out,
            scratch,
            ..
        } = self;
        let Some(receiver) = receiver.as_mut() else {
            return;
        };
        let Ok(prepare) = Prepare::decode_exact(out.get(..forwarded.length).unwrap_or_default())
        else {
            return;
        };
        let length = node.with_rng(|rng| {
            receiver
                .server
                .handle_prepare(Timestamp::from_unix_ms(now), rng, &prepare, scratch)
        });
        let upstream = self.connector.handle_response(
            now,
            RECEIVER,
            forwarded.request_id,
            self.scratch.get(..length).unwrap_or_default(),
            &mut self.ledger,
            &mut self.up,
        );
        if let Some(upstream) = upstream {
            self.upstream(now, upstream);
        }
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
            RECEIVER => {
                let Self {
                    receiver, up, work, ..
                } = self;
                let Some(receiver) = receiver.as_mut() else {
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
            EndpointId::Local(_) => {}
        }
    }

    /// Lets the receiver's connections do what they have to: drain their
    /// events and data, and send their own packets (closes, mostly).
    pub(super) fn pump(&mut self, now: u64) {
        let Some(receiver) = self.receiver.as_mut() else {
            return;
        };
        while let Some(id) = receiver.server.poll_opened() {
            receiver.connections.push(id);
        }
        let ids = receiver.connections.clone();
        for id in ids {
            self.drain_events(id);
            self.transmit(now, id);
        }
        if let Some(receiver) = self.receiver.as_mut() {
            receiver.server.remove_closed();
            let server = &receiver.server;
            receiver
                .connections
                .retain(|id| server.connection(*id).is_some());
        }
    }

    fn drain_events(&mut self, id: ConnectionId) {
        let Self { receiver, work, .. } = self;
        let Some(connection) = receiver
            .as_mut()
            .and_then(|receiver| receiver.server.connection_mut(id))
        else {
            return;
        };
        while let Some(event) = connection.poll_event() {
            match event {
                Event::DataReadable { stream_id } => {
                    // Nobody reads stream data yet (M6): drop it, which
                    // reopens the window.
                    while connection.read(stream_id, work).is_ok_and(|read| read > 0) {}
                }
                Event::MoneyReceived { stream_id } => {
                    let total = connection
                        .stream(stream_id)
                        .map_or(0, dassie_stream::Stream::total_received);
                    tracing::debug!(target: target::PAYMENT, connection = id.0, stream_id, total, "money received");
                }
                Event::ConnectionClosed { code, by_remote } => {
                    tracing::debug!(target: target::PAYMENT, connection = id.0, ?code, by_remote, "STREAM connection closed");
                }
                _ => {}
            }
        }
    }

    /// Sends a receiver connection's packets through the connector.
    fn transmit(&mut self, now: u64, id: ConnectionId) {
        let at = Timestamp::from_unix_ms(now);
        loop {
            let Self {
                node,
                receiver,
                scratch,
                ..
            } = self;
            let Some(receiver) = receiver.as_mut() else {
                return;
            };
            let Some(connection) = receiver.server.connection_mut(id) else {
                return;
            };
            let Some(transmit) = node.with_rng(|rng| connection.poll_transmit(at, rng, scratch))
            else {
                return;
            };
            let local = receiver.next_request;
            receiver.next_request = local.wrapping_add(1);
            receiver.requests.insert(local, (id, transmit.request_id));
            let outcome = self.connector.handle_prepare(
                now,
                RECEIVER,
                local,
                self.scratch.get(..transmit.length).unwrap_or_default(),
                &mut self.ledger,
                &self.rates,
                &mut self.out,
            );
            let Some(receiver) = self.receiver.as_mut() else {
                return;
            };
            match outcome {
                Outcome::Forward(
                    forwarded @ Forwarded {
                        to: EndpointId::Btp(_),
                        ..
                    },
                ) => {
                    self.forwarded(now, forwarded);
                }
                // Back to the receiver, which cannot pay itself: the packet
                // expires.
                Outcome::Forward(_) => {}
                Outcome::Respond(length) => {
                    receiver.requests.remove(&local);
                    if let Some(connection) = receiver.server.connection_mut(id) {
                        let response = self.out.get(..length).unwrap_or_default();
                        connection.handle_response(
                            at,
                            transmit.request_id,
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
    }

    /// Expires what is due: packets in flight, and the receiver's timers.
    pub(super) fn expire(&mut self, now: u64) {
        while let Some(upstream) =
            self.connector
                .handle_timeout(now, &mut self.ledger, &mut self.up)
        {
            self.upstream(now, upstream);
        }
        if let Some(receiver) = self.receiver.as_mut() {
            let at = Timestamp::from_unix_ms(now);
            for id in receiver.connections.clone() {
                if let Some(connection) = receiver.server.connection_mut(id)
                    && connection.poll_timeout().is_some_and(|due| due <= at)
                {
                    connection.handle_timeout(at);
                }
            }
            self.pump(now);
        }
        self.take_journal();
    }

    /// A BTP client went away: its packets in flight fail upstream.
    pub(super) fn btp_closed(&mut self, link: u64) {
        let Some(removed) = self.links.remove(&link) else {
            return;
        };
        if let Some(token) = self.tokens.get_mut(&removed.token) {
            token.connections.remove(&link);
        }
        let now = self.now_ms();
        self.connector.remove_endpoint(now, EndpointId::Btp(link));
        self.expire(now);
        tracing::debug!(target: target::BTP, link, "BTP client gone");
    }

    /// Ends a BTP connection from this side.
    pub(super) fn close_link(&mut self, link: u64) {
        if let Some(existing) = self.links.get(&link) {
            let _ = existing.outbound.try_send(Outbound::Close);
        }
        self.btp_closed(link);
    }
}
