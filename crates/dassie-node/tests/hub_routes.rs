//! The node simulation's `node-hub-routes` run
//! (`dassie_node::sim::hub_routes`, spec/areas/testing.md#over-a-settlement-hub):
//! five nodes in an XRP hub and a DSUSD hub, one of them with two first hops
//! to another, drawn per packet by the fees each earned it. The campaign
//! runs it from random seeds, outside CI: `dassie-sim run node-hub-routes
//! <seed>` replays a seed, `dassie-sim explain node-hub-routes <seed>`
//! prints its trace. This test runs it on a fixed seed.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use dassie_kit::sim::caught;
use dassie_node::sim::hub_routes;

/// The seed of `node-hub-routes` the test runs.
const SEED: u64 = 2;

// spec/decisions/settlement-methods.md, spec/decisions/weighted-next-hops.md:
// payments whose two ends share a ledger cross nodes in both hubs without
// converting; a node's weights for its next hops never exceed the fees it
// earned; an attacker's packets that its receiver refuses leave the weights
// as decay makes them; a hop that refuses is drawn and earns nothing while
// the payment completes through the other; and a hop short of room answers
// T04, moves no packet to its other method, keeps its weight at every step
// past the settlement that restores its room, and carries packets again.
#[test]
fn next_hops_are_drawn_by_fees_and_methods_by_structure() {
    let run = caught(|| hub_routes::run(SEED));
    assert!(!run.outcome.is_failure(), "{}", run.outcome);
    for what in [
        "routes with two first hops",
        "payments kept their ledger across nodes in both hubs",
        hub_routes::ATTACK_REFUSED,
        "payments completed past a hop that refuses",
        hub_routes::SHORT_OF_ROOM,
        "hops short of room kept their weight",
        "hops carried again once their room was back",
    ] {
        assert!(
            run.counts.contains_key(what),
            "never {what}: {:?}",
            run.counts
        );
    }
}
