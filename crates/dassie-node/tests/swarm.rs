//! The node simulation's random run (D27) on fixed seeds:
//! `dassie_node::sim::swarm` draws each seed's network, faults and
//! payments, runs its chaos, heals it, and checks it. A failure names its
//! seed: `dassie-sim run node <seed>` replays it, `dassie-sim explain node
//! <seed>` prints its trace, and `dassie-sim run node <from>..<to>` runs
//! others.
//!
//! The same seeds must reach every kind of fault and payment the run
//! draws, so that a change to the generator that stops reaching one fails
//! here rather than going quiet.

use std::collections::BTreeMap;
use std::ops::Range;

use dassie_node::sim::swarm;
use dassie_node_core::testing::caught;

/// The seeds CI runs.
const SEEDS: Range<u64> = 0..16;

/// What the seeds must reach between them, at least.
const LEAST: [(&str, u64); 14] = [
    ("payments", 10),
    ("capped payments", 1),
    ("outgoing payments", 1),
    ("outgoing payments completed", 1),
    ("settlements taken", 50),
    ("crashes", 5),
    ("crashes mid-payment", 1),
    ("restarts", 1),
    ("cuts", 5),
    ("ledger drops", 1),
    ("replays", 20),
    ("replays refused", 20),
    ("clocks off", 10),
    ("heal seconds", 1),
];

#[test]
fn random_networks_settle_down_after_chaos_and_reach_every_fault() {
    let mut reached: BTreeMap<&str, u64> = BTreeMap::new();
    for seed in SEEDS {
        let run = caught(|| swarm::run(seed));
        assert!(!run.outcome.is_failure(), "seed {seed}: {}", run.outcome);
        for (what, count) in run.counts {
            *reached.entry(what).or_default() += count;
        }
    }
    for (what, least) in LEAST {
        let count = reached.get(what).copied().unwrap_or(0);
        assert!(
            count >= least,
            "seeds {SEEDS:?} reached {what} {count} times, not {least}: {reached:?}"
        );
    }
}

#[test]
fn a_random_seed_replays_the_same_run() {
    assert_eq!(swarm::run(3).trace, swarm::run(3).trace);
}
