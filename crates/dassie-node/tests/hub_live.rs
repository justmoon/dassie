//! The node simulation over settlement hubs on the XRP Ledger's mainnet, in
//! the live realm (`dassie_node::sim::hub_live`,
//! spec/areas/testing.md#over-a-settlement-hub,
//! spec/roadmap.md#mainnet-hubs): members join a hub of XRP or of RLUSD
//! that the run founds on the fake ledger's mainnet, from their owner books,
//! which their owners' deposits and the issuer's mint fund, and pay each
//! other over it while their servers and the hub's validators' servers lie;
//! in the RLUSD hub, while the token is off its peg too. The campaign runs
//! them from random seeds, outside CI: `dassie-sim run node-hub-live <seed>`
//! replays a seed, `dassie-sim explain node-hub-rlusd <seed>` prints its
//! trace.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use dassie_kit::sim::{Outcome, Run};
use dassie_node::sim::hub_live;

/// What every run must reach, beside what each counts of its own: the
/// members joined, their servers lied, the hub's validators' watchers
/// connected to lying servers and dropped them, the members caught lies,
/// and paid each other over the hub's method meanwhile.
fn check_reached(run: &Run, joined: &str, over: &str) {
    for counted in [joined, "members paid each other under lying servers"] {
        assert_eq!(run.counts.get(counted), Some(&1), "{counted}");
    }
    for counted in [
        "lying servers",
        "validator connections to lying servers",
        "lying servers validators dropped",
        "lies caught",
        "XRPL nodes anchored",
        over,
    ] {
        assert!(
            run.counts.get(counted).is_some_and(|count| *count > 0),
            "{counted}"
        );
    }
}

// spec/roadmap.md#mainnet-hubs: one run of `node-hub-live` on a fixed seed
// passes, and shows what it is for: nodes of the live realm joined a hub of
// XRP on the fake ledger's mainnet from their `xrpl-xrp` owner books, and
// paid each other over `hub-xrpl-xrp` while their servers and the hub's
// validators' servers lied, catching the lies.
#[test]
fn live_nodes_pay_over_a_mainnet_xrp_hub_while_servers_lie() {
    let seed = 2;
    let run = hub_live::run_live(seed);
    assert_eq!(
        run.outcome,
        Outcome::Pass,
        "{}: {}",
        hub_live::describe_live(seed),
        run.trace.join("\n")
    );
    check_reached(
        &run,
        "members joined the mainnet XRP hub",
        "prepares over the mainnet XRP hub",
    );
}

// spec/roadmap.md#mainnet-hubs: one run of `node-hub-rlusd` on a fixed seed
// passes, and shows what it is for: nodes of the live realm joined a hub of
// RLUSD on the fake ledger's mainnet from their `xrpl-rlusd` owner books,
// and paid each other over `hub-xrpl-rlusd` while their servers and the
// hub's validators' servers lied; and with RLUSD's pool off its peg, their
// peg guards halted the conversions of `hub-xrpl-rlusd+usd` while they
// still paid each other over it.
#[test]
fn live_nodes_pay_over_a_mainnet_rlusd_hub_while_servers_lie_and_off_its_peg() {
    let seed = 8;
    let run = hub_live::run_rlusd(seed);
    assert_eq!(
        run.outcome,
        Outcome::Pass,
        "{}: {}",
        hub_live::describe_rlusd(seed),
        run.trace.join("\n")
    );
    check_reached(
        &run,
        "members joined the RLUSD hub",
        "prepares over the RLUSD hub",
    );
    for counted in [
        "conversions halted off the peg",
        "members paid each other off the peg",
    ] {
        assert_eq!(run.counts.get(counted), Some(&1), "{counted}");
    }
}
