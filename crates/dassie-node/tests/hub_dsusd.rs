//! The node simulation over a settlement hub of DSUSD
//! (`dassie_node::sim::hub_token`, spec/areas/testing.md#over-a-settlement-hub,
//! spec/decisions/token-hubs.md): members fund their accounts in the hub
//! from their DSUSD owner books, pay each other over it, top up the
//! escrow's XRP before a withdrawal, and are paid out in DSUSD when the hub
//! stalls. The campaign runs it from random seeds, outside CI:
//! `dassie-sim run node-hub-dsusd <seed>` replays a seed,
//! `dassie-sim explain node-hub-dsusd <seed>` prints its trace.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use dassie_kit::sim::Outcome;
use dassie_node::sim::hub_token;

// spec/decisions/token-hubs.md: one run of `node-hub-dsusd` on a fixed
// seed passes, and shows what it is for: members joined the DSUSD hub, XRP
// sent to the escrow credited no one, a withdrawal topped up the escrow's
// gas first, and the hub paid out in DSUSD.
#[test]
fn a_dsusd_hub_runs_from_joining_to_its_payout() {
    let seed = 1;
    let run = hub_token::run(seed);
    assert_eq!(
        run.outcome,
        Outcome::Pass,
        "{}: {}",
        hub_token::describe(seed),
        run.trace.join("\n")
    );
    for counted in [
        "members joined the DSUSD hub",
        "XRP sent to the escrow",
        "escrow gas topped up before a withdrawal",
        "DSUSD payouts checked",
    ] {
        assert_eq!(run.counts.get(counted), Some(&1), "{counted}");
    }
}
