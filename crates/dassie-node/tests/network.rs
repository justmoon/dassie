//! A simulated network of real engine cores (spec 7 §7): each node's core
//! with its own in-memory database, wired through a virtual network with
//! latency and loss, crashed and restarted from its database, paying over
//! STREAM through BTP clients. Everything comes from one seed; a failure
//! names it, and `DASSIE_SIM_SEEDS=from..to` runs others.
//!
//! It checks what only the whole node can show: peerings and balances
//! survive restarts, payments of several times the credit cross hops, so
//! every hop settles, an owner's termination ends a peering settled, and
//! the stubs' "underlying ledger" neither makes nor loses money. One
//! payment's token has a spending cap, which the journal must show it
//! kept to.
//!
//! Each node is judged on its own, by its own books (D19): they hold what
//! its scheme holds and what its clients and receivers really moved, it
//! takes every settlement its peers send, and it settles whatever its
//! books say it owes past its threshold. No check compares two nodes'
//! books, or adds them up: each keeps its own, and they may drift.
//!
//! A node also crashes mid-payment: a sender, a hop or a receiver. Its
//! peers learn from each other's verdicts which Fulfills counted (D17); a
//! hop whose Fulfill its upstream refused books the loss.
//!
//! Outgoing payments go between nodes and to a node's own receiver: each
//! sends exactly its debit amount, across a crash of its node too, and
//! its receiver gets what it sent.
//!
//! Each run settles over the stub, or over the XRP Ledger testnet scheme
//! against a fake ledger in the same process (M7b): its connections lag,
//! lose requests and drop, and the books must still match every node's
//! account, less its reserve, with no XRP made or lost.
//!
//! Runs across currencies (M7c) give some nodes dollars, some XRP and
//! some both, at a fixed snapshot of rates, one charging a fee and a
//! spread: BTP clients pay receivers in the other currency, through the
//! nodes that convert. No ledger makes or loses any asset.
//!
//! An honest network never bans a peer for what it cost (D18), crashes and
//! all. A node that lies to its peers, refusing Fulfills it took or never
//! settling, is banned by the peer it cost, which loses at most its grace
//! limit and the credit both ways. One that spends the credit it was
//! given and vanishes costs that peer no more than that credit.

#![allow(
    clippy::disallowed_macros,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::too_many_lines,
    clippy::panic
)]

use std::collections::{BTreeMap, BTreeSet};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::Arc;

use dassie_connector::{CapUsage, SpendingCap, replay};
use dassie_identity::{NodeId, NodeKey};
use dassie_ilp::{Address, Prepare, Timestamp};
use dassie_ledger::LedgerId;
use dassie_node::engine::{
    Core, CoreParts, EngineEvent, Event, LedgerEvent, Lie, NewIncoming, NewOutgoing, Outbound,
    Output, Severity,
};
use dassie_node::node::Policies;
use dassie_node::store::{BanReason, ForeignAmount, OutgoingState, PaymentId, PeerSettings, Store};
use dassie_node_core::config::Realm;
use dassie_node_core::testing::SeededRng;
use dassie_oer::Decode;
use dassie_peer::{Bootstrap, Failure, InboundId, OwnInfo, RequestId, Response};
use dassie_settlement::policy;
use dassie_stream::{AssetDetails, BUFFER_LENGTH, Config, Connection, Policy, Role};
use dassie_xrpl::fake::{FAUCET_DROPS, FakeLedger};

const START: u64 = 1_800_000_000_000;
const CREDIT: i128 = 1_100_000_000;
/// How often the fake ledger closes a ledger.
const LEDGER_CLOSE_MS: u64 = 3_500;

/// What a run settles over.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Settling {
    Stub,
    Xrpl,
    /// Node 0 on the stub, 2 on the XRP Ledger, 1 and 3 on both (M7c).
    Cross,
}

const USD: LedgerId = LedgerId::StubUsd;
const XRP: LedgerId = LedgerId::XrplTestnetXrp;

impl Settling {
    /// The ledger a run of one ledger settles over.
    fn ledger(self) -> LedgerId {
        match self {
            Self::Stub => USD,
            Self::Xrpl => XRP,
            Self::Cross => panic!("a run across currencies has two ledgers"),
        }
    }

    /// The ledgers of node `index`: the one it pays from first.
    fn ledgers(self, index: usize) -> &'static [LedgerId] {
        match (self, index % 4) {
            (Self::Stub, _) | (Self::Cross, 0) => &[USD],
            (Self::Xrpl, _) | (Self::Cross, 2) => &[XRP],
            (Self::Cross, _) => &[USD, XRP],
        }
    }

    /// What each node's owner starts with on `ledger`: the stub's balance,
    /// or what the faucet gives above the reserve.
    const fn deposit(ledger: LedgerId) -> i128 {
        match ledger {
            LedgerId::XrplTestnetXrp => (FAUCET_DROPS as i128 - 1_000_000) * 1_000,
            _ => 100_000_000_000,
        }
    }

    const fn scheme(ledger: LedgerId) -> (&'static str, &'static str) {
        match ledger {
            LedgerId::XrplTestnetXrp => (
                "xrpl-testnet",
                r#"{"server":"ws://ledger.test","faucet":"https://faucet.test/accounts"}"#,
            ),
            _ => ("stub", ""),
        }
    }
}

/// The rates of runs across currencies: a dollar buys two XRP. A node
/// learns them when it starts, and a run is shorter than they take to age.
const RATES: [(&str, u128); 2] = [
    ("USD", 1_000_000_000_000_000),
    ("XRP", 2_000_000_000_000_000),
];

/// What node 1 charges for what passes between its peers, in runs across
/// currencies: a 1% fee and a 0.5% spread.
const CHARGES: PeerSettings = PeerSettings {
    connector_fee_ppm: 10_000,
    fx_spread_ppm: 5_000,
    ..PeerSettings::DEFAULT
};

/// A node's connection to the fake ledger.
#[derive(Clone, Copy, Debug)]
struct LedgerLink {
    epoch: u64,
    connection: u64,
}

fn key(index: usize) -> Arc<NodeKey> {
    Arc::new(NodeKey::from_bytes(
        &[u8::try_from(index + 40).unwrap(); 32],
    ))
}

fn url(index: usize) -> String {
    format!("https://n{index}.test")
}

enum Delivery {
    Request {
        from: usize,
        generation: u64,
        to: usize,
        id: RequestId,
        body: Vec<u8>,
    },
    Result {
        to: usize,
        generation: u64,
        id: RequestId,
        result: Result<Response, Failure>,
    },
    /// A request on a node's connection to the ledger.
    LedgerRequest {
        from: usize,
        connection: u64,
        message: Vec<u8>,
    },
    /// What the ledger said, or that the connection is up or down, or what
    /// the faucet did, for a node's connection `epoch`.
    LedgerEvent {
        to: usize,
        generation: u64,
        epoch: u64,
        event: LedgerEvent,
    },
}

struct Payment {
    from: usize,
    to: usize,
    /// The incoming payment it pays, and the most that one takes.
    incoming: PaymentId,
    limit: Option<u64>,
    /// The token's ID, and its cap if it has one.
    token: (u64, Option<SpendingCap>),
    /// The ledger it pays from.
    ledger: LedgerId,
    link: u64,
    connection: Connection,
    /// Its address, the receiver's, and the shared secret: what a new
    /// connection needs when the receiver closed the old one (D17).
    credentials: (String, String, [u8; 32]),
    /// What earlier connections sent.
    sent_before: u64,
    amount: u64,
    stream: Option<u64>,
    /// BTP request ID → STREAM request ID.
    in_flight: BTreeMap<u32, u64>,
    next_request: u32,
    done: bool,
}

struct SimNode {
    store: Arc<Store>,
    core: Option<Core>,
    generation: u64,
    next_inbound: u64,
    /// Inbound peer requests waiting for the core's reply.
    inbound: BTreeMap<InboundId, (usize, u64, RequestId)>,
}

struct Sim {
    now: u64,
    nodes: Vec<SimNode>,
    bootstrap: Vec<Bootstrap>,
    queue: BTreeMap<(u64, u64), Delivery>,
    sequence: u64,
    rng: SeededRng,
    loss_per_mille: u64,
    payments: Vec<Payment>,
    /// Payments whose sender's node crashed: its client is gone.
    abandoned: Vec<Payment>,
    /// Outgoing payments: from which node, to which, the incoming payment
    /// they pay and the outgoing payment.
    outgoing: Vec<(usize, usize, PaymentId, PaymentId)>,
    /// Pairs whose requests do not get through, from the first to the
    /// second.
    cut: BTreeSet<(usize, usize)>,
    /// The credit each node extends to its peers, where not the default.
    credits: BTreeMap<usize, u64>,
    /// What the run settles over.
    settling: Settling,
    /// The fake XRP Ledger, and each node's connection to it.
    ledger: FakeLedger,
    links: BTreeMap<usize, LedgerLink>,
    /// When the last message on each node's connection arrives, each way.
    fifo: BTreeMap<(usize, bool), u64>,
    next_connection: u64,
    next_close: u64,
    trace: Vec<String>,
    /// What the nodes reported as errors, or the events they lost.
    errors: Vec<String>,
}

// Every run, whatever it checks besides, ends without an error event: a
// node reports one only for what should not happen (D16).
impl Drop for Sim {
    fn drop(&mut self) {
        if !std::thread::panicking() {
            check_events(self).unwrap();
        }
    }
}

impl Sim {
    fn new(seed: u64, count: usize, bootstraps: usize) -> Self {
        Self::with(seed, count, bootstraps, BTreeMap::new(), Settling::Stub)
    }

