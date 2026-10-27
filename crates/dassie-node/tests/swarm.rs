//! The node simulation's random run
//! (spec/decisions/random-seed-simulations.md): `dassie_node::sim::swarm`
//! draws each seed's network, faults and payments, runs its chaos, heals
//! it, and checks it. The campaign runs it from random seeds, outside CI:
//! `dassie-sim run node <seed>` replays a seed, `dassie-sim explain node
//! <seed>` prints its trace, and `dassie-sim run node <from>..<to>` runs a
//! range and counts what the runs reached between them.

use dassie_kit::sim::{Outcome, caught, each};
use dassie_node::sim::swarm;

/// The seed of a run over a hub the tests run: one of the shortest in wall
/// time, two members of a hub of 4 validators, through cuts, a failed
/// commit, forgeries and dropped connections to the fake XRP Ledger.
const HUB_SEED: u64 = 33;

/// A seed passes, and run twice at once on two threads makes the same run:
/// nothing a run does reaches another, or depends on its thread. A seed
/// whose run takes a fraction of a second: three nodes on the stub with
/// commitments, through crashes, cuts, forgeries and a partition.
#[test]
fn a_random_seed_replays_the_same_run() {
    let runs = each(&[10, 10], swarm::run);
    assert!(!runs[0].outcome.is_failure(), "{}", runs[0].outcome);
    assert_eq!(runs[0].trace, runs[1].trace);
}

// spec/areas/testing.md#the-simulations: a seed that draws a run over a
// hub run in the simulation passes, its members settling over the hub
// through the chaos, and its books are judged as the runs over a hub judge
// them.
#[test]
fn a_random_seed_runs_over_a_hub() {
    let run = caught(|| swarm::run(HUB_SEED));
    assert_eq!(
        run.outcome,
        Outcome::Pass,
        "seed {HUB_SEED}: {}",
        run.outcome
    );
    for reached in [
        "runs over a hub",
        "prepares over a hub",
        "settlements taken",
        "ledger drops",
    ] {
        assert!(
            run.counts.get(reached).is_some_and(|count| *count > 0),
            "seed {HUB_SEED} reached no {reached}: {:?}",
            run.counts
        );
    }
}
