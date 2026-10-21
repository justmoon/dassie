//! The node simulation's random run
//! (spec/decisions/random-seed-simulations.md): every seed draws its own
//! network, faults and payments, as swarm testing does, so that runs differ
//! in kind and not only in detail. Some seeds switch a fault off entirely, so
//! that rare combinations get their turn.
//!
//! A run has two phases. In the chaos, payments start while nodes crash
//! or fail a commit and restart, links are cut and healed, messages are
//! lost, requests are replayed, ledger connections drop, and owners end
//! peerings and ban peers; each node's clock is off the network's by up
//! to 10 s, throughout. The network may also split in two, for minutes
//! or for days: a partition that outlasts the chaos keeps the network as
//! the chaos left it, its cuts too, until it ends. Then the network heals:
//! every node runs, no link is cut, nothing is lost or replayed, and within
//! [`HEAL_MS`] the network must settle down. Then each node is checked by
//! its own books (spec/decisions/own-books.md), as the scripted runs are.

use std::collections::BTreeSet;
use std::fmt;

use dassie_connector::SpendingCap;
use dassie_identity::NodeId;
use dassie_node_core::testing::{Outcome, Run, SeededRng};
use dassie_settlement::LedgerStatus;

use super::checks::{
    check, check_delivered, check_events, check_finished, check_mid_flight, check_settled,
};
use super::scripted::{
    PAID_WITHIN_MS, READY_WITHIN_MS, collected_before_telling, ended, ready,
    settled_nothing_once_ending, trace_ms, until_peered,
};
use super::{Delivery, START, Settling, Sim, key};
use crate::engine::LedgerEvent;
use crate::node::Policies;
use crate::store::{Aim, OutgoingState};

/// How long a healed network has to settle down, from the production rules
/// it runs: the longest a wait begun before the heal may still run after
/// it, then as long as a network takes to be ready. What failed while the
/// network was faulty, and above all while a partition kept it split,
/// waits out its backoff before it is tried again: a registration, a
/// refresh of a link state, a peering attempt, a peer's hold-down, or a
/// settlement to be sent again. Once each has been tried again, the network
/// forms as a new one does, and then the payments left, started once it is
/// ready, take as long as a run's payments may, with the settlements they
/// call for. A run past it is stuck, not slow. It follows those rules, and
/// is never loosened to let a seed pass.
pub const HEAL_MS: u64 = LONGEST_WAIT_MS + READY_WITHIN_MS + PAID_WITHIN_MS;

/// The longest of the backoffs and hold-downs that a fault leaves behind.
const LONGEST_WAIT_MS: u64 = {
    let mut longest = PEER.max_backoff_ms;
    if PEER.advertise_hold_max_ms > longest {
        longest = PEER.advertise_hold_max_ms;
    }
    if PEER.settlement_retry_max_ms > longest {
        longest = PEER.settlement_retry_max_ms;
    }
    longest
};

/// How often a healing network is looked at.
const LOOK_MS: u64 = 5_000;

/// The peer protocol's production timings, which a partition's length is
/// drawn from.
const PEER: dassie_peer::Config = Policies::DEFAULT.peer;

/// The least a payment sends: 0.1 units.
const LEAST_PAYMENT: u64 = 100_000_000;

/// The most a node's clock is off the network's, either way: as the peer
/// simulation's, and well inside the peer protocol's replay window of
/// 60 s, which a node with a clock further off fails anyway.
const MOST_SKEW_MS: u64 = 10_000;

/// How often each kind of chaos happens, relative to the others. A weight
/// of 0 switches it off.
#[derive(Clone, Copy, Debug)]
pub struct Weights {
    /// Starting a payment, while any is left to start.
    pub pay: u64,
    /// Crashing a node.
    pub crash: u64,
    /// Restarting a crashed node.
    pub restart: u64,
    /// Cutting a link, one way or both.
    pub cut: u64,
    /// Healing a cut.
    pub heal: u64,
    /// Dropping a node's connection to the XRP Ledger.
    pub ledger_drop: u64,
    /// Making one of a node's next commits fail
    /// (spec/decisions/random-seed-simulations.md).
    pub fail_commit: u64,
    /// An owner ending one of its node's peerings.
    pub end: u64,
    /// An owner banning one of its node's peers.
    pub ban: u64,
    /// Doing nothing for a while.
    pub idle: u64,
}

/// What a seed draws before its run starts.
#[derive(Clone, Copy, Debug)]
pub struct Options {
    /// How many nodes.
    pub nodes: usize,
    /// How many of them are bootstrap nodes.
    pub bootstraps: usize,
    /// What they settle over.
    pub settling: Settling,
    /// The shortest and the longest a message takes.
    pub latency_ms: (u64, u64),
    /// How many of a thousand messages are lost in the chaos.
    pub loss_per_mille: u64,
    /// How long the chaos lasts.
    pub chaos_ms: u64,
    /// The mean time between two acts of chaos.
    pub step_ms: u64,
    /// How often each kind of chaos happens.
    pub weights: Weights,
    /// How many payments BTP clients make, and how many outgoing payments.
    pub payments: (u64, u64),
    /// The most a payment sends.
    pub largest_payment: u64,
    /// How many of a thousand requests that get through are replayed in
    /// the chaos.
    pub replay_per_mille: u64,
    /// The most a node's clock is off the network's, either way.
    pub skew_ms: u64,
    /// How the network splits in two for a while, if it does.
    pub partition: Option<Partition>,
}