    /// As [`Sim::new`], with some nodes extending other credit to peers.
    fn with_credits(
        seed: u64,
        count: usize,
        bootstraps: usize,
        credits: BTreeMap<usize, u64>,
    ) -> Self {
        Self::with(seed, count, bootstraps, credits, Settling::Stub)
    }

    fn with(
        seed: u64,
        count: usize,
        bootstraps: usize,
        credits: BTreeMap<usize, u64>,
        settling: Settling,
    ) -> Self {
        let bootstrap = (0..bootstraps)
            .map(|index| Bootstrap {
                id: key(index).node_id(),
                url: url(index),
                key: key(index).public_key(),
            })
            .collect();
        let mut sim = Self {
            now: START,
            nodes: Vec::new(),
            bootstrap,
            queue: BTreeMap::new(),
            sequence: 0,
            rng: SeededRng::new(seed),
            loss_per_mille: 10,
            payments: Vec::new(),
            abandoned: Vec::new(),
            outgoing: Vec::new(),
            cut: BTreeSet::new(),
            credits,
            settling,
            ledger: FakeLedger::new(),
            links: BTreeMap::new(),
            fifo: BTreeMap::new(),
            next_connection: 0,
            next_close: START + LEDGER_CLOSE_MS,
            trace: Vec::new(),
            errors: Vec::new(),
        };
        for _ in 0..count {
            sim.nodes.push(SimNode {
                store: Arc::new(Store::open_in_memory().unwrap()),
                core: None,
                generation: 0,
                next_inbound: 0,
                inbound: BTreeMap::new(),
            });
        }
        for index in 0..count {
            sim.start(index);
            for &ledger in settling.ledgers(index) {
                let (scheme, config) = Settling::scheme(ledger);
                sim.step(index, |core| {
                    core.add_scheme(scheme, config.into()).unwrap();
                });
            }
            if settling == Settling::Cross && index == 1 {
                sim.step(index, |core| core.set_peer_settings(CHARGES).unwrap());
            }
        }
        sim
    }

    fn start(&mut self, index: usize) {
        let seed = self.rng.next_u64();
        let node = &mut self.nodes[index];
        node.generation += 1;
        node.inbound.clear();
        let parts = CoreParts {
            store: Arc::clone(&node.store),
            key: key(index),
            realm: Realm::Test,
            policies: Policies {
                peer_credit: self
                    .credits
                    .get(&index)
                    .copied()
                    .unwrap_or(Policies::DEV.peer_credit),
                ..Policies::DEV
            },
            rng: Box::new(SeededRng::new(seed)),
            peer_rng: Box::new(SeededRng::new(seed ^ 1)),
            own_info: OwnInfo {
                url: Some(url(index)),
                alias: format!("n{index}"),
            },
            bootstrap: self.bootstrap.clone(),
        };
        node.core = Some(Core::start(parts, self.now).unwrap());
        self.trace.push(format!("{} start {index}", self.now));
        let now = self.now;
        let rates = (self.settling == Settling::Cross).then(|| RATES.map(|(c, r)| (c, r, now)));
        self.step(index, |core| {
            if let Some(rates) = rates {
                core.set_rates(&rates);
            }
        });
    }

    fn crash(&mut self, index: usize) {
        self.nodes[index].core = None;
        if let Some(link) = self.links.remove(&index) {
            self.ledger.disconnect(link.connection);
        }
        let (gone, kept) = std::mem::take(&mut self.payments)
            .into_iter()
            .partition(|payment| payment.from == index);
        self.payments = kept;
        self.abandoned.extend::<Vec<Payment>>(gone);
        self.trace.push(format!("{} crash {index}", self.now));
    }

    fn schedule(&mut self, delay: u64, delivery: Delivery) {
        self.sequence += 1;
        let mut at = self.now + delay;
        // A connection to the ledger is one WebSocket: what goes over it,
        // either way, arrives in order.
        let fifo = match &delivery {
            Delivery::LedgerRequest { from, .. } => Some((*from, false)),
            Delivery::LedgerEvent { to, .. } => Some((*to, true)),
            _ => None,
        };
        if let Some(fifo) = fifo {
            let last = self.fifo.entry(fifo).or_insert(0);
            at = at.max(*last);
            *last = at;
        }
        self.queue.insert((at, self.sequence), delivery);
    }

    fn latency(&mut self) -> u64 {
        1 + self.rng.next_u64() % 200
    }

    /// Runs `f` on a node's core as one step, then carries out its outputs.
    fn step(&mut self, index: usize, f: impl FnOnce(&mut Core)) {
        let now = self.now;
        let Some(core) = self.nodes[index].core.as_mut() else {
            return;
        };
        core.begin_step(now);
        f(core);
        let mut outputs = Vec::new();
        core.finish_step(&mut outputs);
        for output in outputs {
            self.carry(index, output);
        }
    }

    /// Keeps what a node reported in the trace, which a replay compares,
    /// and an error or a lost event as a failure of the run (D16).
    fn record(&mut self, index: usize, event: &Event) {
        self.trace.push(format!("{} {index} {event:?}", self.now));
        let lost = matches!(event, Event::Engine(EngineEvent::EventsDropped { .. }));
        if event.severity() == Severity::Error || lost {
            self.errors
                .push(format!("{} node {index}: {event:?}", self.now));
        }
    }

    fn carry(&mut self, index: usize, output: Output) {
        match output {
            Output::PeerRequest(request) => {
                let to = self.nodes.len();
                let to = (0..to).find(|i| url(*i) == request.url).unwrap();
                let generation = self.nodes[index].generation;
                if self.rng.next_u64() % 1000 < self.loss_per_mille
                    || self.cut.contains(&(index, to))
                {
                    let delay = self.latency();
                    let result = Err(Failure::NotDelivered);
                    self.schedule(
                        delay,
                        Delivery::Result {
                            to: index,
                            generation,
                            id: request.id,
                            result,
                        },
                    );
                    return;
                }
                let delay = self.latency();
                let body = request.body;
                self.schedule(
                    delay,
                    Delivery::Request {
                        from: index,
                        generation,
                        to,
                        id: request.id,
                        body,
                    },
                );
            }
            Output::PeerReply { inbound, reply } => {
                let Some((to, generation, id)) = self.nodes[index].inbound.remove(&inbound) else {
                    return;
                };
                let delay = self.latency();
                let result = Ok(Response {
                    status: reply.status,
                    body: reply.body,
                });
                self.schedule(
                    delay,
                    Delivery::Result {
                        to,
                        generation,
                        id,
                        result,
                    },
                );
            }
            Output::Btp { link, message } => self.deliver_to_client(index, link, message),
            Output::Event(event) => self.record(index, &event),
            Output::Trace(_) | Output::ProbeDone { .. } => {}
            Output::LedgerConnect { epoch, .. } => self.ledger_connect(index, epoch),
            Output::LedgerClose { epoch, .. } => {
                if self
                    .links
                    .get(&index)
                    .is_some_and(|link| link.epoch == epoch)
                    && let Some(link) = self.links.remove(&index)
                {
                    self.ledger.disconnect(link.connection);
                }
            }
            Output::LedgerSend { epoch, message, .. } => {
                let Some(link) = self.links.get(&index).filter(|link| link.epoch == epoch) else {
                    return;
                };
                let connection = link.connection;
                // A request may be lost; the scheme hears nothing, and
                // connects anew.
                if self.rng.next_u64() % 1000 < self.loss_per_mille {
                    return;
                }
                let delay = self.latency();
                self.schedule(
                    delay,
                    Delivery::LedgerRequest {
                        from: index,
                        connection,
                        message,
                    },
                );
            }
            Output::Fund { epoch, body, .. } => {
                let request: serde_json::Value = serde_json::from_slice(&body).unwrap();
                let to = dassie_xrpl::AccountId::parse(request["destination"].as_str().unwrap())
                    .unwrap();
                self.ledger.fund(to);
                let generation = self.nodes[index].generation;
                let delay = self.latency();
                self.schedule(
                    delay,
                    Delivery::LedgerEvent {
                        to: index,
                        generation,
                        epoch,
                        event: LedgerEvent::Funded(true),
                    },
                );
            }
        }
    }

    /// Opens a node's connection to the fake ledger.
    fn ledger_connect(&mut self, index: usize, epoch: u64) {
        if let Some(link) = self.links.remove(&index) {
            self.ledger.disconnect(link.connection);
        }
        self.next_connection += 1;
        let connection = self.next_connection;
        self.links.insert(index, LedgerLink { epoch, connection });
        let generation = self.nodes[index].generation;
        let delay = self.latency();
        self.schedule(
            delay,
            Delivery::LedgerEvent {
                to: index,
                generation,
                epoch,
                event: LedgerEvent::Connected,
            },
        );
    }

    /// A node's connection to the ledger drops; it comes back after a
    /// while, as the driver's would.
    fn drop_ledger_link(&mut self, index: usize) {
        let Some(link) = self.links.get(&index).copied() else {
            return;
        };
        self.ledger.disconnect(link.connection);
        let generation = self.nodes[index].generation;
        self.step(index, |core| {
            core.ledger_event("xrpl-testnet", link.epoch, &LedgerEvent::Disconnected);
        });
        self.trace
            .push(format!("{} ledger link {index} drops", self.now));
        self.next_connection += 1;
        let connection = self.next_connection;
        self.links.insert(
            index,
            LedgerLink {
                epoch: link.epoch,
                connection,
            },
        );
        self.schedule(
            5_000,
            Delivery::LedgerEvent {
                to: index,
                generation,
                epoch: link.epoch,
                event: LedgerEvent::Connected,
            },
        );
    }

