//! A simulated network of peer nodes
//! (spec/areas/testing.md#deterministic-simulation), for this crate's tests
//! and the simulation runner (D27): virtual time, a network that delays,
//! loses, duplicates and partitions, nodes that crash and restart from what
//! they persisted. Everything comes from one seed.
//!
//! Test code, behind the `sim` feature: a panic here is a failed run.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic,
    clippy::too_many_lines
)]

pub mod chaos;

use alloc::boxed::Box;
use alloc::collections::{BTreeMap, BTreeSet};
use alloc::format;
use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec;
use alloc::vec::Vec;
use core::net::IpAddr;

use crate::auth::{PairKeys, response_mac};
use crate::wire::{
    Authentication, Envelope, RefusalReason, ResponseEnvelope, SchemeId, encode_response,
};
use crate::{
    Agreed, Bootstrap, Config, Declined, Event, Failure, Inbound, InboundId, Output, OwnInfo,
    PeerBatch, PeerNode, PeeringHooks, Request, RequestId, Response, Stored,
};
use dassie_identity::{NodeId, NodeKey};
use dassie_node_core::testing::SeededRng;

/// When every simulation starts, on the network's clock.
pub const START: u64 = 1_800_000_000_000;
/// The credit every simulated peering extends.
pub const CREDIT: u64 = 1_100_000_000;

/// Hooks for the stub scheme: always agrees. Peers in `busy` still have
/// packets in flight, and those in `settling` a settlement of ours under
/// way in the node.
#[derive(Default)]
pub struct StubHooks {
    /// Peers with packets still in flight.
    pub busy: BTreeSet<NodeId>,
    /// Peers with a settlement of ours under way, before its message.
    pub settling: BTreeSet<NodeId>,
}

impl PeeringHooks for StubHooks {
    fn versions(&self, scheme: SchemeId) -> &'static [u16] {
        if scheme.as_str() == "stub" { &[1] } else { &[] }
    }

    fn peering_info(&mut self, scheme: SchemeId, _: &mut Vec<u8>) -> Result<(), Declined> {
        if scheme.as_str() == "stub" {
            Ok(())
        } else {
            Err(Declined)
        }
    }

    fn create_request(
        &mut self,
        _: NodeId,
        _: SchemeId,
        _: &[u8],
        _: &mut Vec<u8>,
    ) -> Result<(), Declined> {
        Ok(())
    }

    fn accept(
        &mut self,
        _: u64,
        _: NodeId,
        _: SchemeId,
        _: u16,
        _: &[u8],
        _: &mut Vec<u8>,
    ) -> Result<Agreed, RefusalReason> {
        Ok(Agreed {
            state: Vec::new(),
            credit_out: CREDIT,
        })
    }

    fn finalize(
        &mut self,
        _: u64,
        _: NodeId,
        _: SchemeId,
        _: u16,
        _: &[u8],
        _: &[u8],
        _: bool,
    ) -> Result<Agreed, Declined> {
        Ok(Agreed {
            state: Vec::new(),
            credit_out: CREDIT,
        })
    }

    fn drained(&self, peer: NodeId) -> bool {
        !self.busy.contains(&peer)
    }

    fn settling_to(&self, peer: NodeId) -> bool {
        self.settling.contains(&peer)
    }
}

/// Node `index`'s key, the same in every run.
pub fn key(index: usize) -> Arc<NodeKey> {
    Arc::new(NodeKey::from_bytes(&[u8::try_from(index + 1).unwrap(); 32]))
}

/// Node `index`'s URL.
pub fn url(index: usize) -> String {
    format!("https://n{index}.test")
}

/// Rewrites a response's body.
pub type Tamper = Box<dyn FnMut(&mut Vec<u8>)>;

/// What the network does to a message.
#[derive(Clone, Copy, Debug)]
pub struct Faults {
    /// The shortest a message takes.
    pub min_latency_ms: u64,
    /// The longest a message takes.
    pub max_latency_ms: u64,
    /// Per mille.
    pub loss: u64,
    /// Per mille.
    pub duplication: u64,
}

impl Faults {
    /// A calm network: short delays, nothing lost.
    pub const NONE: Self = Self {
        min_latency_ms: 5,
        max_latency_ms: 50,
        loss: 0,
        duplication: 0,
    };
    /// A rough network: long delays, losses and duplicates.
    pub const ROUGH: Self = Self {
        min_latency_ms: 1,
        max_latency_ms: 400,
        loss: 30,
        duplication: 20,
    };
}