/// A partition: for a while, no request gets from one side of the network
/// to the other. It starts in the chaos and may outlast it, the network
/// staying split until it ends, and then heals with the rest.
#[derive(Clone, Copy, Debug)]
pub struct Partition {
    /// The nodes on one side, a bit each: the rest are on the other.
    pub apart: u64,
    /// When it starts, after the chaos did.
    pub after_ms: u64,
    /// How long it lasts.
    pub length_ms: u64,
}

impl Partition {
    /// The partition `seed` draws for `nodes` nodes and a chaos of
    /// `chaos_ms`: none in a third of the seeds, one node cut off from the
    /// rest in another, the network split in halves in the last. Its own
    /// generator, so that it draws nothing else's.
    ///
    /// Its length follows the production timings that act on silence.
    /// Three in four are shorter than the silence that ends a peering: from
    /// the time a peer stays confirmed without a word, doubling, so that
    /// the lengths spread over the timings between, which withdraw a peer,
    /// hold it down, let link states go stale and registrations wait to be
    /// renewed. The rest outlast that silence by up to as long again, so
    /// that peerings end, registrations and lists lapse and incoming
    /// payments expire before the network heals. None is longer: the
    /// longest of those timings is that day, so a longer split only repeats
    /// them, and each simulated day costs up to a minute of wall time.
    #[must_use]
    pub fn draw(seed: u64, nodes: usize, chaos_ms: u64) -> Option<Self> {
        let mut rng = SeededRng::new(seed ^ 0x9a27_0000);
        let count = u64::try_from(nodes).unwrap();
        assert!((2..64).contains(&count));
        let apart = match rng.below(3) {
            0 => return None,
            1 => 1 << rng.below(count),
            _ => {
                let mut apart = 0u64;
                while apart.count_ones() < u32::try_from(count / 2).unwrap() {
                    apart |= 1 << rng.below(count);
                }
                apart
            }
        };
        let length_ms = if rng.below(4) != 0 {
            let least = PEER.peer_confirm_ms << rng.below(LONGEST_DOUBLING);
            least + rng.below(least)
        } else {
            PEER.peer_dead_ms + rng.below(PEER.peer_dead_ms + 1)
        };
        let partition = Self {
            apart,
            // After the chaos started, so that every partition drawn
            // splits the network.
            after_ms: 1 + rng.below(chaos_ms - 1),
            length_ms,
        };
        assert!(partition.apart != 0 && partition.apart < 1 << count);
        Some(partition)
    }

    /// The nodes on one side.
    fn side(self) -> impl Iterator<Item = usize> {
        (0..64).filter(move |index| self.apart & (1 << index) != 0)
    }
}

/// How many times, less one, a partition shorter than the silence that
/// ends a peering may double the time a peer stays confirmed: doubled
/// once more, which the draw stays below, it is still within the silence.
const LONGEST_DOUBLING: u64 = 10;
const _: () = assert!(PEER.peer_confirm_ms << LONGEST_DOUBLING <= PEER.peer_dead_ms);

impl Options {
    /// The options `seed` draws.
    #[must_use]
    pub fn swarm(seed: u64) -> Self {
        let mut rng = SeededRng::new(seed ^ 0x5a4a_4d00);
        let nodes = 2 + usize::try_from(rng.below(11)).unwrap();
        let bootstraps = 1 + usize::try_from(rng.below(3)).unwrap().min(nodes - 1);
        let settling = if rng.below(2) == 0 {
            Settling::Stub
        } else {
            Settling::Xrpl
        };
        let shortest = 1 + rng.below(50);
        let latency_ms = (shortest, shortest + rng.below(400));
        let loss_per_mille = switched(&mut rng, 31);
        let chaos_ms = 30_000 + rng.below(210_001);
        let step_ms = 500 + rng.below(5_000);
        let weights = Weights {
            pay: 1 + rng.below(10),
            crash: switched(&mut rng, 11),
            restart: switched(&mut rng, 11),
            cut: switched(&mut rng, 11),
            heal: switched(&mut rng, 11),
            ledger_drop: if settling == Settling::Xrpl {
                switched(&mut rng, 11)
            } else {
                0
            },
            fail_commit: switched(&mut rng, 11),
            end: switched(&mut rng, 4),
            ban: switched(&mut rng, 4),
            idle: rng.below(11),
        };
        let payments = (1 + rng.below(8), rng.below(3));
        let largest_payment = LEAST_PAYMENT + rng.below(3_300_000_001);
        let replay_per_mille = switched(&mut rng, 51);
        let skew_ms = switched(&mut rng, MOST_SKEW_MS + 1);
        let partition = Partition::draw(seed, nodes, chaos_ms);
        Self {
            nodes,
            bootstraps,
            settling,
            latency_ms,
            loss_per_mille,
            chaos_ms,
            step_ms,
            weights,
            payments,
            largest_payment,
            replay_per_mille,
            skew_ms,
            partition,
        }
    }

    /// Each node's clock: how far it is ahead of the network's, or behind
    /// it, drawn from `seed`.
    #[must_use]
    pub fn skews(&self, seed: u64) -> Vec<i64> {
        let mut rng = SeededRng::new(seed ^ 0x5c3e_0000);
        let skew = i64::try_from(self.skew_ms).unwrap();
        (0..self.nodes)
            .map(|_| i64::try_from(rng.below(2 * self.skew_ms + 1)).unwrap() - skew)
            .collect()
    }
}

