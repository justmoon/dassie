//! A deterministic simulation of one STREAM client and one server joined
//! by a connector: mock clock, seeded RNG, latency, loss, an exchange rate
//! and a maximum packet amount. The first, small piece of the simulation
//! testing of spec 7 §7 (D16).

#![allow(
    dead_code,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::too_many_lines,
    clippy::indexing_slicing,
    clippy::panic
)]

use std::collections::BTreeMap;
use std::convert::Infallible;
use std::fmt::Write as _;

use dassie_ilp::{
    Address, AmountTooLarge, ErrorCode as IlpErrorCode, Prepare, Reject, Response, Timestamp,
};
use dassie_oer::{Decode, Encode};
use dassie_stream::{
    Admission, AssetDetails, BUFFER_LENGTH, Config, Connection, ConnectionId, Event, Policy, Role,
    Server, ServerConfig,
};
use rand_core::{TryCryptoRng, TryRng};

/// xoshiro256** seeded with SplitMix64: fast, deterministic, and good
/// enough for simulations. Marked as a crypto RNG only so it can stand in
/// for one in tests.
pub struct TestRng {
    state: [u64; 4],
}

impl TestRng {
    pub fn new(seed: u64) -> Self {
        let mut mix = seed;
        let mut next = || {
            mix = mix.wrapping_add(0x9e37_79b9_7f4a_7c15);
            let mut z = mix;
            z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
            z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
            z ^ (z >> 31)
        };
        Self {
            state: [next(), next(), next(), next()],
        }
    }

    pub fn next(&mut self) -> u64 {
        let [a, b, c, d] = &mut self.state;
        let result = b.wrapping_mul(5).rotate_left(7).wrapping_mul(9);
        let t = *b << 17;
        *c ^= *a;
        *d ^= *b;
        *b ^= *c;
        *a ^= *d;
        *c ^= t;
        *d = d.rotate_left(45);
        result
    }

    /// Uniform in `0..bound`.
    pub fn below(&mut self, bound: u64) -> u64 {
        if bound == 0 { 0 } else { self.next() % bound }
    }
}

impl TryRng for TestRng {
    type Error = Infallible;

    fn try_next_u32(&mut self) -> Result<u32, Infallible> {
        Ok((self.next() >> 32) as u32)
    }

    fn try_next_u64(&mut self) -> Result<u64, Infallible> {
        Ok(self.next())
    }

    fn try_fill_bytes(&mut self, destination: &mut [u8]) -> Result<(), Infallible> {
        for chunk in destination.chunks_mut(8) {
            let bytes = self.next().to_le_bytes();
            chunk.copy_from_slice(&bytes[..chunk.len()]);
        }
        Ok(())
    }
}

impl TryCryptoRng for TestRng {}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Side {
    Client,
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
}

impl Default for Path {
    fn default() -> Self {
        Self {
            rate: (1, 1),
            max_packet_amount: u64::MAX,
            latency_ms: 10,
            loss_permille: 0,
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

pub struct Simulation {
    pub now: Timestamp,
    pub rng: TestRng,
    pub path: Path,
    pub client: Connection,
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
    /// The error the server's application refuses packets with, if any.
    pub refusal: Option<IlpErrorCode>,
    server_config: (u64, [u8; 32], Policy),
    /// The tags the server's application was asked about.
    pub admitted_tags: Vec<Vec<u8>>,
    /// Data each side read, per stream.
    pub received_data: BTreeMap<(Side, u64), Vec<u8>>,
    pub trace: String,
    queue: BTreeMap<(u64, u64), Message>,
    sequence: u64,
    buffer: Vec<u8>,
}

pub const CLIENT_ADDRESS: &str = "test.client";
pub const SERVER_ADDRESS: &str = "test.server";

impl Simulation {
    pub fn new(seed: u64, path: Path, policy: Policy) -> Self {
        Self::with_receipts(seed, path, policy, None)
    }

    pub fn with_receipts(
        seed: u64,
        path: Path,
        policy: Policy,
        receipt: Option<dassie_stream::receipt::ReceiptSecret>,
    ) -> Self {
        let mut rng = TestRng::new(seed);
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
            money_received: 0,
            refusal: None,
            server_config: (seed, secret, policy),
            admitted_tags: Vec::new(),
            received_data: BTreeMap::new(),
            trace: String::new(),
            queue: BTreeMap::new(),
            sequence: 0,
            buffer: vec![0; BUFFER_LENGTH],
        }
    }

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
                let mut buffer = std::mem::take(&mut self.buffer);
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
    fn forward(&self, from: Side, prepare: &Prepare<'_>) -> Result<u64, Vec<u8>> {
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
        let (numerator, denominator) = match from {
            Side::Client => self.path.rate,
            Side::Server(_) => (self.path.rate.1, self.path.rate.0),
        };
        let converted =
            u128::from(prepare.amount) * u128::from(numerator) / u128::from(denominator);
        Ok(u64::try_from(converted).unwrap_or(u64::MAX))
    }

    /// Hands a Prepare to the server through its application's admission,
    /// and takes what it received off the limit.
    fn admitted(&mut self, prepare: &Prepare<'_>, buffer: &mut [u8]) -> usize {
        let (refusal, limit, received) = (self.refusal, self.money_limit, self.money_received);
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
                },
            }
        };
        let handled =
            self.server
                .handle_prepare_admitted(self.now, &mut self.rng, prepare, admit, buffer);
        if let Some(limit) = self.money_limit.as_mut() {
            *limit -= handled.received;
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
                        let mut buffer = std::mem::take(&mut self.buffer);
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
                let mut buffer = std::mem::take(&mut self.buffer);
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

    pub fn check_invariants(&self) {
        self.client.check_invariants();
        for (_, connection) in self.server.connections() {
            connection.check_invariants();
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
    pub fn run_until(&mut self, max_steps: usize, mut done: impl FnMut(&Self) -> bool) {
        for _ in 0..max_steps {
            if done(self) {
                return;
            }
            if !self.step() && !done(self) {
                panic!("simulation went idle before finishing\n{}", self.trace);
            }
        }
        assert!(
            done(self),
            "simulation did not finish in {max_steps} steps\n{}",
            self.trace
        );
    }

    /// Steps until nothing is left to happen.
    pub fn run_to_idle(&mut self, max_steps: usize) {
        for _ in 0..max_steps {
            if !self.step() {
                return;
            }
        }
        let tail: Vec<&str> = self.trace.lines().rev().take(30).collect();
        panic!(
            "simulation did not settle in {max_steps} steps; last events:\n{}\nclient: {:?}",
            tail.into_iter().rev().collect::<Vec<_>>().join("\n"),
            self.client
        );
    }
}
