//! The node simulation over a settlement hub of DSUSD
//! (`dassie_node::sim::hub_token`, spec/areas/testing.md#over-a-settlement-hub,
//! spec/decisions/token-hubs.md): members fund their accounts in the hub
//! from their DSUSD owner books, pay each other over it while the token is
//! off its peg, top up the escrow's XRP before a withdrawal, send and take
//! nothing over the hub while its issuer freezes it and pay each other
//! again after the thaw, and are paid out in DSUSD when the hub ends, pro
//! rata after a clawback. The campaign runs it from random seeds, outside CI:
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

// spec/roadmap.md#token-hubs: one run of `node-hub-dsusd` on a fixed seed
// passes, and shows what it is for: members joined the DSUSD hub and paid
// each other over it while the token was off its peg, which halted the
// conversions of its ledger; XRP sent to the escrow credited no one, and a
// withdrawal topped up the escrow's gas first; the members learned of the
// issuer's freeze, sent no packet over the hub while it lasted, kept the
// claims for packets they took before it, which the frozen hub paid, lost
// nothing on a peer but the fees of their own claims, and paid each other
// again after the thaw; a commitment too far ahead was refused;
// and after the issuer clawed back from the escrow, the hub paid out pro
// rata, to the unit.
#[test]
fn a_dsusd_hub_runs_from_joining_through_a_freeze_to_its_payout_after_a_clawback() {
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
        "conversions halted off the peg",
        "members paid each other off the peg",
        "XRP sent to the escrow",
        "escrow gas topped up before a withdrawal",
        "members learned the freeze",
        "no packet crossed the frozen hub",
        "claims the frozen hub paid",
        "members learned the thaw",
        "members paid each other after the thaw",
        "commitments too far ahead refused",
        "issuer clawbacks from the escrow",
        "pro rata payouts checked",
    ] {
        assert_eq!(run.counts.get(counted), Some(&1), "{counted}");
    }
}