/// Off in a third of the seeds, and below `bound` in the rest.
fn switched(rng: &mut SeededRng, bound: u64) -> u64 {
    if rng.below(3) == 0 {
        0
    } else {
        rng.below(bound)
    }
}

impl fmt::Display for Options {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let w = &self.weights;
        write!(
            f,
            "{} nodes, {} bootstrap, {:?}; latency {}–{} ms, loss {}‰, replays {}‰, \
             clocks off by up to {} ms; chaos {} s, \
             every ~{} ms: pay {} crash {} restart {} cut {} heal {} ledger drop {} \
             fail commit {} end {} ban {} idle {}; \
             {} payments and {} outgoing, up to {}",
            self.nodes,
            self.bootstraps,
            self.settling,
            self.latency_ms.0,
            self.latency_ms.1,
            self.loss_per_mille,
            self.replay_per_mille,
            self.skew_ms,
            self.chaos_ms / 1_000,
            self.step_ms,
            w.pay,
            w.crash,
            w.restart,
            w.cut,
            w.heal,
            w.ledger_drop,
            w.fail_commit,
            w.end,
            w.ban,
            w.idle,
            self.payments.0,
            self.payments.1,
            self.largest_payment,
        )?;
        match self.partition {
            None => write!(f, "; no partition"),
            Some(partition) => write!(
                f,
                "; nodes {:?} cut off at {} s for {} s",
                partition.side().collect::<Vec<_>>(),
                partition.after_ms / 1_000,
                partition.length_ms / 1_000,
            ),
        }
    }
}

/// What a seed draws, before the run starts.
#[must_use]
pub fn describe(seed: u64) -> String {
    Options::swarm(seed).to_string()
}

/// Runs the seed: its chaos, then its heal, then the checks.
#[must_use]
pub fn run(seed: u64) -> Run {
    let options = Options::swarm(seed);
    let mut rng = SeededRng::new(seed ^ 0xc4a0_5000);
    let skews = options.skews(seed);
    let mut sim = Sim::with_skews(
        seed,
        options.bootstraps,
        std::collections::BTreeMap::new(),
        options.settling,
        Policies::DEFAULT.peer,
        &skews,
    );
    for _ in skews.iter().filter(|skew| **skew != 0) {
        sim.count("clocks off");
    }
    sim.latency_ms = options.latency_ms;
    // The network forms first, calm, as the scripted runs' does: at
    // production timings that takes minutes, longer than a payer keeps
    // trying, and a payment before it has nowhere to go.
    sim.loss_per_mille = 0;
    if !sim.run_until_or(START + READY_WITHIN_MS, ready) {
        let late = Err(Outcome::liveness(
            "ready",
            "the network was not ready in time",
        ));
        return ended(sim, late);
    }
    sim.trace.push(format!("{} ready", sim.now));
    sim.loss_per_mille = options.loss_per_mille;
    sim.replay_per_mille = options.replay_per_mille;
    let mut left = options.payments;
    let mut ends = Vec::new();
    let result = chaos(&mut sim, &options, &mut rng, &mut left, &mut ends)
        .and_then(|split| heal(&mut sim, &options, &mut rng, &mut left, &ends, split));
    ended(sim, result)
}

/// The kinds of chaos.
#[derive(Clone, Copy)]
enum Act {
    Pay,
    Crash,
    Restart,
    Cut,
    Heal,
    LedgerDrop,
    FailCommit,
    End,
    Ban,
    Idle,
}

/// The chaos: its acts, and its partition, which may outlast them. The
/// network then stays split, as the acts left it, until the partition
/// ends; only a node that is down starts again. Returns whether it did.
fn chaos(
    sim: &mut Sim,
    options: &Options,
    rng: &mut SeededRng,
    left: &mut (u64, u64),
    ends: &mut Vec<End>,
) -> Result<bool, Outcome> {
    let start = sim.now;
    let end = start + options.chaos_ms;
    let mut at = sim.now;
    while at < end {
        at += 1 + rng.below(2 * options.step_ms);
        advance(sim, options.partition, start, at)?;
        match pick(&options.weights, rng) {
            Act::Pay => pay(sim, options, rng, left),
            Act::Crash => crash(sim, rng),
            Act::Restart => {
                let down: Vec<usize> = (0..sim.nodes.len()).filter(|i| !up(sim, *i)).collect();
                if let Some(&index) = choose(&down, rng) {
                    sim.start(index);
                    sim.count("restarts");
                }
            }
            Act::Cut => cut(sim, rng),
            Act::Heal => {
                if let Some(&pair) = sim.cut.iter().next() {
                    sim.cut.remove(&pair);
                }
            }
            Act::LedgerDrop => {
                let linked: Vec<usize> = sim.links.keys().copied().collect();
                if let Some(&index) = choose(&linked, rng) {
                    sim.drop_ledger_link(index);
                    sim.count("ledger drops");
                }
            }
            Act::FailCommit => fail_commit(sim, rng),
            Act::End => owner_ends(sim, rng, ends, false),
            Act::Ban => owner_ends(sim, rng, ends, true),
            Act::Idle => {}
        }
    }
    let Some(partition) = options.partition else {
        return Ok(false);
    };
    let joined = start + partition.after_ms + partition.length_ms;
    let outlasted = joined > sim.now;
    if outlasted {
        // A node that crashed, or failed a commit, is started again, as
        // its supervisor starts it within seconds: the chaos paces restarts
        // only while it lasts. Held down for the days a split may last, it
        // would be another fault, a long outage, and would leave the split
        // no peering to part.
        sim.let_commits_through();
        for index in 0..sim.nodes.len() {
            if !up(sim, index) {
                sim.start(index);
            }
        }
    }
    advance(sim, Some(partition), start, joined.max(sim.now))?;
    Ok(outlasted)
}

