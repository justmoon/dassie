//! A simulated network of real engine cores
//! (spec/areas/testing.md#deterministic-simulation), for this crate's tests and
//! the simulation runner, dassie-sim (D27): each node's core with its own
//! in-memory database, wired through a virtual network with latency, loss
//! and replayed requests, each on a clock of its own, crashed and restarted
//! from its database, paying over STREAM through BTP clients. Everything
//! comes from one seed.
//!
//! It checks what only the whole node can show: peerings and balances
//! survive restarts, payments of several times the credit cross hops, so
//! every hop settles, an owner's termination ends a peering with nothing
//! settled for its end (D28), and
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
//! A node also crashes mid-payment: a sender, a hop or a receiver. Each
//! node books the Fulfills it sent as final, whether or not its upstream
//! took them (D23), and its books still hold against what its own clients
//! and receivers moved.
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
//! A node that never settles, or spends the credit it was given and
//! vanishes, costs its peer no more than that credit (D23).
//!
//! A node's commit can be made to fail, the one fault injected at the
//! store (D27): its core stops without handing over anything of the step,
//! as it does in the daemon, and the node is down until restarted from
//! its database, as after a crash.
//!
//! Test code, behind the `testing` feature: a panic here is a failed run.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::too_many_lines,
    clippy::panic
)]

pub mod checks;
pub mod scripted;
pub mod swarm;

use std::cell::Cell;
use std::collections::{BTreeMap, BTreeSet};
use std::panic::{AssertUnwindSafe, catch_unwind, resume_unwind};
use std::sync::{Arc, Once};

use dassie_connector::SpendingCap;
use dassie_identity::{NodeId, NodeKey};
use dassie_ilp::{Address, Prepare, Timestamp};
use dassie_ledger::LedgerId;
use dassie_node_core::config::Realm;
use dassie_node_core::testing::SeededRng;
use dassie_oer::Decode;
use dassie_peer::{Bootstrap, Failure, InboundId, OwnInfo, RequestId, Response};
use dassie_settlement::policy;
use dassie_stream::{AssetDetails, BUFFER_LENGTH, Config, Connection, Policy, Role};
use dassie_xrpl::fake::{FAUCET_DROPS, FakeLedger};

use crate::engine::{
    Core, CoreParts, EngineEvent, Event, LedgerEvent, NewIncoming, NewOutgoing, Outbound, Output,
    SettlementEvent, Severity,
};
use crate::node::Policies;
use crate::store::{Aim, FailedCommit, ForeignAmount, Holds, PaymentId, PeerSettings, Store};

/// When every simulation starts.
pub const START: u64 = 1_800_000_000_000;
/// The credit a node extends by default.
pub const CREDIT: i128 = 1_100_000_000;
/// Every node's trigger, one and a half of the largest packets (D25).
pub const TRIGGER: u64 = policy::trigger(Policies::DEFAULT.connector.max_packet_amount);
/// How long a payment's client waits to measure the exchange rate again,
/// after a measurement failed, or to connect again, after its connection
/// closed.
pub const RATE_RETRY_MS: u64 = 5_000;
/// How often the fake ledger closes a ledger.
pub const LEDGER_CLOSE_MS: u64 = 3_500;
/// The longest a replayed request arrives after the original: past the
/// peer protocol's replay window of 60 s, so that some copies are refused
/// for their age rather than as copies.
pub const REPLAY_WITHIN_MS: u64 = 90_000;

/// What a run settles over.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Settling {
    /// The stub scheme, in dollars.
    Stub,
    /// The XRP Ledger testnet scheme, against the fake ledger.
    Xrpl,
    /// Node 0 on the stub, 2 on the XRP Ledger, 1 and 3 on both (M7c).
    Cross,
}

/// The stub ledger, in dollars.
pub const USD: LedgerId = LedgerId::StubUsd;
/// The XRP Ledger testnet.
pub const XRP: LedgerId = LedgerId::XrplTestnetXrp;

impl Settling {
    /// The ledger a run of one ledger settles over.
    pub fn ledger(self) -> LedgerId {
        match self {
            Self::Stub => USD,
            Self::Xrpl => XRP,
            Self::Cross => panic!("a run across currencies has two ledgers"),
        }
    }

