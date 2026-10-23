//! A simulated hub (spec/areas/testing.md#deterministic-simulation), for
//! this crate's tests and the simulation runner
//! (spec/decisions/random-seed-simulations.md): validators running the
//! consensus engine over a virtual network that delays, loses, duplicates
//! and partitions messages, with skewed clocks, crashes and restarts from
//! what each persisted, and Byzantine validators. Everything comes from one
//! seed.
//!
//! Test code, behind the `sim` feature: a panic here is a failed run.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic,
    clippy::too_many_lines,
    clippy::cast_possible_truncation
)]

pub mod chaos;

use alloc::collections::{BTreeMap, BTreeSet};
use alloc::format;
use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec;
use alloc::vec::Vec;

use dassie_identity::{NodeKey, PublicKey};
use dassie_node_core::testing::SeededRng;
use dassie_oer::Encode;

use crate::consensus::wire::{Message, Proposal};
use crate::consensus::{
    Application, Block, Config, Declined, Engine, Event, Evidence, PeerId, ProposeContext,
    Recipient, Stored, VerifyContext, Vote, VoteKind,
};
use crate::digest::{Digest, Domain};
use crate::validators::{Signers, ValidatorIndex, ValidatorSet};

/// When every simulation starts, on the network's clock.
pub const START: u64 = 1_800_000_000_000;

/// The marker of a payload no honest validator accepts.
const INVALID: u8 = 0xff;

/// Validator `index`'s consensus key, the same in every run.
#[must_use]
pub fn key(index: usize) -> Arc<NodeKey> {
    Arc::new(NodeKey::from_bytes(&[u8::try_from(index + 1).unwrap(); 32]))
}

/// The simulated hub's ID.
#[must_use]
pub fn hub() -> Digest {
    Digest::of(Domain::Rules, &[b"simulated hub"])
}

/// The genesis block every run starts from.
#[must_use]
pub fn genesis() -> Block {
    Block {
        view: 0,
        height: 0,
        parent: Digest::default(),
        parent_view: 0,
        timestamp_ms: START - 1_000,
        payload: b"genesis".to_vec(),
    }
}

/// The set of validators `0..n`.
#[must_use]
pub fn set(indexes: core::ops::Range<usize>) -> ValidatorSet {
    ValidatorSet::new(indexes.map(|index| key(index).public_key()).collect()).unwrap()
}

/// What a simulated validator does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Role {
    /// Follows the protocol.
    Honest,
    /// Sends nothing.
    Silent,
    /// Never sends its proposals.
    Withhold,
    /// Proposes payloads no honest validator accepts.
    Invalid,
    /// Proposes two blocks when it leads, each to half of the others.
    Equivocate,
    /// One of f + 1 that collude: when one leads, they propose two blocks
    /// and sign both, each to half of the honest validators.
    Collude,
}

impl Role {
    /// Whether it follows the protocol.
    #[must_use]
    pub const fn is_honest(self) -> bool {
        matches!(self, Self::Honest)
    }
}

/// The application: payloads that name their proposer and height, and the
/// finalized chain as the host would persist it.
#[derive(Clone, Debug, Default)]
pub struct SimApp {
    /// The validator's index.
    pub index: usize,
    /// Whether it proposes invalid payloads.
    pub invalid: bool,
    /// The finalized blocks by height: the host's durable state. A
    /// validator that loaded the state after a later block lacks those
    /// before it.
    pub finalized: BTreeMap<u64, Digest>,
    /// The votes the last proposal could record.
    pub votes_offered: usize,
}

impl Application for SimApp {
    fn propose(
        &mut self,
        context: &ProposeContext<'_>,
        payload: &mut Vec<u8>,
    ) -> Result<(), Declined> {
        payload.push(if self.invalid { INVALID } else { 1 });
        payload.extend_from_slice(&context.height.to_be_bytes());
        payload.push(u8::try_from(self.index).unwrap());
        self.votes_offered = context.votes.len();
        Ok(())
    }