    /// The fake ledger closes a ledger, and its streams go out.
    fn close_ledger(&mut self) {
        self.next_close = self.now + LEDGER_CLOSE_MS;
        for (connection, message) in self.ledger.close() {
            let Some((&to, link)) = self
                .links
                .iter()
                .find(|(_, link)| link.connection == connection)
            else {
                continue;
            };
            let (epoch, generation) = (link.epoch, self.nodes[to].generation);
            let delay = self.latency();
            self.schedule(
                delay,
                Delivery::LedgerEvent {
                    to,
                    generation,
                    epoch,
                    event: LedgerEvent::Message(message),
                },
            );
        }
    }

    fn deliver(&mut self, delivery: Delivery) {
        match delivery {
            Delivery::Request {
                from,
                generation,
                to,
                id,
                body,
            } => {
                if self.nodes[to].core.is_none() {
                    let delay = self.latency();
                    let result = Err(Failure::NotDelivered);
                    self.schedule(
                        delay,
                        Delivery::Result {
                            to: from,
                            generation,
                            id,
                            result,
                        },
                    );
                    return;
                }
                let node = &mut self.nodes[to];
                let inbound = InboundId(node.next_inbound);
                node.next_inbound += 1;
                node.inbound.insert(inbound, (from, generation, id));
                self.step(to, |core| core.peer_inbound(inbound, &body, None));
            }
            Delivery::LedgerRequest {
                from,
                connection,
                message,
            } => {
                let Some(link) = self.links.get(&from).copied() else {
                    return;
                };
                if link.connection != connection {
                    return;
                }
                let reply = self.ledger.request(connection, &message);
                let generation = self.nodes[from].generation;
                let delay = self.latency();
                self.schedule(
                    delay,
                    Delivery::LedgerEvent {
                        to: from,
                        generation,
                        epoch: link.epoch,
                        event: LedgerEvent::Message(reply),
                    },
                );
            }
            Delivery::LedgerEvent {
                to,
                generation,
                epoch,
                event,
            } => {
                if self.nodes[to].generation == generation {
                    self.step(to, |core| core.ledger_event("xrpl-testnet", epoch, &event));
                }
            }
            Delivery::Result {
                to,
                generation,
                id,
                result,
            } => {
                if self.nodes[to].generation == generation {
                    self.step(to, |core| core.peer_response(id, result));
                }
            }
        }
    }

    /// Runs until `until`, also steering the payments.
    fn run_until(&mut self, until: u64) {
        self.run_until_or(until, |_| false);
    }

    /// Runs until `until`, or until `stop` holds after an event; returns
    /// whether it stopped.
    fn run_until_or(&mut self, until: u64, stop: impl Fn(&Self) -> bool) -> bool {
        let mut steps = 0u64;
        loop {
            steps += 1;
            assert!(steps < 5_000_000, "no progress");
            if stop(self) {
                return true;
            }
            self.drive_payments();
            let delivery = self.queue.keys().next().map(|(at, _)| *at);
            let timer = self
                .nodes
                .iter()
                .filter_map(|node| node.core.as_ref()?.deadline())
                .min();
            let earliest = [delivery, timer].into_iter().flatten().min();
            let close = (self.settling != Settling::Stub).then_some(self.next_close);
            if close.is_some_and(|at| at <= until && earliest.is_none_or(|e| at <= e)) {
                self.now = self.now.max(self.next_close);
                self.close_ledger();
                continue;
            }
            let Some(next) = [delivery, timer]
                .into_iter()
                .flatten()
                .min()
                .filter(|at| *at <= until)
            else {
                self.now = until;
                return false;
            };
            self.now = self.now.max(next);
            if delivery == Some(next) {
                let key = *self.queue.keys().next().unwrap();
                let delivery = self.queue.remove(&key).unwrap();
                self.deliver(delivery);
                continue;
            }
            for index in 0..self.nodes.len() {
                let due = self.nodes[index]
                    .core
                    .as_ref()
                    .and_then(Core::deadline)
                    .is_some_and(|at| at <= self.now);
                if due {
                    self.step(index, |_| {});
                }
            }
        }
    }

    /// Starts a payment of `amount` from a BTP client of `from` to the
    /// receiver of `to`.
    fn pay(
        &mut self,
        from: usize,
        to: usize,
        amount: u64,
        cap: Option<SpendingCap>,
        limit: Option<u64>,
    ) {
        // Paid from the sender's first ledger, into the receiver's last.
        let ledger = self.settling.ledgers(from)[0];
        let into = *self.settling.ledgers(to).last().unwrap();
        let mut credentials = None;
        self.step(to, |core| {
            let payment = core
                .create_incoming_payment(NewIncoming {
                    ledger: into,
                    wallet_address: "https://sim.example/.well-known/pay".into(),
                    client: None,
                    incoming_amount: limit,
                    expires_at_ms: None,
                    metadata: None,
                })
                .unwrap();
            credentials = Some((payment.id, payment.ilp_address, payment.shared_secret));
        });
        let (incoming, destination, shared_secret) = credentials.unwrap();
        let mut session = None;
        self.step(from, |core| {
            let token = core
                .create_token("sim".into(), ledger.as_str(), cap)
                .unwrap();
            session = Some((token.view.id, core.btp_connect(&token.secret).unwrap()));
        });
        let (token, session) = session.unwrap();
        let credentials = (session.address.clone(), destination, shared_secret);
        let connection = client(&credentials, ledger);
        self.payments.push(Payment {
            from,
            to,
            incoming,
            limit,
            token: (token, cap),
            ledger,
            link: session.link,
            connection,
            credentials,
            sent_before: 0,
            amount,
            stream: None,
            in_flight: BTreeMap::new(),
            next_request: 1,
            done: false,
        });
        self.trace
            .push(format!("{} pay {from} -> {to} {amount}", self.now));
    }

    /// An outgoing payment of `amount` from node `from` to an incoming
    /// payment of node `to`, which may be the same node.
    fn pay_outgoing(&mut self, from: usize, to: usize, amount: u64) {
        let ledger = self.settling.ledger();
        let mut incoming = None;
        self.step(to, |core| {
            let payment = core
                .create_incoming_payment(NewIncoming {
                    ledger,
                    wallet_address: "https://sim.example/.well-known/pay".into(),
                    client: None,
                    incoming_amount: Some(amount),
                    expires_at_ms: None,
                    metadata: None,
                })
                .unwrap();
            incoming = Some(payment);
        });
        let incoming = incoming.unwrap();
        let mut outgoing = None;
        self.step(from, |core| {
            let created = core
                .create_outgoing_payment(NewOutgoing {
                    ledger,
                    wallet_address: "https://sim.example/.well-known/pay".into(),
                    client: None,
                    grant: None,
                    quote_id: None,
                    receiver: format!("https://n{to}.test/incoming-payments/{}", incoming.id),
                    destination: incoming.ilp_address.clone(),
                    shared_secret: incoming.shared_secret,
                    debit_amount: amount,
                    receive_amount: ForeignAmount {
                        value: amount,
                        asset_code: ledger.asset().code.into(),
                        asset_scale: 9,
                    },
                    metadata: None,
                    recipient: Some(format!("https://n{to}.test/.well-known/pay")),
                    fixed_debit: false,
                })
                .unwrap();
            outgoing = Some(created.row.id);
        });
        self.outgoing
            .push((from, to, incoming.id, outgoing.unwrap()));
        self.trace
            .push(format!("{} outgoing {from} -> {to} {amount}", self.now));
    }

    /// Lets every payment send what it can.
    fn drive_payments(&mut self) {
        let now = Timestamp::from_unix_ms(self.now);
        let mut buffer = vec![0u8; BUFFER_LENGTH];
        let mut rng = SeededRng::new(self.now);
        for index in 0..self.payments.len() {
            let payment = &mut self.payments[index];
            if payment.done {
                continue;
            }
            // An incoming payment that takes less stops the stream there.
            let expected = payment.amount.min(payment.limit.unwrap_or(u64::MAX));
            if sent(payment) == expected {
                payment.done = true;
                continue;
            }
            // The receiver closed the connection, when a refusal took back
            // what it had counted: a sender starts anew for the rest.
            if payment.connection.is_closed() && payment.in_flight.is_empty() {
                payment.sent_before = sent(payment);
                payment.connection = client(&payment.credentials, payment.ledger);
                payment.stream = None;
            }
            if payment.stream.is_none() && payment.connection.exchange_rate().is_some() {
                let id = payment.connection.open_stream().unwrap();
                let left = payment.amount - payment.sent_before;
                payment.connection.set_send_max(id, left).unwrap();
                payment.stream = Some(id);
            }
            let mut sends = Vec::new();
            while let Some(transmit) = payment.connection.poll_transmit(now, &mut rng, &mut buffer)
            {
                let request = payment.next_request;
                payment.next_request += 1;
                payment.in_flight.insert(request, transmit.request_id);
                sends.push((request, buffer[..transmit.length].to_vec()));
            }
            let (from, link) = (payment.from, payment.link);
            for (request, packet) in sends {
                self.step(from, |core| core.btp_packet(link, request, &packet, false));
            }
        }
    }

    /// A BTP message to a payment's client.
    fn deliver_to_client(&mut self, node: usize, link: u64, message: Outbound) {
        let now = Timestamp::from_unix_ms(self.now);
        let Some(payment) = self
            .payments
            .iter_mut()
            .find(|payment| payment.from == node && payment.link == link)
        else {
            return;
        };
        let mut buffer = vec![0u8; BUFFER_LENGTH];
        match message {
            Outbound::Response { request_id, packet } => {
                if let Some(request) = payment.in_flight.remove(&request_id) {
                    payment
                        .connection
                        .handle_response(now, request, &packet, &mut buffer);
                }
            }
            Outbound::Prepare { request_id, packet } => {
                if let Ok(prepare) = Prepare::decode_exact(&packet) {
                    let mut rng = SeededRng::new(u64::from(request_id));
                    let length =
                        payment
                            .connection
                            .handle_prepare(now, &mut rng, &prepare, &mut buffer);
                    let reply = buffer[..length].to_vec();
                    self.step(node, |core| core.btp_packet(link, request_id, &reply, true));
                }
            }
            Outbound::Error { .. } | Outbound::Close => {}
        }
    }

