//! A simulated network of peer nodes
//! (spec/areas/testing.md#deterministic-simulation), for this crate's tests
//! and the simulation runner (spec/decisions/random-seed-simulations.md):
//! virtual time, a network that delays, loses, duplicates and partitions,
//! nodes that crash and restart from what they persisted. Everything comes
//! from one seed.
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
pub mod names;

use alloc::boxed::Box;
use alloc::collections::{BTreeMap, BTreeSet};
use alloc::format;
use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec;
use alloc::vec::Vec;
use core::net::IpAddr;

use crate::auth::{PairKeys, response_mac};
use crate::names::Name;
use crate::wire::{
    Authentication, Envelope, Kind, NameHolding, NameListResponse, RefusalReason, ResponseEnvelope,
    SchemeId, encode_response,
};
use crate::{
    Agreed, Bootstrap, Config, Declined, Event, Failure, Inbound, InboundId, NameEvent, Output,
    OwnInfo, PeerBatch, PeerNode, PeeringHooks, Request, RequestId, Response, Stored,
};
use dassie_identity::{NodeId, NodeKey};
use dassie_kit::sim::SeededRng;
use dassie_oer::Encode;
use sha2::{Digest, Sha256};

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
    /// Why the scheme refuses peering requests, if it does.
    pub refuse: Option<RefusalReason>,
    /// Whether the stub scheme's peerings are over a hub: Prepares on them
    /// come with commitments.
    pub commitments: bool,
    /// What the node puts after the verdict in its reply to a Fulfill on a
    /// peering over a hub, as a payer's claim.
    pub claim: Vec<u8>,
    /// What stays of the closed channels, as the node last wrote it.
    pub closed: BTreeMap<NodeId, crate::Closed>,
    /// Nodes whose channels the books hold open.
    pub holding: BTreeSet<NodeId>,
    /// Whether the node no longer runs the stub scheme, as one disabled or
    /// of the other realm: it speaks no version of it.
    pub unrun: bool,
    /// Whether the stub scheme takes entry payments
    /// (spec/decisions/pay-to-enter.md).
    pub takes_entries: bool,
    /// The nodes it holds one from, each with the request data it is bound
    /// to.
    pub entries: BTreeSet<(NodeId, Vec<u8>)>,
}

impl StubHooks {
    /// The reply to a packet: its claim after the verdict, where one
    /// goes.
    fn reply(&self, answer: crate::PacketAnswer) -> crate::Reply {
        let claim: &[u8] = if answer.takes_extra() {
            &self.claim
        } else {
            &[]
        };
        answer.reply(claim)
    }
}

