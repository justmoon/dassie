//! A deterministic simulation of one STREAM client and one server joined
//! by a connector: mock clock, seeded RNG, latency, loss, an exchange rate
//! and a maximum packet amount (spec/areas/testing.md#deterministic-simulation,
//! spec/decisions/tigerstyle.md). For this crate's tests and the simulation
//! runner (spec/decisions/random-seed-simulations.md).
//!
//! Test code, behind the `sim` feature: a panic here is a failed run.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::too_many_lines,
    clippy::indexing_slicing,
    clippy::panic
)]

pub mod payment;

use alloc::collections::BTreeMap;
use alloc::format;
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;
use core::fmt::Write as _;

use crate::{
    Admission, AssetDetails, BUFFER_LENGTH, Config, Connection, ConnectionId, ErrorCode, Event,
    Policy, Role, Server, ServerConfig,
};
use dassie_ilp::{
    Address, AmountTooLarge, ErrorCode as IlpErrorCode, Prepare, Reject, Response, Timestamp,
};
use dassie_node_core::testing::SeededRng;
use dassie_oer::{Decode, Encode};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
/// One end of the simulation.
pub enum Side {
    /// The client.
    Client,
    /// A connection of the server.
    Server(ConnectionId),
}

/// The connector between the two.
#[derive(Clone, Copy, Debug)]
pub struct Path {
    /// Destination units per source unit from client to server, as a
    /// fraction; the reverse direction uses the inverse.
    pub rate: (u64, u64),
    /// Largest amount the connector forwards, in the sender's units.
    pub max_packet_amount: u64,
    /// One-way latency.
    pub latency_ms: u64,
    /// Chance that a packet or response is dropped, in thousandths.
    pub loss_permille: u64,
    /// Chance that a packet from the client takes the detour instead, in
    /// thousandths, drawn per packet as a sender that spreads its packets
    /// over several routes would.
    pub detour_permille: u64,
    /// The other route.
    pub detour: Detour,
}

/// Another route to the server, which some of the client's packets take.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Detour {
    /// A node on it cannot reach the server, and says so with F02.
    Unreachable,
    /// It delivers at this rate instead of the path's, as a dearer route
    /// does.
    Rate(u64, u64),
}

impl Default for Path {
    fn default() -> Self {
        Self {
            rate: (1, 1),
            max_packet_amount: u64::MAX,
            latency_ms: 10,
            loss_permille: 0,
            detour_permille: 0,
            detour: Detour::Unreachable,
        }
    }
}

enum Message {
    Prepare {
        from: Side,
        request_id: u64,
        bytes: Vec<u8>,
    },
    Response {
        to: Side,
        request_id: u64,
        bytes: Vec<u8>,
    },
}

/// A client and a server, the connector between them, and their clock.
pub struct Simulation {
    /// The clock.
    pub now: Timestamp,
    /// Where all randomness comes from.
    pub rng: SeededRng,
    /// The connector between them.
    pub path: Path,
    /// The client.
    pub client: Connection,
    /// The server.
    pub server: Server,
    /// What server connections accept per stream they are offered.
    pub server_receive_max: u64,
    /// What the client accepts per stream the server opens.
    pub client_receive_max: u64,
    /// What the server's application still lets in, over all connections,
    /// as an incoming payment would: `None` for no limit.
    pub money_limit: Option<u64>,
    /// What the server's application counts as received, over every
    /// connection and restart.
    pub money_received: u64,
    /// How far what the server's application lets in may land from its
    /// limit and count as reached.
    pub money_tolerance: u64,
    /// The error the server's application refuses packets with, if any.
    pub refusal: Option<IlpErrorCode>,
    server_config: (u64, [u8; 32], Policy),
    /// Whether the server restarted: a receiver that forgot a connection
    /// may tell it a lower maximum, and only then.
    server_restarted: bool,
    /// The tags the server's application was asked about.
    pub admitted_tags: Vec<Vec<u8>>,
    /// Data each side read, per stream.
    pub received_data: BTreeMap<(Side, u64), Vec<u8>>,
    /// What happened, a line per event.
    pub trace: String,
    queue: BTreeMap<(u64, u64), Message>,
    sequence: u64,
    buffer: Vec<u8>,
}

/// The client's address.
pub const CLIENT_ADDRESS: &str = "test.client";
/// The server's address.
pub const SERVER_ADDRESS: &str = "test.server";

impl Simulation {
    /// A client with fresh credentials for the server, from `seed`.
    pub fn new(seed: u64, path: Path, policy: Policy) -> Self {
        Self::with_receipts(seed, path, policy, None)
    }

