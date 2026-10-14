//! A simulated network of real engine cores (spec 7 §7): each node's core
//! with its own in-memory database, wired through a virtual network with
//! latency and loss, crashed and restarted from its database, paying over
//! STREAM through BTP clients. Everything comes from one seed; a failure
//! names it, and `DASSIE_SIM_SEEDS=from..to` runs others.
//!
//! It checks what only the whole node can show: peerings and balances
//! survive restarts, payments of several times the credit cross hops, so
//! every hop settles, an owner's termination ends a peering settled, every
//! pair of accounts mirrors each other, and the money is conserved: the
//! owners', and the stubs' on their "underlying ledger". One payment's
//! token has a spending cap, which the journal must show it kept to.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use std::collections::{BTreeMap, BTreeSet};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::Arc;

use dassie_connector::{CapUsage, SpendingCap, replay};
use dassie_identity::{NodeId, NodeKey};
use dassie_ilp::{Address, Prepare, Timestamp};
use dassie_ledger::LedgerId;
use dassie_node::engine::{Core, CoreParts, NewIncoming, Outbound, Output};
use dassie_node::node::Policies;
use dassie_node::store::Store;
use dassie_node_core::config::Realm;
use dassie_node_core::testing::SeededRng;
use dassie_oer::Decode;
use dassie_peer::{Bootstrap, Failure, InboundId, OwnInfo, RequestId, Response};
use dassie_stream::{AssetDetails, BUFFER_LENGTH, Config, Connection, Policy, Role};

const START: u64 = 1_800_000_000_000;
const DEPOSIT: i128 = 100_000_000_000;
const CREDIT: i128 = 1_100_000_000;

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
}

struct Payment {
    from: usize,
    /// The token's ID, and its cap if it has one.
    token: (u64, Option<SpendingCap>),
    link: u64,
    connection: Connection,
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
    trace: Vec<String>,
}

impl Sim {
    fn new(seed: u64, count: usize, bootstraps: usize) -> Self {
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
            trace: Vec::new(),
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
            sim.step(index, |core| {
                core.add_scheme("stub", String::new()).unwrap();
            });
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
            policies: Policies::DEV,
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
        self.step(index, |_| {});
    }

    fn crash(&mut self, index: usize) {
        self.nodes[index].core = None;
        self.payments.retain(|payment| payment.from != index);
        self.trace.push(format!("{} crash {index}", self.now));
    }

