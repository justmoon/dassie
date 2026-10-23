//! The node simulation's random run
//! (spec/decisions/random-seed-simulations.md) on fixed seeds:
//! `dassie_node::sim::swarm` draws each seed's network, faults and payments,
//! runs its chaos, heals it, and checks it. A failure names its seed:
//! `dassie-sim run node <seed>` replays it, `dassie-sim explain node <seed>`
//! prints its trace, and `dassie-sim run node <from>..<to>` runs others.
//!
//! The same seeds must reach every kind of fault and payment the run
//! draws, so that a change to the generator that stops reaching one fails
//! here rather than going quiet.
//!
//! The seeds run at once, as many at a time as the machine runs threads,
//! so that the test takes about as long as its slowest seed rather than
//! as long as all of them together.

use std::collections::BTreeMap;
use std::num::NonZero;
use std::ops::Range;
use std::panic::resume_unwind;
use std::sync::{Mutex, PoisonError};
use std::thread;

use dassie_node::sim::swarm;
use dassie_node_core::testing::{Run, caught};

/// The seeds CI runs.
const SEEDS: Range<u64> = 0..16;

/// What the seeds must reach between them, at least. A settlement booked
/// after the end of its peering is too rare in the drawn runs, a seed in
/// forty or so, for a fixed handful to be sure to reach it:
/// `a_settlement_that_lands_after_the_end_is_credited_once` in
/// `tests/network.rs` reaches it every time.
const LEAST: [(&str, u64); 24] = [
    ("payments", 10),
    ("capped payments", 1),
    ("outgoing payments", 1),
    ("outgoing payments completed", 1),
    ("settlements taken", 50),
    ("crashes", 5),
    ("crashes mid-payment", 1),
    ("fulfills lost to crashes", 5),
    ("restarts", 1),
    ("cuts", 5),
    ("ledger drops", 1),
    ("failed commits", 10),
    ("failed commits mid-payment", 2),
    ("failed commits mid-settlement", 2),
    ("peerings ended", 10),
    ("bans", 10),
    ("ends that collected a settlement on its way", 5),
    ("replays", 20),
    ("replays refused", 20),
    ("clocks off", 10),
    ("heal seconds", 1),
    ("partitions", 5),
    ("partitions longer than the silence that ends a peering", 1),
    ("peerings ended for silence", 5),
];

#[test]
fn random_networks_settle_down_after_chaos_and_reach_every_fault() {
    let seeds: Vec<u64> = SEEDS.collect();
    // A run's trace is dropped as soon as it ends: nothing here reads it,
    // and every seed's would otherwise be kept until the slowest ends.
    let runs = each(&seeds, |seed| {
        let mut run = caught(|| swarm::run(seed));
        run.trace = Vec::new();
        run
    });
    // Every seed has run by now, so each one that failed is named.
    let mut failed = Vec::new();
    let mut reached: BTreeMap<&str, u64> = BTreeMap::new();
    for (seed, run) in seeds.iter().zip(runs) {
        if run.outcome.is_failure() {
            failed.push(format!("seed {seed}: {}", run.outcome));
        }
        for (what, count) in run.counts {
            *reached.entry(what).or_default() += count;
        }
    }
    assert!(failed.is_empty(), "{}", failed.join("\n"));
    for (what, least) in LEAST {
        let count = reached.get(what).copied().unwrap_or(0);
        assert!(
            count >= least,
            "seeds {SEEDS:?} reached {what} {count} times, not {least}: {reached:?}"
        );
    }
}

/// The same seed, run twice at once on two threads, makes the same run:
/// nothing a run does reaches another, or depends on its thread.
#[test]
fn a_random_seed_replays_the_same_run() {
    let runs = each(&[3, 3], swarm::run);
    assert_eq!(runs[0].trace, runs[1].trace);
}

/// Each seed's run, in the order of `seeds`, made at once on as many
/// threads as the machine runs, and never more than there are seeds. Each
/// thread takes the next seed left until none is. A run is driven from the
/// one thread that makes it and draws everything from its seed, so it is
/// the run it would be alone; which thread makes it, and when, changes
/// nothing. A panic that `run` does not catch fails the test, as it would
/// on the test's own thread.
fn each(seeds: &[u64], run: impl Fn(u64) -> Run + Sync) -> Vec<Run> {
    let most = thread::available_parallelism().map_or(1, NonZero::get);
    let threads = most.min(seeds.len());
    let left = Mutex::new(seeds.iter().copied().enumerate());
    // The lock is held only to take a seed, never while running one.
    let next = || left.lock().unwrap_or_else(PoisonError::into_inner).next();
    let mut runs: Vec<(usize, Run)> = thread::scope(|scope| {
        let workers: Vec<_> = (0..threads)
            .map(|_| {
                scope.spawn(|| {
                    let mut runs = Vec::new();
                    while let Some((index, seed)) = next() {
                        runs.push((index, run(seed)));
                    }
                    runs
                })
            })
            .collect();
        workers
            .into_iter()
            .flat_map(|worker| worker.join().unwrap_or_else(|panic| resume_unwind(panic)))
            .collect()
    });
    runs.sort_by_key(|(index, _)| *index);
    assert!(
        runs.iter().map(|(index, _)| *index).eq(0..seeds.len()),
        "each seed runs once"
    );
    runs.into_iter().map(|(_, run)| run).collect()
}