/// Runs the chaos that started at `start` to `until`, splitting the
/// network when its partition starts and joining it when it ends.
fn advance(
    sim: &mut Sim,
    partition: Option<Partition>,
    start: u64,
    until: u64,
) -> Result<(), Outcome> {
    if let Some(partition) = partition {
        let split = start + partition.after_ms;
        let joined = split + partition.length_ms;
        if sim.now < split && split <= until {
            sim.run_until(split);
            sim.apart = partition.side().collect();
            sim.trace.push(format!("{} split {:?}", sim.now, sim.apart));
        }
        if sim.now < joined && joined <= until {
            // An hour at a time: the run's own bound on its steps is for a
            // stretch of chaos, not for days of it.
            while sim.now < joined {
                sim.run_until(joined.min(sim.now + 3_600_000));
            }
            sim.apart.clear();
            sim.trace.push(format!("{} joined", sim.now));
            sim.count("partitions");
            if partition.length_ms >= PEER.peer_dead_ms {
                sim.count("partitions longer than the silence that ends a peering");
            }
            check_mid_flight(sim)?;
            check_expired(sim)
                .map_err(|problem| Outcome::liveness("outgoing-deadline", problem))?;
        }
    }
    sim.run_until(until);
    Ok(())
}

/// Every outgoing payment past its deadline ended, completed or failed,
/// once its last packets were held as long as a node holds one: whatever
/// the network does, a payment across a partition does not wait for it to
/// heal. A node that is down holds its payments until it starts.
fn check_expired(sim: &Sim) -> Result<(), String> {
    let hold = Policies::DEFAULT.connector.max_hold_ms;
    for &(from, to, _, outgoing) in &sim.outgoing {
        if !up(sim, from) {
            continue;
        }
        let row = sim.nodes[from]
            .store
            .outgoing_payment(outgoing)
            .unwrap()
            .unwrap();
        let local = sim.now.saturating_add_signed(sim.nodes[from].skew_ms);
        if row.state == OutgoingState::Sending && row.deadline_ms + hold < local {
            return Err(format!(
                "the outgoing payment from node {from} to node {to} still sends {} s past its deadline",
                (local - row.deadline_ms) / 1_000
            ));
        }
    }
    Ok(())
}

fn pick(weights: &Weights, rng: &mut SeededRng) -> Act {
    let kinds = [
        (weights.pay, Act::Pay),
        (weights.crash, Act::Crash),
        (weights.restart, Act::Restart),
        (weights.cut, Act::Cut),
        (weights.heal, Act::Heal),
        (weights.ledger_drop, Act::LedgerDrop),
        (weights.fail_commit, Act::FailCommit),
        (weights.end, Act::End),
        (weights.ban, Act::Ban),
        (weights.idle, Act::Idle),
    ];
    let total: u64 = kinds.iter().map(|(weight, _)| weight).sum();
    let mut drawn = rng.below(total);
    for (weight, act) in kinds {
        if drawn < weight {
            return act;
        }
        drawn -= weight;
    }
    Act::Idle
}

fn choose<'a, T>(items: &'a [T], rng: &mut SeededRng) -> Option<&'a T> {
    let len = u64::try_from(items.len()).unwrap();
    items.get(usize::try_from(rng.below(len)).unwrap())
}

fn up(sim: &Sim, index: usize) -> bool {
    sim.nodes[index].core.is_some()
}

/// Starts a payment, if any is left: by a BTP client to another node's
/// receiver, sometimes with a limit or a capped token, or an outgoing
/// payment, sometimes to the node's own receiver. Both ends must run. A
/// payment that a failed commit stopped before it started is left for
/// later, as its owner or client, told of the failure, would try again.
fn pay(sim: &mut Sim, options: &Options, rng: &mut SeededRng, left: &mut (u64, u64)) {
    let running: Vec<usize> = (0..sim.nodes.len()).filter(|i| up(sim, *i)).collect();
    let (Some(&from), Some(&to)) = (choose(&running, rng), choose(&running, rng)) else {
        return;
    };
    let amount = LEAST_PAYMENT + rng.below(options.largest_payment - LEAST_PAYMENT + 1);
    if left.0 > 0 && (left.1 == 0 || rng.below(4) != 0) {
        if from == to {
            return;
        }
        let capped = rng.below(8) == 0;
        let started = if capped {
            // A token capped at 0.1 units per 10 s pays 0.4: it must wait
            // for its cap to refill.
            let cap = SpendingCap {
                amount: LEAST_PAYMENT,
                window_ms: 10_000,
            };
            sim.pay(from, to, 4 * LEAST_PAYMENT, Some(cap), None)
        } else {
            let limit = (rng.below(4) == 0).then_some(amount / 2);
            sim.pay(from, to, amount, None, limit)
        };
        if started {
            left.0 -= 1;
            sim.count("payments");
            if capped {
                sim.count("capped payments");
            }
        }
    } else if left.1 > 0 {
        match sim.pay_outgoing(from, to, amount) {
            Ok(()) => {
                left.1 -= 1;
                sim.count("outgoing payments");
            }
            Err(_) if !up(sim, from) || !up(sim, to) => {}
            Err(refusal) => {
                left.1 -= 1;
                // Refused rightly only while the owner's pool holds less
                // than the payment: on the XRP Ledger, before the faucet's
                // funding is booked. Any other refusal fails the run.
                let pool = sim.balances(from).get("equity/owner").copied().unwrap_or(0);
                if pool < i128::from(amount) {
                    sim.count("outgoing refused");
                } else {
                    let now = sim.now;
                    sim.refused.push(format!(
                        "{now} node {from} refused an outgoing payment of {amount} \
                         its owner's {pool} covers: {refusal}"
                    ));
                }
            }
        }
    }
}