#[derive(Debug)]
enum Delivery {
    /// A request arriving at `to`.
    Request {
        from: usize,
        to: usize,
        request: RequestId,
        body: Vec<u8>,
    },
    /// A request's outcome arriving back at `to`.
    Result {
        to: usize,
        generation: u64,
        request: RequestId,
        result: Result<Response, Failure>,
    },
}

/// A node, up or crashed, and what it persisted.
pub struct SimNode {
    /// The running node, or `None` while it is down.
    pub node: Option<PeerNode>,
    /// What it starts from: what it persisted when it last started.
    pub stored: Stored,
    stored_nodes: BTreeMap<NodeId, crate::StoredNode>,
    /// The channels it persisted.
    pub stored_channels: BTreeMap<NodeId, crate::StoredChannel>,
    stored_bans: BTreeSet<NodeId>,
    /// How far its clock is off the network's.
    pub skew_ms: i64,
    generation: u64,
    next_inbound: u64,
    /// Inbound requests waiting on a deferred reply: who asked, and which
    /// request of theirs it was.
    deferred: BTreeMap<InboundId, (usize, u64, RequestId)>,
    /// Its scheme's hooks.
    pub hooks: StubHooks,
}

/// A settlement a node applied: who got it, from whom, how much, and its
/// sequence.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Applied {
    /// The node that applied it.
    pub to: usize,
    /// The node that sent it.
    pub from: NodeId,
    /// How much it settled.
    pub amount: u64,
    /// Its sequence number.
    pub sequence: u64,
}

/// The simulated network: its nodes, its clock and its messages.
pub struct Network {
    /// The network's clock.
    pub now: u64,
    /// Every node, by index.
    pub nodes: Vec<SimNode>,
    /// Every node's configuration.
    pub config: Config,
    /// What the network does to messages now.
    pub faults: Faults,
    /// The bootstrap nodes: the first of the nodes.
    pub bootstrap: Vec<Bootstrap>,
    by_url: BTreeMap<String, usize>,
    queue: BTreeMap<(u64, u64), Delivery>,
    sequence: u64,
    rng: SeededRng,
    /// Pairs that cannot reach each other.
    pub cut: BTreeSet<(usize, usize)>,
    /// The address each node's requests come from, as the HTTP layer sees
    /// it: none, unless set here.
    pub sources: BTreeMap<usize, IpAddr>,
    /// What happened, for comparing runs.
    pub trace: Vec<String>,
    /// Every event, with the node that reported it.
    pub events: Vec<(usize, Event)>,
    /// ILP packets received: by which node, from whom, with which ID.
    pub packets: Vec<(usize, NodeId, u32)>,
    /// Settlements applied.
    pub applied: Vec<Applied>,
    /// Settlement requests taken: by which node, from whom.
    pub settlement_requests: Vec<(usize, NodeId)>,
    /// Settlements sent: by which node, to whom, how much, with which
    /// sequence.
    pub settled: Vec<(usize, NodeId, u64, u64)>,
    /// Nodes that apply no settlement message: they answer it with the
    /// status, as a busy node would with 503, or not at all (`None`), as
    /// one would that drops the connection.
    pub refuse_settlements: BTreeMap<usize, Option<u16>>,
    /// What a node answers a request to settle with as on its way: nothing,
    /// unless set here. Nothing here settles on request.
    pub promise: BTreeMap<usize, u64>,
    /// Settlement messages refused for being in `refuse_settlements`.
    pub settlements_refused: usize,
    /// Nodes that refuse requests to settle with this status, as one would
    /// that is overloaded (503) or finds the request malformed (400).
    pub refuse_settlement_requests: BTreeMap<usize, u16>,
    /// Rewrites the body of every response with status 200 before it
    /// arrives, as a node would that answers anything it likes: the MAC is
    /// made again, so the body gets past it to what reads it.
    pub tamper: Option<Tamper>,
    /// While tampering, each request's MAC and the node it went to, by who
    /// sent it, in which generation, with which ID.
    macs: BTreeMap<(usize, u64, RequestId), (usize, [u8; 32])>,
}

