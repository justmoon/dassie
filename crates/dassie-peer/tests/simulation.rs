//! Deterministic simulation of peer networks
//! (spec/areas/testing.md#deterministic-simulation, D16), on fixed seeds:
//! `dassie_peer::sim::chaos` is the run. A failure names its seed;
//! `dassie-sim run peer <seed>` replays it, `dassie-sim explain peer <seed>`
//! prints its trace, and `dassie-sim run peer <from>..<to>` runs others
//! (D27).

use dassie_node_core::testing::assert_seeds_pass;
use dassie_peer::sim::chaos;

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
