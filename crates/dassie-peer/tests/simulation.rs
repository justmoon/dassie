//! Deterministic simulation of peer networks
//! (spec/areas/testing.md#deterministic-simulation,
//! spec/decisions/tigerstyle.md): `dassie_peer::sim::chaos` is the run, and
//! `dassie_peer::sim::names` the names' run (spec/areas/testing.md#names).
//! The campaign runs both from random seeds, outside CI: `dassie-sim run
//! peer <seed>` replays a seed, `dassie-sim explain peer <seed>` prints its
//! trace, and `dassie-sim run peer <from>..<to>` runs others, as `names`
//! does for the names' run (spec/decisions/random-seed-simulations.md).

use dassie_kit::sim::each;
use dassie_peer::sim::chaos;

/// A seed passes, and run twice at once on two threads makes the same run.
/// A seed of three nodes, whose run takes a fraction of a second.
#[test]
fn a_seed_replays_the_same_run() {
    let runs = each(&[14, 14], chaos::run);
    assert!(!runs[0].outcome.is_failure(), "{}", runs[0].outcome);
    assert_eq!(runs[0].trace, runs[1].trace);
}