impl Network {
    /// `count` nodes, the first `bootstrap_count` of them bootstrap
    /// nodes, each started with its own clock skew.
    pub fn new(seed: u64, count: usize, bootstrap_count: usize, config: Config) -> Self {
        let mut rng = SeededRng::new(seed);
        let bootstrap: Vec<Bootstrap> = (0..bootstrap_count)
            .map(|index| Bootstrap {
                id: key(index).node_id(),
                url: url(index),
                key: key(index).public_key(),
            })
            .collect();
        let mut network = Self {
            now: START,
            nodes: Vec::new(),
            config,
            faults: Faults::NONE,
            bootstrap,
            by_url: BTreeMap::new(),
            queue: BTreeMap::new(),
            sequence: 0,
            rng: SeededRng::new(rng.next_u64()),
            cut: BTreeSet::new(),
            sources: BTreeMap::new(),
            trace: Vec::new(),
            events: Vec::new(),
            packets: Vec::new(),
            applied: Vec::new(),
            settlement_requests: Vec::new(),
            settled: Vec::new(),
            refuse_settlements: BTreeMap::new(),
            refuse_settlement_requests: BTreeMap::new(),
            promise: BTreeMap::new(),
            settlements_refused: 0,
            tamper: None,
            macs: BTreeMap::new(),
        };
        for index in 0..count {
            let skew = i64::try_from(rng.next_u64() % 20_001).unwrap() - 10_000;
            network.by_url.insert(url(index), index);
            network.nodes.push(SimNode {
                node: None,
                stored: Stored::default(),
                stored_nodes: BTreeMap::new(),
                stored_channels: BTreeMap::new(),
                stored_bans: BTreeSet::new(),
                skew_ms: skew,
                generation: 0,
                next_inbound: 0,
                deferred: BTreeMap::new(),
                hooks: StubHooks::default(),
            });
            network.start(index);
        }
        network
    }

    fn local_now(&self, index: usize) -> u64 {
        self.now.saturating_add_signed(self.nodes[index].skew_ms)
    }

    /// Starts node `index` from what it persisted.
    pub fn start(&mut self, index: usize) {
        let seed = self.rng.next_u64();
        let now = self.local_now(index);
        let sim = &mut self.nodes[index];
        sim.generation += 1;
        sim.deferred.clear();
        sim.stored.nodes = sim.stored_nodes.values().cloned().collect();
        sim.stored.channels = sim.stored_channels.values().cloned().collect();
        sim.stored.bans = sim.stored_bans.iter().copied().collect();
        let mut node = PeerNode::new(
            self.config,
            key(index),
            OwnInfo {
                url: Some(url(index)),
                alias: format!("n{index}"),
            },
            self.bootstrap.clone(),
            sim.stored.clone(),
            Box::new(SeededRng::new(seed)),
            now,
        );
        node.set_schemes(now, vec![SchemeId::parse("stub").unwrap()]);
        sim.node = Some(node);
        self.trace.push(format!("{} start {index}", self.now));
    }

    /// Crashes node `index`: what it did not persist is lost.
    pub fn crash(&mut self, index: usize) {
        self.nodes[index].node = None;
        self.trace.push(format!("{} crash {index}", self.now));
    }

    /// Whether node `index` is running.
    pub fn is_up(&self, index: usize) -> bool {
        self.nodes[index].node.is_some()
    }

    /// Node `index`, which must be running.
    pub fn node(&self, index: usize) -> &PeerNode {
        self.nodes[index].node.as_ref().unwrap()
    }

    fn schedule(&mut self, delay: u64, delivery: Delivery) {
        self.sequence += 1;
        self.queue
            .insert((self.now + delay, self.sequence), delivery);
    }

    fn latency(&mut self) -> u64 {
        let span = self.faults.max_latency_ms - self.faults.min_latency_ms + 1;
        self.faults.min_latency_ms + self.rng.next_u64() % span
    }

    fn chance(&mut self, per_mille: u64) -> bool {
        self.rng.next_u64() % 1000 < per_mille
    }

