//! Deterministic simulation of peer networks (spec 7 §7, D16), on fixed
//! seeds: `dassie_peer::sim::chaos` is the run. A failure names its seed;
//! `dassie-sim run peer <seed>` replays it, `dassie-sim explain peer <seed>`
//! prints its trace, and `dassie-sim run peer <from>..<to>` runs others
//! (D27).

use dassie_node_core::testing::assert_seeds_pass;
use dassie_peer::sim::chaos;

#[test]
fn networks_converge_after_chaos() {
    assert_seeds_pass(0..12, chaos::run);
}

#[test]
fn a_seed_replays_the_same_run() {
    assert_eq!(chaos::run(3).trace, chaos::run(3).trace);
}
