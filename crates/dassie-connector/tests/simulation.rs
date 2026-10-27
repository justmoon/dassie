//! A deterministic simulation of the connector
//! (spec/areas/testing.md#the-simulations): `dassie_connector::sim::traffic`
//! is the run. The campaign runs it from random seeds, outside CI:
//! `dassie-sim run connector <seed>` replays a seed, and `dassie-sim run
//! connector <from>..<to>` runs others
//! (spec/decisions/random-seed-simulations.md).

use dassie_connector::sim::traffic;

#[test]
fn the_runs_exercise_every_outcome() {
    let (trace, harness) = traffic::finished(1);
    // Peers' packets kept their ledger, and converted where no hop could.
    let last = trace.last().unwrap();
    let counts: Vec<u64> = last
        .split(' ')
        .filter_map(|word| word.parse().ok())
        .collect();
    assert!(last.starts_with("kept") && counts.len() == 2, "{last}");
    assert!(counts.iter().all(|count| *count > 0), "{last}");

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
    let (run, other) = (traffic::run(7), traffic::run(8));
    assert!(!run.outcome.is_failure(), "{}", run.outcome);
    assert!(!other.outcome.is_failure(), "{}", other.outcome);
    assert_eq!(run.trace, traffic::run(7).trace);
    assert_ne!(run.trace, other.trace);
}
