//! Deterministic simulation of a hub's consensus
//! (spec/areas/testing.md#deterministic-simulation,
//! spec/decisions/tigerstyle.md): `dassie_hub::sim::chaos` is the run. The
//! campaign runs it from random seeds, outside CI: `dassie-sim run hub
//! <seed>` replays a seed, `dassie-sim explain hub <seed>` prints its trace,
//! and `dassie-sim run hub <from>..<to>` runs others
//! (spec/decisions/random-seed-simulations.md).

use dassie_hub::sim::chaos;

#[test]
fn a_seed_replays_the_same_run() {
    assert_eq!(chaos::run(5).trace, chaos::run(5).trace);
}