    fn verify(&mut self, context: &VerifyContext, payload: &[u8]) -> bool {
        payload.first().is_some_and(|first| *first != INVALID)
            && payload.get(1..9) == Some(&context.height.to_be_bytes()[..])
    }

    fn finalized(&mut self, block: &Block, digest: &Digest) {
        let expected = self
            .finalized
            .last_key_value()
            .map_or(1, |(height, _)| height + 1);
        assert_eq!(block.height, expected, "blocks are finalized in order");
        self.finalized.insert(block.height, *digest);
    }
}

/// What the network does to messages.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Faults {
    /// The shortest a message takes.
    pub min_latency_ms: u64,
    /// The longest a message usually takes: within a region.
    pub max_latency_ms: u64,
    /// Per mille lost.
    pub loss: u64,
    /// Per mille delivered twice.
    pub duplication: u64,
    /// Per mille delayed beyond the region's bound, up to `slow_ms`.
    pub slow: u64,
    /// The longest a slow message takes.
    pub slow_ms: u64,
}

impl Faults {
    /// A calm regional network: 1–5 ms, nothing lost.
    pub const CALM: Self = Self {
        min_latency_ms: 1,
        max_latency_ms: 5,
        loss: 0,
        duplication: 0,
        slow: 0,
        slow_ms: 0,
    };
    /// A rough one: losses, duplicates, and some messages far slower than
    /// the region's bound.
    pub const ROUGH: Self = Self {
        min_latency_ms: 1,
        max_latency_ms: 5,
        loss: 50,
        duplication: 30,
        slow: 30,
        slow_ms: 400,
    };
}

/// A validator, up or crashed, and what it persisted.
pub struct SimValidator {
    /// The running engine, or `None` while it is down.
    pub engine: Option<Engine>,
    /// What it persisted.
    pub stored: Stored,
    /// Its application, whose finalized chain is durable.
    pub app: SimApp,
    /// How far its clock is off the network's.
    pub skew_ms: i64,
    /// What it does.
    pub role: Role,
    /// The evidence it reported.
    pub evidence: Vec<Evidence>,
    /// Whether it declared the hub stalled.
    pub stalled: bool,
    /// The longest a block took from its stamp to being final here, by
    /// this validator's clock.
    pub slowest_ms: u64,
}

#[derive(Debug)]
struct Delivery {
    from: usize,
    to: usize,
    bytes: Vec<u8>,
}

/// The simulated network.
pub struct Network {
    /// The network's clock.
    pub now: u64,
    /// Every validator, by index.
    pub validators: Vec<SimValidator>,
    /// Their consensus keys, by index.
    pub keys: Vec<PublicKey>,
    /// The settings every engine runs.
    pub config: Config,
    /// What the network does now.
    pub faults: Faults,
    /// Pairs that cannot reach each other, either way.
    pub cut: BTreeSet<(usize, usize)>,
    /// What happened, for comparing and explaining runs.
    pub trace: Vec<String>,
    /// Whether the colluders already forked the chain once.
    pub forked: bool,
    /// How many times a validator loaded the state after a later block.
    pub state_loads: u64,
    /// How much longer the messages of some validators take.
    pub slow: BTreeMap<usize, u64>,
    queue: BTreeMap<(u64, u64), Delivery>,
    sequence: u64,
    rng: SeededRng,
}

impl Network {
    /// A hub of `roles.len()` validators, all in its genesis set, each with
    /// a clock off by up to half the precision.
    #[must_use]
    pub fn new(seed: u64, roles: &[Role], config: Config) -> Self {
        Self::with_genesis(seed, roles, config, roles.len())
    }

