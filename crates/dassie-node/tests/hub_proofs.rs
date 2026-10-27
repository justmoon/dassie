//! A simulated settlement hub whose validators' XRPL servers lie
//! (spec/areas/hubs.md#what-validators-attest): each validator's watcher
//! proves what it attests under the fake ledger's UNL, so a lying server
//! makes it attest nothing no validated ledger holds. The members' hub
//! books hold throughout: the escrow holds at least what the hub owes them
//! and what the sponsor left (deposits not yet credited are beyond it), and
//! no reset is written off that never happened; once the servers are
//! honest again, the members join, and it holds exactly that.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use std::collections::BTreeMap;

use dassie_node::sim::{START, Settling, Sim, hubbed};
use dassie_xrpl::fake::Lie;

/// The hub's books hold: the escrow holds at least what the hub owes and
/// what the sponsor left, exactly that once `settled`, and the hub wrote
/// off no reset.
fn books_hold(sim: &Sim, settled: bool) {
    let hub = sim.hub.as_deref().unwrap();
    let ledger = hub.ledger().unwrap();
    let held = sim.ledger.balance(hub.escrow()).unwrap();
    let owed = hub.escrow_funding + ledger.supply();
    if settled {
        assert_eq!(held, owed, "the escrow holds what the hub owes");
    } else {
        assert!(held >= owed, "the hub owes no more than the escrow holds");
    }
    assert_eq!(ledger.written_off_at(), None, "no reset happened");
}

// Every validator's server lies at once, each another lie, for longer than
// the members take to deposit: what the validators attest meanwhile is
// only what the ledger holds. Then they are honest, and the members join.
#[test]
fn validators_on_lying_servers_attest_nothing_false() {
    let mut sim = Sim::with(11, 3, 0, BTreeMap::new(), Settling::Hub(4));
    sim.loss_per_mille = 0;
    let lies = [
        Lie::Transaction,
        Lie::MadeUpReset,
        Lie::AlteredObject,
        Lie::ForgedValidations,
    ];
    for (index, lie) in lies.into_iter().enumerate() {
        sim.lie_to_validator(index, Some(lie));
    }
    let mut at = START;
    while at < START + 300_000 {
        at += 10_000;
        sim.run_until(at);
        books_hold(&sim, false);
    }
    for lie in lies {
        assert!(sim.ledger.lies_told(lie) > 0, "{lie:?} was told");
    }
    for index in 0..lies.len() {
        sim.lie_to_validator(index, None);
    }
    assert!(
        sim.run_until_or(sim.now + 600_000, hubbed::joined),
        "the members did not join the hub once the servers were honest"
    );
    sim.run_until(sim.now + 60_000);
    books_hold(&sim, true);
    assert!(
        sim.hub
            .as_deref()
            .unwrap()
            .ledger()
            .unwrap()
            .escrow()
            .checked
    );
}

// Every validator's server tells the same lie: a quorum of watchers that
// took their servers' word would agree on it, attesting a deposit, a
// changed setup or a reset no validated ledger holds. Proving, they attest
// none of it: the books hold, and the members join once the servers are
// honest.
#[test]
fn a_quorum_of_servers_telling_one_lie_makes_nothing_false() {
    for (seed, lie) in [
        Lie::Transaction,
        Lie::MadeUpReset,
        Lie::AlteredObject,
        Lie::FalseAbsence,
    ]
    .into_iter()
    .enumerate()
    {
        let mut sim = Sim::with(
            13 + u64::try_from(seed).unwrap(),
            3,
            0,
            BTreeMap::new(),
            Settling::Hub(4),
        );
        sim.loss_per_mille = 0;
        for index in 0..4 {
            sim.lie_to_validator(index, Some(lie));
        }
        let mut at = START;
        while at < START + 200_000 {
            at += 10_000;
            sim.run_until(at);
            books_hold(&sim, false);
        }
        assert!(sim.ledger.lies_told(lie) > 0, "{lie:?} was told");
        for index in 0..4 {
            sim.lie_to_validator(index, None);
        }
        assert!(
            sim.run_until_or(sim.now + 600_000, hubbed::joined),
            "the members did not join the hub once the servers were honest ({lie:?})"
        );
        sim.run_until(sim.now + 60_000);
        books_hold(&sim, true);
    }
}

// One validator's server tells every kind of lie in turn while the others
// are honest: the hub runs on, and its books hold.
#[test]
fn a_hub_runs_on_while_one_validators_server_lies() {
    let mut sim = Sim::with(12, 3, 0, BTreeMap::new(), Settling::Hub(4));
    sim.loss_per_mille = 0;
    for lie in Lie::ALL {
        sim.lie_to_validator(0, Some(lie));
        sim.run_until(sim.now + 40_000);
        books_hold(&sim, false);
    }
    sim.lie_to_validator(0, None);
    assert!(
        sim.run_until_or(sim.now + 600_000, hubbed::joined),
        "the members did not join the hub"
    );
    sim.run_until(sim.now + 60_000);
    books_hold(&sim, true);
    let lied = u64::try_from(Lie::ALL.len()).unwrap() + 1;
    assert_eq!(sim.counts["lying validator servers"], lied);
}