/// Crashes a running node: half the time one at either end of a payment
/// still under way, if there is one.
fn crash(sim: &mut Sim, rng: &mut SeededRng) {
    let running: Vec<usize> = (0..sim.nodes.len()).filter(|i| up(sim, *i)).collect();
    let paying: Vec<usize> = sim
        .payments
        .iter()
        .filter(|payment| !payment.done)
        .flat_map(|payment| [payment.from, payment.to])
        .filter(|index| up(sim, *index))
        .collect();
    let victim = if !paying.is_empty() && rng.below(2) == 0 {
        choose(&paying, rng)
    } else {
        choose(&running, rng)
    };
    if let Some(&victim) = victim {
        if sim.payments.iter().any(|payment| !payment.done) {
            sim.count("crashes mid-payment");
        }
        sim.crash(victim);
    }
}

/// Makes a running node fail one of its commits, aimed at a moment that
/// matters: half the time a commit that books a packet's transfer, at a
/// node that has packets in flight or is at either end of a payment under
/// way; a quarter of the time one that holds a settlement, at whichever
/// node first commits one, since few settle at any one time; else any
/// commit. Which of the commits aimed at fails varies, so that it may be a
/// later one, such as the commit of a settlement once it was executed.
fn fail_commit(sim: &mut Sim, rng: &mut SeededRng) {
    let skip = u32::try_from(rng.below(3)).unwrap();
    let aim = match rng.below(4) {
        0 | 1 => Aim::Packet,
        2 => {
            sim.fail_commit_anywhere(Aim::Settlement, skip);
            return;
        }
        _ => Aim::Any,
    };
    let running: Vec<usize> = (0..sim.nodes.len())
        .filter(|i| up(sim, *i) && !sim.failing(*i))
        .collect();
    let paying: Vec<usize> = sim
        .payments
        .iter()
        .filter(|payment| !payment.done)
        .flat_map(|payment| [payment.from, payment.to])
        .chain(
            running
                .iter()
                .copied()
                .filter(|index| sim.core(*index).packets_in_flight()),
        )
        .filter(|index| running.contains(index))
        .collect();
    let victim = if aim == Aim::Packet && !paying.is_empty() {
        choose(&paying, rng)
    } else {
        choose(&running, rng)
    };
    if let Some(&victim) = victim {
        sim.fail_commit(victim, aim, skip);
    }
}

/// Cuts a link one way, or both.
fn cut(sim: &mut Sim, rng: &mut SeededRng) {
    let all: Vec<usize> = (0..sim.nodes.len()).collect();
    let (Some(&a), Some(&b)) = (choose(&all, rng), choose(&all, rng)) else {
        return;
    };
    if a == b {
        return;
    }
    sim.cut.insert((a, b));
    if rng.below(2) == 0 {
        sim.cut.insert((b, a));
    }
    sim.count("cuts");
}

/// An owner's end of a peering, or a ban, drawn in the chaos.
struct End {
    /// Where the trace stood when the owner acted: what the nodes did
    /// about it follows.
    mark: usize,
    /// The node whose owner acted.
    by: usize,
    /// Its peer.
    peer: usize,
    /// Whether the owner banned the peer, rather than only ending the
    /// peering.
    ban: bool,
}

/// An owner ends one of its running node's peerings, or bans the peer,
/// through the owner's API, as the scripted run's owner does: an end only
/// of a peering still active, a ban also of one being ended. Never one
/// that would split the network: after an owner's end the pair stays
/// apart for the owner's cooldown of a day
/// (spec/decisions/ended-for-room.md), which outlasts the heal unless a
/// partition held the network split for most of it, and after a ban for good,
/// while the heal needs every node to reach every other.
fn owner_ends(sim: &mut Sim, rng: &mut SeededRng, ends: &mut Vec<End>, ban: bool) {
    let running: Vec<usize> = (0..sim.nodes.len()).filter(|i| up(sim, *i)).collect();
    let Some(&by) = choose(&running, rng) else {
        return;
    };
    let network = sim.core(by).network();
    let peers: Vec<usize> = network
        .peers
        .iter()
        .filter(|peer| {
            if ban {
                !network.bans.contains(&peer.id)
            } else {
                peer.termination.is_none() && peer.peering.depeering.is_none()
            }
        })
        .map(|peer| index_of(sim, &peer.id))
        .filter(|peer| connected(sim.nodes.len(), ends, (by, *peer)))
        .collect();
    let Some(&peer) = choose(&peers, rng) else {
        return;
    };
    let id = key(peer).node_id();
    let what = if ban { "bans" } else { "ends its peering with" };
    sim.trace
        .push(format!("{} {by}'s owner {what} {peer}", sim.now));
    let mark = sim.trace.len();
    let done = if ban {
        sim.step(by, |core| core.ban_node(id).unwrap())
    } else {
        sim.step(by, |core| core.terminate_peering(id).unwrap())
    };
    // A commit that failed lost the owner's word with the rest of the
    // step: the node stopped, and starts again as if it was never given.
    if !done {
        sim.count("owners' acts lost to a failed commit");
        return;
    }
    sim.count(if ban { "bans" } else { "peerings ended" });
    ends.push(End {
        mark,
        by,
        peer,
        ban,
    });
}