impl PeeringHooks for StubHooks {
    fn versions(&self, scheme: SchemeId) -> &'static [u16] {
        if scheme.as_str() == "stub" && !self.unrun {
            &[1]
        } else {
            &[]
        }
    }

    fn peering_info(&mut self, scheme: SchemeId, _: &mut Vec<u8>) -> Result<(), Declined> {
        if scheme.as_str() == "stub" && !self.unrun {
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
        _: bool,
        _: SchemeId,
        _: u16,
        _: &[u8],
        _: &mut Vec<u8>,
    ) -> Result<Agreed, RefusalReason> {
        if let Some(reason) = self.refuse {
            return Err(reason);
        }
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

    fn entry(&self, node: NodeId, scheme: SchemeId, data: &[u8]) -> crate::EntryPayment {
        if !self.takes_entries || scheme.as_str() != "stub" {
            crate::EntryPayment::NotTaken
        } else if self.entries.contains(&(node, data.to_vec())) {
            crate::EntryPayment::Held
        } else {
            crate::EntryPayment::Missing
        }
    }

    fn uses_commitments(&self, scheme: SchemeId) -> bool {
        self.commitments && scheme.as_str() == "stub"
    }

    fn drained(&self, peer: NodeId) -> bool {
        !self.busy.contains(&peer)
    }

    fn settling_to(&self, peer: NodeId) -> bool {
        self.settling.contains(&peer)
    }

    fn closed_channel(&self, node: NodeId) -> Option<crate::Closed> {
        self.closed.get(&node).copied()
    }

    fn closable(&self, node: NodeId) -> bool {
        !self.holding.contains(&node) && !self.busy.contains(&node)
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

/// Rewrites the body of a response from one node to another, as a faulty
/// node would that tells each asker something else: given the node that
/// answers, the node that asked and the kind of request.
pub type Forge = Box<dyn FnMut(usize, usize, Kind, &mut Vec<u8>)>;

/// A request sent while tampering or forging: the node it went to, its MAC
/// and its kind.
type Sent = (usize, [u8; 32], Kind);

/// What a simulated registrar does, a bootstrap node faulty in one of the
/// ways the names' design allows for (spec/designs/names.md#6-why-it-holds).
/// Its own state follows the protocol: what it does wrong is what it tells
/// others.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Role {
    /// Follows the protocol.
    Honest,
    /// Shows nodes different name lists, each a list it could have
    /// served: to the registrars their own, so that each sees its side of a
    /// split confirmed; to the other nodes, by the parity of their index,
    /// for each name the holder an honest registrar lists that sorts first,
    /// or last.
    Equivocate,
    /// Logs every commitment, and never answers one: a registrant never
    /// holds every receipt.
    Withhold,
    /// Passes every name unsealed at it to [`Network::adversary`], which at
    /// once registers it itself at every registrar, and lists the adversary
    /// for every name it passed on.
    TipOff,
}

impl Role {
    /// Whether it follows the protocol.
    #[must_use]
    pub const fn is_honest(self) -> bool {
        matches!(self, Self::Honest)
    }
}

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
        kind: Kind,
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
    stored_floors: BTreeMap<NodeId, u64>,
    stored_commitments: BTreeMap<crate::names::Commitment, crate::names::Logged>,
    stored_names: BTreeMap<crate::names::Name, crate::names::Granted>,
    stored_holders: BTreeMap<crate::names::Name, crate::names::HeldName>,
    /// Its view of who holds each name, as last seen after a step.
    seen_holders: BTreeMap<Name, NodeId>,
    stored_registrations: BTreeMap<crate::names::Name, crate::names::NameRegistration>,
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
    /// Pairs cut one way: the first's requests never reach the second,
    /// while the second's reach the first and are answered, so each still
    /// hears the other.
    pub cut_one_way: BTreeSet<(usize, usize)>,
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
    /// Rewrites the body of every response with status 200 after
    /// `tamper`, knowing who answers whom and what: a faulty node, say,
    /// that shows each node a list of its own. The MAC is made again.
    pub forge: Option<Forge>,
    /// While tampering or forging, each request's MAC, the node it went to
    /// and its kind, by who sent it, in which generation, with which ID;
    /// while a registrar is faulty, each name list request's.
    macs: BTreeMap<(usize, u64, RequestId), Sent>,
    /// Periphery nodes: without a URL, dialling out over QUIC, reached by
    /// node ID over the connections they opened. Their uplinks are the
    /// other nodes, which then take QUIC too.
    pub periphery: BTreeSet<usize>,
    /// Whether the nodes have a QUIC endpoint: core nodes then take
    /// downlinks.
    pub quic: bool,
    /// The uplinks each node is pinned to, kept across restarts
    /// (spec/areas/network.md#pinned-uplinks).
    pub pinned: BTreeMap<usize, Vec<NodeId>>,
    /// What each bootstrap node does as a registrar of names: as the
    /// protocol says, unless set here.
    pub roles: BTreeMap<usize, Role>,
    /// The node a registrar that tips off passes names to.
    pub adversary: Option<usize>,
    /// The names passed to the adversary, with the expiry the registrars
    /// that tip off list it for each with.
    pub tipped: BTreeMap<Name, u64>,
    /// What happened to names, at which node, in order: a node reports
    /// these in counts only (spec/areas/network.md#the-registrar,
    /// spec/areas/network.md#holders), so they are read from its state
    /// after each step.
    pub name_changes: Vec<(usize, NameChange)>,
    /// The nodes drained since [`Self::run_until`] last asked for their
    /// timers: every step that changes a node drains it.
    touched: BTreeSet<usize>,
}

/// What happened to a name at a node, as read from its state.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NameChange {
    /// In the node's view, the name has this holder now, or none.
    Holder {
        /// The name.
        name: Name,
        /// Its holder now, if it has one.
        holder: Option<NodeId>,
    },
    /// The node, a registrar, granted the name to `holder`.
    Granted {
        /// The name.
        name: Name,
        /// Its holder.
        holder: NodeId,
    },
    /// The node, a registrar, released the name it listed for `holder`.
    Released {
        /// The name.
        name: Name,
        /// Its holder until then.
        holder: NodeId,
    },
}