    /// The same, with only the first `members` in the genesis set: the
    /// others follow the hub as observers, until a new set takes them in.
    #[must_use]
    pub fn with_genesis(seed: u64, roles: &[Role], config: Config, members: usize) -> Self {
        let mut rng = SeededRng::new(seed);
        let count = roles.len();
        let validators = set(0..members);
        let half = config.precision_ms / 2;
        let mut network = Self {
            now: START,
            validators: Vec::new(),
            keys: (0..count).map(|index| key(index).public_key()).collect(),
            config,
            faults: Faults::CALM,
            cut: BTreeSet::new(),
            trace: Vec::new(),
            forked: false,
            state_loads: 0,
            slow: BTreeMap::new(),
            queue: BTreeMap::new(),
            sequence: 0,
            rng: SeededRng::new(rng.next_u64()),
        };
        for (index, role) in roles.iter().enumerate() {
            let skew =
                i64::try_from(rng.below(2 * half + 1)).unwrap() - i64::try_from(half).unwrap();
            network.validators.push(SimValidator {
                engine: None,
                stored: Stored::genesis(genesis(), validators.clone()),
                app: SimApp {
                    index,
                    invalid: *role == Role::Invalid,
                    ..SimApp::default()
                },
                skew_ms: skew,
                role: *role,
                evidence: Vec::new(),
                stalled: false,
                slowest_ms: 0,
            });
            network.start(index);
        }
        network
    }

    /// The clock of validator `index`.
    #[must_use]
    pub fn local_now(&self, index: usize) -> u64 {
        self.now
            .saturating_add_signed(self.validators[index].skew_ms)
    }

    /// Starts validator `index` from what it persisted.
    pub fn start(&mut self, index: usize) {
        let now = self.local_now(index);
        let sim = &mut self.validators[index];
        let engine = Engine::new(self.config, hub(), key(index), sim.stored.clone(), now);
        sim.engine = Some(engine);
        self.trace.push(format!("{} start {index}", self.now));
        self.drain(index);
    }

    /// Crashes validator `index`: what it did not persist is lost.
    pub fn crash(&mut self, index: usize) {
        self.validators[index].engine = None;
        self.trace.push(format!("{} crash {index}", self.now));
    }

    /// Whether validator `index` runs.
    #[must_use]
    pub fn is_up(&self, index: usize) -> bool {
        self.validators[index].engine.is_some()
    }

    /// The engine of validator `index`, which must run.
    #[must_use]
    pub fn engine(&self, index: usize) -> &Engine {
        self.validators[index].engine.as_ref().unwrap()
    }

    /// Cuts every link between `side` and the rest.
    pub fn partition(&mut self, side: &[usize]) {
        for a in side {
            for b in 0..self.validators.len() {
                if !side.contains(&b) {
                    self.cut.insert(((*a).min(b), (*a).max(b)));
                }
            }
        }
        self.trace.push(format!("{} partition {side:?}", self.now));
    }

    /// Heals every cut.
    pub fn heal(&mut self) {
        self.cut.clear();
        self.trace.push(format!("{} heal", self.now));
    }

