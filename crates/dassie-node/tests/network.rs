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
//!
//! A node also crashes mid-payment: a sender, a hop or a receiver. Its
//! peers learn from each other's verdicts which Fulfills counted (D17), so
//! their books still mirror each other; a hop whose Fulfill its upstream
//! refused books the loss, and the losses are what receivers got beyond
//! what senders paid.
//!
//! Outgoing payments go between nodes and to a node's own receiver: each
//! sends exactly its debit amount, across a crash of its node too, and
//! its receiver gets what it sent.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use std::collections::{BTreeMap, BTreeSet};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::Arc;

use dassie_connector::{CapUsage, SpendingCap, replay};
use dassie_identity::{NodeId, NodeKey};
use dassie_ilp::{Address, Prepare, Timestamp};
use dassie_ledger::LedgerId;
use dassie_node::engine::{Core, CoreParts, NewIncoming, NewOutgoing, Outbound, Output};
use dassie_node::node::Policies;
use dassie_node::store::{ForeignAmount, OutgoingState, PaymentId, Store};
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
    to: usize,
    /// The incoming payment it pays, and the most that one takes.
    incoming: PaymentId,
    limit: Option<u64>,
    /// The token's ID, and its cap if it has one.
    token: (u64, Option<SpendingCap>),
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
            abandoned: Vec::new(),
            outgoing: Vec::new(),
            cut: BTreeSet::new(),
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
        let (gone, kept) = std::mem::take(&mut self.payments)
            .into_iter()
            .partition(|payment| payment.from == index);
        self.payments = kept;
        self.abandoned.extend::<Vec<Payment>>(gone);
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
        let mut credentials = None;
        self.step(to, |core| {
            let payment = core
                .create_incoming_payment(NewIncoming {
                    ledger: LedgerId::StubUsd,
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
            let token = core.create_token("sim".into(), "stub+usd", cap).unwrap();
            session = Some((token.view.id, core.btp_connect(&token.secret).unwrap()));
        });
        let (token, session) = session.unwrap();
        let credentials = (session.address.clone(), destination, shared_secret);
        let connection = client(&credentials);
        self.payments.push(Payment {
            from,
            to,
            incoming,
            limit,
            token: (token, cap),
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
        let mut incoming = None;
        self.step(to, |core| {
            let payment = core
                .create_incoming_payment(NewIncoming {
                    ledger: LedgerId::StubUsd,
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
                    ledger: LedgerId::StubUsd,
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
                        asset_code: "USD".into(),
                        asset_scale: 9,
                    },
                    metadata: None,
                    recipient: Some(format!("https://n{to}.test/.well-known/pay")),
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
                payment.connection = client(&payment.credentials);
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

/// What a run did.
struct Run {
    /// Its trace, to compare runs by.
    trace: Vec<String>,
    /// Fulfills peers refused, as journal rows.
    refusals: usize,
    /// What hops lost to refusals.
    lost: i128,
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
    Ok(Run {
        refusals: refusals(&sim),
        lost: (0..count).map(|index| -losses(&sim, index)).sum(),
        trace: sim.trace,
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

/// A STREAM client paying the receiver at `credentials`.
fn client(credentials: &(String, String, [u8; 32])) -> Connection {
    let (own, destination, secret) = credentials;
    let mut connection = Connection::new(&Config {
        role: Role::Client,
        shared_secret: secret,
        local_address: Some(Address::parse(own).unwrap()),
        remote_address: Some(Address::parse(destination).unwrap()),
        local_asset: Some(AssetDetails {
            code: "USD",
            scale: 9,
        }),
        receipt: None,
        policy: Policy::DEFAULT,
    });
    connection.measure_exchange_rate();
    connection
}

/// Each owner's pool moved by exactly what its incoming payments
/// received, and each live client's token by what it sent. What receivers
/// got beyond what senders paid is what hops lost to refusals.
fn check_owner_books(sim: &Sim) -> Result<(), String> {
    let all = || sim.payments.iter().chain(&sim.abandoned);
    for index in 0..sim.nodes.len() {
        let balances = sim.balances(index);
        let pool = balances["stub+usd:equity/owner"];
        let received: i128 = all()
            .filter(|payment| payment.to == index)
            .map(|payment| i128::from(received(sim, payment)))
            .sum::<i128>()
            + outgoing_flows(sim)
                .filter(|flow| flow.to == index)
                .map(|flow| i128::from(flow.received))
                .sum::<i128>();
        if pool - DEPOSIT != received {
            return Err(format!(
                "node {index}'s owner gained {} for {received} received",
                pool - DEPOSIT
            ));
        }
        for payment in sim.payments.iter().filter(|payment| payment.from == index) {
            let token = -balances[&format!("stub+usd:equity/owner/btp/{}", payment.token.0)];
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
        let payer = -balances
            .get("stub+usd:equity/owner/local/pay")
            .copied()
            .unwrap_or(0);
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
    let beyond: i128 = all()
        .map(|payment| i128::from(received(sim, payment)) - i128::from(sent(payment)))
        .sum::<i128>()
        + outgoing_flows(sim)
            .map(|flow| i128::from(flow.received) - i128::from(flow.sent))
            .sum::<i128>();
    let lost: i128 = (0..sim.nodes.len()).map(|index| -losses(sim, index)).sum();
    // A client whose node crashed may have paid for packets it never heard
    // were fulfilled: those count as sent too.
    let unheard: i128 = sim
        .abandoned
        .iter()
        .map(|payment| {
            -sim.balances(payment.from)[&format!("stub+usd:equity/owner/btp/{}", payment.token.0)]
                - i128::from(sent(payment))
        })
        .sum();
    if beyond != lost + unheard {
        return Err(format!(
            "receivers got {beyond} beyond what clients heard they sent; hops lost {lost}, \
             crashed nodes' clients paid {unheard} unheard"
        ));
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
        .get("stub+usd:expenses/losses")
        .copied()
        .unwrap_or(0)
}

fn check(sim: &Sim) -> Result<(), String> {
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

/// The books: owners', settlements and every pair of accounts.
fn check_books(sim: &Sim) -> Result<(), String> {
    check_owner_books(sim)?;
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
            .sum::<i128>()
            + losses(sim, index);
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
// crash at all: the upstream refuses it, and both sides agree.
#[test]
fn a_late_fulfill_is_refused_and_both_sides_agree() {
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
            });
            refusals.push(created.is_err());
        });
    }
    assert_eq!(refusals, [true, true, true]);
    assert!(sim.nodes[0].core.is_some(), "the engine kept running");
}