/// The index of the node `id`.
fn index_of(sim: &Sim, id: &NodeId) -> usize {
    (0..sim.nodes.len())
        .find(|index| key(*index).node_id() == *id)
        .unwrap()
}

/// Whether the nodes still reach each other with `pair` kept apart too:
/// over every pair but those an owner's end or a ban keeps apart. Kept
/// apart for the whole run, as the cooldown is unless both restart or a
/// long partition outlasts it.
fn connected(count: usize, ends: &[End], pair: (usize, usize)) -> bool {
    let apart = |a: usize, b: usize| {
        ends.iter()
            .map(|end| (end.by, end.peer))
            .chain([pair])
            .any(|(x, y)| (x, y) == (a, b) || (y, x) == (a, b))
    };
    let mut reached = BTreeSet::from([0]);
    let mut next = vec![0];
    while let Some(a) = next.pop() {
        for b in 0..count {
            if !apart(a, b) && reached.insert(b) {
                next.push(b);
            }
        }
    }
    reached.len() == count
}

/// Heals the network, starts what payments are left, waits for it to
/// settle down, and checks it. After a `split` that outlasted the chaos,
/// commits already go through.
fn heal(
    sim: &mut Sim,
    options: &Options,
    rng: &mut SeededRng,
    left: &mut (u64, u64),
    ends: &[End],
    split: bool,
) -> Result<(), Outcome> {
    if !split {
        sim.let_commits_through();
    }
    for index in 0..sim.nodes.len() {
        if !up(sim, index) {
            sim.start(index);
        }
    }
    sim.cut.clear();
    assert!(sim.apart.is_empty(), "the partition ends in the chaos");
    sim.loss_per_mille = 0;
    sim.replay_per_mille = 0;
    sim.trace.push(format!("{} heal", sim.now));
    // A payer gives up after its reconnects (a few minutes), and the chaos
    // may have cut it off that long, or the network not yet formed: what
    // failed before the heal may stay failed. So may one whose payer was
    // reconnecting at the heal: what is left of its reconnects may run out
    // before the healed network routes it, as when an owner's end left a
    // pair that must peer anew, waiting out a backoff from attempts that
    // failed in the chaos. Only reconnects since its node last started
    // count: a start gives the payer all its reconnects again, so one on a
    // node that restarted since, the heal's own restarts included, must
    // complete, as must anything else.
    let mut reconnecting = Vec::new();
    for &(from, _, _, outgoing) in &sim.outgoing.clone() {
        let state = sim.nodes[from]
            .store
            .outgoing_payment(outgoing)
            .unwrap()
            .map(|row| row.state);
        if state == Some(OutgoingState::Failed) {
            sim.given_up.insert(outgoing);
            sim.count("outgoing given up in the chaos");
        } else if state == Some(OutgoingState::Sending)
            && reconnected_since_start(&sim.trace, from, &format!("{outgoing:?}"))
        {
            sim.given_up.insert(outgoing);
            reconnecting.push((from, outgoing));
        }
    }
    // After a split that outlasted the chaos, what payments are left start
    // once the network is ready again, as the run's first ones do: it forms
    // anew, which takes longer than a payer keeps trying, and a payment
    // started before has nowhere to go. Otherwise they start at once, while
    // the network re-forms after the chaos, and must complete all the same.
    // Every node runs, and there are at least two: each try that draws two
    // ends starts one.
    let healed = sim.now;
    if split {
        sim.run_until_or(healed + HEAL_MS, ready);
    }
    while *left != (0, 0) {
        pay(sim, options, rng, left);
    }
    let mut stuck = unsettled(sim);
    while stuck.is_some() && sim.now < healed + HEAL_MS {
        sim.run_until(sim.now + LOOK_MS);
        stuck = unsettled(sim);
    }
    let seconds = (sim.now - healed) / 1_000;
    *sim.counts.entry("heal seconds").or_default() += seconds;
    for (from, outgoing) in reconnecting {
        let failed = sim.nodes[from]
            .store
            .outgoing_payment(outgoing)
            .unwrap()
            .is_some_and(|row| row.state == OutgoingState::Failed);
        if failed {
            sim.count("outgoing given up after the heal");
        }
    }
    let completed = sim
        .outgoing
        .iter()
        .filter(|&&(from, _, _, outgoing)| {
            sim.nodes[from]
                .store
                .outgoing_payment(outgoing)
                .unwrap()
                .is_some_and(|row| row.state == OutgoingState::Completed)
        })
        .count();
    if completed > 0 {
        let completed = u64::try_from(completed).unwrap_or(u64::MAX);
        sim.counts.insert("outgoing payments completed", completed);
    }
    count_ends(sim, ends);
    // An error a node reported ranks first, then what must not happen even
    // with money in flight. A network that never settled down is a liveness
    // failure, and its books are not judged: with money in flight they
    // rightly differ. Nor is a payment refused that should have been made:
    // nothing was lost, but something did not happen.
    check_events(sim).map_err(|problem| Outcome::safety("events", problem))?;
    check_mid_flight(sim)?;
    for end in ends {
        check_end(sim, end)?;
    }
    if let Some(what) = stuck {
        return Err(Outcome::liveness(
            "settle-down",
            format!(
                "healed, the network did not settle down in {} s: {what}",
                HEAL_MS / 1_000
            ),
        ));
    }
    check(sim)?;
    for end in ends.iter().filter(|end| !end.ban) {
        // A ban of the peer while the node collects tells it at once.
        let banned = ends
            .iter()
            .find(|ban| ban.ban && ban.mark > end.mark && (ban.by, ban.peer) == (end.by, end.peer))
            .map_or(sim.trace.len(), |ban| ban.mark);
        collected_before_telling(sim, end.mark, banned, end.by, end.peer)?;
    }
    match sim.refused.first() {
        Some(refused) => Err(Outcome::liveness("refused", refused.clone())),
        None => Ok(()),
    }
}