    fn core(&self, index: usize) -> &Core {
        self.nodes[index].core.as_ref().unwrap()
    }

    fn peers(&self, index: usize) -> BTreeSet<NodeId> {
        self.core(index)
            .network()
            .peers
            .iter()
            .map(|peer| peer.id)
            .collect()
    }

    /// Each account's net credit, by name: a run has one ledger.
    fn balances(&self, index: usize) -> BTreeMap<String, i128> {
        self.core(index)
            .account_list()
            .iter()
            .map(|(_, account)| {
                let path = account.path().to_string();
                let name = path.split_once(':').map_or(path.as_str(), |(_, name)| name);
                (name.to_string(), account.balance().net_credit())
            })
            .collect()
    }

    /// Each account's net credit on `ledger`, by name.
    fn balances_on(&self, index: usize, ledger: LedgerId) -> BTreeMap<String, i128> {
        self.core(index)
            .account_list()
            .iter()
            .filter(|(_, account)| account.path().ledger == ledger)
            .map(|(_, account)| {
                let path = account.path().to_string();
                let name = path.split_once(':').map_or(path.as_str(), |(_, name)| name);
                (name.to_string(), account.balance().net_credit())
            })
            .collect()
    }
}

// Which seeds to run is the one input read from the environment; each run
// is still fixed by its seed.
#[allow(clippy::disallowed_methods)]
fn seeds() -> std::ops::Range<u64> {
    std::env::var("DASSIE_SIM_SEEDS")
        .ok()
        .and_then(|range| {
            let (from, to) = range.split_once("..")?;
            Some(from.parse().ok()?..to.parse().ok()?)
        })
        .unwrap_or(0..6)
}

/// What a run did.
struct Run {
    /// Its trace, to compare runs by.
    trace: Vec<String>,
    /// Fulfills peers refused, as journal rows.
    refusals: usize,
    /// What hops lost to refusals.
    lost: i128,
    /// The most a peer cost a node at the end, in parts per million of the
    /// credit the node extends to it.
    worst_cost_ppm: u64,
}

/// The nodes each node banned for what they cost (D18).
fn automatic_bans(sim: &Sim) -> Vec<(usize, NodeId)> {
    let mut bans = Vec::new();
    for (index, node) in sim.nodes.iter().enumerate() {
        let Some(core) = node.core.as_ref() else {
            continue;
        };
        for (banned, record) in core.network().bans {
            if record.is_some_and(|record| record.reason == BanReason::GraceLimit) {
                bans.push((index, banned));
            }
        }
    }
    bans
}

/// The most any peer cost any node, as a share of its credit.
fn worst_cost_ppm(sim: &Sim) -> u64 {
    let mut worst = 0;
    for node in &sim.nodes {
        let Some(core) = node.core.as_ref() else {
            continue;
        };
        for peer in core.network().peers {
            if let Some(cost) = peer.cost {
                let credit = u128::from(peer.peering.credit_out.max(1));
                let ppm = u128::from(cost.total()) * 1_000_000 / credit;
                worst = worst.max(u64::try_from(ppm).unwrap_or(u64::MAX));
            }
        }
    }
    worst
}

/// The refusals in every node's journal.
fn refusals(sim: &Sim) -> usize {
    let mut count = 0;
    for node in &sim.nodes {
        let mut after = None;
        loop {
            let page = node.store.transfers_after(after, 1_000, None).unwrap();
            let Some(last) = page.last() else { break };
            after = Some(last.sequence);
            count += page
                .iter()
                .filter(|row| {
                    matches!(row.transfer.reference, dassie_ledger::Reference::Refusal(_))
                })
                .count();
        }
    }
    count
}

/// A run: the network forms, a node crashes and comes back, payments cross
/// it while another node crashes, and the books are checked.
fn run(seed: u64) -> Result<Run, String> {
    run_on(seed, Settling::Stub)
}

/// A run, settling over `settling`.
fn run_on(seed: u64, settling: Settling) -> Result<Run, String> {
    let mut rng = SeededRng::new(seed ^ 0xfeed);
    let count = 2 + usize::try_from(rng.next_u64() % 4).unwrap();
    let bootstraps = 1 + usize::try_from(rng.next_u64() % 2).unwrap().min(count - 1);
    let mut sim = Sim::with(seed, count, bootstraps, BTreeMap::new(), settling);
    sim.run_until(START + 60_000);
    // A node crashes; what it persisted brings it back.
    let victim = usize::try_from(rng.next_u64() % count as u64).unwrap();
    let before = sim.peers(victim);
    let balances = sim.balances(victim);
    sim.crash(victim);
    sim.run_until(sim.now + 5_000);
    sim.start(victim);
    if !before.is_subset(&sim.peers(victim)) || sim.balances(victim) != balances {
        return Err(format!(
            "node {victim} lost its peerings or balances in a restart"
        ));
    }
    sim.loss_per_mille = 0;
    sim.run_until(sim.now + 60_000);
    for index in 0..count {
        if sim.peers(index).is_empty() {
            return Err(format!("node {index} has no peer"));
        }
    }
    for round in 0..3 {
        let from = usize::try_from(rng.next_u64() % count as u64).unwrap();
        let to = (from + 1 + usize::try_from(rng.next_u64() % (count as u64 - 1)).unwrap()) % count;
        // Up to three times the credit a peer extends: only settling lets
        // it through.
        let amount = 100_000_000 + rng.next_u64() % 3_300_000_000;
        // One incoming payment asks for half of what its sender has: it
        // takes that, and no more.
        let limit = (round == 1).then_some(amount / 2);
        sim.pay(from, to, amount, None, limit);
    }
    // A token capped at 0.1 USD per 10 s pays 0.4 USD: it must wait for
    // its cap to refill.
    let from = usize::try_from(rng.next_u64() % count as u64).unwrap();
    let cap = SpendingCap {
        amount: 100_000_000,
        window_ms: 10_000,
    };
    sim.pay(from, (from + 1) % count, 400_000_000, Some(cap), None);
    // Outgoing payments: to another node, and to the node's own receiver.
    let from = usize::try_from(rng.next_u64() % count as u64).unwrap();
    sim.pay_outgoing(
        from,
        (from + 1) % count,
        50_000_000 + rng.next_u64() % 2_000_000_000,
    );
    sim.pay_outgoing(from, from, 10_000_000 + rng.next_u64() % 100_000_000);
    // A node's connection to its ledger drops while it settles.
    if settling == Settling::Xrpl {
        sim.run_until(sim.now + rng.next_u64() % 4_000);
        let dropped = usize::try_from(rng.next_u64() % count as u64).unwrap();
        sim.drop_ledger_link(dropped);
    }
    // A node crashes mid-payment, with packets in flight: a sender, a hop
    // or a receiver.
    sim.run_until(sim.now + 500 + rng.next_u64() % 4_000);
    let victim = usize::try_from(rng.next_u64() % count as u64).unwrap();
    sim.crash(victim);
    sim.run_until(sim.now + 1_000 + rng.next_u64() % 4_000);
    sim.start(victim);
    sim.run_until(sim.now + 120_000);
    // An owner ends a peering: it drains, ends on both sides, and what is
    // owed is settled.
    let ender = usize::try_from(rng.next_u64() % count as u64).unwrap();
    if let Some(peer) = sim.peers(ender).into_iter().next() {
        sim.trace
            .push(format!("{} {ender} terminates {peer}", sim.now));
        sim.step(ender, |core| core.terminate_peering(peer).unwrap());
        sim.run_until(sim.now + 60_000);
        if sim.peers(ender).contains(&peer) {
            return Err(format!("node {ender}'s peering with {peer} did not end"));
        }
        // Each side settled everything its own books say it owed the
        // other.
        let other = (0..count).find(|i| key(*i).node_id() == peer).unwrap();
        let mine = sim.balances(ender)[&format!("assets/interledger/{peer}")];
        let theirs = sim.balances(other)[&format!("assets/interledger/{}", key(ender).node_id())];
        // Less than the ledger can move stays owed: a drop on the XRP
        // Ledger.
        let unit = if settling == Settling::Xrpl { 1_000 } else { 1 };
        if (mine - CREDIT).abs() >= unit || (theirs - CREDIT).abs() >= unit {
            return Err(format!(
                "nodes {ender} and {other} parted owing: {mine}, {theirs}"
            ));
        }
    }
    check(&sim)?;
    Ok(Run {
        refusals: refusals(&sim),
        lost: (0..count).map(|index| -losses(&sim, index)).sum(),
        worst_cost_ppm: worst_cost_ppm(&sim),
        trace: std::mem::take(&mut sim.trace),
    })
}

