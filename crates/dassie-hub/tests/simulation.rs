//! Deterministic simulation of a hub's consensus and its ledger
//! (spec/areas/testing.md#deterministic-simulation,
//! spec/decisions/tigerstyle.md): `dassie_hub::sim::chaos` is the run. The
//! campaign runs it from random seeds, outside CI: `dassie-sim run hub
//! <seed>` replays a seed, `dassie-sim explain hub <seed>` prints its trace,
//! and `dassie-sim run hub <from>..<to>` runs others
//! (spec/decisions/random-seed-simulations.md).

use dassie_hub::sim::chaos;

#[test]
fn a_seed_replays_the_same_run() {
    let run = chaos::run(5);
    assert!(!run.outcome.is_failure(), "{}", run.outcome);
    assert_eq!(run.trace, chaos::run(5).trace);
}

// spec/decisions/token-hubs.md: a seed whose token hub freezes and thaws,
// takes a transfer fee as a freeze, holds a reduction over a freeze, takes
// claims within a freeze's window, shuts down for an absence counted by
// its clock, is clawed back, and pays out pro rata, every oracle passing.
#[test]
fn a_token_hub_freezes_thaws_and_is_clawed_back() {
    let run = chaos::run(360);
    assert!(!run.outcome.is_failure(), "{}", run.outcome);
    for count in [
        "freeze",
        "thaw",
        "fee",
        "window-claim",
        "reduction-after-thaw",
        "absence-shutdown",
        "clawback",
        "pro-rata",
    ] {
        assert!(run.counts.contains_key(count), "{count}: {:?}", run.counts);
    }
}