    /// Persists what a node changed and carries out its outputs.
    fn drain(&mut self, index: usize) {
        let Some(node) = self.nodes[index].node.as_mut() else {
            return;
        };
        node.check_invariants();
        let mut batch = PeerBatch::default();
        node.take_changes(&mut batch);
        let mut outputs = Vec::new();
        while let Some(output) = node.poll_output() {
            outputs.push(output);
        }
        let mut events = Vec::new();
        while let Some(event) = node.poll_event() {
            self.trace.push(format!("{} {index} {event:?}", self.now));
            events.push(event);
        }
        let sim = &mut self.nodes[index];
        for stored in batch.nodes {
            sim.stored_nodes.insert(stored.id, stored);
        }
        for removed in batch.removed {
            sim.stored_nodes.remove(&removed);
        }
        if let Some(sequence) = batch.own_sequence {
            sim.stored.own_sequence = sequence;
        }
        for channel in batch.channels {
            sim.stored_channels.insert(channel.node, channel);
        }
        for banned in batch.banned {
            sim.stored_bans.insert(banned);
        }
        for unbanned in batch.unbanned {
            sim.stored_bans.remove(&unbanned);
        }
        for output in outputs {
            self.carry(index, output);
        }
        for event in events {
            self.events.push((index, event));
        }
    }

    /// Node `index` settles `amount` with `peer`, if nothing is on its way.
    pub fn settle(&mut self, index: usize, peer: NodeId, amount: u64) -> bool {
        let now = self.local_now(index);
        let Some(node) = self.nodes[index].node.as_mut() else {
            return false;
        };
        let scheme = SchemeId::parse("stub").unwrap();
        let Ok(sequence) = node.send_settlement(now, &peer, scheme, amount, Vec::new()) else {
            return false;
        };
        self.settled.push((index, peer, amount, sequence));
        self.trace
            .push(format!("{} {index} settles {amount} #{sequence}", self.now));
        self.drain(index);
        true
    }

    /// Whether the node `node` holds a channel with node `with`, as it
    /// persisted it: up or down.
    pub fn holds_channel(&self, node: &NodeId, with: usize) -> bool {
        let own = key(with).node_id();
        (0..self.nodes.len())
            .find(|index| key(*index).node_id() == *node)
            .is_some_and(|index| self.nodes[index].stored_channels.contains_key(&own))
    }

    fn carry(&mut self, index: usize, output: Output) {
        match output {
            Output::Request(request) => self.send(index, request),
            Output::Reply { inbound, reply } => {
                let Some((to, generation, request)) = self.nodes[index].deferred.remove(&inbound)
                else {
                    return;
                };
                let response = Response {
                    status: reply.status,
                    body: reply.body,
                };
                let delay = self.latency();
                self.schedule(
                    delay,
                    Delivery::Result {
                        to,
                        generation,
                        request,
                        result: Ok(response),
                    },
                );
            }
        }
    }

    fn send(&mut self, from: usize, request: Request) {
        self.trace.push(format!(
            "{} {from} -> {:?} {:?}",
            self.now, request.to, request.kind
        ));
        let generation = self.nodes[from].generation;
        let Some(&to) = self.by_url.get(&request.url) else {
            self.schedule(
                1,
                Delivery::Result {
                    to: from,
                    generation,
                    request: request.id,
                    result: Err(Failure::NotDelivered),
                },
            );
            return;
        };
        if self.tamper.is_some()
            && let Ok(envelope) = Envelope::decode(&request.body)
            && let Authentication::Hmac(hmac) = envelope.authentication
        {
            self.macs
                .insert((from, generation, request.id), (to, hmac.mac));
        }
        if self.cut.contains(&(from, to))
            || self.cut.contains(&(to, from))
            || self.chance(self.faults.loss)
        {
            let delay = self.latency();
            self.schedule(
                delay,
                Delivery::Result {
                    to: from,
                    generation,
                    request: request.id,
                    result: Err(Failure::NotDelivered),
                },
            );
            return;
        }
        let copies = if self.chance(self.faults.duplication) {
            2
        } else {
            1
        };
        for _ in 0..copies {
            let delay = self.latency();
            self.schedule(
                delay,
                Delivery::Request {
                    from,
                    to,
                    request: request.id,
                    body: request.body.clone(),
                },
            );
        }
    }

    fn deliver(&mut self, delivery: Delivery) {
        match delivery {
            Delivery::Request {
                from,
                to,
                request,
                body,
            } => self.arrive(from, to, request, &body),
            Delivery::Result {
                to,
                generation,
                request,
                result,
            } => {
                if self.nodes[to].generation != generation {
                    return;
                }
                let result = self.tampered(to, generation, request, result);
                let now = self.local_now(to);
                let sim = &mut self.nodes[to];
                if let Some(node) = sim.node.as_mut() {
                    node.handle_response(now, &mut sim.hooks, request, result);
                    self.drain(to);
                }
            }
        }
    }

