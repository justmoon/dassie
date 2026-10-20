//! The node simulation's random run (D27): every seed draws its own
//! network, faults and payments, as swarm testing does, so that runs
//! differ in kind and not only in detail. Some seeds switch a fault off
//! entirely, so that rare combinations get their turn.
//!
//! A run has two phases. In the chaos, payments start while nodes crash
//! or fail a commit and restart, links are cut and healed, messages are
//! lost, requests are replayed and ledger connections drop; each node's
//! clock is off the
//! network's by up to 10 s, throughout. Then the network heals: every node
//! runs, no link is cut, nothing is lost or replayed, and within
//! [`HEAL_MS`] the network must settle down. Then each node is checked by
//! its own books (D19), as the scripted runs are.

use std::fmt;

use dassie_connector::SpendingCap;
use dassie_node_core::testing::{Outcome, Run, SeededRng};
use dassie_settlement::LedgerStatus;

use super::checks::{
    check, check_delivered, check_events, check_finished, check_mid_flight, check_settled,
};
use super::scripted::{READY_WITHIN_MS, ended, ready};
use super::{Delivery, START, Settling, Sim};
use crate::engine::LedgerEvent;
use crate::node::Policies;
use crate::store::{Aim, OutgoingState};

/// How long a healed network has to settle down, from the production rules
/// it runs: as long as a network takes to be ready, plus the longest a
/// settlement that failed during the chaos waits before it is sent again
/// (its backoff grows to an hour). A run past it is stuck, not slow. It
/// follows those rules, and is never loosened to let a seed pass.
pub const HEAL_MS: u64 = READY_WITHIN_MS + Policies::DEFAULT.peer.settlement_retry_max_ms;

/// How often a healing network is looked at.
const LOOK_MS: u64 = 5_000;

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
    /// Making one of a node's next commits fail (D27).
    pub fail_commit: u64,
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
}

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
            idle: rng.below(11),
        };
        let payments = (1 + rng.below(8), rng.below(3));
        let largest_payment = LEAST_PAYMENT + rng.below(3_300_000_001);
        let replay_per_mille = switched(&mut rng, 51);
        let skew_ms = switched(&mut rng, MOST_SKEW_MS + 1);
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
             fail commit {} idle {}; \
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
            w.idle,
            self.payments.0,
            self.payments.1,
            self.largest_payment,
        )
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
    chaos(&mut sim, &options, &mut rng, &mut left);
    let result = heal(&mut sim, &options, &mut rng, &mut left);
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
    Idle,
}

fn chaos(sim: &mut Sim, options: &Options, rng: &mut SeededRng, left: &mut (u64, u64)) {
    let end = sim.now + options.chaos_ms;
    let mut at = sim.now;
    while at < end {
        at += 1 + rng.below(2 * options.step_ms);
        sim.run_until(at);
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
            Act::Idle => {}
        }
    }
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

/// Heals the network, starts what payments are left, waits for it to
/// settle down, and checks it.
fn heal(
    sim: &mut Sim,
    options: &Options,
    rng: &mut SeededRng,
    left: &mut (u64, u64),
) -> Result<(), Outcome> {
    sim.let_commits_through();
    for index in 0..sim.nodes.len() {
        if !up(sim, index) {
            sim.start(index);
        }
    }
    sim.cut.clear();
    sim.loss_per_mille = 0;
    sim.replay_per_mille = 0;
    sim.trace.push(format!("{} heal", sim.now));
    // A payer gives up after its reconnects (a few minutes), and the chaos
    // may have cut it off that long, or the network not yet formed: what
    // failed before the heal may stay failed. After it, nothing may fail.
    for &(from, _, _, outgoing) in &sim.outgoing.clone() {
        let failed = sim.nodes[from]
            .store
            .outgoing_payment(outgoing)
            .unwrap()
            .is_some_and(|row| row.state == OutgoingState::Failed);
        if failed {
            sim.given_up.insert(outgoing);
            sim.count("outgoing given up in the chaos");
        }
    }
    // Every node runs, and there are at least two: each try that draws
    // two ends starts one.
    while *left != (0, 0) {
        pay(sim, options, rng, left);
    }
    let healed = sim.now;
    let mut stuck = unsettled(sim);
    while stuck.is_some() && sim.now < healed + HEAL_MS {
        sim.run_until(sim.now + LOOK_MS);
        stuck = unsettled(sim);
    }
    let seconds = (sim.now - healed) / 1_000;
    *sim.counts.entry("heal seconds").or_default() += seconds;
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
    // An error a node reported ranks first, then what must not happen even
    // with money in flight. A network that never settled down is a liveness
    // failure, and its books are not judged: with money in flight they
    // rightly differ. Nor is a payment refused that should have been made:
    // nothing was lost, but something did not happen.
    check_events(sim).map_err(|problem| Outcome::safety("events", problem))?;
    check_mid_flight(sim)?;
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
    match sim.refused.first() {
        Some(refused) => Err(Outcome::liveness("refused", refused.clone())),
        None => Ok(()),
    }
}

/// What is left to happen, if anything: a node whose scheme does not see
/// its ledger, or has not heard of all it applied; a node without a peer;
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

/// Whether a JSON message to or from the ledger has `key` set to `value`.
fn names(message: &[u8], key: &str, value: &str) -> bool {
    serde_json::from_slice::<serde_json::Value>(message)
        .ok()
        .and_then(|json| json.get(key)?.as_str().map(|found| found == value))
        .unwrap_or(false)
}