    /// As [`new`](Self::new), with receipts by `receipt`.
    pub fn with_receipts(
        seed: u64,
        path: Path,
        policy: Policy,
        receipt: Option<crate::receipt::ReceiptSecret>,
    ) -> Self {
        let mut rng = SeededRng::new(seed);
        let mut secret = [0u8; 32];
        rand_core::Rng::fill_bytes(&mut rng, &mut secret);
        let server = Server::new(&ServerConfig {
            secret,
            address: Address::parse(SERVER_ADDRESS).unwrap(),
            asset: Some(AssetDetails {
                code: "XRP",
                scale: 9,
            }),
            policy,
            max_connections: 4,
        });
        let credentials = server.generate_credentials(&mut rng, b"payment-1", receipt);
        let client = Connection::new(&Config {
            role: Role::Client,
            shared_secret: &credentials.shared_secret,
            local_address: Some(Address::parse(CLIENT_ADDRESS).unwrap()),
            remote_address: Some(Address::parse(&credentials.destination).unwrap()),
            local_asset: Some(AssetDetails {
                code: "USD",
                scale: 6,
            }),
            receipt: None,
            policy,
        });
        Self {
            now: Timestamp::from_unix_ms(1_700_000_000_000),
            rng,
            path,
            client,
            server,
            server_receive_max: u64::MAX,
            client_receive_max: u64::MAX,
            money_limit: None,
            money_tolerance: 0,
            money_received: 0,
            refusal: None,
            server_config: (seed, secret, policy),
            server_restarted: false,
            admitted_tags: Vec::new(),
            received_data: BTreeMap::new(),
            trace: String::new(),
            queue: BTreeMap::new(),
            sequence: 0,
            buffer: vec![0; BUFFER_LENGTH],
        }
    }

    /// The server's first connection.
    pub fn server_connection(&self) -> Option<&Connection> {
        self.server
            .connections()
            .next()
            .map(|(_, connection)| connection)
    }

    fn connection_mut(&mut self, side: Side) -> Option<&mut Connection> {
        match side {
            Side::Client => Some(&mut self.client),
            Side::Server(id) => self.server.connection_mut(id),
        }
    }

    fn schedule(&mut self, message: Message) {
        // Only Prepares are lost. A connector that loses a Fulfill after the
        // receiver credited the money absorbs the loss itself, and the
        // sender rightly counts the packet as failed, so dropping responses
        // here would show the receiver more than the sender sent.
        if let Message::Prepare {
            from,
            request_id,
            bytes,
        } = &message
            && self.rng.below(1000) < self.path.loss_permille
        {
            let _ = writeln!(self.trace, "{} drop", self.now.unix_ms);
            // A connector that loses a Prepare downstream still answers the
            // sender, with R00 when the Prepare expires.
            let Ok(dassie_ilp::Packet::Prepare(prepare)) = dassie_ilp::Packet::decode_exact(bytes)
            else {
                panic!("connections must send valid prepares");
            };
            let reject = Reject {
                code: IlpErrorCode::R00_TRANSFER_TIMED_OUT,
                triggered_by: Address::parse("test.connector").ok(),
                message: "",
                data: &[],
            };
            let response = Message::Response {
                to: *from,
                request_id: *request_id,
                bytes: reject.to_vec(),
            };
            self.sequence += 1;
            self.queue
                .insert((prepare.expires_at.unix_ms, self.sequence), response);
            return;
        }
        let at = self.now.unix_ms + self.path.latency_ms;
        self.sequence += 1;
        self.queue.insert((at, self.sequence), message);
    }

    fn transmit_all(&mut self) {
        let mut sides = vec![Side::Client];
        sides.extend(self.server.connections().map(|(id, _)| Side::Server(id)));
        for side in sides {
            loop {
                let now = self.now;
                let mut buffer = core::mem::take(&mut self.buffer);
                let transmit = {
                    let rng = &mut self.rng;
                    match side {
                        Side::Client => self.client.poll_transmit(now, rng, &mut buffer),
                        Side::Server(id) => self
                            .server
                            .connection_mut(id)
                            .and_then(|connection| connection.poll_transmit(now, rng, &mut buffer)),
                    }
                };
                let Some(transmit) = transmit else {
                    self.buffer = buffer;
                    break;
                };
                let bytes = buffer[..transmit.length].to_vec();
                self.buffer = buffer;
                self.schedule(Message::Prepare {
                    from: side,
                    request_id: transmit.request_id,
                    bytes,
                });
            }
        }
    }

