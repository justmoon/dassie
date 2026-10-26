//! A deterministic simulation of the connector
//! (spec/areas/testing.md#the-simulations), on fixed seeds:
//! `dassie_connector::sim::traffic` is the run. A failure names its
//! seed; `dassie-sim run connector <seed>` replays it, and
//! `dassie-sim run connector <from>..<to>` runs others
//! (spec/decisions/random-seed-simulations.md).

use dassie_connector::sim::traffic;
use dassie_kit::sim::assert_seeds_pass;

#[test]
fn random_traffic_keeps_the_connector_sound() {
    assert_seeds_pass(0..24, traffic::run);
}

#[test]
fn the_runs_exercise_every_outcome() {
    let (_, harness) = traffic::finished(1);
    let stats = harness.connector.stats();
    for (name, count) in [
        ("forwarded", stats.forwarded),
        ("answered", stats.answered),
        ("fulfilled", stats.fulfilled),
        ("rejected", stats.rejected),
        ("expired", stats.expired),
        ("wrong fulfillments", stats.wrong_fulfillments),
        ("unexpected responses", stats.unexpected_responses),
        ("duplicates", stats.duplicates),
        ("refused by the ledger", harness.ledger.stats().refused),
    ] {
        assert!(count > 0, "no {name}: {stats:?}");
    }
}

#[test]
fn a_seed_replays_the_same_run() {
    assert_eq!(traffic::run(7).trace, traffic::run(7).trace);
    assert_ne!(traffic::run(7).trace, traffic::run(8).trace);
}