    /// `result`, its body rewritten and MACed again if tampering.
    fn tampered(
        &mut self,
        to: usize,
        generation: u64,
        request: RequestId,
        result: Result<Response, Failure>,
    ) -> Result<Response, Failure> {
        let (Some(tamper), Ok(response)) = (self.tamper.as_mut(), &result) else {
            return result;
        };
        let Some(&(responder, request_mac)) = self.macs.get(&(to, generation, request)) else {
            return result;
        };
        let Ok(envelope) = ResponseEnvelope::decode(&response.body) else {
            return result;
        };
        if response.status != 200 || envelope.mac.is_none() {
            return result;
        }
        let mut body = envelope.body.to_vec();
        tamper(&mut body);
        let keys = PairKeys::derive(&key(responder), &key(to).public_key()).unwrap();
        let mac = response_mac(&keys.incoming_response, &request_mac, &body);
        Ok(Response {
            status: 200,
            body: encode_response(Some(&mac), &body),
        })
    }

    fn arrive(&mut self, from: usize, to: usize, request: RequestId, body: &[u8]) {
        let generation = self.nodes[from].generation;
        let now = self.local_now(to);
        let sim = &mut self.nodes[to];
        let Some(node) = sim.node.as_mut() else {
            let delay = self.latency();
            self.schedule(
                delay,
                Delivery::Result {
                    to: from,
                    generation,
                    request,
                    result: Err(Failure::NotDelivered),
                },
            );
            return;
        };
        let inbound = InboundId(sim.next_inbound);
        sim.next_inbound += 1;
        let source = self.sources.get(&from).copied();
        let reply = match node.handle_inbound(now, &mut sim.hooks, inbound, body, source) {
            Inbound::Reply(reply) => Some(reply),
            Inbound::Deferred => {
                sim.deferred.insert(inbound, (from, generation, request));
                None
            }
            Inbound::Packet {
                reply,
                from: sender,
                request_id,
                ..
            } => {
                self.packets.push((to, sender, request_id));
                Some(reply)
            }
            Inbound::Settlement { .. } if self.refuse_settlements.contains_key(&to) => {
                self.settlements_refused += 1;
                self.refuse_settlements[&to].map(|status| crate::Reply::refuse(status, "refused"))
            }
            Inbound::Settlement {
                from: sender,
                amount,
                sequence,
                reply,
                ..
            } => {
                node.settlement_applied(now, &sender, sequence);
                self.applied.push(Applied {
                    to,
                    from: sender,
                    amount,
                    sequence,
                });
                Some(reply)
            }
            Inbound::SchemeMessage { reply, .. } => Some(reply),
            Inbound::SettlementRequest { .. }
                if self.refuse_settlement_requests.contains_key(&to) =>
            {
                let status = self.refuse_settlement_requests[&to];
                Some(crate::Reply::refuse(status, "refused"))
            }
            // Nothing settles here: nothing is on its way.
            Inbound::SettlementRequest {
                from: sender,
                answer,
                ..
            } => {
                self.settlement_requests.push((to, sender));
                Some(answer.reply(self.promise.get(&to).copied().unwrap_or(0)))
            }
        };
        self.drain(to);
        if let Some(reply) = reply {
            let delay = self.latency();
            let response = Response {
                status: reply.status,
                body: reply.body,
            };
            self.schedule(
                delay,
                Delivery::Result {
                    to: from,
                    generation,
                    request,
                    result: Ok(response),
                },
            );
        }
    }

