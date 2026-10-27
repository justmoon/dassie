//! A simulated settlement hub whose validators' XRPL servers lie
//! (spec/areas/hubs.md#what-validators-attest): each validator's watcher
//! proves what it attests under the fake ledger's UNL, so a lying server
//! makes it attest nothing no validated ledger holds. The members' hub
//! books hold throughout: the escrow holds at least what the hub owes them
//! and what the sponsor left (deposits not yet credited are beyond it), and
//! no reset is written off that never happened; once the servers are
//! honest again, the members join, and it holds exactly that. A DSUSD hub's
//! validators, through a freeze of the escrow's line, lose no deposit the
//! freeze left without room to a restart, and sign the payout set of a hub
//! that stalled while held once they proved the thaw
//! (spec/decisions/token-hubs.md).

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use std::collections::BTreeMap;

use dassie_node::sim::{START, Settling, Sim, hub_token, hubbed};
use dassie_xrpl::Wallet;
use dassie_xrpl::api;
use dassie_xrpl::codec::{Amount, Payment, Signing, TokenValue, Transact, TrustSet};
use dassie_xrpl::fake::{FEE_BASE, Lie};

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

/// DSUSD, the token hub's asset, on the fake ledger.
const DSUSD: dassie_xrpl::amount::IssuedAsset = dassie_xrpl::amount::IssuedAsset {
    issuer: dassie_xrpl::fake::DSUSD_ISSUER,
    currency: dassie_xrpl::fake::DSUSD_CURRENCY,
};

/// The connection the test's own transactions go on: never a node's.
const OUTSIDE: u64 = u64::MAX - 1;

/// What the hub's escrow holds, as its ledger has it.
fn escrow_balance(sim: &Sim) -> u64 {
    sim.hub
        .as_deref()
        .unwrap()
        .ledger()
        .unwrap()
        .escrow()
        .balance
}

/// Whether the hub's ledger has the issuer holding its escrow.
fn held(sim: &Sim) -> bool {
    sim.hub
        .as_deref()
        .unwrap()
        .ledger()
        .unwrap()
        .issuer()
        .holds()
}

/// Submits `transaction` on the fake ledger from outside the network, and
/// lets a ledger close over it.
fn outside(sim: &mut Sim, blob: &[u8]) {
    sim.ledger.request(OUTSIDE, &api::submit(1, blob));
    sim.run_until(sim.now + 2 * dassie_xrpl::fake::CLOSE_INTERVAL_S * 1_000);
}

/// Someone outside the network who holds DSUSD, to deposit it.
fn dsusd_holder(sim: &mut Sim) -> Wallet {
    let holder = Wallet::from_seed([5; 16]);
    sim.ledger.deposit(holder.account(), 100_000_000);
    sim.run_until(sim.now + 10_000);
    let trust = TrustSet {
        account: holder.account(),
        limit: DSUSD.amount(TokenValue::parse("1000").unwrap()),
        fee: FEE_BASE,
        sequence: sim.ledger.sequence(holder.account()).unwrap(),
        last_ledger_sequence: None,
        flags: 0,
        network_id: None,
        memos: Vec::new(),
        signing: Signing::by(holder.public_key()),
    }
    .sign(&holder);
    outside(sim, &trust.blob);
    let issued = DSUSD.amount(TokenValue::parse("100").unwrap());
    sim.ledger.issue(holder.account(), issued).unwrap();
    sim.run_until(sim.now + 10_000);
    holder
}

/// `holder` deposits one DSUSD to the escrow for the hub's account `tag`.
fn deposit_dsusd(sim: &mut Sim, holder: &Wallet, escrow: dassie_xrpl::AccountId, tag: u32) {
    let paid = Payment {
        account: holder.account(),
        destination: escrow,
        destination_tag: Some(tag),
        amount: Amount::Token(DSUSD.amount(TokenValue::parse("1").unwrap())),
        send_max: None,
        fee: FEE_BASE,
        sequence: sim.ledger.sequence(holder.account()).unwrap(),
        last_ledger_sequence: None,
        flags: 0,
        network_id: None,
        memos: Vec::new(),
        signing: Signing::by(holder.public_key()),
    }
    .sign(holder);
    outside(sim, &paid.blob);
}