impl Network {
    /// `count` nodes, the first `bootstrap_count` of them bootstrap
    /// nodes, each started with its own clock skew.
    pub fn new(seed: u64, count: usize, bootstrap_count: usize, config: Config) -> Self {
        Self::build(seed, (count, bootstrap_count, 0), false, config)
    }

    /// `count` core nodes, the first `bootstrap_count` of them bootstrap
    /// nodes, and after them `periphery` periphery nodes
    /// (spec/areas/network.md#periphery-nodes). The core nodes take QUIC,
    /// and so downlinks.
    pub fn with_periphery(
        seed: u64,
        count: usize,
        bootstrap_count: usize,
        periphery: usize,
        config: Config,
    ) -> Self {
        Self::build(seed, (count, bootstrap_count, periphery), true, config)
    }

    fn build(
        seed: u64,
        (count, bootstrap_count, periphery): (usize, usize, usize),
        quic: bool,
        config: Config,
    ) -> Self {
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
            cut_one_way: BTreeSet::new(),
            sources: BTreeMap::new(),
            trace: Vec::new(),
            touched: BTreeSet::new(),
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
            forge: None,
            macs: BTreeMap::new(),
            periphery: (count..count + periphery).collect(),
            quic,
            pinned: BTreeMap::new(),
            roles: BTreeMap::new(),
            adversary: None,
            tipped: BTreeMap::new(),
            name_changes: Vec::new(),
        };
        for index in 0..count + periphery {
            let skew = i64::try_from(rng.next_u64() % 20_001).unwrap() - 10_000;
            if index < count {
                network.by_url.insert(url(index), index);
            }
            network.nodes.push(SimNode {
                node: None,
                stored: Stored::default(),
                stored_nodes: BTreeMap::new(),
                stored_channels: BTreeMap::new(),
                stored_bans: BTreeSet::new(),
                stored_floors: BTreeMap::new(),
                stored_commitments: BTreeMap::new(),
                stored_names: BTreeMap::new(),
                stored_holders: BTreeMap::new(),
                seen_holders: BTreeMap::new(),
                stored_registrations: BTreeMap::new(),
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
        sim.stored.floors = sim
            .stored_floors
            .iter()
            .map(|(node, floor_ms)| crate::StoredFloor {
                node: *node,
                floor_ms: *floor_ms,
            })
            .collect();
        sim.stored.commitments = sim.stored_commitments.values().copied().collect();
        sim.stored.names = sim.stored_names.values().copied().collect();
        sim.stored.holders = sim.stored_holders.values().copied().collect();
        sim.stored.registrations = sim.stored_registrations.values().copied().collect();
        let periphery = self.periphery.contains(&index);
        let mut node = PeerNode::new(
            self.config,
            key(index),
            OwnInfo {
                url: (!periphery).then(|| url(index)),
                alias: format!("n{index}"),
                quic: self.quic,
                feed_port: None,
                hub: None,
            },
            self.bootstrap.clone(),
            sim.stored.clone(),
            Box::new(SeededRng::new(seed)),
            now,
        );
        node.set_schemes(now, vec![SchemeId::parse("stub").unwrap()]);
        if let Some(uplinks) = self.pinned.get(&index) {
            node.pin_uplinks(uplinks.clone());
        }
        let changes = view_changes(&mut sim.seen_holders, &node);
        sim.node = Some(node);
        self.trace.push(format!("{} start {index}", self.now));
        self.name_changes
            .extend(changes.into_iter().map(|change| (index, change)));
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
        self.touched.insert(index);
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
        let node_now = node.now_ms;
        let sim = &mut self.nodes[index];
        let mut changes = granted(sim, &events, &batch);
        changes.extend(released(sim, &events, &batch, node_now));
        let viewed = view_changes(&mut sim.seen_holders, sim.node.as_ref().unwrap());
        let reported = events
            .iter()
            .any(|event| matches!(event, Event::Name(NameEvent::HoldersChanged { .. })));
        assert!(
            reported || viewed.is_empty(),
            "node {index} changed its view of holders without reporting it"
        );
        changes.extend(viewed);
        self.name_changes
            .extend(changes.into_iter().map(|change| (index, change)));
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
        // Closed and open again in one batch, a channel is open: as the
        // store writes them.
        for closed in batch.closed {
            sim.stored_channels.remove(&closed.node);
            sim.hooks.closed.insert(closed.node, closed.closed);
        }
        for channel in batch.channels {
            sim.hooks.closed.remove(&channel.node);
            sim.stored_channels.insert(channel.node, channel);
        }
        for banned in batch.banned {
            sim.stored_bans.insert(banned);
        }
        for unbanned in batch.unbanned {
            sim.stored_bans.remove(&unbanned);
        }
        for removed in batch.floors_removed {
            sim.stored_floors.remove(&removed);
        }
        for floor in batch.floors {
            sim.stored_floors.insert(floor.node, floor.floor_ms);
        }
        // Removals first, as the store writes them.
        if let Some(head) = batch.name_log {
            sim.stored.name_log = head;
        }
        for removed in batch.commitments_removed {
            sim.stored_commitments.remove(&removed);
        }
        for logged in batch.commitments {
            sim.stored_commitments.insert(logged.commitment, logged);
        }
        for removed in batch.names_removed {
            sim.stored_names.remove(&removed);
        }
        for granted in batch.names {
            sim.stored_names.insert(granted.name, granted);
        }
        for removed in batch.holders_removed {
            sim.stored_holders.remove(&removed);
        }
        for held in batch.holders {
            sim.stored_holders.insert(held.name, held);
        }
        for removed in batch.registrations_removed {
            sim.stored_registrations.remove(&removed);
        }
        for registration in batch.registrations {
            sim.stored_registrations
                .insert(registration.name, registration);
        }
        for output in outputs {
            self.carry(index, output);
        }
        let tips: Vec<Name> = if self.role(index) == Role::TipOff {
            events.iter().filter_map(tip).collect()
        } else {
            Vec::new()
        };
        for event in events {
            self.events.push((index, event));
        }
        for name in tips {
            self.tip_off(name);
        }
    }

    /// What node `index` does as a registrar.
    pub fn role(&self, index: usize) -> Role {
        self.roles.get(&index).copied().unwrap_or(Role::Honest)
    }

    /// Passes `name` to the adversary, which registers it at once if it is
    /// up, and lists the adversary for it from now on.
    fn tip_off(&mut self, name: Name) {
        let Some(adversary) = self.adversary else {
            return;
        };
        let expires_ms = self.now + self.config.names.ttl_ms;
        self.tipped.entry(name).or_insert(expires_ms);
        self.trace
            .push(format!("{} tips off {adversary}: {name}", self.now));
        if self.is_up(adversary) {
            self.act(adversary, |node, now| {
                let _ = node.register_name(now, name);
            });
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

    /// Whether the node `node` holds an open channel with node `with`, as
    /// it persisted it: up or down.
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
        // A periphery node is reached by its ID, over the connection it
        // opened; nothing else has an empty URL.
        let to = if request.url.is_empty() {
            self.periphery
                .iter()
                .copied()
                .find(|index| key(*index).node_id() == request.to)
        } else {
            self.by_url.get(&request.url).copied()
        };
        let Some(to) = to else {
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
        let listing = matches!(
            request.kind,
            Kind::NameListRequest | Kind::NameListHashRequest
        );
        if (self.tamper.is_some() || self.forge.is_some() || (listing && !self.roles.is_empty()))
            && let Ok(envelope) = Envelope::decode(&request.body)
            && let Authentication::Hmac(hmac) = envelope.authentication
        {
            self.macs
                .insert((from, generation, request.id), (to, hmac.mac, request.kind));
        }
        if self.cut.contains(&(from, to))
            || self.cut.contains(&(to, from))
            || self.cut_one_way.contains(&(from, to))
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
                    kind: request.kind,
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
                kind,
                body,
            } => self.arrive(from, to, request, kind, &body),
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

    /// `result`, its body rewritten and MACed again if tampering or forging.
    fn tampered(
        &mut self,
        to: usize,
        generation: u64,
        request: RequestId,
        result: Result<Response, Failure>,
    ) -> Result<Response, Failure> {
        let Ok(response) = &result else {
            return result;
        };
        let Some(&(responder, request_mac, kind)) = self.macs.get(&(to, generation, request))
        else {
            return result;
        };
        let Ok(envelope) = ResponseEnvelope::decode(&response.body) else {
            return result;
        };
        if response.status != 200 || envelope.mac.is_none() {
            return result;
        }
        let mut body = envelope.body.to_vec();
        if let Some(tamper) = self.tamper.as_mut() {
            tamper(&mut body);
        }
        if let Some(forge) = self.forge.as_mut() {
            forge(responder, to, kind, &mut body);
        }
        if let Some(list) = self.forged_list(responder, to) {
            let list = list.to_vec();
            match kind {
                Kind::NameListRequest => body = list,
                Kind::NameListHashRequest => body = Sha256::digest(&list).to_vec(),
                _ => {}
            }
        }
        let keys = PairKeys::derive(&key(responder), &key(to).public_key()).unwrap();
        let mac = response_mac(&keys.incoming_response, &request_mac, &body);
        Ok(Response {
            status: 200,
            body: encode_response(Some(&mac), &body),
        })
    }

    fn arrive(&mut self, from: usize, to: usize, request: RequestId, kind: Kind, body: &[u8]) {
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
                answer,
                from: sender,
                request_id,
                ..
            } => {
                self.packets.push((to, sender, request_id));
                Some(sim.hooks.reply(answer))
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
        // A registrar that withholds receipts logged the commitment, and
        // never answers.
        let withheld = kind == Kind::NameCommit && self.role(to) == Role::Withhold;
        if let Some(reply) = reply.filter(|_| !withheld) {
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
        // Each node's next timer, on its own clock. A node's timer changes
        // only when the node does, and every step that changes one drains
        // it, so only the nodes a step drained are asked again: asking
        // every node after every step cost most of a large network's run.
        let mut timers: Vec<Option<u64>> = (0..self.nodes.len())
            .map(|index| self.timer(index))
            .collect();
        self.touched.clear();
        let mut steps = 0u64;
        loop {
            steps += 1;
            assert!(steps < 10_000_000, "the simulation made no progress");
            let next_delivery = self.queue.keys().next().map(|(at, _)| *at);
            let next_timer = timers
                .iter()
                .zip(&self.nodes)
                // Back from the node's clock to the network's.
                .filter_map(|(local, sim)| {
                    Some(local.as_ref()?.saturating_add_signed(-sim.skew_ms))
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
                self.refresh(&mut timers);
                continue;
            }
            for index in 0..self.nodes.len() {
                let now = self.local_now(index);
                if !timers[index].is_some_and(|at| at <= now) {
                    continue;
                }
                let sim = &mut self.nodes[index];
                let Some(node) = sim.node.as_mut() else {
                    continue;
                };
                node.handle_timeout(now, &mut sim.hooks);
                self.drain(index);
                self.refresh(&mut timers);
                assert!(
                    timers[index].is_none_or(|at| at > now),
                    "node {index} is due again at once"
                );
            }
        }
    }

    /// When node `index` is next due, on its own clock: never while down.
    fn timer(&self, index: usize) -> Option<u64> {
        self.nodes[index].node.as_ref()?.poll_timeout()
    }

    /// Asks the nodes drained since the last time for their timers again.
    fn refresh(&mut self, timers: &mut [Option<u64>]) {
        for index in core::mem::take(&mut self.touched) {
            timers[index] = self.timer(index);
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
    /// Pins node `index` to `uplinks`, now and after each restart.
    pub fn pin_uplinks(&mut self, index: usize, uplinks: Vec<NodeId>) {
        self.node_mut(index).pin_uplinks(uplinks.clone());
        self.pinned.insert(index, uplinks);
    }

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
            Inbound::Reply(reply) | Inbound::SchemeMessage { reply, .. } => reply,
            Inbound::Packet { answer, .. } => sim.hooks.reply(answer),
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

    /// The same, with the node's hooks, as the node's books act.
    pub fn act_with_hooks(
        &mut self,
        index: usize,
        f: impl FnOnce(&mut PeerNode, &mut StubHooks, u64),
    ) {
        let now = self.local_now(index);
        let sim = &mut self.nodes[index];
        f(sim.node.as_mut().unwrap(), &mut sim.hooks, now);
        self.drain(index);
    }
}

/// The name a registrar that tips off passes on: one unsealed at it.
/// The names a registrar granted in a step, from what it persisted before
/// and in it: a name it had no record of, or one granted anew since. As
/// many as its events report.
fn granted(sim: &SimNode, events: &[Event], batch: &PeerBatch) -> Vec<NameChange> {
    let granted: Vec<NameChange> = batch
        .names
        .iter()
        .filter(|granted| {
            sim.stored_names
                .get(&granted.name)
                .is_none_or(|before| before.granted_ms != granted.granted_ms)
        })
        .map(|granted| NameChange::Granted {
            name: granted.name,
            holder: granted.holder,
        })
        .collect();
    let reported: usize = events
        .iter()
        .map(|event| match event {
            Event::Name(NameEvent::RegistrarChanged { granted, .. }) => *granted,
            _ => 0,
        })
        .sum();
    assert_eq!(granted.len(), reported, "every grant is read from state");
    granted
}

/// The names a registrar released in a step, from what it persisted before
/// and in it: a name it listed, unfreed, that the step freed before it
/// expired, or freed and forgot while it was still listed. As many as its
/// events report.
fn released(sim: &SimNode, events: &[Event], batch: &PeerBatch, now_ms: u64) -> Vec<NameChange> {
    let unfreed = |name: &Name| {
        sim.stored_names
            .get(name)
            .filter(|before| before.freed_ms.is_none())
    };
    let freed = batch.names.iter().filter_map(|granted| {
        let early = granted.freed_ms.is_some_and(|at| at < granted.expires_ms);
        let before = unfreed(&granted.name)?;
        early.then_some((granted.name, before.holder))
    });
    let forgotten = batch.names_removed.iter().filter_map(|name| {
        let before = unfreed(name)?;
        (now_ms < before.expires_ms).then_some((*name, before.holder))
    });
    let released: Vec<NameChange> = freed
        .chain(forgotten)
        .map(|(name, holder)| NameChange::Released { name, holder })
        .collect();
    let reported: usize = events
        .iter()
        .map(|event| match event {
            Event::Name(NameEvent::Released { names }) => *names,
            _ => 0,
        })
        .sum();
    assert_eq!(released.len(), reported, "every release is read from state");
    released
}

/// How `node`'s view of holders differs from `seen`, which it becomes.
fn view_changes(seen: &mut BTreeMap<Name, NodeId>, node: &PeerNode) -> Vec<NameChange> {
    let now: BTreeMap<Name, NodeId> = node
        .holders()
        .held()
        .map(|held| (held.name, held.holder))
        .collect();
    let unheld = seen
        .keys()
        .filter(|name| !now.contains_key(name))
        .map(|name| NameChange::Holder {
            name: *name,
            holder: None,
        });
    let held = now
        .iter()
        .filter(|(name, holder)| seen.get(name) != Some(holder))
        .map(|(name, holder)| NameChange::Holder {
            name: *name,
            holder: Some(*holder),
        });
    let changes = unheld.chain(held).collect();
    *seen = now;
    changes
}

const fn tip(event: &Event) -> Option<Name> {
    match event {
        Event::Name(NameEvent::Unsealed { name, .. }) => Some(*name),
        _ => None,
    }
}

impl Network {
    /// The names node `index` lists now, as a registrar, from what it
    /// persisted, which is what it holds: whether it is up or down.
    pub fn listed(&self, index: usize) -> BTreeMap<Name, NameHolding> {
        let now = self.local_now(index);
        self.nodes[index]
            .stored_names
            .values()
            .filter(|granted| granted.is_listed(now))
            .map(|granted| {
                let holding = NameHolding {
                    holder: granted.holder,
                    expires_at_ms: granted.expires_ms,
                };
                (granted.name, holding)
            })
            .collect()
    }

    /// The name list a faulty registrar shows `requester` in place of its
    /// own, if it shows one.
    fn forged_list(&self, registrar: usize, requester: usize) -> Option<NameListResponse> {
        match self.role(registrar) {
            Role::Honest | Role::Withhold => None,
            Role::Equivocate => Some(self.equivocated(registrar, requester)),
            Role::TipOff => {
                let adversary = key(self.adversary?).node_id();
                let mut list = self.listed(registrar);
                for (name, expires_ms) in &self.tipped {
                    let holding = NameHolding {
                        holder: adversary,
                        expires_at_ms: *expires_ms,
                    };
                    list.insert(*name, holding);
                }
                Some(NameListResponse(list))
            }
        }
    }

    /// What a registrar that equivocates shows `requester`: a registrar its
    /// own list; any other node, for each name an honest registrar lists,
    /// the holder that sorts first among theirs if its index is even, and
    /// last if it is odd.
    fn equivocated(&self, registrar: usize, requester: usize) -> NameListResponse {
        let registrars = self.bootstrap.len();
        if requester < registrars {
            return NameListResponse(self.listed(requester));
        }
        let mut shown: BTreeMap<Name, NameHolding> = BTreeMap::new();
        for honest in (0..registrars).filter(|index| *index != registrar) {
            for (name, holding) in self.listed(honest) {
                let pick = shown.get(&name).is_none_or(|kept| {
                    if requester.is_multiple_of(2) {
                        holding.holder < kept.holder
                    } else {
                        holding.holder > kept.holder
                    }
                });
                if pick {
                    shown.insert(name, holding);
                }
            }
        }
        NameListResponse(shown)
    }
}
