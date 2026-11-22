//! What went back to a hold's payer over a hub, booked as that much of the
//! settlements that funded the hold, undone (spec/decisions/hold-returns.md):
//! on our side as the payer and as the payee, and what the books lack room
//! for on the payee's, through suspense with a warning rather than
//! stopping the scheme. On one engine core running the stub with
//! commitments, which takes the effects a hub's scheme reports.

use std::sync::Arc;

use dassie_ledger::{AccountId, AccountName, Balance, LedgerId, Name, Reference};
use dassie_settlement::{Effect, ReconcileCause};

use super::super::{Core, Event, SettlementEvent};
use super::{COMMITTING, NOW, add_stub, core_on};
use crate::engine::Output;
use crate::store::Store;

/// The peer the hold is with.
fn peer() -> Name {
    Name::new("peeraaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa").unwrap()
}

fn stub() -> Name {
    Name::new("stub").unwrap()
}

fn id(text: &str) -> Name {
    Name::new(text).unwrap()
}

/// A core with the stub, and the peer's accounts opened with `trust`:
/// the core, the store, and the peer's account.
fn with_peer(trust: u64) -> (Core, Arc<Store>, AccountId) {
    let store = Arc::new(Store::open_in_memory().unwrap());
    let mut core = core_on(Arc::clone(&store), 9);
    add_stub(&mut core, COMMITTING);
    let (interledger, _) = core
        .ledger
        .open_peer(NOW, LedgerId::StubUsd, peer(), trust)
        .unwrap();
    (core, store, interledger)
}

fn balance(core: &Core, name: AccountName) -> Balance {
    let id = core.account(LedgerId::StubUsd, name).unwrap();
    *core.ledger.account(id).unwrap().balance()
}

fn returned(amount: u64, incoming: bool, name: &str) -> Effect {
    Effect::HoldReturned {
        peer: peer(),
        amount,
        incoming,
        id: id(name),
    }
}

/// What the step reported returned: amount, and whether to us.
type Returns = Vec<(u64, bool)>;

/// What the step reported reconciled: amount, whether a gain, and why.
type Reconciled = Vec<(u64, bool, ReconcileCause)>;

/// The settlement events the step reported: what was returned, and what
/// was reconciled, by cause.
fn reported(core: &mut Core) -> (Returns, Reconciled) {
    let mut outputs = Vec::new();
    core.finish_step(&mut outputs);
    let mut returns = Vec::new();
    let mut reconciled = Vec::new();
    for output in outputs {
        match output {
            Output::Event(Event::Settlement(SettlementEvent::HoldReturned {
                amount,
                incoming,
                ..
            })) => returns.push((amount, incoming)),
            Output::Event(Event::Settlement(SettlementEvent::Reconciled {
                amount,
                gain,
                cause,
                ..
            })) => reconciled.push((amount, gain, cause)),
            _ => {}
        }
    }
    (returns, reconciled)
}

/// Our hold came back to us: Dr `assets/settlement`, Cr the peer's
/// account, journaled as a settlement under the scheme's ID for it, and
/// nothing through suspense.
#[test]
fn a_hold_of_ours_given_back_comes_back_from_the_peer() {
    let (mut core, store, interledger) = with_peer(0);
    let before = balance(&core, AccountName::AssetsSettlement);
    core.apply_effects(stub(), LedgerId::StubUsd, &[returned(300, true, "r1")])
        .unwrap();
    assert_eq!(reported(&mut core), (vec![(300, true)], vec![]));
    let peer_account = *core.ledger.account(interledger).unwrap().balance();
    assert_eq!(peer_account.credits_posted, 300);
    let settlement = balance(&core, AccountName::AssetsSettlement);
    assert_eq!(settlement.debits_posted - before.debits_posted, 300);
    assert_eq!(
        balance(&core, AccountName::EquitySuspense),
        Balance::default()
    );
    core.flush();
    let rows = store.transfers_after(None, 100, None).unwrap();
    assert!(rows.iter().any(|row| row.transfer.reference
        == Reference::Settlement {
            scheme: stub(),
            id: id("r1"),
        }
        && row.transfer.amount == 300));
}

/// The peer's hold toward us went back to it: Dr the peer's account, Cr
/// `assets/settlement`, undoing that much of its settlement to us, within
/// the room its earlier settlement left.
#[test]
fn a_peers_hold_given_back_goes_back_to_the_peer() {
    let (mut core, _, interledger) = with_peer(0);
    let before = balance(&core, AccountName::AssetsSettlement);
    let funded = Effect::IncomingSettlement {
        peer: peer(),
        amount: 500,
        id: Some(id("f1")),
    };
    core.apply_effects(stub(), LedgerId::StubUsd, &[funded])
        .unwrap();
    reported(&mut core);
    core.apply_effects(stub(), LedgerId::StubUsd, &[returned(300, false, "r1")])
        .unwrap();
    assert_eq!(reported(&mut core), (vec![(300, false)], vec![]));
    let peer_account = *core.ledger.account(interledger).unwrap().balance();
    assert_eq!(
        (peer_account.credits_posted, peer_account.debits_posted),
        (500, 300)
    );
    let settlement = balance(&core, AccountName::AssetsSettlement);
    assert_eq!(
        (
            settlement.debits_posted - before.debits_posted,
            settlement.credits_posted - before.credits_posted
        ),
        (500, 300)
    );
    assert_eq!(
        balance(&core, AccountName::EquitySuspense),
        Balance::default()
    );
}

/// What the books lack room for of a hold given back to the peer goes out
/// through suspense, as a difference they cannot explain, and the scheme
/// carries on: the room counts what the batch before it moved, a
/// settlement from the peer in the same batch included.
#[test]
fn what_the_books_lack_room_for_goes_through_suspense() {
    let (mut core, _, interledger) = with_peer(0);
    let before = balance(&core, AccountName::AssetsSettlement);
    let effects = [
        Effect::IncomingSettlement {
            peer: peer(),
            amount: 100,
            id: Some(id("f1")),
        },
        returned(300, false, "r1"),
    ];
    core.apply_effects(stub(), LedgerId::StubUsd, &effects)
        .unwrap();
    assert_eq!(
        reported(&mut core),
        (
            vec![(100, false)],
            vec![(200, false, ReconcileCause::Unexplained)]
        )
    );
    let peer_account = *core.ledger.account(interledger).unwrap().balance();
    assert_eq!(
        (peer_account.credits_posted, peer_account.debits_posted),
        (100, 100)
    );
    assert_eq!(
        balance(&core, AccountName::EquitySuspense).debits_posted,
        200
    );
    let settlement = balance(&core, AccountName::AssetsSettlement);
    assert_eq!(settlement.credits_posted - before.credits_posted, 300);
    assert!(core.schemes[&stub()].failed.is_none(), "it carries on");
}

/// A hold with a peer that has no accounts on the ledger goes through
/// suspense whole, either way.
#[test]
fn a_hold_with_a_peer_without_accounts_goes_through_suspense() {
    let store = Arc::new(Store::open_in_memory().unwrap());
    let mut core = core_on(store, 9);
    add_stub(&mut core, COMMITTING);
    let effects = [returned(300, true, "r1"), returned(200, false, "r2")];
    core.apply_effects(stub(), LedgerId::StubUsd, &effects)
        .unwrap();
    assert_eq!(
        reported(&mut core),
        (
            vec![],
            vec![
                (300, true, ReconcileCause::Unexplained),
                (200, false, ReconcileCause::Unexplained)
            ]
        )
    );
    assert!(core.schemes[&stub()].failed.is_none());
}
