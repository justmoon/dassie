//! Deterministic simulation of a hub's consensus
//! (spec/areas/testing.md#deterministic-simulation,
//! spec/decisions/tigerstyle.md), on fixed seeds: `dassie_hub::sim::chaos`
//! is the run. A failure names its seed; `dassie-sim run hub <seed>` replays
//! it, `dassie-sim explain hub <seed>` prints its trace, and `dassie-sim run
//! hub <from>..<to>` runs others (spec/decisions/random-seed-simulations.md).

use dassie_hub::sim::chaos;
use dassie_node_core::testing::assert_seeds_pass;

#[test]
fn hubs_stay_safe_and_finalize_after_chaos() {
    assert_seeds_pass(0..8, chaos::run);
}

#[test]
fn a_seed_replays_the_same_run() {
    assert_eq!(chaos::run(5).trace, chaos::run(5).trace);
}