    /// Runs until `until`.
    pub fn run_until(&mut self, until: u64) {
        let mut steps = 0u64;
        loop {
            steps += 1;
            assert!(steps < 10_000_000, "the simulation made no progress");
            let next_delivery = self.queue.keys().next().map(|(at, _)| *at);
            let next_timer = (0..self.nodes.len())
                .filter_map(|index| {
                    let node = self.nodes[index].node.as_ref()?;
                    let local = node.poll_timeout()?;
                    // Back from the node's clock to the network's.
                    Some(local.saturating_add_signed(-self.nodes[index].skew_ms))
                })
                .min();
            let next = [next_delivery, next_timer].into_iter().flatten().min();
            let Some(next) = next.filter(|at| *at <= until) else {
                self.now = until;
                return;
            };
            self.now = self.now.max(next);
            if next_delivery == Some(next) {
                let key = *self.queue.keys().next().unwrap();
                let delivery = self.queue.remove(&key).unwrap();
                self.deliver(delivery);
                continue;
            }
            for index in 0..self.nodes.len() {
                let now = self.local_now(index);
                let sim = &mut self.nodes[index];
                let Some(node) = sim.node.as_mut() else {
                    continue;
                };
                if node.poll_timeout().is_some_and(|at| at <= now) {
                    node.handle_timeout(now, &mut sim.hooks);
                    self.drain(index);
                    let node = self.nodes[index].node.as_ref().unwrap();
                    assert!(
                        node.poll_timeout().is_none_or(|at| at > now),
                        "node {index} is due again at once"
                    );
                }
            }
        }
    }

    /// The nodes that are running.
    pub fn up(&self) -> impl Iterator<Item = usize> + '_ {
        (0..self.nodes.len()).filter(|index| self.is_up(*index))
    }

    /// The peer graph as both ends see it: an edge where both are peered.
    pub fn peer_graph(&self) -> BTreeMap<NodeId, Vec<NodeId>> {
        let mut graph: BTreeMap<NodeId, Vec<NodeId>> = BTreeMap::new();
        for index in self.up() {
            let node = self.node(index);
            graph.entry(*node.own()).or_default();
            for (peer, _) in node.peers() {
                let mutual = self.up().any(|other| {
                    self.node(other).own() == peer
                        && self.node(other).peering_of(node.own()).is_some()
                });
                if mutual {
                    graph.entry(*node.own()).or_default().push(*peer);
                }
            }
        }
        for list in graph.values_mut() {
            list.sort_unstable();
        }
        graph
    }
}

/// Whether `graph` is connected.
pub fn connected(graph: &BTreeMap<NodeId, Vec<NodeId>>) -> bool {
    let Some(start) = graph.keys().next() else {
        return true;
    };
    let mut seen = BTreeSet::from([*start]);
    let mut stack = vec![*start];
    while let Some(node) = stack.pop() {
        for next in graph.get(&node).into_iter().flatten() {
            if seen.insert(*next) {
                stack.push(*next);
            }
        }
    }
    seen.len() == graph.len()
}

impl Network {
    /// Node `index`, which must be running.
    pub fn node_mut(&mut self, index: usize) -> &mut PeerNode {
        self.nodes[index].node.as_mut().unwrap()
    }

    /// Hands `body` to node `index` as an inbound request, at its clock.
    pub fn inbound(&mut self, index: usize, body: &[u8]) -> crate::Reply {
        self.inbound_from(index, body, None)
    }

    /// The same, as a request from `source`.
    pub fn inbound_from(
        &mut self,
        index: usize,
        body: &[u8],
        source: Option<IpAddr>,
    ) -> crate::Reply {
        let now = self.local_now(index);
        let sim = &mut self.nodes[index];
        let inbound = InboundId(sim.next_inbound);
        sim.next_inbound += 1;
        let node = sim.node.as_mut().unwrap();
        let reply = match node.handle_inbound(now, &mut sim.hooks, inbound, body, source) {
            Inbound::Reply(reply)
            | Inbound::Packet { reply, .. }
            | Inbound::SchemeMessage { reply, .. } => reply,
            Inbound::SettlementRequest { answer, .. } => {
                answer.reply(self.promise.get(&index).copied().unwrap_or(0))
            }
            Inbound::Settlement {
                from,
                sequence,
                reply,
                ..
            } => {
                node.settlement_applied(now, &from, sequence);
                reply
            }
            Inbound::Deferred => crate::Reply {
                status: 0,
                body: Vec::new(),
            },
        };
        self.drain(index);
        reply
    }

    /// Node `index`'s clock.
    pub fn clock(&self, index: usize) -> u64 {
        self.local_now(index)
    }
}

impl Network {
    /// Runs `f` on node `index` at its clock, then carries out its outputs.
    pub fn act(&mut self, index: usize, f: impl FnOnce(&mut PeerNode, u64)) {
        let now = self.local_now(index);
        f(self.nodes[index].node.as_mut().unwrap(), now);
        self.drain(index);
    }
}