/// A capped token spent at most its cap plus what refilled, over every
/// period the journal shows, and its checkpoint is the journal's replay.
fn check_cap(sim: &Sim, payment: &Payment) -> Result<(), String> {
    let (id, Some(cap)) = payment.token else {
        return Ok(());
    };
    let store = &sim.nodes[payment.from].store;
    let row = store
        .btp_tokens()
        .unwrap()
        .into_iter()
        .find(|row| row.id == id)
        .unwrap();
    let ledger = sim.settling.ledger();
    let path = format!("{ledger}:equity/owner/btp/{id}");
    let mut spent: Vec<(u64, u64)> = Vec::new();
    let mut after = None;
    loop {
        let page = store.transfers_after(after, 1_000, None).unwrap();
        let Some(last) = page.last() else { break };
        after = Some(last.sequence);
        spent.extend(
            page.iter()
                .filter(|row| row.debit_path == path)
                .filter(|row| {
                    !row.credit_path
                        .starts_with(&format!("{ledger}:equity/owner"))
                })
                .filter(|row| row.state == dassie_ledger::TransferState::Posted)
                .map(|row| (row.resolved_at_ms.unwrap(), row.transfer.amount)),
        );
    }
    spent.sort_unstable();
    let total: u64 = spent.iter().map(|(_, amount)| amount).sum();
    if total != payment.amount {
        return Err(format!(
            "the capped token spent {total} of {}",
            payment.amount
        ));
    }
    let window = u128::from(cap.window_ms);
    for (i, &(start, _)) in spent.iter().enumerate() {
        let mut total = 0u128;
        for &(at, amount) in &spent[i..] {
            total += u128::from(amount);
            let allowed = u128::from(cap.amount) * (window + u128::from(at - start));
            if total * window > allowed {
                return Err(format!(
                    "the capped token spent {total} from {start} to {at}"
                ));
            }
        }
    }
    let expected = replay(cap, CapUsage::new(row.created_at_ms), spent.iter().copied());
    if row.cap_usage != expected {
        return Err(format!(
            "the cap's checkpoint {:?} is not the journal's {expected:?}",
            row.cap_usage
        ));
    }
    Ok(())
}

/// What an incoming payment received.
fn received(sim: &Sim, payment: &Payment) -> u64 {
    sim.nodes[payment.to]
        .store
        .incoming_payment(payment.incoming)
        .unwrap()
        .map_or(0, |row| row.received_amount)
}

/// An incoming payment received at least what its sender sent, more only
/// what hops lost (checked over all payments), never more than it asked
/// for, and is complete once it has that.
fn check_incoming(sim: &Sim, payment: &Payment) -> Result<(), String> {
    let row = sim.nodes[payment.to]
        .store
        .incoming_payment(payment.incoming)
        .unwrap()
        .ok_or("an incoming payment vanished")?;
    let sent = sent(payment);
    if row.received_amount < sent {
        return Err(format!(
            "node {} received {} of the {sent} sent",
            payment.to, row.received_amount
        ));
    }
    if let Some(limit) = payment.limit
        && (row.received_amount != limit || row.completed_at_ms.is_none())
    {
        return Err(format!(
            "an incoming payment of {limit} received {} (completed {:?})",
            row.received_amount, row.completed_at_ms
        ));
    }
    Ok(())
}

fn sent(payment: &Payment) -> u64 {
    payment.sent_before
        + payment
            .stream
            .and_then(|id| payment.connection.stream(id))
            .map_or(0, |stream| stream.total_sent())
}

/// A STREAM client on `ledger` paying the receiver at `credentials`.
fn client(credentials: &(String, String, [u8; 32]), ledger: LedgerId) -> Connection {
    let (own, destination, secret) = credentials;
    let asset = ledger.asset();
    let mut connection = Connection::new(&Config {
        role: Role::Client,
        shared_secret: secret,
        local_address: Some(Address::parse(own).unwrap()),
        remote_address: Some(Address::parse(destination).unwrap()),
        local_asset: Some(AssetDetails {
            code: asset.code,
            scale: asset.scale,
        }),
        receipt: None,
        policy: Policy::DEFAULT,
    });
    connection.measure_exchange_rate();
    connection
}

/// Each owner's pool moved by exactly what its incoming payments
/// received, and each live client's token by what it sent: each node's
/// books against what its own clients and receivers really moved.
fn check_owner_books(sim: &Sim) -> Result<(), String> {
    let all = || sim.payments.iter().chain(&sim.abandoned);
    for index in 0..sim.nodes.len() {
        let balances = sim.balances(index);
        let pool = balances["equity/owner"];
        let received: i128 = all()
            .filter(|payment| payment.to == index)
            .map(|payment| i128::from(received(sim, payment)))
            .sum::<i128>()
            + outgoing_flows(sim)
                .filter(|flow| flow.to == index)
                .map(|flow| i128::from(flow.received))
                .sum::<i128>();
        let deposit = Settling::deposit(sim.settling.ledger());
        if pool - deposit != received {
            return Err(format!(
                "node {index}'s owner gained {} for {received} received",
                pool - deposit
            ));
        }
        for payment in sim.payments.iter().filter(|payment| payment.from == index) {
            let token = -balances[&format!("equity/owner/btp/{}", payment.token.0)];
            if token != i128::from(sent(payment)) {
                return Err(format!(
                    "node {index}'s token {} paid {token} for {} sent",
                    payment.token.0,
                    sent(payment)
                ));
            }
        }
        let paid: i128 = outgoing_flows(sim)
            .filter(|flow| flow.from == index)
            .map(|flow| i128::from(flow.sent))
            .sum();
        let payer = -balances.get("equity/owner/local/pay").copied().unwrap_or(0);
        if payer != paid {
            return Err(format!(
                "node {index}'s payer paid {payer} for {paid} its outgoing payments sent"
            ));
        }
        if let Some(owed) = sim.nodes[index].store.owed_fulfills().unwrap().first() {
            return Err(format!(
                "node {index} still waits for {}'s verdict on {:?}",
                owed.peer, owed.trace
            ));
        }
    }
    Ok(())
}

/// What an outgoing payment sent, and what its receiver got.
struct Flow {
    from: usize,
    to: usize,
    sent: u64,
    received: u64,
}

fn outgoing_flows(sim: &Sim) -> impl Iterator<Item = Flow> + '_ {
    sim.outgoing
        .iter()
        .map(|&(from, to, incoming, outgoing)| Flow {
            from,
            to,
            sent: sim.nodes[from]
                .store
                .outgoing_payment(outgoing)
                .unwrap()
                .map_or(0, |row| row.sent_amount),
            received: sim.nodes[to]
                .store
                .incoming_payment(incoming)
                .unwrap()
                .map_or(0, |row| row.received_amount),
        })
}

/// What a node booked as lost to refusals, as a net credit (so negative).
fn losses(sim: &Sim, index: usize) -> i128 {
    sim.balances(index)
        .get("expenses/losses")
        .copied()
        .unwrap_or(0)
}

/// No honest node owes a peer past its own threshold once the network is
/// calm, by its own books and the credit the peer announced (spec 7
/// §4.7): it settled.
fn check_settled(sim: &Sim) -> Result<(), String> {
    for index in 0..sim.nodes.len() {
        for peer in sim.core(index).network().peers {
            let Some(owed) = peer.owed.filter(|_| peer.termination.is_none()) else {
                continue;
            };
            let terms = policy::Terms {
                credit_in: peer.peering.credit_in,
                credit_out: peer.peering.credit_out,
                ratio_ppm: policy::DEFAULT_RATIO_PPM,
            };
            let position = u128::try_from(owed + i128::from(terms.credit_out)).unwrap();
            if policy::amount(position, terms) > 0 {
                return Err(format!(
                    "node {index} owes {} {owed} and never settled",
                    peer.id
                ));
            }
        }
    }
    Ok(())
}

/// No node reported an error, nor lost an event.
fn check_events(sim: &Sim) -> Result<(), String> {
    match sim.errors.as_slice() {
        [] => Ok(()),
        errors => Err(format!(
            "{} error events, the first: {}",
            errors.len(),
            errors
                .iter()
                .take(10)
                .cloned()
                .collect::<Vec<_>>()
                .join("; ")
        )),
    }
}

fn check(sim: &Sim) -> Result<(), String> {
    check_events(sim)?;
    if let Some(payment) = sim.payments.iter().find(|payment| !payment.done) {
        return Err(format!(
            "a payment from node {} to {} did not finish: sent {} of {} (limit {:?}), {:?}",
            payment.from,
            payment.to,
            sent(payment),
            payment.amount,
            payment.limit,
            payment.connection.stats(),
        ) + &format!(
            ", received {:?}, remote received {:?}, closed {}",
            sim.nodes[payment.to]
                .store
                .incoming_payment(payment.incoming)
                .unwrap()
                .map(|row| row.received_amount),
            payment
                .stream
                .and_then(|id| payment.connection.stream(id))
                .map(|stream| stream.remote_received()),
            payment.connection.is_closed(),
        ));
    }
    for payment in &sim.payments {
        check_cap(sim, payment)?;
        check_incoming(sim, payment)?;
    }
    check_outgoing(sim)?;
    check_books(sim)
}

/// Each outgoing payment completed, having sent its debit amount or what
/// its receiver still lacked when a hop lost a packet, and its receiver
/// got what it asked for.
fn check_outgoing(sim: &Sim) -> Result<(), String> {
    for &(from, to, incoming, outgoing) in &sim.outgoing {
        let row = sim.nodes[from]
            .store
            .outgoing_payment(outgoing)
            .unwrap()
            .ok_or("an outgoing payment vanished")?;
        if row.state != OutgoingState::Completed {
            let incoming_row = sim.nodes[to].store.incoming_payment(incoming).unwrap();
            return Err(format!(
                "an outgoing payment from {from} to {to} ended {:?} ({:?}) having sent {} of {}; its receiver: {incoming_row:?}",
                row.state, row.failure, row.sent_amount, row.debit_amount
            ));
        }
        let received = sim.nodes[to]
            .store
            .incoming_payment(incoming)
            .unwrap()
            .map_or(0, |row| row.received_amount);
        if received < row.sent_amount || received != row.receive_amount.value {
            return Err(format!(
                "node {to} received {received} of the {} node {from} sent, for {}",
                row.sent_amount, row.receive_amount.value
            ));
        }
    }
    Ok(())
}