/// What is left to happen, if anything: a node whose scheme does not see
/// its ledger, or has not heard of all it applied; a peering being ended;
/// a node without a peer;
/// a payment that did not finish; a node that waits for a settlement or
/// still has one to send. The checks may judge the books only once nothing
/// is, when no packet or transaction is in flight and every scheme knows
/// its balance and reserve.
fn unsettled(sim: &Sim) -> Option<String> {
    let count = sim.nodes.len();
    for index in 0..count {
        for view in sim.core(index).ledger_views() {
            if view.status != LedgerStatus::Synced {
                return Some(format!(
                    "node {index}'s scheme for {} is {:?}",
                    view.id, view.status
                ));
            }
        }
    }
    // A settlement the ledger applied is booked when its node hears of it:
    // until then, the books and the account differ, as they should.
    if sim.ledger.queued() > 0 {
        return Some("transactions wait for the ledger to close".into());
    }
    // The ledger closes every few seconds and the nodes talk to it all
    // the time: what matters is a transaction on its way in or out.
    let in_flight = sim.queue.values().any(|delivery| match delivery {
        Delivery::LedgerRequest { message, .. } => names(message, "command", "submit"),
        Delivery::LedgerEvent {
            event: LedgerEvent::Message(message),
            ..
        } => names(message, "type", "transaction"),
        _ => false,
    });
    if in_flight {
        return Some("a transaction is on its way to or from the ledger".into());
    }
    // An end, the owner's or the node's own, takes a bounded time
    // (spec/decisions/ending-a-peering.md).
    for index in 0..count {
        let network = sim.core(index).network();
        let ending = network
            .peers
            .iter()
            .find(|peer| peer.termination.is_some() || peer.peering.depeering.is_some());
        if let Some(peer) = ending {
            return Some(format!(
                "node {index} is still ending its peering with {}",
                peer.id
            ));
        }
    }
    if count > 1
        && let Some(index) = (0..count).find(|i| sim.peers(*i).is_empty())
    {
        return Some(format!("node {index} has no peer"));
    }
    for &(from, to, _, outgoing) in &sim.outgoing {
        let state = sim.nodes[from]
            .store
            .outgoing_payment(outgoing)
            .unwrap()
            .map(|row| row.state);
        let given_up = state == Some(OutgoingState::Failed) && sim.given_up.contains(&outgoing);
        if state != Some(OutgoingState::Completed) && !given_up {
            return Some(format!(
                "the outgoing payment from node {from} to node {to} is {state:?}"
            ));
        }
    }
    check_finished(sim)
        .err()
        .or_else(|| check_settled(sim).err())
        .or_else(|| check_delivered(sim).err())
}

/// What each node did about an owner's end or ban, by its own books and
/// what it did (spec/decisions/ending-a-peering.md), whether or not the
/// network settled down: neither node settles with the other once it knows
/// the peering ends, until they peer again; a node that bans a peer tells it
/// at once, asks it for nothing, and never peers with it again; and neither
/// node is owed by the other past the credit it extends
/// (spec/decisions/own-fulfills.md). What the node ended collects, and a
/// settlement that lands after the end, credited once to the peer's account,
/// the books against the scheme and the owner's funds show ([`check`]).
fn check_end(sim: &Sim, end: &End) -> Result<(), Outcome> {
    let &End {
        mark,
        by,
        peer,
        ban,
    } = end;
    let decided = if ban { "Terminating" } else { "Depeering" };
    settled_nothing_once_ending(sim, mark, by, peer, decided)?;
    settled_nothing_once_ending(sim, mark, peer, by, "Terminating")?;
    if ban {
        banned(sim, mark, by, peer)?;
    }
    for (index, other) in [(by, peer), (peer, by)] {
        let id = key(other).node_id();
        let balances = sim.balances(index);
        let (Some(position), Some(trust)) = (
            balances.get(&format!("assets/interledger/{id}")),
            balances.get(&format!("contra/trust/{id}")),
        ) else {
            // A peering only the other side recorded, as when its
            // requester crashed before the acceptance arrived: this node
            // never opened the accounts, and extended nothing.
            continue;
        };
        let (credit, owed) = (-trust, -(position + trust));
        if owed > credit {
            return Err(Outcome::safety(
                "owed-past-credit",
                format!(
                    "node {other} owes node {index} {owed}, past the {credit} credit it extends"
                ),
            ));
        }
    }
    Ok(())
}

