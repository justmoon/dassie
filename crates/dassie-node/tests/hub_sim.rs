//! The node simulation with a settlement hub run in it
//! (`dassie_node::sim::hub`, spec/areas/testing.md#the-simulations): its
//! validators, each the node's own validator role without its IO, found
//! the hub on the fake XRP Ledger, and nodes join it by deposit.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use std::collections::BTreeMap;

use dassie_node::sim::{START, Settling, Sim, key};

/// Whether every node has its account in the hub, in step with it.
fn joined(sim: &Sim) -> bool {
    (0..sim.nodes.len()).all(|index| {
        sim.core(index).hub_status().is_some_and(|status| {
            status.account.is_some() && status.status == dassie_settlement::LedgerStatus::Synced
        })
    })
}

// spec/areas/hubs.md#in-the-node: a node opens its account in the hub by a
// deposit from its XRP Ledger testnet account, which the validators see on
// the escrow and credit once a quorum attested it.
#[test]
fn nodes_join_a_hub_by_deposit() {
    let mut sim = Sim::with(1, 3, 0, BTreeMap::new(), Settling::Hub(4));
    sim.loss_per_mille = 0;
    assert!(
        sim.run_until_or(START + 600_000, joined),
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
        sim.run_until_or(START + 600_000, joined),
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