/// Each node's books, on their own: they hold what its scheme holds, it
/// settled what it owed, and it banned no honest peer for what it cost.
/// The underlying ledgers neither made nor lost money. No check compares
/// two nodes' books, or adds them up: each keeps its own, and they may
/// drift (D19).
fn check_books(sim: &Sim) -> Result<(), String> {
    if let Some((index, node)) = automatic_bans(sim).first() {
        let record = sim
            .core(*index)
            .network()
            .bans
            .into_iter()
            .find(|(id, _)| id == node);
        return Err(format!(
            "node {index} banned {node} for what it cost, in an honest network: {record:?}"
        ));
    }
    check_owner_books(sim)?;
    check_settled(sim)?;
    let count = sim.nodes.len();
    let mut stubs = 0i128;
    for index in 0..count {
        let network = sim.core(index).network();
        if let Some((peer, _)) = network
            .channels
            .iter()
            .find(|(_, channel)| channel.outbox.is_some())
        {
            return Err(format!(
                "node {index}'s settlement to {peer} is still on its way"
            ));
        }
        let balances = sim.balances(index);
        // The books follow the scheme's balance: what settled was posted.
        let view = &sim.core(index).ledger_views()[0];
        let booked = -balances["assets/settlement"];
        if booked != i128::from(view.on_ledger_balance) {
            return Err(format!(
                "node {index} books {booked} in settlement, its scheme holds {}",
                view.on_ledger_balance
            ));
        }
        // On the XRP Ledger, that is the account's balance less its
        // reserve, exactly.
        if sim.settling == Settling::Xrpl {
            let drops = sim.ledger.balance(xrpl_account(index)).unwrap_or(0);
            let bookable = i128::from(drops) * 1_000 - i128::from(view.reserve);
            if booked != bookable.max(0) {
                return Err(format!(
                    "node {index} books {booked}; its account holds {drops} drops"
                ));
            }
        }
        stubs += booked;
    }
    match sim.settling {
        Settling::Stub => {
            let total = Settling::deposit(USD) * i128::try_from(count).unwrap();
            if stubs != total {
                return Err(format!("the stubs hold {stubs}, not {total}"));
            }
        }
        Settling::Xrpl => {
            if sim.ledger.total() != dassie_xrpl::MAX_DROPS {
                return Err("the ledger made or lost XRP".into());
            }
        }
        Settling::Cross => return Err("a run across currencies is checked apart".into()),
    }
    Ok(())
}

/// Node `index`'s account on the XRP Ledger: the original's wallet of its
/// key.
fn xrpl_account(index: usize) -> dassie_xrpl::AccountId {
    let entropy = key(index)
        .settlement_entropy("xrpl-testnet", "seed-testnet")
        .unwrap();
    dassie_xrpl::Wallet::from_entropy(entropy.as_bytes()).account()
}

#[test]
fn simulated_networks_pay_across_restarts() {
    for seed in seeds() {
        match catch_unwind(AssertUnwindSafe(|| run(seed))) {
            Ok(Ok(_)) => {}
            Ok(Err(problem)) => panic!("seed {seed}: {problem}"),
            Err(_) => panic!("seed {seed} panicked"),
        }
    }
}

// M7b: the same runs, settling on the XRP Ledger testnet scheme against
// the fake ledger.
#[test]
fn simulated_networks_settle_on_the_xrp_ledger() {
    for seed in seeds() {
        match catch_unwind(AssertUnwindSafe(|| run_on(seed, Settling::Xrpl))) {
            Ok(Ok(_)) => {}
            Ok(Err(problem)) => panic!("seed {seed}: {problem}"),
            Err(_) => panic!("seed {seed} panicked"),
        }
    }
}

#[test]
fn a_seed_replays_the_same_run() {
    assert_eq!(run(2).unwrap().trace, run(2).unwrap().trace);
}

// The crashes mid-payment are not vacuous: some Fulfills are refused, and
// some hop books a loss, which the checks then account for.
#[test]
fn crashes_mid_payment_bring_refusals_and_losses() {
    let runs: Vec<Run> = (0..40).map(|seed| run(seed).unwrap()).collect();
    assert!(
        runs.iter().any(|run| run.refusals > 0),
        "no refusal in 40 seeds"
    );
    assert!(runs.iter().any(|run| run.lost > 0), "no loss in 40 seeds");
}

/// Two nodes, peered, and a client of node 0 paying node 1's receiver,
/// run until node 1 owes node 0 a verdict: it posted a packet and wrote
/// the Fulfill down, which is on its way.
fn owing(seed: u64) -> Sim {
    owing_after(seed, |_| {})
}

/// As [`owing`], with `before` done to the network once the payment knows
/// its rate, before any money moves.
fn owing_after(seed: u64, before: impl FnOnce(&mut Sim)) -> Sim {
    let mut sim = Sim::new(seed, 2, 1);
    sim.loss_per_mille = 0;
    sim.run_until(START + 60_000);
    assert!(!sim.peers(0).is_empty(), "the nodes did not peer");
    sim.pay(0, 1, 500_000_000, None, None);
    let rate = |sim: &Sim| sim.payments[0].connection.exchange_rate().is_some();
    assert!(sim.run_until_or(sim.now + 30_000, rate), "no rate");
    before(&mut sim);
    let owes = |sim: &Sim| !sim.nodes[1].store.owed_fulfills().unwrap().is_empty();
    assert!(sim.run_until_or(sim.now + 30_000, owes), "nothing owed");
    sim
}

// D17: the hop crashes once it posted, before the verdict comes back. The
// upstream took the Fulfill; asked again after the restart, it says so
// from its record, and nothing is refused.
#[test]
fn a_hop_that_crashes_after_posting_learns_the_verdict() {
    let mut sim = owing(70);
    sim.crash(1);
    sim.run_until(sim.now + 2_000);
    sim.start(1);
    sim.run_until(sim.now + 120_000);
    check(&sim).unwrap();
    assert_eq!(refusals(&sim), 0);
}

// D17: the upstream crashes before the Fulfill reaches it, and forgets the
// packet. Asked after its restart, it refuses: the receiver's side reverses
// the packet, and the payment does not count it.
#[test]
fn an_upstream_that_forgot_the_packet_refuses_it() {
    let mut sim = owing(71);
    sim.crash(0);
    sim.run_until(sim.now + 3_000);
    sim.start(0);
    sim.run_until(sim.now + 60_000);
    check_books(&sim).unwrap();
    assert!(refusals(&sim) > 0);
    assert_eq!(losses(&sim, 1), 0, "a reversal, not a loss");
}

// D17: the Fulfill gets through only after the upstream's expiry, with no
// crash at all: the upstream refuses it, and the downstream books what it
// said.
#[test]
fn a_late_fulfill_is_refused_and_the_downstream_books_it() {
    let mut sim = owing_after(72, |sim| {
        sim.cut.insert((1, 0));
    });
    sim.run_until(sim.now + 40_000);
    sim.cut.clear();
    sim.run_until(sim.now + 120_000);
    check(&sim).unwrap();
    assert!(refusals(&sim) > 0);
}

// D17: a peering being ended waits for the verdicts it is owed, which
// still come once the peers reach each other again.
#[test]
fn a_peering_ends_only_once_its_verdicts_came() {
    let mut sim = owing_after(73, |sim| {
        sim.cut.insert((1, 0));
    });
    let peer = key(1).node_id();
    sim.step(0, |core| core.terminate_peering(peer).unwrap());
    sim.run_until(sim.now + 40_000);
    assert!(
        sim.peers(1).contains(&key(0).node_id()),
        "node 1 ended the peering with verdicts owed"
    );
    sim.cut.clear();
    sim.run_until(sim.now + 60_000);
    assert!(sim.peers(1).is_empty() && sim.peers(0).is_empty());
    check_books(&sim).unwrap();
}

// D17: verdicts hold up an ending peering for 10 minutes at most; asked
// for over the settlement channel after, they still come.
#[test]
fn a_silent_peer_holds_up_its_end_for_a_while_only() {
    let mut sim = owing_after(74, |sim| {
        sim.cut.insert((1, 0));
    });
    let peer = key(1).node_id();
    sim.step(0, |core| core.terminate_peering(peer).unwrap());
    sim.run_until(sim.now + 12 * 60_000);
    assert!(sim.peers(1).is_empty(), "the peering never ended");
    assert!(!sim.nodes[1].store.owed_fulfills().unwrap().is_empty());
    sim.cut.clear();
    sim.run_until(sim.now + 2 * 60 * 60_000);
    check_books(&sim).unwrap();
}

// Found in review: a receiver's details may come from another wallet, and
// what the rows cannot hold must be refused before the engine takes it,
// or its commit fails and the engine stops.
#[test]
fn outgoing_payments_refuse_what_their_rows_cannot_hold() {
    let mut sim = Sim::new(80, 1, 1);
    sim.run_until(START + 1_000);
    let long = "g.x".repeat(400);
    let mut refusals = Vec::new();
    for destination in ["", "not an address", long.as_str()] {
        sim.step(0, |core| {
            let created = core.create_outgoing_payment(NewOutgoing {
                ledger: LedgerId::StubUsd,
                wallet_address: "https://sim.example/.well-known/pay".into(),
                client: None,
                grant: None,
                quote_id: None,
                receiver: "https://elsewhere.example/incoming-payments/1".into(),
                destination: destination.to_string(),
                shared_secret: [1; 32],
                debit_amount: 10,
                receive_amount: ForeignAmount {
                    value: 10,
                    asset_code: "USD".into(),
                    asset_scale: 9,
                },
                metadata: None,
                recipient: None,
                fixed_debit: false,
            });
            refusals.push(created.is_err());
        });
    }
    assert_eq!(refusals, [true, true, true]);
    assert!(sim.nodes[0].core.is_some(), "the engine kept running");
}

