//! The node simulation with a settlement hub run in it
//! (`dassie_node::sim::hub`, spec/areas/testing.md#over-a-settlement-hub):
//! its validators, each the node's own validator role without its IO, found
//! the hub on the fake XRP Ledger, and nodes join it by deposit.
//!
//! The `node-hub` run (`dassie_node::sim::hubbed`) draws a hub (4 to 7
//! validators, up to f of them faulty), its members, a lying member and a
//! validator's restart, pays across hops over the hub, then has the hub
//! stall and its escrow pay out, or f + 1 validators collude. The campaign
//! runs it from random seeds, outside CI: `dassie-sim run node-hub <seed>`
//! replays a seed, `dassie-sim explain node-hub <seed>` prints its trace.
//!
//! The `node-hub-lies` and `node-hub-currencies` runs
//! (`dassie_node::sim::hub_runs`) put lying XRP Ledger servers under a hub's
//! members, and payments across dollars and XRP in a hub; these tests run
//! them on fixed seeds, and `dassie-sim` on any.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use std::collections::BTreeMap;

use dassie_kit::sim::{assert_passes, caught, each};
use dassie_node::sim::{START, Settling, Sim, hub_runs, hubbed, key};

/// The seeds of `node-hub-lies` the tests run, which between them tell
/// every kind of lie, and in which an owner and a node move funds while
/// lied to: how long a lying server's connection lasts decides which it
/// gets to tell. One seed reaches all of it today; should a change stop
/// it, add another rather than lose a kind.
const LIES_SEEDS: [u64; 1] = [7];

/// The seed of `node-hub-currencies` the tests run.
const ACROSS_SEED: u64 = 3;

// spec/areas/hubs.md#in-the-node: a node opens its account in the hub by a
// deposit from its XRP Ledger testnet account, which the validators see on
// the escrow and credit once a quorum attested it.
#[test]
fn nodes_join_a_hub_by_deposit() {
    let mut sim = Sim::with(1, 3, 0, BTreeMap::new(), Settling::Hub(4));
    sim.loss_per_mille = 0;
    assert!(
        sim.run_until_or(START + 600_000, hubbed::joined),
        "the nodes did not join the hub"
    );
    let hub = sim.hub.as_deref().unwrap();
    let ledger = hub.ledger().unwrap();
    assert_eq!(ledger.members(), 3);
    assert!(ledger.escrow().checked);
    assert_eq!(
        sim.ledger.balance(hub.escrow()),
        Some(hub.escrow_funding + ledger.supply()),
        "the escrow holds what the hub owes and what the sponsor left"
    );
}

// spec/designs/settlement-hubs.md#4-peering-over-a-hub: each way of a
// peering over a hub has a hold of its own, and a payer can send only what
// its hold has room for, whatever the payee owes it the other way. So the
// payee asks it to settle once that room runs low, even while its books,
// which net both ways, show room enough. Two members paying each other at
// once through a third, each of its holds drawn on both ways: both
// payments complete.
#[test]
fn payments_both_ways_through_a_hop_over_a_hub_complete() {
    let mut sim = Sim::with(3, 3, 0, BTreeMap::new(), Settling::Hub(4));
    sim.loss_per_mille = 0;
    assert!(
        sim.run_until_or(START + 600_000, hubbed::joined),
        "the members did not join the hub"
    );
    sim.step(0, |core| core.ban_node(key(2).node_id()).unwrap());
    sim.step(2, |core| core.ban_node(key(0).node_id()).unwrap());
    sim.join_network(1);
    let routed =
        |sim: &Sim| sim.peers(1).len() == 2 && !sim.peers(0).is_empty() && !sim.peers(2).is_empty();
    assert!(
        sim.run_until_or(sim.now + 600_000, routed),
        "the members did not peer"
    );
    sim.run_until(sim.now + 60_000);
    sim.pay(0, 2, 3_000_000_000, None, None);
    sim.pay(2, 0, 3_000_000_000, None, None);
    let paid = |sim: &Sim| sim.payments.iter().all(|payment| payment.done);
    assert!(
        sim.run_until_or(sim.now + 600_000, paid),
        "a payment stopped: {:?}",
        sim.payments
            .iter()
            .map(|payment| (payment.from, payment.to, dassie_node::sim::sent(payment)))
            .collect::<Vec<_>>()
    );
}

// spec/areas/testing.md#over-a-settlement-hub: members peered over a hub,
// whose XRP Ledger servers tell each kind of lie in turn while they pay,
// their owners fund their accounts, one owner moves funds into the hub and
// the other members' nodes move theirs by themselves, then a
// validator of the ledger rotates its key, a quorum is withheld and a list
// lapses. Each member's books hold to the fake ledger's truth and to the
// hub, every member anchored, the payments complete and the hub keeps
// running. Between them, the seeds tell every kind of lie, and the members
// catch what the verifier catches: so a change that quietly bypasses it
// fails.
#[test]
fn hub_members_hold_to_the_ledger_through_lying_servers() {
    let mut reached: BTreeMap<&str, u64> = BTreeMap::new();
    for (seed, run) in LIES_SEEDS.iter().zip(each(&LIES_SEEDS, |seed| {
        caught(|| hub_runs::run_lies(seed))
    })) {
        assert!(!run.outcome.is_failure(), "seed {seed}: {}", run.outcome);
        for (what, count) in run.counts {
            *reached.entry(what).or_default() += count;
        }
    }
    let lies = dassie_xrpl::fake::Lie::ALL.map(dassie_node::sim::told_as);
    for what in lies.into_iter().chain([
        "lies caught",
        "anchors",
        "transactions proven",
        "absences proven",
        "validator lists fetched",
        "validator keys rotated",
        "quorums withheld",
        "validator lists lapsed",
        "owner moves into the hub",
        "owner moves while lied to",
        "own moves while lied to",
    ]) {
        assert!(
            reached.contains_key(what),
            "the seeds never reached {what}: {reached:?}"
        );
    }
}

// spec/areas/testing.md#over-a-settlement-hub: across dollars over the stub
// with commitments and XRP in a hub, through nodes on both that convert and
// price by their inventory, one of them charging: what arrives is what was
// sent at the rates, less the fees, an outgoing payment crosses, and the
// hub and its members' books hold.
#[test]
fn hub_members_pay_across_currencies() {
    assert_passes(ACROSS_SEED, hub_runs::run_across);
}

// spec/designs/settlement-hubs.md#5-joining: a node with no account in the
// hub peers with a member over it, is paid in without depositing once the
// member owes it the hub's minimum for a new account and not before, though
// both crash around the `pay` (on seed 1 the payer with it on its way),
// sets its disbursal address, and pays and is paid over the hub from then
// on, each node's books holding to the hub's: the `pay` is booked once on
// each side. Fresh keys cycled through a member, each paid less than the
// minimum, are never paid in, spend no more than they are owed, and cost
// the member nothing (spec/decisions/hubs-only.md).
#[test]
fn a_node_joins_a_hub_by_being_paid() {
    let runs = [
        (
            caught(|| hubbed::paid_in(1)),
            &[
                "newcomers not paid in short of the minimum",
                "payers crashed paying a newcomer in",
                "newcomers paid in",
            ][..],
        ),
        (caught(|| hubbed::cycled(1)), &["fresh keys cycled"][..]),
    ];
    for (run, counted) in runs {
        assert!(!run.outcome.is_failure(), "{}", run.outcome);
        for what in counted {
            assert!(
                run.counts.contains_key(*what),
                "never {what}: {:?}",
                run.counts
            );
        }
    }
}