    /// The ledgers of node `index`: the one it pays from first.
    pub fn ledgers(self, index: usize) -> &'static [LedgerId] {
        match (self, index % 4) {
            (Self::Stub, _) | (Self::Cross, 0) => &[USD],
            (Self::Xrpl, _) | (Self::Cross, 2) => &[XRP],
            (Self::Cross, _) => &[USD, XRP],
        }
    }

    /// What each node's owner starts with on `ledger`: the stub's balance,
    /// or what the faucet gives above the reserve.
    pub const fn deposit(ledger: LedgerId) -> i128 {
        match ledger {
            LedgerId::XrplTestnetXrp => (FAUCET_DROPS as i128 - 1_000_000) * 1_000,
            _ => 100_000_000_000,
        }
    }

    /// The scheme of `ledger`, and its configuration.
    pub const fn scheme(ledger: LedgerId) -> (&'static str, &'static str) {
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
pub const RATES: [(&str, u128); 2] = [
    ("USD", 1_000_000_000_000_000),
    ("XRP", 2_000_000_000_000_000),
];

/// What node 1 charges for what passes between its peers, in runs across
/// currencies: a 1% fee and a 0.5% spread.
pub const CHARGES: PeerSettings = PeerSettings {
    connector_fee_ppm: 10_000,
    fx_spread_ppm: 5_000,
};

/// A node's connection to the fake ledger.
#[derive(Clone, Copy, Debug)]
struct LedgerLink {
    epoch: u64,
    connection: u64,
}

/// Node `index`'s key, the same in every run.
pub fn key(index: usize) -> Arc<NodeKey> {
    Arc::new(NodeKey::from_bytes(
        &[u8::try_from(index + 40).unwrap(); 32],
    ))
}

/// Node `index`'s URL.
pub fn url(index: usize) -> String {
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
    /// A copy of a request, as an attacker who saw it would send it again:
    /// its answer goes to no one.
    Replay {
        from: usize,
        to: usize,
        /// When its original arrived.
        arrived: u64,
        /// Its original, in [`Sim::originals`].
        original: usize,
        body: Vec<u8>,
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

/// What became of a replayed request's original.
#[derive(Clone, Copy, Debug)]
enum Original {
    /// Not answered yet, by the node in its `generation`: if the node
    /// crashed since, it was lost.
    Pending(u64),
    /// Answered with this status.
    Answered(u16),
}

/// A payment by a BTP client of one node to the receiver of another.
pub struct Payment {
    /// The node whose client pays.
    pub from: usize,
    /// The node whose receiver is paid.
    pub to: usize,
    /// The incoming payment it pays.
    pub incoming: PaymentId,
    /// The most that incoming payment takes, if it has a limit.
    pub limit: Option<u64>,
    /// The token's ID, and its cap if it has one.
    pub token: (u64, Option<SpendingCap>),
    /// The ledger it pays from.
    pub ledger: LedgerId,
    link: u64,
    /// The client's STREAM connection.
    pub connection: Connection,
    /// Its address, the receiver's, and the shared secret: what a new
    /// connection needs when the old one closed.
    credentials: (String, String, [u8; 32]),
    /// What earlier connections sent.
    sent_before: u64,
    /// How much it sends.
    pub amount: u64,
    stream: Option<u64>,
    /// BTP request ID → STREAM request ID.
    in_flight: BTreeMap<u32, u64>,
    next_request: u32,
    /// Whether it sent all it meant to.
    pub done: bool,
    /// Whether its incoming payment expired before it got all it asked
    /// for, so that its client stopped there.
    pub expired: bool,
    /// When its client measures the exchange rate again, after a
    /// measurement failed, or connects again, after its connection closed.
    rate_retry_at: Option<u64>,
}

/// A node: its database, and its core while it runs.
pub struct SimNode {
    /// Its database, which outlives a crash.
    pub store: Arc<Store>,
    /// Its engine core, or `None` while it is down.
    pub core: Option<Core>,
    /// How far its clock is ahead of the network's, or behind it.
    pub skew_ms: i64,
    generation: u64,
    next_inbound: u64,
    /// Inbound peer requests waiting for the core's reply.
    inbound: BTreeMap<InboundId, (usize, u64, RequestId)>,
    /// Inbound requests that are copied later: the original's place in
    /// [`Sim::originals`].
    copied: BTreeMap<InboundId, usize>,
    /// Copies waiting for the core's reply, which goes nowhere: their
    /// original's place in [`Sim::originals`], and whether it only reads.
    replays: BTreeMap<InboundId, (usize, bool)>,
    /// The commit it is to fail, while it has one.
    failing: Option<FailedCommit>,
}

/// The simulated network: its nodes, its clock, its messages, the fake
/// ledger, and the payments under way.
pub struct Sim {
    /// The network's clock.
    pub now: u64,
    /// Every node, by index.
    pub nodes: Vec<SimNode>,
    bootstrap: Vec<Bootstrap>,
    queue: BTreeMap<(u64, u64), Delivery>,
    sequence: u64,
    rng: SeededRng,
    /// How many of a thousand messages are lost.
    pub loss_per_mille: u64,
    /// How many of a thousand requests that arrive are replayed, up to
    /// [`REPLAY_WITHIN_MS`] later.
    pub replay_per_mille: u64,
    /// What became of each replayed request's original.
    originals: Vec<Original>,
    /// The payments under way or done, of nodes that did not crash.
    pub payments: Vec<Payment>,
    /// Payments whose sender's node crashed: its client is gone.
    pub abandoned: Vec<Payment>,
    /// Outgoing payments: from which node, to which, the incoming payment
    /// they pay and the outgoing payment.
    pub outgoing: Vec<(usize, usize, PaymentId, PaymentId)>,
    /// Pairs whose requests do not get through, from the first to the
    /// second.
    pub cut: BTreeSet<(usize, usize)>,
    /// While the network is partitioned, the nodes on one side of it: no
    /// request gets from a node on one side to a node on the other.
    pub apart: BTreeSet<usize>,
    /// The credit each node extends to its peers, where not the default.
    credits: BTreeMap<usize, u64>,
    /// What the run settles over.
    pub settling: Settling,
    /// The fake XRP Ledger, and each node's connection to it.
    pub ledger: FakeLedger,
    links: BTreeMap<usize, LedgerLink>,
    /// When the last message on each node's connection arrives, each way.
    fifo: BTreeMap<(usize, bool), u64>,
    next_connection: u64,
    next_close: u64,
    /// What happened, a line per event, to compare runs by.
    pub trace: Vec<String>,
    /// What the nodes reported as errors, or the events they lost.
    pub errors: Vec<String>,
    /// Outgoing payments a node refused although its owner could pay them.
    pub refused: Vec<String>,
    /// Nodes whose connection to the ledger carries nothing.
    pub ledger_down: BTreeSet<usize>,
    /// The peer protocol's settings.
    peer: dassie_peer::Config,
    /// Each withdrawal of a peer short of room: when, by which node, of
    /// which peer.
    pub withdrawn: Vec<(u64, usize, NodeId)>,
    /// The shortest and the longest a message takes, in milliseconds.
    pub latency_ms: (u64, u64),
    /// How many times the run reached each thing it counts (D27).
    pub counts: BTreeMap<&'static str, u64>,
    /// How many times the faucet funded each node: again after a crash cut
    /// its first request short.
    pub fundings: BTreeMap<usize, u64>,
    /// Outgoing payments that may end failed: those whose payer gave up,
    /// as it does after its reconnects, or was reconnecting, while the
    /// network was faulty.
    pub given_up: BTreeSet<PaymentId>,
    /// The nodes that are to fail a commit for [`Sim::fail_commit_anywhere`]:
    /// once one of them has, the others let their commits through.
    anywhere: BTreeSet<usize>,
    /// The node and the copy of a request it takes in the step under way.
    replaying: Option<(usize, InboundId)>,
}

// Every run, whatever it checks besides, ends without an error event: a
// node reports one only for what should not happen (D16).
impl Drop for Sim {
    fn drop(&mut self) {
        if !std::thread::panicking() {
            checks::check_events(self).unwrap();
        }
    }
}

impl Sim {
    /// `count` nodes over the stub, the first `bootstraps` of them
    /// bootstrap nodes.
    pub fn new(seed: u64, count: usize, bootstraps: usize) -> Self {
        Self::with(seed, count, bootstraps, BTreeMap::new(), Settling::Stub)
    }

    /// `count` nodes settling over `settling`, extending `credits` where
    /// not the default.
    pub fn with(
        seed: u64,
        count: usize,
        bootstraps: usize,
        credits: BTreeMap<usize, u64>,
        settling: Settling,
    ) -> Self {
        Self::with_peer(
            seed,
            count,
            bootstraps,
            credits,
            settling,
            Policies::DEFAULT.peer,
        )
    }

    /// As [`Sim::with`], with the peer protocol's settings `peer`.
    pub fn with_peer(
        seed: u64,
        count: usize,
        bootstraps: usize,
        credits: BTreeMap<usize, u64>,
        settling: Settling,
        peer: dassie_peer::Config,
    ) -> Self {
        Self::with_skews(seed, bootstraps, credits, settling, peer, &vec![0; count])
    }

    /// As [`Sim::with_peer`], a node for each of `skews_ms`, its clock that
    /// far ahead of the network's, or behind it.
    pub fn with_skews(
        seed: u64,
        bootstraps: usize,
        credits: BTreeMap<usize, u64>,
        settling: Settling,
        peer: dassie_peer::Config,
        skews_ms: &[i64],
    ) -> Self {
        let count = skews_ms.len();
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
            replay_per_mille: 0,
            originals: Vec::new(),
            payments: Vec::new(),
            abandoned: Vec::new(),
            outgoing: Vec::new(),
            cut: BTreeSet::new(),
            apart: BTreeSet::new(),
            credits,
            settling,
            ledger: FakeLedger::new(),
            links: BTreeMap::new(),
            fifo: BTreeMap::new(),
            next_connection: 0,
            next_close: START + LEDGER_CLOSE_MS,
            trace: Vec::new(),
            errors: Vec::new(),
            refused: Vec::new(),
            ledger_down: BTreeSet::new(),
            peer,
            withdrawn: Vec::new(),
            latency_ms: (1, 200),
            counts: BTreeMap::new(),
            fundings: BTreeMap::new(),
            given_up: BTreeSet::new(),
            anywhere: BTreeSet::new(),
            replaying: None,
        };
        for &skew_ms in skews_ms {
            sim.nodes.push(SimNode {
                store: Arc::new(Store::open_in_memory().unwrap()),
                core: None,
                skew_ms,
                generation: 0,
                next_inbound: 0,
                inbound: BTreeMap::new(),
                copied: BTreeMap::new(),
                replays: BTreeMap::new(),
                failing: None,
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

    /// Starts node `index` from its database.
    pub fn start(&mut self, index: usize) {
        let seed = self.rng.next_u64();
        let now = self.local_now(index);
        let node = &mut self.nodes[index];
        node.generation += 1;
        node.inbound.clear();
        node.copied.clear();
        node.replays.clear();
        let parts = CoreParts {
            store: Arc::clone(&node.store),
            key: key(index),
            realm: Realm::Test,
            policies: Policies {
                peer_credit: self
                    .credits
                    .get(&index)
                    .copied()
                    .unwrap_or(Policies::DEFAULT.peer_credit),
                peer: self.peer,
                ..Policies::DEFAULT
            },
            rng: Box::new(SeededRng::new(seed)),
            peer_rng: Box::new(SeededRng::new(seed ^ 1)),
            own_info: OwnInfo {
                url: Some(url(index)),
                alias: format!("n{index}"),
                quic: false,
            },
            bootstrap: self.bootstrap.clone(),
        };
        node.core = Some(Core::start(parts, now).unwrap());
        self.trace.push(format!("{} start {index}", self.now));
        let rates = (self.settling == Settling::Cross).then(|| RATES.map(|(c, r)| (c, r, now)));
        self.step(index, |core| {
            if let Some(rates) = rates {
                core.set_rates(&rates);
            }
        });
    }

    /// Crashes node `index`: its clients are gone, its database stays.
    pub fn crash(&mut self, index: usize) {
        self.count("crashes");
        self.stop(index);
        self.trace.push(format!("{} crash {index}", self.now));
    }

    /// Makes node `index` fail the commit after the next `skip` that `aim`
    /// aims at (D27). The node stops in the step that commits it, and is
    /// down until restarted.
    pub fn fail_commit(&mut self, index: usize, aim: Aim, skip: u32) {
        let node = &mut self.nodes[index];
        assert!(node.core.is_some(), "a node that runs fails a commit");
        node.failing = Some(node.store.fail_commit(aim, skip));
        self.trace.push(format!(
            "{} {index} is to fail a commit, after {skip}: {aim:?}",
            self.now
        ));
    }

    /// Makes the first running node to commit what `aim` aims at, after
    /// its next `skip` such commits, fail that commit (D27): for a moment
    /// few nodes reach at any one time, such as a settlement. Nodes that
    /// have a commit to fail already are left as they are.
    pub fn fail_commit_anywhere(&mut self, aim: Aim, skip: u32) {
        for (index, node) in self.nodes.iter_mut().enumerate() {
            if node.core.is_some() && node.failing.is_none() {
                node.failing = Some(node.store.fail_commit(aim, skip));
                self.anywhere.insert(index);
            }
        }
        self.trace.push(format!(
            "{} a node is to fail a commit, after {skip}: {aim:?}",
            self.now
        ));
    }

    /// Whether node `index` has a commit to fail.
    pub fn failing(&self, index: usize) -> bool {
        self.nodes[index].failing.is_some()
    }

    /// Lets every commit through again: no node fails one any more.
    pub fn let_commits_through(&mut self) {
        for index in 0..self.nodes.len() {
            self.let_through(index);
        }
        self.trace.push(format!("{} commits go through", self.now));
    }

    /// Lets node `index`'s commits through again.
    fn let_through(&mut self, index: usize) {
        let node = &mut self.nodes[index];
        if node.failing.take().is_some() {
            node.store.let_commits_through();
        }
        self.anywhere.remove(&index);
    }

    /// Node `index`'s commit failed, and its core stopped without handing
    /// over anything of the step, as it does in the daemon. What it was
    /// answering gets the 503 the daemon's peer endpoint answers when the
    /// engine stops before its reply (`http/peer.rs`).
    fn commit_failed(&mut self, index: usize, held: Holds) {
        self.count("failed commits");
        if self.anywhere.contains(&index) {
            for other in std::mem::take(&mut self.anywhere) {
                self.let_through(other);
            }
        }
        // What the lost commit held: the floors on these counts prove that
        // a packet's commit and a settlement's commit failed.
        if held.packet {
            self.count("failed commits mid-payment");
        }
        if held.settlement {
            self.count("failed commits mid-settlement");
        }
        self.trace
            .push(format!("{} {index} failed a commit and stopped", self.now));
        let inbound = std::mem::take(&mut self.nodes[index].inbound);
        for (inbound, (to, generation, id)) in inbound {
            if let Some(original) = self.nodes[index].copied.remove(&inbound) {
                self.originals[original] = Original::Answered(503);
            }
            let delay = self.latency();
            let result = Ok(Response {
                status: 503,
                body: b"Service Unavailable".to_vec(),
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
        self.stop(index);
    }

    /// Stops node `index`: its clients are gone, its database stays, and
    /// its commits go through again once it restarts.
    fn stop(&mut self, index: usize) {
        self.nodes[index].core = None;
        self.let_through(index);
        // A copy it was still working on passed the replay check. The copy
        // whose own step failed a commit may not have: what it did was
        // lost with its answer.
        let replays = std::mem::take(&mut self.nodes[index].replays);
        for (inbound, (original, reads)) in replays {
            if self.replaying == Some((index, inbound)) {
                self.trace.push(format!(
                    "{} {index} lost a replay with its commit",
                    self.now
                ));
                continue;
            }
            self.replay_answered(index, original, reads, None);
        }
        if let Some(link) = self.links.remove(&index) {
            self.ledger.disconnect(link.connection);
        }
        let (gone, kept) = std::mem::take(&mut self.payments)
            .into_iter()
            .partition(|payment| payment.from == index);
        self.payments = kept;
        self.abandoned.extend::<Vec<Payment>>(gone);
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

    /// Node `index` answered a copy of a request with `status`, or crashed
    /// still working on it (`None`). Refused as a replay or for its age
    /// (401), it is refused. Anything else passed the replay check: taken,
    /// it must be a copy of a request that had no effect, one refused, or
    /// lost when the node crashed before answering it. A copy of one
    /// answered 200, or of one still under way, is taken twice, and fails
    /// the run for safety, unless it only reads (`reads`): from a sender
    /// whose replay state is not kept for good, such a request is not
    /// recorded, and taking it twice changes nothing
    /// (spec/areas/peer-protocol.md#replay-protection). A 503 for want of
    /// a place for the sender's replay state also comes before the check,
    /// but every sender here is one of a few nodes, which always have one.
    fn replay_answered(&mut self, index: usize, original: usize, reads: bool, status: Option<u16>) {
        self.trace.push(format!(
            "{} {index} answers the replay {status:?}",
            self.now
        ));
        if status == Some(401) {
            self.count("replays refused");
            return;
        }
        if reads {
            self.count("replays of requests that only read");
            return;
        }
        let twice = match self.originals[original] {
            Original::Answered(answered) => answered == 200,
            Original::Pending(generation) => generation == self.nodes[index].generation,
        };
        if twice {
            self.errors.push(format!(
                "{} node {index} took a replayed request twice: answered {status:?}, \
                 its original {:?}",
                self.now, self.originals[original]
            ));
        } else {
            self.count("replays of requests that had no effect");
        }
    }

    /// The time on node `index`'s clock.
    fn local_now(&self, index: usize) -> u64 {
        self.now.saturating_add_signed(self.nodes[index].skew_ms)
    }

    /// When node `index`'s core is next due, on the network's clock.
    fn deadline(&self, index: usize) -> Option<u64> {
        let node = &self.nodes[index];
        let local = node.core.as_ref()?.deadline()?;
        Some(local.saturating_add_signed(-node.skew_ms))
    }

    fn latency(&mut self) -> u64 {
        let (shortest, longest) = self.latency_ms;
        shortest + self.rng.next_u64() % (longest - shortest + 1)
    }

    /// Counts one more of `what`.
    pub fn count(&mut self, what: &'static str) {
        *self.counts.entry(what).or_default() += 1;
    }

    /// Runs `f` on a node's core as one step, then carries out its outputs.
    /// Says whether the node took the step: not while it is down, nor when
    /// the step's commit failed and stopped it.
    pub fn step(&mut self, index: usize, f: impl FnOnce(&mut Core)) -> bool {
        let now = self.local_now(index);
        let node = &mut self.nodes[index];
        let failing = node.failing.clone();
        let Some(core) = node.core.as_mut() else {
            return false;
        };
        let mut outputs = Vec::new();
        let step = || {
            core.begin_step(now);
            f(core);
            core.finish_step(&mut outputs);
        };
        match failing {
            None => step(),
            Some(failed) => {
                if let Err(panic) = catching(step) {
                    // Only the commit made to fail stops the node; any
                    // other panic fails the run.
                    let message = panic.downcast_ref::<String>().map(String::as_str);
                    let held = failed.held().filter(|_| {
                        message.is_some_and(|message| message.starts_with(COMMIT_FAILED))
                    });
                    let Some(held) = held else {
                        resume_unwind(panic);
                    };
                    self.commit_failed(index, held);
                    return false;
                }
            }
        }
        for output in outputs {
            self.carry(index, output);
        }
        true
    }

    /// Keeps what a node reported in the trace, which a replay compares,
    /// and an error or a lost event as a failure of the run (D16).
    pub fn record(&mut self, index: usize, event: &Event) {
        if matches!(
            event,
            Event::Settlement(SettlementEvent::PeerSettled { .. })
        ) {
            self.count("settlements taken");
        }
        if let Event::PeerNode(dassie_peer::Event::Depeering { reason, .. }) = event {
            match reason {
                dassie_peer::wire::TerminationReason::Unresponsive => {
                    self.count("peerings ended for silence")
                }
                dassie_peer::wire::TerminationReason::ShortOfRoom => {
                    self.count("peerings ended for room")
                }
                _ => {}
            }
        }
        self.trace.push(format!("{} {index} {event:?}", self.now));
        if let Event::PeerNode(dassie_peer::Event::Advertised {
            peer, short: true, ..
        }) = event
        {
            self.withdrawn.push((self.now, index, *peer));
        }
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
                // When a node asks a peer to settle, so that the checks can
                // tell how long it waited for the answer (D28).
                if dassie_peer::wire::peek_kind(&request.body)
                    == Some(dassie_peer::wire::Kind::SettlementRequest)
                {
                    self.trace
                        .push(format!("{} {index} asks {to} to settle", self.now));
                }
                if self.rng.next_u64() % 1000 < self.loss_per_mille
                    || self.cut.contains(&(index, to))
                    || self.apart.contains(&index) != self.apart.contains(&to)
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
                if let Some((original, reads)) = self.nodes[index].replays.remove(&inbound) {
                    self.replay_answered(index, original, reads, Some(reply.status));
                    return;
                }
                if let Some(original) = self.nodes[index].copied.remove(&inbound) {
                    self.originals[original] = Original::Answered(reply.status);
                }
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
                if self.ledger_down.contains(&index) {
                    return;
                }
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
                *self.fundings.entry(index).or_default() += 1;
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
    pub fn drop_ledger_link(&mut self, index: usize) {
        let Some(link) = self.links.get(&index).copied() else {
            return;
        };
        self.ledger.disconnect(link.connection);
        let generation = self.nodes[index].generation;
        let stepped = self.step(index, |core| {
            core.ledger_event("xrpl-testnet", link.epoch, &LedgerEvent::Disconnected);
        });
        if !stepped {
            return;
        }
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
                // Someone who saw it arrive sends it again, later: a request
                // that never arrived was never sent, so no one has it. A run
                // without replays draws nothing for them.
                if self.replay_per_mille > 0 && self.rng.next_u64() % 1000 < self.replay_per_mille {
                    let original = self.originals.len();
                    let node = &mut self.nodes[to];
                    self.originals.push(Original::Pending(node.generation));
                    node.copied.insert(InboundId(node.next_inbound), original);
                    let later = 1 + self.rng.next_u64() % REPLAY_WITHIN_MS;
                    let replay = Delivery::Replay {
                        from,
                        to,
                        arrived: self.now,
                        original,
                        body: body.clone(),
                    };
                    self.schedule(later, replay);
                }
                let node = &mut self.nodes[to];
                let inbound = InboundId(node.next_inbound);
                node.next_inbound += 1;
                node.inbound.insert(inbound, (from, generation, id));
                self.step(to, |core| core.peer_inbound(inbound, &body, None));
            }
            Delivery::Replay {
                from,
                to,
                arrived,
                original,
                body,
            } => {
                if self.nodes[to].core.is_none() {
                    return;
                }
                self.count("replays");
                self.trace.push(format!(
                    "{} replay to {to} of {from}'s request that arrived at {arrived}",
                    self.now
                ));
                let reads = dassie_peer::wire::peek_kind(&body)
                    .is_some_and(dassie_peer::wire::Kind::is_read_only);
                let node = &mut self.nodes[to];
                let inbound = InboundId(node.next_inbound);
                node.next_inbound += 1;
                node.replays.insert(inbound, (original, reads));
                self.replaying = Some((to, inbound));
                self.step(to, |core| core.peer_inbound(inbound, &body, None));
                self.replaying = None;
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
    pub fn run_until(&mut self, until: u64) {
        self.run_until_or(until, |_| false);
    }

    /// Runs until `until`, or until `stop` holds after an event; returns
    /// whether it stopped.
    pub fn run_until_or(&mut self, until: u64, stop: impl Fn(&Self) -> bool) -> bool {
        let mut steps = 0u64;
        loop {
            steps += 1;
            assert!(steps < 5_000_000, "no progress");
            if stop(self) {
                return true;
            }
            self.drive_payments();
            let delivery = self.queue.keys().next().map(|(at, _)| *at);
            let timer = (0..self.nodes.len())
                .filter_map(|index| self.deadline(index))
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
                if self.deadline(index).is_some_and(|at| at <= self.now) {
                    self.step(index, |_| {});
                }
            }
        }
    }

    /// Starts a payment of `amount` from a BTP client of `from` to the
    /// receiver of `to`. Says whether it started: not when a failed commit
    /// stopped either node before its part was on disk.
    pub fn pay(
        &mut self,
        from: usize,
        to: usize,
        amount: u64,
        cap: Option<SpendingCap>,
        limit: Option<u64>,
    ) -> bool {
        // Paid from the sender's first ledger, into the receiver's last.
        let ledger = self.settling.ledgers(from)[0];
        let into = *self.settling.ledgers(to).last().unwrap();
        let mut credentials = None;
        let created = self.step(to, |core| {
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
        if !created {
            return false;
        }
        let (incoming, destination, shared_secret) = credentials.unwrap();
        let mut session = None;
        let connected = self.step(from, |core| {
            let token = core
                .create_token("sim".into(), ledger.as_str(), cap)
                .unwrap();
            session = Some((token.view.id, core.btp_connect(&token.secret).unwrap()));
        });
        if !connected {
            return false;
        }
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
            expired: false,
            rate_retry_at: None,
        });
        self.trace
            .push(format!("{} pay {from} -> {to} {amount}", self.now));
        true
    }

    /// An outgoing payment of `amount` from node `from` to an incoming
    /// payment of node `to`, which may be the same node.
    ///
    /// # Errors
    ///
    /// When node `from` refuses it, as it does while its owner has too
    /// little to pay it: on the XRP Ledger, before the faucet funded it.
    /// Or when a failed commit stopped either node before its part was on
    /// disk.
    pub fn pay_outgoing(&mut self, from: usize, to: usize, amount: u64) -> Result<(), String> {
        let ledger = self.settling.ledger();
        let mut incoming = None;
        let stopped = |index: usize| format!("node {index} stopped");
        let created = self.step(to, |core| {
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
        if !created {
            return Err(stopped(to));
        }
        let incoming = incoming.unwrap();
        let mut outgoing = None;
        let sent = self.step(from, |core| {
            let created = core.create_outgoing_payment(NewOutgoing {
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
            });
            outgoing = Some(created.map(|created| created.row.id));
        });
        if !sent {
            return Err(stopped(from));
        }
        let outgoing = outgoing.unwrap().map_err(|error| format!("{error:?}"))?;
        self.outgoing.push((from, to, incoming.id, outgoing));
        self.trace
            .push(format!("{} outgoing {from} -> {to} {amount}", self.now));
        Ok(())
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
            // Its receiver may have all of it before the sender counts it:
            // a hop that crashed counted a Fulfill the sender never got,
            // and the receiver says so (D23).
            // The receiver reports what its connection key delivered, over
            // every connection the client made with the same credentials:
            // it always counts against a limit, the payment's or the most
            // any payment takes.
            let expected = payment.amount.min(payment.limit.unwrap_or(u64::MAX));
            let remote = payment
                .stream
                .and_then(|id| payment.connection.stream(id))
                .map_or(0, dassie_stream::Stream::remote_received);
            if sent(payment) == expected || remote >= expected {
                payment.done = true;
                continue;
            }
            // The connection closed, its receiver gone or out of reach: a
            // sender starts anew for the rest, a while later.
            let closed = payment.connection.is_closed() && payment.in_flight.is_empty();
            if closed {
                payment
                    .rate_retry_at
                    .get_or_insert(self.now + RATE_RETRY_MS);
            }
            // A measurement of the rate fails when no probe gets through,
            // as across a cut link. The library reports it and leaves the
            // rest to its user: a sender tries again a while later.
            while let Some(event) = payment.connection.poll_event() {
                if event == dassie_stream::Event::ExchangeRateMeasurementFailed {
                    payment.rate_retry_at = Some(self.now + RATE_RETRY_MS);
                }
            }
            if payment.rate_retry_at.is_some_and(|at| at <= self.now) {
                payment.rate_retry_at = None;
                // Its receiver takes nothing more once its incoming payment
                // expired, as one does a day after it was made unless it
                // says otherwise: a sender that tries again stops there,
                // with what got there. The trace names it, since the checks
                // then no longer ask for all of its limit.
                if lapsed(&self.nodes, self.now, payment) {
                    payment.done = true;
                    payment.expired = true;
                    *self.counts.entry("payments expired").or_default() += 1;
                    self.trace.push(format!(
                        "{} pay {} -> {} stops, its incoming payment expired, having sent {}",
                        self.now,
                        payment.from,
                        payment.to,
                        sent(payment)
                    ));
                    continue;
                }
                if closed {
                    payment.sent_before = sent(payment);
                    payment.connection = client(&payment.credentials, payment.ledger);
                    payment.stream = None;
                } else {
                    payment.connection.measure_exchange_rate();
                }
            } else if closed {
                continue;
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
                // A node that stops abandons its payments, which moves the
                // rest: they go on at the next turn.
                if !self.step(from, |core| core.btp_packet(link, request, &packet, false)) {
                    return;
                }
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

    /// Node `index`'s core, which must be running.
    pub fn core(&self, index: usize) -> &Core {
        self.nodes[index].core.as_ref().unwrap()
    }

    /// Whether node `index` lists `peer` as its neighbor.
    pub fn advertises(&self, index: usize, peer: &NodeId) -> bool {
        self.core(index)
            .network()
            .peers
            .iter()
            .any(|view| view.id == *peer && view.advertised)
    }

    /// Node `index`'s peers.
    pub fn peers(&self, index: usize) -> BTreeSet<NodeId> {
        self.core(index)
            .network()
            .peers
            .iter()
            .map(|peer| peer.id)
            .collect()
    }

    /// Each account's net credit, by name: a run has one ledger.
    pub fn balances(&self, index: usize) -> BTreeMap<String, i128> {
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

    /// What node `index`'s owner deposited on the run's one ledger: the
    /// stub's balance, or what the faucet gave above the reserve, each time
    /// it funded the node.
    pub fn deposit(&self, index: usize) -> i128 {
        match self.settling.ledger() {
            LedgerId::XrplTestnetXrp => {
                let fundings = i128::from(self.fundings.get(&index).copied().unwrap_or(0));
                (fundings * i128::from(FAUCET_DROPS) - 1_000_000).max(0) * 1_000
            }
            ledger => Settling::deposit(ledger),
        }
    }

    /// What node `index` owes `peer` net, by its own books: its position
    /// less the credit it extends.
    pub fn owes(&self, index: usize, peer: &NodeId) -> i128 {
        let balances = self.balances(index);
        balances[&format!("assets/interledger/{peer}")] + balances[&format!("contra/trust/{peer}")]
    }

    /// Each account's net credit on `ledger`, by name.
    pub fn balances_on(&self, index: usize, ledger: LedgerId) -> BTreeMap<String, i128> {
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

/// How a core's panic starts when a commit fails (`engine/core/mod.rs`).
const COMMIT_FAILED: &str = "persisting the ledger failed";

thread_local! {
    /// Whether this thread runs a step whose commit is made to fail.
    static FAILING: Cell<bool> = const { Cell::new(false) };
}

/// Runs a step whose commit is made to fail, catching the panic that
/// stops the core. That panic is the fault injected, not a failure of the
/// run, so it is not printed; any other panic is, by the hook there was.
fn catching(step: impl FnOnce()) -> std::thread::Result<()> {
    static QUIET: Once = Once::new();
    QUIET.call_once(|| {
        let print = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            let injected = FAILING.get()
                && info
                    .payload()
                    .downcast_ref::<String>()
                    .is_some_and(|message| message.starts_with(COMMIT_FAILED));
            if !injected {
                print(info);
            }
        }));
    });
    FAILING.set(true);
    let result = catch_unwind(AssertUnwindSafe(step));
    FAILING.set(false);
    result
}

/// Whether `payment`'s incoming payment expired before it received all
/// it asked for, on its receiver's clock.
fn lapsed(nodes: &[SimNode], now: u64, payment: &Payment) -> bool {
    let to = &nodes[payment.to];
    to.store
        .incoming_payment(payment.incoming)
        .unwrap()
        .is_some_and(|row| {
            row.completed_at_ms.is_none()
                && row.expires_at_ms <= now.saturating_add_signed(to.skew_ms)
        })
}

/// What an incoming payment received.
pub fn received(sim: &Sim, payment: &Payment) -> u64 {
    sim.nodes[payment.to]
        .store
        .incoming_payment(payment.incoming)
        .unwrap()
        .map_or(0, |row| row.received_amount)
}

/// What a payment's client sent, over all its connections.
pub fn sent(payment: &Payment) -> u64 {
    payment.sent_before
        + payment
            .stream
            .and_then(|id| payment.connection.stream(id))
            .map_or(0, |stream| stream.total_sent())
}

/// A STREAM client on `ledger` paying the receiver at `credentials`.
pub fn client(credentials: &(String, String, [u8; 32]), ledger: LedgerId) -> Connection {
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

/// Node `index`'s account on the XRP Ledger: the original's wallet of its
/// key.
pub fn xrpl_account(index: usize) -> dassie_xrpl::AccountId {
    let entropy = key(index)
        .settlement_entropy("xrpl-testnet", "seed-testnet")
        .unwrap();
    dassie_xrpl::Wallet::from_entropy(entropy.as_bytes()).account()
}

impl Sim {
    /// An outgoing payment across currencies, of a fixed `debit` from
    /// `from`'s first ledger to a receiver on `to`'s last, at no worse than
    /// 1.9 XRP a dollar, or 0.45 dollars an XRP.
    pub fn pay_outgoing_across(&mut self, from: usize, to: usize, debit: u64) {
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