    /// Converts a Prepare as a connector would, or rejects it.
    fn forward(&mut self, from: Side, prepare: &Prepare<'_>) -> Result<u64, Vec<u8>> {
        if prepare.amount > self.path.max_packet_amount {
            let data = AmountTooLarge {
                received_amount: prepare.amount,
                maximum_amount: self.path.max_packet_amount,
            }
            .to_bytes();
            let reject = Reject {
                code: IlpErrorCode::F08_AMOUNT_TOO_LARGE,
                triggered_by: Address::parse("test.connector").ok(),
                message: "",
                data: &data,
            };
            return Err(reject.to_vec());
        }
        // Drawn only when there is a detour, so runs without one draw as
        // they always did.
        let detour = from == Side::Client
            && self.path.detour_permille > 0
            && self.rng.below(1000) < self.path.detour_permille;
        if detour {
            let _ = writeln!(self.trace, "{} detour", self.now.unix_ms);
        }
        let (numerator, denominator) = match (from, detour, self.path.detour) {
            (Side::Client, true, Detour::Unreachable) => {
                let reject = Reject {
                    code: IlpErrorCode::F02_UNREACHABLE,
                    triggered_by: Address::parse("test.detour").ok(),
                    message: "",
                    data: &[],
                };
                return Err(reject.to_vec());
            }
            (Side::Client, true, Detour::Rate(numerator, denominator)) => (numerator, denominator),
            (Side::Client, ..) => self.path.rate,
            (Side::Server(_), ..) => (self.path.rate.1, self.path.rate.0),
        };
        let converted =
            u128::from(prepare.amount) * u128::from(numerator) / u128::from(denominator);
        Ok(u64::try_from(converted).unwrap_or(u64::MAX))
    }

    /// Hands a Prepare to the server through its application's admission,
    /// and takes what it received off the limit.
    fn admitted(&mut self, prepare: &Prepare<'_>, buffer: &mut [u8]) -> usize {
        let (refusal, limit, received) = (self.refusal, self.money_limit, self.money_received);
        let tolerance = self.money_tolerance;
        let tags = &mut self.admitted_tags;
        let admit = |tag: &[u8]| {
            if tags.len() < 64 {
                tags.push(tag.to_vec());
            }
            match refusal {
                Some(code) => Admission::Refuse(code),
                None => Admission::Accept {
                    money_limit: limit.unwrap_or(u64::MAX),
                    received,
                    tolerance,
                    exhausted: ErrorCode::NO_ERROR,
                },
            }
        };
        let handled =
            self.server
                .handle_prepare_admitted(self.now, &mut self.rng, prepare, admit, buffer);
        if let Some(limit) = self.money_limit.as_mut() {
            *limit = limit.saturating_sub(handled.received);
        }
        self.money_received += handled.received;
        handled.length
    }

    /// The server restarts: its connections are gone, its secret is not,
    /// so the client's next packet opens a connection anew.
    pub fn restart_server(&mut self) {
        let (_, secret, policy) = self.server_config;
        self.server = Server::new(&ServerConfig {
            secret,
            address: Address::parse(SERVER_ADDRESS).unwrap(),
            asset: Some(AssetDetails {
                code: "XRP",
                scale: 9,
            }),
            policy,
            max_connections: 4,
        });
        self.server_restarted = true;
        let _ = writeln!(self.trace, "{} server restarts", self.now.unix_ms);
    }

    fn deliver(&mut self, message: Message) {
        match message {
            Message::Prepare {
                from,
                request_id,
                bytes,
            } => {
                let Ok(dassie_ilp::Packet::Prepare(prepare)) =
                    dassie_ilp::Packet::decode_exact(&bytes)
                else {
                    panic!("connections must send valid prepares");
                };
                let to = match from {
                    Side::Client => None,
                    Side::Server(_) => Some(Side::Client),
                };
                let response = match self.forward(from, &prepare) {
                    Err(reject) => reject,
                    Ok(amount) => {
                        let forwarded = Prepare { amount, ..prepare };
                        let mut buffer = core::mem::take(&mut self.buffer);
                        let length = match to {
                            None => self.admitted(&forwarded, &mut buffer),
                            Some(_) => self.client.handle_prepare(
                                self.now,
                                &mut self.rng,
                                &forwarded,
                                &mut buffer,
                            ),
                        };
                        let bytes = buffer[..length].to_vec();
                        self.buffer = buffer;
                        bytes
                    }
                };
                self.schedule(Message::Response {
                    to: from,
                    request_id,
                    bytes: response,
                });
            }
            Message::Response {
                to,
                request_id,
                bytes,
            } => {
                assert!(
                    Response::decode_exact(&bytes).is_ok(),
                    "responses must parse"
                );
                let now = self.now;
                let mut buffer = core::mem::take(&mut self.buffer);
                if let Some(connection) = self.connection_mut(to) {
                    connection.handle_response(now, request_id, &bytes, &mut buffer);
                }
                self.buffer = buffer;
            }
        }
    }