// Each outgoing payment has a STREAM connection of its own: a second one
// with the credentials of the first is refused, while the first sends and
// after, since the receiver would tell each what both delivered.
#[test]
fn outgoing_payments_never_share_credentials() {
    let mut sim = Sim::new(81, 1, 1);
    sim.run_until(START + 1_000);
    let ledger = sim.settling.ledger();
    let mut incoming = None;
    sim.step(0, |core| {
        incoming = Some(
            core.create_incoming_payment(NewIncoming {
                ledger,
                wallet_address: "https://sim.example/.well-known/pay".into(),
                client: None,
                incoming_amount: Some(20),
                expires_at_ms: None,
                metadata: None,
            })
            .unwrap(),
        );
    });
    let incoming = incoming.unwrap();
    let pay = |destination: &str, shared_secret| NewOutgoing {
        ledger,
        wallet_address: "https://sim.example/.well-known/pay".into(),
        client: None,
        grant: None,
        quote_id: None,
        receiver: format!("https://n0.test/incoming-payments/{}", incoming.id),
        destination: destination.to_string(),
        shared_secret,
        debit_amount: 10,
        receive_amount: ForeignAmount {
            value: 10,
            asset_code: ledger.asset().code.into(),
            asset_scale: 9,
        },
        metadata: None,
        recipient: None,
        fixed_debit: false,
    };
    let mut fresh = None;
    sim.step(0, |core| {
        fresh = Some(core.incoming_credentials(incoming.id, ledger).unwrap());
    });
    let fresh = fresh.unwrap();
    assert_ne!(fresh.destination, incoming.ilp_address);
    let mut created = Vec::new();
    for (destination, secret) in [
        (incoming.ilp_address.as_str(), incoming.shared_secret),
        (incoming.ilp_address.as_str(), incoming.shared_secret),
        (fresh.destination.as_str(), fresh.shared_secret),
    ] {
        sim.step(0, |core| {
            created.push(
                core.create_outgoing_payment(pay(destination, secret))
                    .is_ok(),
            );
        });
    }
    assert_eq!(created, [true, false, true]);
    // Once the first is done too.
    sim.run_until(START + 120_000);
    sim.step(0, |core| {
        let again =
            core.create_outgoing_payment(pay(&incoming.ilp_address, incoming.shared_secret));
        assert!(again.is_err());
    });
}

// D18: honest peers cost each other only through crashes and late
// Fulfills, which stay below the default grace limit. A crash of an
// upstream with packets in flight is what costs most.
#[test]
fn honest_peers_stay_below_their_grace_limits() {
    let mut costs: Vec<u64> = seeds()
        .map(|seed| run(seed).unwrap_or_else(|problem| panic!("seed {seed}: {problem}")))
        .map(|run| run.worst_cost_ppm)
        .collect();
    costs.sort_unstable();
    let worst = costs.last().copied().unwrap_or(0);
    let costly = costs.iter().filter(|cost| **cost > 0).count();
    eprintln!(
        "the most a peer cost: {worst} ppm of its credit; {costly} of {} runs cost anything",
        costs.len()
    );
    assert!(
        worst < u64::from(PeerSettings::DEFAULT.grace_share_ppm),
        "an honest peer cost {worst} ppm of its credit"
    );
}

/// Three nodes where the liar, node 1, reaches node 2 only through node
/// 0: node 2 bans it before they can peer. Node 1 then lies.
fn liar_network(seed: u64, lie: Lie) -> Sim {
    let mut sim = Sim::new(seed, 3, 1);
    sim.loss_per_mille = 0;
    sim.step(2, |core| core.ban_node(key(1).node_id()).unwrap());
    sim.run_until(START + 60_000);
    let (victim, target) = (key(0).node_id(), key(2).node_id());
    assert!(
        sim.peers(1).contains(&victim) && sim.peers(2).contains(&victim),
        "seed {seed}: the nodes did not peer"
    );
    assert!(
        !sim.peers(1).contains(&target),
        "seed {seed}: the ban did not hold"
    );
    sim.step(1, |core| core.lie(lie));
    sim.trace.push(format!("{} 1 lies: {lie:?}", sim.now));
    sim
}

/// Whether node 0 banned node 1 for what it cost.
fn victim_banned_liar(sim: &Sim) -> bool {
    automatic_bans(sim)
        .iter()
        .any(|(index, node)| *index == 0 && *node == key(1).node_id())
}

/// What node 1 cost node 0, as node 0's ban says.
fn liar_cost(sim: &Sim) -> dassie_node::store::PeerCost {
    sim.core(0)
        .network()
        .bans
        .into_iter()
        .find(|(node, _)| *node == key(1).node_id())
        .and_then(|(_, record)| record?.cost)
        .expect("a ban with what the liar cost")
}

/// What a liar tries to pay: far more than the credit both ways and the
/// grace limit, so the exposure bound is put to the test.
const LIAR_PAYS: u64 = 10_000_000_000;

/// What node 1 cost node 0 is at most its grace limit and the credit both
/// ways (D18): its losses to the liar, and what the liar owes it.
fn check_exposure(sim: &Sim, seed: u64) {
    let liar = key(1).node_id();
    let balances = sim.balances(0);
    let lost = -losses(sim, 0);
    let position = balances[&format!("assets/interledger/{liar}")];
    let owes = (CREDIT - position).max(0);
    let grace = CREDIT * i128::from(PeerSettings::DEFAULT.grace_share_ppm) / 1_000_000;
    assert!(
        lost + owes <= grace + 2 * CREDIT,
        "seed {seed}: the liar cost {lost} lost and {owes} owed, past {grace} + 2 × {CREDIT}"
    );
}

// D18: a peer that takes Fulfills and says it refused them costs the hop
// that paid on, which bans it once its losses pass the grace limit.
#[test]
fn a_peer_that_refuses_what_it_took_is_banned() {
    for seed in 0..20 {
        let mut sim = liar_network(seed, Lie::RefuseFulfills);
        sim.pay(1, 2, LIAR_PAYS, None, None);
        assert!(
            sim.run_until_or(sim.now + 120_000, victim_banned_liar),
            "seed {seed}: node 0 never banned the liar; it lost {}",
            -losses(&sim, 0)
        );
        let cost = liar_cost(&sim);
        assert!(
            cost.refused > cost.grace && cost.overdue == 0,
            "seed {seed}: banned for {cost:?}"
        );
        // The ban ends the peering, and only the liar's: once the verdicts
        // it is owed came, or after ten minutes.
        let ending = sim
            .core(0)
            .network()
            .peers
            .iter()
            .any(|peer| peer.id == key(1).node_id() && peer.termination.is_some());
        assert!(ending, "seed {seed}: the peering is not ending");
        // It keeps paying on: the bound must hold however long it goes.
        sim.run_until(sim.now + 12 * 60_000);
        check_exposure(&sim, seed);
        assert!(!sim.peers(0).contains(&key(1).node_id()), "seed {seed}");
        assert!(sim.peers(0).contains(&key(2).node_id()), "seed {seed}");
    }
}

// D18: a peer that never settles what it owes is banned once its debt has
// stayed past 1.1 times half its credit for longer than the grace for
// settling.
#[test]
fn a_peer_that_never_settles_is_banned() {
    for seed in 0..20 {
        let mut sim = liar_network(seed, Lie::NeverSettle);
        sim.pay(1, 0, LIAR_PAYS, None, None);
        // The liar owes past where it should have settled from here on.
        let due = |sim: &Sim| {
            let position = sim.balances(0)[&format!("assets/interledger/{}", key(1).node_id())];
            CREDIT - position > CREDIT * 55 / 100
        };
        assert!(sim.run_until_or(sim.now + 120_000, due), "seed {seed}");
        let due_at = sim.now;
        assert!(
            sim.run_until_or(sim.now + 10 * 60_000, victim_banned_liar),
            "seed {seed}: node 0 never banned the liar"
        );
        let grace = Policies::DEV.grace.settle_grace_ms;
        assert!(
            sim.now - due_at >= grace,
            "seed {seed}: banned before its grace for settling passed"
        );
        let cost = liar_cost(&sim);
        assert!(
            cost.overdue > cost.grace && cost.refused == 0,
            "seed {seed}: banned for {cost:?}"
        );
        sim.run_until(sim.now + 60_000);
        check_exposure(&sim, seed);
        assert_eq!(losses(&sim, 0), 0, "seed {seed}");
    }
}

// D19: a peer that spends the credit it was given and vanishes, as a
// retiring node may, costs the peer it spent through no more than that
// credit. That peer ends the peering once it stops answering, and the rest
// of the network loses nothing.
#[test]
fn a_peer_that_spends_its_credit_and_vanishes_costs_at_most_that_credit() {
    let owes = |sim: &Sim| {
        let position = sim.balances(0)[&format!("assets/interledger/{}", key(1).node_id())];
        CREDIT - position
    };
    for seed in 0..20 {
        let mut sim = liar_network(seed, Lie::NeverSettle);
        sim.pay(1, 2, LIAR_PAYS, None, None);
        assert!(
            sim.run_until_or(sim.now + 120_000, |sim| owes(sim) > CREDIT * 9 / 10),
            "seed {seed}: it never spent its credit"
        );
        sim.crash(1);
        sim.run_until(sim.now + 10 * 60_000);
        assert!(
            !sim.peers(0).contains(&key(1).node_id()),
            "seed {seed}: node 0 still peers with it"
        );
        let owed = owes(&sim);
        assert!(owed <= CREDIT, "seed {seed}: it left owing {owed}");
        assert_eq!(losses(&sim, 0), 0, "seed {seed}");
        assert_eq!(losses(&sim, 2), 0, "seed {seed}");
        assert!(sim.peers(0).contains(&key(2).node_id()), "seed {seed}");
    }
}