// spec/areas/hubs.md#the-validators-service: during a freeze, deposits
// beyond the validators' shares of the pending table are refused for room.
// Every validator restarts while the freeze lasts, forgetting what its
// service kept to attest again; its watcher's cursor passed none of them,
// so each is read again, and once the issuer lets go, every deposit is
// credited, once.
#[test]
fn deposits_refused_for_room_in_a_freeze_are_credited_after_a_restart() {
    let mut sim = Sim::with(21, 3, 0, BTreeMap::new(), Settling::HubDsusd(4));
    sim.loss_per_mille = 0;
    assert!(
        sim.run_until_or(START + 1_200_000, hub_token::joined),
        "the members did not join the hub"
    );
    let hub = sim.hub.as_deref().unwrap();
    let escrow = hub.escrow();
    let ledger = hub.ledger().unwrap();
    let account = ledger.accounts().next().unwrap().0;
    let share = usize::try_from(ledger.rules().terms.limits.pending_deposits).unwrap()
        / ledger.validators().len();

    let holder = dsusd_holder(&mut sim);

    // The issuer freezes the escrow's line, which can still be paid.
    sim.ledger.freeze(escrow, DSUSD, true).unwrap();
    assert!(
        sim.run_until_or(sim.now + 300_000, held),
        "the hub did not freeze"
    );
    let before = escrow_balance(&sim);
    let deposits = share + 3;
    for _ in 0..deposits {
        deposit_dsusd(&mut sim, &holder, escrow, account.0);
    }
    sim.run_until(sim.now + 60_000);
    assert_eq!(
        escrow_balance(&sim),
        before,
        "nothing credited while frozen"
    );

    // Every validator restarts during the freeze.
    for index in 0..4 {
        sim.crash_validator(index);
    }
    sim.run_until(sim.now + 10_000);
    for index in 0..4 {
        sim.start_validator(index);
    }
    sim.run_until(sim.now + 60_000);
    assert!(held(&sim));

    sim.ledger.freeze(escrow, DSUSD, false).unwrap();
    let total = u64::try_from(deposits).unwrap() * 1_000_000;
    assert!(
        sim.run_until_or(sim.now + 600_000, |sim| escrow_balance(sim)
            >= before + total),
        "every deposit is credited after the thaw: {} of {total}",
        escrow_balance(&sim) - before
    );
    sim.run_until(sim.now + 60_000);
    assert_eq!(escrow_balance(&sim), before + total, "each once");
}

// spec/areas/hubs.md#payout-bundles: a hub that stalls while the issuer
// holds its escrow makes no block that could show the thaw. Its validators
// sign no payout set while the line is frozen; once the issuer lets go,
// each proves the thaw through its own verifier, signs, and the escrow
// pays out.
#[test]
fn a_hub_that_stalled_while_held_pays_out_after_the_thaw() {
    let mut sim = Sim::with(22, 3, 0, BTreeMap::new(), Settling::HubDsusd(4));
    sim.loss_per_mille = 0;
    assert!(
        sim.run_until_or(START + 1_200_000, hub_token::joined),
        "the members did not join the hub"
    );
    let escrow = sim.hub.as_deref().unwrap().escrow();
    sim.ledger.freeze(escrow, DSUSD, true).unwrap();
    assert!(
        sim.run_until_or(sim.now + 300_000, held),
        "the hub did not freeze"
    );

    // The validators lose each other for longer than a stall, then find
    // each other again, the hub stalled.
    let validators = sim.hub.as_deref().unwrap().validators.len();
    let stall = sim
        .hub
        .as_deref()
        .unwrap()
        .ledger()
        .unwrap()
        .terms()
        .timers
        .stall;
    let hub = sim.hub.as_deref_mut().unwrap();
    for a in 0..validators {
        for b in a + 1..validators {
            hub.cut.insert((a, b));
        }
    }
    sim.run_until(sim.now + stall + 60_000);
    let hub = sim.hub.as_deref().unwrap();
    assert!(
        hub.validators.iter().all(|v| v.stalled()),
        "the hub stalled"
    );
    sim.hub.as_deref_mut().unwrap().cut.clear();
    sim.run_until(sim.now + 120_000);
    assert_eq!(
        sim.counts.get("payout sets signed"),
        None,
        "none while held"
    );
    assert!(sim.hub.as_deref().unwrap().payout.is_none());

    sim.ledger.freeze(escrow, DSUSD, false).unwrap();
    assert!(
        sim.run_until_or(sim.now + 600_000, |sim| {
            sim.hub
                .as_deref()
                .unwrap()
                .payout
                .as_ref()
                .is_some_and(|payout| payout.escrow_after.is_some())
        }),
        "the escrow paid out after the thaw"
    );
    assert_eq!(sim.counts["payout sets submitted"], 1);
}