    fn handle_events(&mut self) {
        while let Some(id) = self.server.poll_opened() {
            let _ = writeln!(self.trace, "{} server opened {:?}", self.now.unix_ms, id);
        }
        let mut sides = vec![Side::Client];
        sides.extend(self.server.connections().map(|(id, _)| Side::Server(id)));
        for side in sides {
            loop {
                let receive_max = match side {
                    Side::Client => self.client_receive_max,
                    Side::Server(_) => self.server_receive_max,
                };
                let Some(connection) = self.connection_mut(side) else {
                    break;
                };
                let Some(event) = connection.poll_event() else {
                    break;
                };
                let mut read = Vec::new();
                match event {
                    Event::StreamOpened { stream_id } => {
                        connection.set_receive_max(stream_id, receive_max).unwrap();
                    }
                    Event::DataReadable { stream_id } => {
                        let mut chunk = [0u8; 4096];
                        loop {
                            let count = connection.read(stream_id, &mut chunk).unwrap();
                            if count == 0 {
                                break;
                            }
                            read.extend_from_slice(&chunk[..count]);
                        }
                        self.received_data
                            .entry((side, stream_id))
                            .or_default()
                            .extend_from_slice(&read);
                    }
                    _ => {}
                }
                let _ = writeln!(self.trace, "{} {:?} {:?}", self.now.unix_ms, side, event);
            }
        }
    }

    /// Checks both sides' invariants.
    pub fn check_invariants(&self) {
        self.client.check_invariants();
        self.server.check_invariants();
        // A receiver never lowers a maximum it told, unless it forgot the
        // connection (spec/decisions/close-rather-than-lower.md).
        if !self.server_restarted {
            for stream in self.client.streams() {
                assert!(
                    !stream.remote_max_fell,
                    "stream {} heard a lower maximum\n{}",
                    stream.id, self.trace
                );
            }
        }
    }

    /// Runs one step: send, then advance to the next delivery or deadline.
    /// Returns false when nothing is left to happen.
    pub fn step(&mut self) -> bool {
        self.transmit_all();
        self.handle_events();
        self.check_invariants();
        let next_delivery = self.queue.keys().next().map(|&(at, _)| at);
        let mut deadlines = vec![self.client.poll_timeout()];
        deadlines.extend(self.server.connections().map(|(_, c)| c.poll_timeout()));
        let next_deadline = deadlines.into_iter().flatten().map(|t| t.unix_ms).min();
        let next = match (next_delivery, next_deadline) {
            (Some(a), Some(b)) => a.min(b),
            (Some(a), None) | (None, Some(a)) => a,
            (None, None) => return false,
        };
        self.now = Timestamp::from_unix_ms(next.max(self.now.unix_ms));
        while let Some(entry) = self.queue.first_entry() {
            if entry.key().0 > self.now.unix_ms {
                break;
            }
            let message = entry.remove();
            self.deliver(message);
            self.handle_events();
            self.check_invariants();
        }
        let now = self.now;
        self.client.handle_timeout(now);
        let ids: Vec<_> = self.server.connections().map(|(id, _)| id).collect();
        for id in ids {
            if let Some(connection) = self.server.connection_mut(id) {
                connection.handle_timeout(now);
            }
        }
        self.handle_events();
        self.check_invariants();
        true
    }

    /// Steps until `done` holds, failing after `max_steps`.
    pub fn run_until(&mut self, max_steps: usize, done: impl FnMut(&Self) -> bool) {
        if let Err(problem) = self.try_run_until(max_steps, done) {
            panic!("{problem}\n{}", self.trace);
        }
    }

    /// Steps until `done` holds, or says why it never did.
    ///
    /// # Errors
    ///
    /// When the simulation goes idle, or runs `max_steps`, first.
    pub fn try_run_until(
        &mut self,
        max_steps: usize,
        mut done: impl FnMut(&Self) -> bool,
    ) -> Result<(), String> {
        for _ in 0..max_steps {
            if done(self) {
                return Ok(());
            }
            if !self.step() && !done(self) {
                return Err("simulation went idle before finishing".into());
            }
        }
        if done(self) {
            Ok(())
        } else {
            Err(format!("simulation did not finish in {max_steps} steps"))
        }
    }

    /// Steps until nothing is left to happen.
    pub fn run_to_idle(&mut self, max_steps: usize) {
        if let Err(problem) = self.try_run_to_idle(max_steps) {
            let tail: Vec<&str> = self.trace.lines().rev().take(30).collect();
            panic!(
                "{problem}; last events:\n{}\nclient: {:?}",
                tail.into_iter().rev().collect::<Vec<_>>().join("\n"),
                self.client
            );
        }
    }

    /// Steps until nothing is left to happen, or says that it never was.
    ///
    /// # Errors
    ///
    /// When something is still left after `max_steps`.
    pub fn try_run_to_idle(&mut self, max_steps: usize) -> Result<(), String> {
        for _ in 0..max_steps {
            if !self.step() {
                return Ok(());
            }
        }
        Err(format!("simulation did not settle in {max_steps} steps"))
    }
}
