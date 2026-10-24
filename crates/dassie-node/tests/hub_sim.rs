//! The node simulation with a settlement hub run in it
//! (`dassie_node::sim::hub`, spec/areas/testing.md#over-a-settlement-hub):
//! its validators, each the node's own validator role without its IO, found
//! the hub on the fake XRP Ledger, and nodes join it by deposit.
//!
//! The `node-hub` run (`dassie_node::sim::hubbed`) on fixed seeds: each
//! draws its hub (4 to 7 validators, up to f of them faulty), its members,
//! a lying member and a validator's restart, pays across hops over the hub,
//! then has the hub stall and its escrow pay out, or f + 1 validators
//! collude. A failure names its seed: `dassie-sim run node-hub <seed>`
//! replays it, `dassie-sim explain node-hub <seed>` prints its trace. The
//! seeds must reach every fault and lie the run draws, and the payout, so
//! that a change to the generator that stops reaching one fails here rather
//! than going quiet. They run at once, as many at a time as the machine
//! runs threads.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use std::collections::BTreeMap;
use std::num::NonZero;
use std::panic::resume_unwind;
use std::sync::{Mutex, PoisonError};
use std::thread;

use dassie_node::sim::{START, Settling, Sim, hubbed, key};
use dassie_node_core::testing::{Run, caught};

/// The seeds CI runs: between them, every validator fault and member lie
/// the run draws, restarts, hubs of 4, 5 and 7, and a collusion.
const SEEDS: [u64; 10] = [8, 13, 16, 27, 30, 42, 46, 47, 56, 62];

/// What the seeds must reach between them, at least.
const LEAST: [(&str, u64); 16] = [
    ("prepares over a hub", 100),
    ("settlements taken", 50),
    ("losses booked over a hub", 1),
    ("validator crashes", 10),
    ("states fetched", 1),
    ("equivocations", 1),
    ("invalid proposals", 1),
    ("claims made twice", 1),
    ("claims held back", 1),
    ("fulfills withheld", 1),
    ("hubs stalled", 8),
    ("payout sets signed", 24),
    ("payout sets gathered", 8),
    ("payouts checked", 8),
    ("evidence found", 1),
    ("forks with evidence", 1),
];

#[test]
fn members_pay_over_a_hub_and_its_escrow_pays_them_out() {
    let runs = each(&SEEDS, |seed| {
        let mut run = caught(|| hubbed::run(seed));
        run.trace = Vec::new();
        run
    });
    let mut failed = Vec::new();
    let mut reached: BTreeMap<&str, u64> = BTreeMap::new();
    for (seed, run) in SEEDS.iter().zip(runs) {
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

/// The same seed, run twice at once on two threads, makes the same run.
#[test]
fn a_seed_over_a_hub_replays_the_same_run() {
    let runs = each(&[56, 56], hubbed::run);
    assert_eq!(runs[0].trace, runs[1].trace);
}

// spec/areas/hubs.md#in-the-node: a node opens its account in the hub by a
// deposit from its XRP Ledger testnet account, which the validators see on
// the escrow and credit once a quorum attested it.
#[test]
fn nodes_join_a_hub_by_deposit() {
    let mut sim = Sim::with(1, 3, 0, BTreeMap::new(), Settling::Hub(4));
    sim.loss_per_mille = 0;
    assert!(
        sim.run_until_or(START + 600_000, hubbed::joined),
        "the nodes did not join the hub"
    );
    let hub = sim.hub.as_deref().unwrap();
    let ledger = hub.ledger().unwrap();
    assert_eq!(ledger.members(), 3);
    assert!(ledger.escrow().checked);
    assert_eq!(
        sim.ledger.balance(hub.escrow()),
        Some(hub.escrow_funding + ledger.supply()),
        "the escrow holds what the hub owes and what the sponsor left"
    );
}

// spec/designs/settlement-hubs.md#4-peering-over-a-hub: each way of a
// peering over a hub has a hold of its own, and a payer can send only what
// its hold has room for, whatever the payee owes it the other way. So the
// payee asks it to settle once that room runs low, even while its books,
// which net both ways, show room enough. Two members paying each other at
// once through a third, each of its holds drawn on both ways: both
// payments complete.
#[test]
fn payments_both_ways_through_a_hop_over_a_hub_complete() {
    let mut sim = Sim::with(3, 3, 0, BTreeMap::new(), Settling::Hub(4));
    sim.loss_per_mille = 0;
    assert!(
        sim.run_until_or(START + 600_000, hubbed::joined),
        "the members did not join the hub"
    );
    sim.step(0, |core| core.ban_node(key(2).node_id()).unwrap());
    sim.step(2, |core| core.ban_node(key(0).node_id()).unwrap());
    sim.join_network(1);
    let routed =
        |sim: &Sim| sim.peers(1).len() == 2 && !sim.peers(0).is_empty() && !sim.peers(2).is_empty();
    assert!(
        sim.run_until_or(sim.now + 600_000, routed),
        "the members did not peer"
    );
    sim.run_until(sim.now + 60_000);
    sim.pay(0, 2, 3_000_000_000, None, None);
    sim.pay(2, 0, 3_000_000_000, None, None);
    let paid = |sim: &Sim| sim.payments.iter().all(|payment| payment.done);
    assert!(
        sim.run_until_or(sim.now + 600_000, paid),
        "a payment stopped: {:?}",
        sim.payments
            .iter()
            .map(|payment| (payment.from, payment.to, dassie_node::sim::sent(payment)))
            .collect::<Vec<_>>()
    );
}

/// Each seed's run, in the order of `seeds`, made at once on as many
/// threads as the machine runs, and never more than there are seeds
/// (`tests/swarm.rs` says why that changes no run).
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