    /// The honest validators.
    pub fn honest(&self) -> impl Iterator<Item = usize> + '_ {
        (0..self.validators.len()).filter(|index| self.validators[*index].role.is_honest())
    }

    /// The highest height every honest validator finalized.
    #[must_use]
    pub fn honest_floor(&self) -> u64 {
        self.honest()
            .map(|index| self.height(index))
            .min()
            .unwrap_or(0)
    }

    /// The height validator `index` finalized.
    #[must_use]
    pub fn height(&self, index: usize) -> u64 {
        self.validators[index]
            .app
            .finalized
            .last_key_value()
            .map_or(0, |(height, _)| *height)
    }

    /// The Byzantine validators.
    #[must_use]
    pub fn faulty(&self) -> Signers {
        (0..self.validators.len())
            .filter(|index| !self.validators[*index].role.is_honest())
            .filter_map(ValidatorIndex::new)
            .fold(Signers::NONE, Signers::with)
    }

    /// Runs `act` on validator `index`'s engine, if it runs, and carries
    /// out what it did.
    pub fn act(&mut self, index: usize, act: impl FnOnce(&mut Engine, &mut SimApp, u64)) {
        let now = self.local_now(index);
        let sim = &mut self.validators[index];
        let Some(engine) = sim.engine.as_mut() else {
            return;
        };
        act(engine, &mut sim.app, now);
        self.drain(index);
    }

    /// Runs the network until `until`.
    pub fn run_until(&mut self, until: u64) {
        // A timer that is due and changes nothing would spin a host at
        // one instant for ever.
        let mut spins = (self.now, 0u32);
        loop {
            let message = self.queue.first_key_value().map(|((at, _), _)| *at);
            let timer = self.next_timer();
            let next = match (message, timer) {
                (Some(m), Some((t, _))) => m.min(t),
                (Some(m), None) => m,
                (None, Some((t, _))) => t,
                (None, None) => break,
            };
            if next > until {
                break;
            }
            self.now = self.now.max(next);
            if message == Some(next) {
                let (_, delivery) = self.queue.pop_first().unwrap();
                self.deliver(delivery);
            } else if let Some((_, index)) = timer {
                spins = if spins.0 == self.now {
                    (self.now, spins.1 + 1)
                } else {
                    (self.now, 0)
                };
                assert!(
                    spins.1 < 10_000,
                    "validator {index}'s timer is due and does nothing"
                );
                self.act(index, |engine, app, now| engine.handle_timeout(now, app));
            }
        }
        self.now = self.now.max(until);
    }

    /// The earliest timer of a running validator, on the network's clock.
    fn next_timer(&self) -> Option<(u64, usize)> {
        (0..self.validators.len())
            .filter_map(|index| {
                let engine = self.validators[index].engine.as_ref()?;
                let local = engine.poll_timeout()?;
                let at = local.saturating_add_signed(-self.validators[index].skew_ms);
                Some((at.max(self.now), index))
            })
            .min()
    }

    fn deliver(&mut self, delivery: Delivery) {
        let Delivery { from, to, bytes } = delivery;
        self.act(to, |engine, app, now| {
            engine.handle_message(now, app, PeerId(from as u64), &bytes);
        });
    }

    /// Checks a validator's invariants, persists its batch, and carries
    /// out its outputs and events.
    fn drain(&mut self, index: usize) {
        let sim = &mut self.validators[index];
        let Some(engine) = sim.engine.as_mut() else {
            return;
        };
        engine.check_invariants();
        let batch = engine.take_batch();
        sim.stored.apply(batch);
        let mut behind = None;
        let mut outputs = Vec::new();
        while let Some(output) = engine.poll_output() {
            outputs.push(output);
        }
        while let Some(event) = engine.poll_event() {
            match &event {
                Event::Evidence(evidence) => {
                    let culprits = evidence.culprits();
                    self.trace.push(format!(
                        "{} {index} evidence against {culprits:?}",
                        self.now
                    ));
                    sim.evidence.push((**evidence).clone());
                }
                Event::Stalled => sim.stalled = true,
                Event::Behind(finalized) => behind = Some((**finalized).clone()),
                Event::Finalized { height, .. } => {
                    let stamped = engine
                        .history()
                        .find(|(_, finalized)| finalized.block.height == *height)
                        .map_or(0, |(_, finalized)| finalized.block.timestamp_ms);
                    let local = self.now.saturating_add_signed(sim.skew_ms);
                    sim.slowest_ms = sim.slowest_ms.max(local.saturating_sub(stamped));
                    self.trace.push(format!("{} {index} {event:?}", self.now));
                }
                event => self.trace.push(format!("{} {index} {event:?}", self.now)),
            }
        }
        for output in outputs {
            let to = self.recipients(index, &output.to);
            self.carry(index, &to, output.message);
        }
        if let Some(finalized) = behind {
            self.load_state(index, finalized);
        }
    }

    /// What the host does when its validator is behind what peers keep:
    /// loads the state after a certified block from a peer, and jumps
    /// there. The simulated state is the chain itself.
    fn load_state(&mut self, index: usize, finalized: crate::consensus::Finalized) {
        let digest = finalized.block.digest(&hub());
        let height = finalized.block.height;
        self.trace.push(format!(
            "{} {index} loads the state after height {height}",
            self.now
        ));
        let mut jumped = false;
        self.act(index, |engine, app, now| {
            if engine.jump(now, finalized).is_ok() {
                app.finalized.insert(height, digest);
                jumped = true;
            }
        });
        if jumped {
            self.state_loads += 1;
        }
    }

    fn recipients(&self, from: usize, to: &Recipient) -> Vec<usize> {
        match to {
            Recipient::All => (0..self.validators.len()).filter(|i| *i != from).collect(),
            Recipient::Validator(key) => self
                .keys
                .iter()
                .position(|held| held == &**key)
                .into_iter()
                .collect(),
            // A peer outside the network, as a test's, hears nothing.
            Recipient::Peer(PeerId(peer)) => usize::try_from(*peer)
                .ok()
                .filter(|peer| *peer < self.validators.len())
                .into_iter()
                .collect(),
        }
    }

    /// Sends what validator `from` said, as its role has it.
    fn carry(&mut self, from: usize, to: &[usize], bytes: Vec<u8>) {
        match self.validators[from].role {
            Role::Silent => {}
            Role::Withhold | Role::Equivocate | Role::Collude => {
                let message = Message::decode_message(&bytes).unwrap();
                match (self.validators[from].role, message) {
                    (Role::Withhold, Message::Proposal(_)) => {}
                    (Role::Equivocate, Message::Proposal(proposal)) => {
                        self.equivocate(from, to, proposal, &[from]);
                    }
                    (Role::Collude, Message::Proposal(proposal)) if !self.forked => {
                        let colluders: Vec<usize> = (0..self.validators.len())
                            .filter(|i| self.validators[*i].role == Role::Collude)
                            .collect();
                        self.forked = true;
                        self.equivocate(from, to, proposal, &colluders);
                    }
                    _ => self.send_all(from, to, &bytes),
                }
            }
            Role::Honest | Role::Invalid => self.send_all(from, to, &bytes),
        }
    }

    /// Proposes `proposal` to one half of `to` and another block to the
    /// other half; every signer in `signers` notarizes and finalizes each
    /// block to its half.
    fn equivocate(&mut self, leader: usize, to: &[usize], proposal: Proposal, signers: &[usize]) {
        let mut other = proposal.block.clone();
        other.payload.push(0xaa);
        // A lone equivocator, in odd views, builds its second block on an
        // older final block, skipping a notarized one: only the rule that
        // skipped views be nullified stops validators that lag.
        let older = self.validators[leader].engine.as_ref().and_then(|engine| {
            let history: Vec<_> = engine.history().collect();
            history.len().checked_sub(2).map(|at| history[at].clone())
        });
        if let (true, Some((digest, finalized))) =
            (signers.len() == 1 && other.view % 2 == 1, older)
        {
            other.parent = digest;
            other.parent_view = finalized.block.view;
            other.height = finalized.block.height + 1;
            other.payload = vec![1];
            other.payload.extend_from_slice(&other.height.to_be_bytes());
            other
                .payload
                .extend_from_slice(&[u8::try_from(leader).unwrap(), 0xaa]);
        }
        let view = other.view;
        let digests = [proposal.block.digest(&hub()), other.digest(&hub())];
        let blocks = [proposal.block, other];
        let targets: Vec<usize> = to
            .iter()
            .copied()
            .filter(|i| !signers.contains(i))
            .collect();
        let halves = [
            targets.iter().copied().step_by(2).collect::<Vec<_>>(),
            targets
                .iter()
                .copied()
                .skip(1)
                .step_by(2)
                .collect::<Vec<_>>(),
        ];
        self.trace
            .push(format!("{} {leader} equivocates in view {view}", self.now));
        for ((block, digest), half) in blocks.into_iter().zip(digests).zip(halves) {
            let leader_index = ValidatorIndex::new(leader).unwrap();
            let signature = Vote::sign(
                &hub(),
                &key(leader),
                leader_index,
                VoteKind::Notarize,
                view,
                digest,
            )
            .signature;
            let proposal = Message::Proposal(Proposal { block, signature }).to_vec();
            self.send_all(leader, &half, &proposal);
            // A lone equivocator's notarize is its proposal; colluders all
            // vote both ways.
            let colluding = if signers.len() > 1 { signers } else { &[] };
            for signer in colluding {
                let index = ValidatorIndex::new(*signer).unwrap();
                for kind in [VoteKind::Notarize, VoteKind::Finalize] {
                    let vote = Vote::sign(&hub(), &key(*signer), index, kind, view, digest);
                    self.send_all(*signer, &half, &Message::Vote(vote).to_vec());
                }
            }
        }
    }

    fn send_all(&mut self, from: usize, to: &[usize], bytes: &[u8]) {
        for &to in to {
            let cut = self.cut.contains(&(from.min(to), from.max(to)));
            if cut || self.rng.below(1000) < self.faults.loss {
                continue;
            }
            let copies = if self.rng.below(1000) < self.faults.duplication {
                2
            } else {
                1
            };
            for _ in 0..copies {
                let delay = self.latency() + self.slow.get(&from).copied().unwrap_or(0);
                self.sequence += 1;
                self.queue.insert(
                    (self.now + delay, self.sequence),
                    Delivery {
                        from,
                        to,
                        bytes: bytes.to_vec(),
                    },
                );
            }
        }
    }

    fn latency(&mut self) -> u64 {
        let faults = self.faults;
        if faults.slow_ms > 0 && self.rng.below(1000) < faults.slow {
            return faults.max_latency_ms + self.rng.below(faults.slow_ms);
        }
        let span = faults.max_latency_ms - faults.min_latency_ms + 1;
        faults.min_latency_ms + self.rng.below(span)
    }

    /// Whether the honest validators' finalized chains agree wherever both
    /// reach: the first pair that differs, if any.
    #[must_use]
    pub fn disagreement(&self) -> Option<String> {
        let honest: Vec<usize> = self.honest().collect();
        for (at, a) in honest.iter().enumerate() {
            for b in honest.iter().skip(at + 1) {
                let (chain_a, chain_b) = (
                    &self.validators[*a].app.finalized,
                    &self.validators[*b].app.finalized,
                );
                let differs = chain_a
                    .iter()
                    .filter_map(|(height, x)| Some((height, x, chain_b.get(height)?)))
                    .find(|(_, x, y)| x != y);
                if let Some((height, x, y)) = differs {
                    return Some(format!(
                        "validators {a} and {b} finalized {x:?} and {y:?} at height {height}"
                    ));
                }
            }
        }
        None
    }

    /// Evidence an honest validator reported against an honest one, if any.
    #[must_use]
    pub fn blamed_honest(&self) -> Option<String> {
        let faulty = self.faulty();
        self.honest().find_map(|index| {
            self.validators[index].evidence.iter().find_map(|evidence| {
                let culprits = evidence.culprits();
                (culprits.bits() & !faulty.bits() != 0).then(|| {
                    format!(
                        "validator {index} blamed {culprits:?}, of whom only {faulty:?} are faulty"
                    )
                })
            })
        })
    }

    /// Whether an honest validator reported evidence naming at least
    /// `count` validators, or one validator's conflicting votes.
    #[must_use]
    pub fn evidence_found(&self) -> bool {
        self.honest()
            .any(|index| !self.validators[index].evidence.is_empty())
    }
}

/// The roles of a run: `byzantine` of `count` validators, from the end, in
/// the roles drawn.
#[must_use]
pub fn roles(count: usize, byzantine: &[Role]) -> Vec<Role> {
    let mut roles = vec![Role::Honest; count];
    for (at, role) in byzantine.iter().enumerate() {
        roles[count - 1 - at] = *role;
    }
    roles
}