    fn schedule(&mut self, delay: u64, delivery: Delivery) {
        self.sequence += 1;
        self.queue
            .insert((self.now + delay, self.sequence), delivery);
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

    fn carry(&mut self, index: usize, output: Output) {
        match output {
            Output::PeerRequest(request) => {
                let to = self.nodes.len();
                let to = (0..to).find(|i| url(*i) == request.url).unwrap();
                let generation = self.nodes[index].generation;
                if self.rng.next_u64() % 1000 < self.loss_per_mille {
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
            Output::Trace(_) => {}
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
        let mut steps = 0u64;
        loop {
            steps += 1;
            assert!(steps < 5_000_000, "no progress");
            self.drive_payments();
            let delivery = self.queue.keys().next().map(|(at, _)| *at);
            let timer = self
                .nodes
                .iter()
                .filter_map(|node| node.core.as_ref()?.deadline())
                .min();
            let Some(next) = [delivery, timer]
                .into_iter()
                .flatten()
                .min()
                .filter(|at| *at <= until)
            else {
                self.now = until;
                return;
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
    fn pay(&mut self, from: usize, to: usize, amount: u64, cap: Option<SpendingCap>) {
        let mut credentials = None;
        self.step(to, |core| {
            let payment = core
                .create_incoming_payment(NewIncoming {
                    ledger: LedgerId::StubUsd,
                    wallet_address: "https://sim.example/.well-known/pay".into(),
                    client: None,
                    incoming_amount: None,
                    expires_at_ms: None,
                    metadata: None,
                })
                .unwrap();
            credentials = Some((payment.ilp_address, payment.shared_secret));
        });
        let (destination, shared_secret) = credentials.unwrap();
        let mut session = None;
        self.step(from, |core| {
            let token = core.create_token("sim".into(), "stub+usd", cap).unwrap();
            session = Some((token.view.id, core.btp_connect(&token.secret).unwrap()));
        });
        let (token, session) = session.unwrap();
        let mut connection = Connection::new(&Config {
            role: Role::Client,
            shared_secret: &shared_secret,
            local_address: Some(Address::parse(&session.address).unwrap()),
            remote_address: Some(Address::parse(&destination).unwrap()),
            local_asset: Some(AssetDetails {
                code: "USD",
                scale: 9,
            }),
            receipt: None,
            policy: Policy::DEFAULT,
        });
        connection.measure_exchange_rate();
        self.payments.push(Payment {
            from,
            token: (token, cap),
            link: session.link,
            connection,
            amount,
            stream: None,
            in_flight: BTreeMap::new(),
            next_request: 1,
            done: false,
        });
        self.trace
            .push(format!("{} pay {from} -> {to} {amount}", self.now));
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
            if payment.stream.is_none() && payment.connection.exchange_rate().is_some() {
                let id = payment.connection.open_stream().unwrap();
                payment.connection.set_send_max(id, payment.amount).unwrap();
                payment.stream = Some(id);
            }
            if let Some(id) = payment.stream
                && payment
                    .connection
                    .stream(id)
                    .is_some_and(|s| s.total_sent() == payment.amount)
            {
                payment.done = true;
                continue;
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

    /// Each account's net credit, by path.
    fn balances(&self, index: usize) -> BTreeMap<String, i128> {
        self.core(index)
            .account_list()
            .iter()
            .map(|(_, account)| (account.path().to_string(), account.balance().net_credit()))
            .collect()
    }
}

fn seeds() -> std::ops::Range<u64> {
    std::env::var("DASSIE_SIM_SEEDS")
        .ok()
        .and_then(|range| {
            let (from, to) = range.split_once("..")?;
            Some(from.parse().ok()?..to.parse().ok()?)
        })
        .unwrap_or(0..6)
}

/// A run: the network forms, a node crashes and comes back, payments cross
/// it, and the books are checked.
fn run(seed: u64) -> Result<Vec<String>, String> {
    let mut rng = SeededRng::new(seed ^ 0xfeed);
    let count = 2 + usize::try_from(rng.next_u64() % 4).unwrap();
    let bootstraps = 1 + usize::try_from(rng.next_u64() % 2).unwrap().min(count - 1);
    let mut sim = Sim::new(seed, count, bootstraps);
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
    for _ in 0..3 {
        let from = usize::try_from(rng.next_u64() % count as u64).unwrap();
        let to = (from + 1 + usize::try_from(rng.next_u64() % (count as u64 - 1)).unwrap()) % count;
        // Up to three times the credit a peer extends: only settling lets
        // it through.
        let amount = 100_000_000 + rng.next_u64() % 3_300_000_000;
        sim.pay(from, to, amount, None);
    }
    // A token capped at 0.1 USD per 10 s pays 0.4 USD: it must wait for
    // its cap to refill.
    let from = usize::try_from(rng.next_u64() % count as u64).unwrap();
    let cap = SpendingCap {
        amount: 100_000_000,
        window_ms: 10_000,
    };
    sim.pay(from, (from + 1) % count, 400_000_000, Some(cap));
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
        // Each side settled everything it owed the other.
        let other = (0..count).find(|i| key(*i).node_id() == peer).unwrap();
        let mine = sim.balances(ender)[&format!("stub+usd:assets/interledger/{peer}")];
        let theirs =
            sim.balances(other)[&format!("stub+usd:assets/interledger/{}", key(ender).node_id())];
        if (mine, theirs) != (CREDIT, CREDIT) {
            return Err(format!(
                "nodes {ender} and {other} parted owing: {mine}, {theirs}"
            ));
        }
    }
    check(&sim)?;
    Ok(sim.trace)
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
    let path = format!("stub+usd:equity/owner/btp/{id}");
    let mut spent: Vec<(u64, u64)> = Vec::new();
    let mut after = None;
    loop {
        let page = store.transfers_after(after, 1_000, None).unwrap();
        let Some(last) = page.last() else { break };
        after = Some(last.sequence);
        spent.extend(
            page.iter()
                .filter(|row| row.debit_path == path)
                .filter(|row| !row.credit_path.starts_with("stub+usd:equity/owner"))
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

fn check(sim: &Sim) -> Result<(), String> {
    if let Some(payment) = sim.payments.iter().find(|payment| !payment.done) {
        return Err(format!(
            "a payment from node {} did not finish",
            payment.from
        ));
    }
    for payment in &sim.payments {
        check_cap(sim, payment)?;
    }
    let count = sim.nodes.len();
    let (mut owners, mut stubs) = (0i128, 0i128);
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
        owners += balances
            .iter()
            .filter(|(path, _)| path.starts_with("stub+usd:equity/owner"))
            .map(|(_, net)| *net)
            .sum::<i128>();
        // The books follow the stub's balance: what settled was posted.
        let on_ledger = sim.core(index).ledger_views()[0].on_ledger_balance;
        let booked = -balances["stub+usd:assets/settlement"];
        if booked != i128::from(on_ledger) {
            return Err(format!(
                "node {index} books {booked} in settlement, its stub holds {on_ledger}"
            ));
        }
        stubs += booked;
        // Every pair of accounts mirrors the other, peered or no longer.
        for other in (0..count).filter(|other| *other != index) {
            let peer = key(other).node_id();
            let Some(mine) = balances.get(&format!("stub+usd:assets/interledger/{peer}")) else {
                continue;
            };
            let theirs = sim.balances(other)
                [&format!("stub+usd:assets/interledger/{}", key(index).node_id())];
            if mine + theirs != 2 * CREDIT {
                return Err(format!(
                    "nodes {index} and {other} disagree: {mine} + {theirs}"
                ));
            }
        }
    }
    let total = DEPOSIT * i128::try_from(count).unwrap();
    if owners != total {
        return Err(format!("the owners hold {owners}, not {} each", DEPOSIT));
    }
    if stubs != total {
        return Err(format!("the stubs hold {stubs}, not {} each", DEPOSIT));
    }
    Ok(())
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

#[test]
fn a_seed_replays_the_same_run() {
    assert_eq!(run(2).unwrap(), run(2).unwrap());
}