// Found in review: a peer that extends us little credit settles only past
// half the credit we extend, at 1.1 times half. Owing short of that, it
// is not overdue, however long it waits.
#[test]
fn an_honest_peer_with_little_credit_is_not_overdue() {
    let mut credits = BTreeMap::new();
    credits.insert(1, 1);
    let mut sim = Sim::with_credits(90, 2, 1, credits);
    sim.loss_per_mille = 0;
    sim.run_until(START + 60_000);
    assert!(
        sim.peers(0).contains(&key(1).node_id()),
        "the nodes did not peer"
    );
    // With credit this uneven, node 0 settled half of it to node 1 as
    // they peered, so node 1 owes it half (spec 5 §4.6). Node 1 then
    // pays 0.03 more: it owes more than half of node 0's credit of 1.1,
    // and less than the 0.605 it settles at, 1.1 × 1.1 / 2.
    sim.pay(1, 0, 30_000_000, None, None);
    sim.run_until(sim.now + 10 * Policies::DEV.grace.settle_grace_ms);
    let peer = sim
        .core(0)
        .network()
        .peers
        .into_iter()
        .find(|peer| peer.id == key(1).node_id())
        .unwrap();
    let owes = CREDIT - peer.owed.map(|owed| owed + CREDIT).unwrap();
    assert!(owes > CREDIT / 2, "it owes {owes}");
    assert_eq!(peer.cost.unwrap().overdue, 0);
    assert!(automatic_bans(&sim).is_empty());
}

impl Sim {
    /// An outgoing payment across currencies, of a fixed `debit` from
    /// `from`'s first ledger to a receiver on `to`'s last, at no worse than
    /// 1.9 XRP a dollar, or 0.45 dollars an XRP.
    fn pay_outgoing_across(&mut self, from: usize, to: usize, debit: u64) {
        let ledger = self.settling.ledgers(from)[0];
        let into = *self.settling.ledgers(to).last().unwrap();
        let mut incoming = None;
        self.step(to, |core| {
            let payment = core
                .create_incoming_payment(NewIncoming {
                    ledger: into,
                    wallet_address: "https://sim.example/.well-known/pay".into(),
                    client: None,
                    incoming_amount: None,
                    expires_at_ms: None,
                    metadata: None,
                })
                .unwrap();
            incoming = Some(payment);
        });
        let incoming = incoming.unwrap();
        let receive = match (ledger, into) {
            (USD, XRP) => debit / 10 * 19,
            (XRP, USD) => debit / 100 * 45,
            _ => debit / 100 * 95,
        };
        let mut outgoing = None;
        self.step(from, |core| {
            let created = core
                .create_outgoing_payment(NewOutgoing {
                    ledger,
                    wallet_address: "https://sim.example/.well-known/pay".into(),
                    client: None,
                    grant: None,
                    quote_id: None,
                    receiver: format!("https://n{to}.test/incoming-payments/{}", incoming.id),
                    destination: incoming.ilp_address.clone(),
                    shared_secret: incoming.shared_secret,
                    debit_amount: debit,
                    receive_amount: ForeignAmount {
                        value: receive,
                        asset_code: into.asset().code.into(),
                        asset_scale: 9,
                    },
                    metadata: None,
                    recipient: None,
                    fixed_debit: true,
                })
                .unwrap();
            outgoing = Some(created.row.id);
        });
        self.outgoing
            .push((from, to, incoming.id, outgoing.unwrap()));
        self.trace
            .push(format!("{} across {from} -> {to} {debit}", self.now));
    }
}

/// A run across currencies (M7c): node 0 has dollars, node 2 XRP, nodes 1
/// and 3 both, node 1 charging for what passes. Clients pay receivers in
/// the other currency, an outgoing payment crosses too, and a node
/// crashes mid-payment.
fn run_across(seed: u64) -> Result<Run, String> {
    let mut rng = SeededRng::new(seed ^ 0xc055);
    let count = 3 + usize::try_from(rng.next_u64() % 2).unwrap();
    let mut sim = Sim::with(seed, count, 2, BTreeMap::new(), Settling::Cross);
    sim.run_until(START + 60_000);
    sim.loss_per_mille = 0;
    sim.run_until(sim.now + 60_000);
    // Dollars to XRP, and back; each up to twice the credit, so the hops
    // settle on both ledgers.
    let amount = |rng: &mut SeededRng| 100_000_000 + rng.next_u64() % 2_200_000_000;
    let there = amount(&mut rng);
    sim.pay(0, 2, there, None, None);
    let back = amount(&mut rng);
    sim.pay(2, 0, back, None, None);
    // Into a node's own receiver in the other currency: it converts, and
    // charges nothing.
    let own = amount(&mut rng);
    sim.pay(0, 1, own, None, None);
    let debit = 50_000_000 + rng.next_u64() % 1_000_000_000;
    sim.pay_outgoing_across(2, 0, debit);
    // A node crashes mid-payment.
    sim.run_until(sim.now + 500 + rng.next_u64() % 4_000);
    let victim = usize::try_from(rng.next_u64() % count as u64).unwrap();
    sim.crash(victim);
    sim.run_until(sim.now + 1_000 + rng.next_u64() % 4_000);
    sim.start(victim);
    sim.run_until(sim.now + 180_000);
    check_across(&sim)?;
    Ok(Run {
        refusals: refusals(&sim),
        lost: 0,
        worst_cost_ppm: worst_cost_ppm(&sim),
        trace: std::mem::take(&mut sim.trace),
    })
}

/// A run across currencies: every payment finished, each node settled
/// what it owed and took every settlement, and no ledger made or lost an
/// asset. Each node is judged by its own books (D19).
fn check_across(sim: &Sim) -> Result<(), String> {
    check_events(sim)?;
    if let Some(payment) = sim.payments.iter().find(|payment| !payment.done) {
        return Err(format!(
            "a payment from node {} to {} did not finish: sent {} of {}",
            payment.from,
            payment.to,
            sent(payment),
            payment.amount
        ));
    }
    // What arrived is what was sent at the rates, less what the hops
    // charged; more only if a hop lost a Fulfill to a crash.
    for payment in &sim.payments {
        let into = *sim.settling.ledgers(payment.to).last().unwrap();
        let (got, sent) = (
            u128::from(received(sim, payment)),
            u128::from(sent(payment)),
        );
        let least = match (payment.ledger, into) {
            (USD, XRP) => sent * 19 / 10,
            (XRP, USD) => sent * 45 / 100,
            _ => sent * 95 / 100,
        };
        if got < least {
            return Err(format!(
                "node {} received {got} of node {}'s {sent}, {:?} to {into:?}",
                payment.to, payment.from, payment.ledger
            ));
        }
    }
    for &(from, to, _, outgoing) in &sim.outgoing {
        let row = sim.nodes[from]
            .store
            .outgoing_payment(outgoing)
            .unwrap()
            .ok_or("an outgoing payment vanished")?;
        if row.state != OutgoingState::Completed || row.sent_amount != row.debit_amount {
            return Err(format!(
                "an outgoing payment from {from} to {to} ended {:?} ({:?}) having sent {} of {}",
                row.state, row.failure, row.sent_amount, row.debit_amount
            ));
        }
    }
    if let Some((index, node)) = automatic_bans(sim).first() {
        return Err(format!("node {index} banned {node} in an honest network"));
    }
    let count = sim.nodes.len();
    check_settled(sim)?;
    for ledger in [USD, XRP] {
        let (mut schemes, mut nodes) = (0i128, 0i128);
        for index in (0..count).filter(|i| sim.settling.ledgers(*i).contains(&ledger)) {
            nodes += 1;
            schemes -= sim.balances_on(index, ledger)["assets/settlement"];
            if let Some(owed) = sim.nodes[index].store.owed_fulfills().unwrap().first() {
                return Err(format!(
                    "node {index} still waits for {}'s verdict on {:?}",
                    owed.peer, owed.trace
                ));
            }
        }
        let made = match ledger {
            USD => schemes != Settling::deposit(USD) * nodes,
            _ => sim.ledger.total() != dassie_xrpl::MAX_DROPS,
        };
        if made {
            return Err(format!(
                "{ledger} was made or lost: the schemes hold {schemes}"
            ));
        }
    }
    // Node 1 charged for what passed between its peers.
    let fees: i128 = [USD, XRP]
        .iter()
        .filter_map(|ledger| sim.balances_on(1, *ledger).get("revenue/fees").copied())
        .sum();
    let through_one =
        sim.peers(0).contains(&key(1).node_id()) || sim.peers(2).contains(&key(1).node_id());
    if through_one && fees <= 0 && count == 3 {
        return Err("node 1 charged nothing for what passed".into());
    }
    Ok(())
}

#[test]
fn simulated_networks_pay_across_currencies() {
    for seed in seeds() {
        match catch_unwind(AssertUnwindSafe(|| run_across(seed))) {
            Ok(Ok(_)) => {}
            Ok(Err(problem)) => panic!("seed {seed}: {problem}"),
            Err(_) => panic!("seed {seed} panicked"),
        }
    }
}

// A run that reports an error fails, whatever else it checks.
#[test]
fn an_error_event_fails_the_run() {
    let mut sim = Sim::new(1, 2, 1);
    let node = key(1).node_id();
    sim.record(
        0,
        &dassie_node::engine::PeerEvent::BanListFull { node }.into(),
    );
    assert!(check_events(&sim).is_err());
    sim.record(0, &EngineEvent::EventsDropped { count: 1 }.into());
    assert_eq!(sim.errors.len(), 2);
    sim.errors.clear();
}
