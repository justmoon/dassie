//! The node simulation's random run
//! (spec/decisions/random-seed-simulations.md): `dassie_node::sim::swarm`
//! draws each seed's network, faults and payments, runs its chaos, heals
//! it, and checks it. The campaign runs it from random seeds, outside CI:
//! `dassie-sim run node <seed>` replays a seed, `dassie-sim explain node
//! <seed>` prints its trace, and `dassie-sim run node <from>..<to>` runs a
//! range and counts what the runs reached between them.

use dassie_kit::sim::each;
use dassie_node::sim::swarm;

/// A seed passes, and run twice at once on two threads makes the same run:
/// nothing a run does reaches another, or depends on its thread. A seed
/// whose run takes a fraction of a second, on the XRP Ledger.
#[test]
fn a_random_seed_replays_the_same_run() {
    let runs = each(&[0, 0], swarm::run);
    assert!(!runs[0].outcome.is_failure(), "{}", runs[0].outcome);
    assert_eq!(runs[0].trace, runs[1].trace);
}