/// A ban collects nothing (spec/decisions/ending-a-peering.md): node `by`,
/// banning node `peer`, tells it the peering ends at once, unless it was
/// ending already, and asks it to settle no more. And it never peers with the
/// peer again: it neither asks it nor accepts it
/// (spec/areas/network.md#bans). The peer may still record a peering it asked
/// for before the ban, once the acceptance arrives, and is then told it ends.
fn banned(sim: &Sim, mark: usize, by: usize, peer: usize) -> Result<(), Outcome> {
    let lines = &sim.trace[mark..];
    if until_peered(lines, 0, by, peer) < lines.len() {
        return Err(Outcome::safety(
            "banned-peered-again",
            format!("node {by} peered with node {peer} again after banning it"),
        ));
    }
    let banned = trace_ms(&sim.trace[mark - 1]);
    let telling = format!(
        " {by} PeerNode(Terminating {{ peer: {:?}",
        key(peer).node_id()
    );
    if let Some(told) = lines.iter().find(|line| line.contains(&telling))
        && trace_ms(told) != banned
    {
        return Err(Outcome::safety(
            "banned-told-later",
            format!("node {by} banned node {peer}, and told it only later: {told}"),
        ));
    }
    let asking = format!(" {by} asks {peer} to settle");
    match lines.iter().find(|line| line.contains(&asking)) {
        Some(asked) => Err(Outcome::safety(
            "banned-asked-to-settle",
            format!("node {by} banned node {peer}, and asked it to settle: {asked}"),
        )),
        None => Ok(()),
    }
}

/// Counts what the owners' ends reached: an end whose last request found
/// a settlement on its way, and a settlement booked after the end, which
/// the books then credit to the peer's account, once
/// (spec/decisions/ending-a-peering.md).
fn count_ends(sim: &mut Sim, ends: &[End]) {
    let mut collected = 0;
    let mut late = 0;
    for &End {
        mark,
        by,
        peer,
        ban,
    } in ends
    {
        let lines = &sim.trace[mark..];
        let answered = format!(
            " {by} PeerNode(SettlementRequestAnswered {{ peer: {:?}, settling: Some(",
            key(peer).node_id()
        );
        let ending = &lines[..until_peered(lines, 0, by, peer)];
        let on_its_way = !ban
            && ending.iter().any(|line| {
                line.find(&answered)
                    .map(|at| &line[at + answered.len()..])
                    .and_then(|rest| rest[..rest.find(')')?].parse::<u64>().ok())
                    .is_some_and(|amount| amount > 0)
            });
        if on_its_way {
            collected += 1;
        }
        for (index, other) in [(by, peer), (peer, by)] {
            let id = format!("{:?}", key(other).node_id());
            let ended = format!(" {index} PeerNode(PeeringEnded {{ peer: {id}");
            let booked = format!(" {index} Settlement(SettlementBooked {{ from: {id}");
            let Some(start) = lines.iter().position(|line| line.contains(&ended)) else {
                continue;
            };
            let after = &lines[start..until_peered(lines, start, index, other)];
            if after.iter().any(|line| line.contains(&booked)) {
                late += 1;
            }
        }
    }
    for (what, count) in [
        ("ends that collected a settlement on its way", collected),
        ("settlements booked after the end", late),
    ] {
        if count > 0 {
            *sim.counts.entry(what).or_default() += count;
        }
    }
}

/// Whether the payer of node `from` reconnected the outgoing payment
/// `outgoing` (as the trace prints it) since the node last started: each
/// start loads the payment with all its reconnects again.
fn reconnected_since_start(trace: &[String], from: usize, outgoing: &str) -> bool {
    let started = format!("start {from}");
    let since = trace
        .iter()
        .rposition(|line| {
            line.split_once(' ')
                .is_some_and(|(_, what)| what == started)
        })
        .unwrap_or(0);
    let reconnects = format!(" {from} Payment(OutgoingReconnects {{ id: {outgoing}");
    trace[since..].iter().any(|line| line.contains(&reconnects))
}

/// Whether a JSON message to or from the ledger has `key` set to `value`.
fn names(message: &[u8], key: &str, value: &str) -> bool {
    serde_json::from_slice::<serde_json::Value>(message)
        .ok()
        .and_then(|json| json.get(key)?.as_str().map(|found| found == value))
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_reconnects_since_the_payers_last_start_count() {
        let reconnect = |at: u64, node: usize, id: &str| {
            format!("{at} {node} Payment(OutgoingReconnects {{ id: {id}, reason: \"closed\" }})")
        };
        let mut trace = vec![
            "1 start 1".to_owned(),
            "2 start 11".to_owned(),
            reconnect(3, 1, "PaymentId([7])"),
        ];
        assert!(reconnected_since_start(&trace, 1, "PaymentId([7])"));
        assert!(!reconnected_since_start(&trace, 1, "PaymentId([8])"));
        assert!(!reconnected_since_start(&trace, 11, "PaymentId([7])"));
        // A restart, by the chaos or by the heal, gives the payer all its
        // reconnects again.
        trace.push("4 start 1".to_owned());
        assert!(!reconnected_since_start(&trace, 1, "PaymentId([7])"));
        trace.push(reconnect(5, 1, "PaymentId([7])"));
        assert!(reconnected_since_start(&trace, 1, "PaymentId([7])"));
    }
}
