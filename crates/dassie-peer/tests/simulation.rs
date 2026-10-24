//! Deterministic simulation of peer networks
//! (spec/areas/testing.md#deterministic-simulation,
//! spec/decisions/tigerstyle.md), on fixed seeds: `dassie_peer::sim::chaos`
//! is the run, and `dassie_peer::sim::names` the names' run
//! (spec/areas/testing.md#names). A failure names its seed; `dassie-sim run
//! peer <seed>` replays it, `dassie-sim explain peer <seed>` prints its
//! trace, and `dassie-sim run peer <from>..<to>` runs others, as `names`
//! does for the names' run (spec/decisions/random-seed-simulations.md).

#![allow(clippy::unwrap_used, clippy::panic)]

use std::collections::BTreeMap;
use std::num::NonZero;
use std::panic::resume_unwind;
use std::sync::{Mutex, PoisonError};
use std::thread;

use dassie_node_core::testing::{Run, assert_seeds_pass, caught};
use dassie_peer::sim::{chaos, names};

#[test]
fn networks_converge_after_chaos() {
    // Each seed simulates a day of calm at production timings, about 6 s of
    // wall time: CI runs a few, and `dassie-sim` the others.
    assert_seeds_pass(0..3, chaos::run);
}

#[test]
fn a_seed_replays_the_same_run() {
    assert_eq!(chaos::run(3).trace, chaos::run(3).trace);
}

/// The names' seeds CI runs: between them, every kind of registrar, each
/// seed some seven hours at production timings and about 10 s of wall time.
const NAMES_SEEDS: [u64; 8] = [0, 1, 2, 3, 4, 5, 6, 7];

/// What the names' seeds must reach between them, at least. Releases with
/// every registrar honest count apart, as only those runs check that no
/// split is left.
const NAMES_LEAST: [(&str, u64); 16] = [
    ("runs with honest registrars", 1),
    ("equivocating registrars", 1),
    ("registrars withholding receipts", 1),
    ("registrars tipping off", 1),
    ("names granted", 40),
    ("registrations that must hold", 35),
    ("names contested", 8),
    ("names released", 4),
    ("names released with honest registrars", 1),
    ("names kept by a registrar cut off", 30),
    ("views frozen while holding names", 15),
    ("restarts holding names", 100),
    ("names tipped off", 10),
    ("names front-run", 4),
    ("registrations abandoned", 20),
    ("crashes", 20),
];

// spec/designs/names.md#1-what-it-guarantees: names registered through
// chaos with one registrar of four perhaps faulty, then granted, read and
// released through chaos again, never have two holders, are never
// front-run, are never released while a registration holds them, and are
// held once calm, or, with a registrar that withholds receipts, never
// registered. The seeds must reach every kind of
// registrar and every case, so that a change to the generator that stops
// reaching one fails here rather than going quiet. They run at once, as
// many at a time as the machine runs threads.
#[test]
fn names_hold_through_chaos() {
    let runs = each(&NAMES_SEEDS, |seed| {
        let mut run = caught(|| names::run(seed));
        run.trace = Vec::new();
        run
    });
    let mut failed = Vec::new();
    let mut reached: BTreeMap<&str, u64> = BTreeMap::new();
    for (seed, run) in NAMES_SEEDS.iter().zip(runs) {
        if run.outcome.is_failure() {
            failed.push(format!("seed {seed}: {}", run.outcome));
        }
        for (what, count) in run.counts {
            *reached.entry(what).or_default() += count;
        }
    }
    assert!(failed.is_empty(), "{}", failed.join("\n"));
    for (what, least) in NAMES_LEAST {
        let count = reached.get(what).copied().unwrap_or(0);
        assert!(
            count >= least,
            "seeds {NAMES_SEEDS:?} reached {what} {count} times, not {least}: {reached:?}"
        );
    }
}

/// The same names' seed, run twice at once on two threads, makes the same
/// run.
#[test]
fn a_names_seed_replays_the_same_run() {
    let runs = each(&[5, 5], names::run);
    assert_eq!(runs[0].trace, runs[1].trace);
}

/// Runs every seed, as many at once as the machine runs threads, and gives
/// their runs in the seeds' order.
fn each(seeds: &[u64], run: impl Fn(u64) -> Run + Sync) -> Vec<Run> {
    let most = thread::available_parallelism().map_or(1, NonZero::get);
    let threads = most.min(seeds.len());
    let left = Mutex::new(seeds.iter().copied().enumerate());
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
    runs.into_iter().map(|(_, run)| run).collect()
}
