//! Scenarios of version 2 of the hub's protocol
//! (spec/decisions/signed-rejects.md): claims that wait for the hold's
//! notice before they pay, the payer's dispute with its payee's release,
//! and moving a hub of version 1 to version 2
//! (spec/decisions/hub-protocol-version.md). Every block asserts the
//! invariants.

use alloc::vec;
use alloc::vec::Vec;

use dassie_identity::NodeKey;

use super::tests::{Hub, OP_FEE, commitment, hold_for_bob, resigned};
use super::*;
use crate::operation::{MemberOperation, ValidatorOperation};
use crate::promise::{Claim, Fulfilled, Release, Signed};
use crate::rules::tests::rules;

/// Bob's claim of commitment `sequence` of `amount` on `hold`, which Alice
/// signed.
fn claim(
    hub: &Hub,
    alice: &NodeKey,
    hold: HoldNumber,
    sequence: u64,
    amount: Amount,
) -> MemberOperation {
    claim_until(
        hub,
        alice,
        hold,
        sequence,
        amount,
        hub.ledger.timestamp() + 10_000,
    )
}

/// As [`claim`], of a commitment that expires at `expiry`.
fn claim_until(
    hub: &Hub,
    alice: &NodeKey,
    hold: HoldNumber,
    sequence: u64,
    amount: Amount,
    expiry: Millis,
) -> MemberOperation {
    let id = *hub.ledger.hub();
    let preimage = u8::try_from(sequence).unwrap();
    let (c, f) = commitment(hold, sequence, amount, expiry, preimage);
    MemberOperation::Claim {
        hold,
        fulfilled: vec![Fulfilled {
            commitment: Signed::sign(c, &id, alice),
            fulfillment: f,
        }],
    }
}

/// The dispute of commitment `sequence` on `hold`, with `signer`'s
/// release of it, as [`claim`] made it.
fn release(hub: &Hub, signer: &NodeKey, hold: HoldNumber, sequence: u64) -> MemberOperation {
    let expiry = hub
        .ledger
        .hold(hold)
        .unwrap()
        .pending
        .get(&sequence)
        .map_or(0, |p| p.expiry);
    release_of(hub, signer, (hold, sequence, expiry))
}

/// The dispute with `signer`'s release of commitment `sequence` on `hold`
/// expiring at `expiry`.
fn release_of(
    hub: &Hub,
    signer: &NodeKey,
    (hold, sequence, expiry): (HoldNumber, u64, Millis),
) -> MemberOperation {
    MemberOperation::Dispute {
        hold,
        releases: vec![Signed::sign(
            Release {
                hold,
                sequence,
                expiry,
            },
            hub.ledger.hub(),
            signer,
        )],
    }
}

/// Lets the hold's notice pass from now.
fn after_notice(hub: &mut Hub) {
    let at = hub.ledger.timestamp() + hub.ledger.terms().timers.hold_notice;
    hub.block_at(at, vec![], &[]);
}

#[test]
fn a_claim_pays_once_the_holds_notice_passed() {
    let mut hub = Hub::new();
    assert!(hub.ledger.version() >= crate::rules::RELEASES_VERSION);
    let (_, alice_key, bob, bob_key, hold) = hold_for_bob(&mut hub);
    let before = hub.balance(bob);
    let op = claim(&hub, &alice_key, hold, 1, 100);
    assert_eq!(
        hub.run(bob, &bob_key, op),
        Outcome::Applied {
            fee: OP_FEE,
            detail: Detail::Pending(100)
        }
    );
    // Taken from the hold, not paid yet.
    assert_eq!(hub.ledger.hold(hold).unwrap().amount, 1_000_000 - 100);
    assert_eq!(hub.balance(bob), before - OP_FEE);
    let due = hub.ledger.hold(hold).unwrap().pending_due;
    hub.block_at(due - 1, vec![], &[]);
    assert_eq!(hub.balance(bob), before - OP_FEE);
    hub.block_at(due, vec![], &[]);
    assert_eq!(hub.balance(bob), before - OP_FEE + 100);
    assert!(hub.ledger.hold(hold).unwrap().pending.is_empty());
    assert_eq!(hub.ledger.hold(hold).unwrap().claimed[&1], 100);
}

#[test]
fn the_payers_dispute_with_the_payees_release_puts_the_claim_back() {
    let mut hub = Hub::new();
    let (alice, alice_key, bob, bob_key, hold) = hold_for_bob(&mut hub);
    let op = claim(&hub, &alice_key, hold, 1, 100);
    hub.run(bob, &bob_key, op.clone());
    let before = hub.balance(bob);
    // Alice's own signature is no release, and Bob cannot dispute.
    let forged = release(&hub, &alice_key, hold, 1);
    assert_eq!(
        hub.run(alice, &alice_key, forged),
        Outcome::Failed {
            fee: OP_FEE,
            reason: Failure::Claim
        }
    );
    let released = release(&hub, &bob_key, hold, 1);
    assert_eq!(
        hub.run(bob, &bob_key, released.clone()),
        Outcome::Failed {
            fee: OP_FEE,
            reason: Failure::NotYours
        }
    );
    assert_eq!(
        hub.run(alice, &alice_key, released.clone()),
        Outcome::Applied {
            fee: OP_FEE,
            detail: Detail::Returned(100)
        }
    );
    assert_eq!(hub.ledger.hold(hold).unwrap().amount, 1_000_000);
    after_notice(&mut hub);
    assert_eq!(hub.balance(bob), before - OP_FEE, "nothing paid");
    // The sequence stays claimed, for nothing.
    assert_eq!(
        hub.run(bob, &bob_key, op),
        Outcome::Failed {
            fee: OP_FEE,
            reason: Failure::Claim
        }
    );
    // A redeem past it pays what Alice's claim adds, all of it.
    let id = *hub.ledger.hub();
    let total = Signed::sign(
        Claim {
            hold,
            total: 50,
            through: 2,
        },
        &id,
        &alice_key,
    );
    assert_eq!(
        hub.run(bob, &bob_key, MemberOperation::Redeem(total)),
        Outcome::Applied {
            fee: OP_FEE,
            detail: Detail::Paid(50)
        }
    );
}

// A release names its commitment's expiry: one of another commitment of
// the same sequence, which the payer signed after the payee forgot the
// first, puts nothing back, and the payee's claim is paid.
#[test]
fn a_release_of_another_commitment_of_the_sequence_puts_nothing_back() {
    let mut hub = Hub::new();
    let (alice, alice_key, bob, bob_key, hold) = hold_for_bob(&mut hub);
    let op = claim(&hub, &alice_key, hold, 1, 100);
    hub.run(bob, &bob_key, op);
    let expiry = hub.ledger.hold(hold).unwrap().pending[&1].expiry;
    let earlier = release_of(&hub, &bob_key, (hold, 1, expiry - 1));
    assert_eq!(
        hub.run(alice, &alice_key, earlier),
        Outcome::Failed {
            fee: OP_FEE,
            reason: Failure::Missing
        }
    );
    assert_eq!(hub.ledger.hold(hold).unwrap().pending.len(), 1);
}

#[test]
fn a_dispute_after_the_payout_finds_nothing() {
    let mut hub = Hub::new();
    let (alice, alice_key, bob, bob_key, hold) = hold_for_bob(&mut hub);
    let op = claim(&hub, &alice_key, hold, 1, 100);
    hub.run(bob, &bob_key, op);
    let released = release(&hub, &bob_key, hold, 1);
    after_notice(&mut hub);
    assert_eq!(
        hub.run(alice, &alice_key, released.clone()),
        Outcome::Failed {
            fee: OP_FEE,
            reason: Failure::Missing
        }
    );
}

// A redeem leaves what waits to its payout, and subtracts what `claim`
// took up to its W, paid or waiting: the payee is paid the payer's total,
// whatever it claimed. Nothing up to the W can be disputed any more.
#[test]
fn a_redeem_counts_waiting_claims_as_taken() {
    let mut hub = Hub::new();
    let (alice, alice_key, bob, bob_key, hold) = hold_for_bob(&mut hub);
    let op = claim(&hub, &alice_key, hold, 1, 100);
    hub.run(bob, &bob_key, op);
    let before = hub.balance(bob);
    let id = *hub.ledger.hub();
    let total = Signed::sign(
        Claim {
            hold,
            total: 150,
            through: 2,
        },
        &id,
        &alice_key,
    );
    assert_eq!(
        hub.run(bob, &bob_key, MemberOperation::Redeem(total)),
        Outcome::Applied {
            fee: OP_FEE,
            detail: Detail::Paid(50)
        }
    );
    assert_eq!(hub.ledger.hold(hold).unwrap().pending.len(), 1);
    let released = release(&hub, &bob_key, hold, 1);
    assert_eq!(
        hub.run(alice, &alice_key, released.clone()),
        Outcome::Failed {
            fee: OP_FEE,
            reason: Failure::Missing
        }
    );
    after_notice(&mut hub);
    assert_eq!(hub.balance(bob), before - OP_FEE + 150);
    let hold = hub.ledger.hold(hold).unwrap();
    assert!(hold.pending.is_empty() && hold.claimed.is_empty());
    assert_eq!(hold.amount, 1_000_000 - 150);
}

// A hold's waiting claims pay together, the hold's notice after the last
// claim that added to them: each waits at least that long for its payer's
// dispute.
#[test]
fn a_later_claim_moves_the_payout_of_what_waits() {
    let mut hub = Hub::new();
    let (_, alice_key, bob, bob_key, hold) = hold_for_bob(&mut hub);
    let first = claim(&hub, &alice_key, hold, 1, 100);
    hub.run(bob, &bob_key, first);
    let due = hub.ledger.hold(hold).unwrap().pending_due;
    let second = claim(&hub, &alice_key, hold, 2, 100);
    hub.run(bob, &bob_key, second);
    let later = hub.ledger.hold(hold).unwrap().pending_due;
    assert!(later > due);
    let before = hub.balance(bob);
    hub.block_at(due, vec![], &[]);
    assert_eq!(hub.balance(bob), before);
    hub.block_at(later, vec![], &[]);
    assert_eq!(hub.balance(bob), before + 200);
}

// A hold whose claims wait for their payout keeps them: its payee cannot
// give it back, and its payer's reduction waits for them.
#[test]
fn a_hold_with_waiting_claims_stays_open() {
    let mut hub = Hub::new();
    let (alice, alice_key, bob, bob_key, hold) = hold_for_bob(&mut hub);
    let op = claim(&hub, &alice_key, hold, 1, 100);
    hub.run(bob, &bob_key, op);
    assert_eq!(
        hub.run(bob, &bob_key, MemberOperation::HoldRelease { hold }),
        Outcome::Failed {
            fee: OP_FEE,
            reason: Failure::Busy
        }
    );
    hub.run(
        alice,
        &alice_key,
        MemberOperation::HoldReduce {
            hold,
            amount: 1_000_000,
        },
    );
    let reduction = hub.ledger.hold(hold).unwrap().reduction.unwrap();
    let due = hub.ledger.hold(hold).unwrap().pending_due;
    assert!(reduction.at > due, "the reduction came later");
    // The claim pays first; the reduction then closes the hold.
    hub.block_at(due, vec![], &[]);
    assert!(hub.ledger.hold(hold).is_some());
    hub.block_at(reduction.at, vec![], &[]);
    assert!(hub.ledger.hold(hold).is_none());
}

// A hub of version 1 has no dispute: no block of it holds one, and the
// ledger refuses one put to it, so validators that run a release from
// before version 2 are never handed one.
#[test]
fn a_hub_of_version_1_takes_no_dispute() {
    let mut hub = Hub::with(|rules| rules.terms.version = 1);
    let (alice, alice_key, bob, bob_key, hold) = hold_for_bob(&mut hub);
    let op = claim(&hub, &alice_key, hold, 1, 100);
    assert_eq!(
        hub.run(bob, &bob_key, op),
        Outcome::Applied {
            fee: OP_FEE,
            detail: Detail::Paid(100)
        }
    );
    let released = release(&hub, &bob_key, hold, 1);
    let dispute = hub.member(alice, &alice_key, released);
    let block = Block {
        height: hub.ledger.height() + 1,
        timestamp: hub.ledger.timestamp() + 10,
        operations: core::slice::from_ref(&dispute),
        votes: &[],
    };
    assert_eq!(hub.ledger.check_block(&block), Err(BlockError::Version));
    let Operation::Member(signed) = dispute else {
        panic!("a member's operation");
    };
    assert_eq!(
        hub.ledger.apply_member(&signed),
        Outcome::Refused(Refusal::NotAllowed)
    );
}

/// A quorum's votes to move `hub` to `version`.
fn vote(hub: &mut Hub, voters: core::ops::Range<usize>, version: u16) -> Vec<Outcome> {
    let ops = voters
        .map(|i| hub.validator_op(i, ValidatorOperation::Upgrade { version }))
        .collect();
    hub.block(ops)
}

#[test]
fn a_quorum_moves_a_hub_to_version_2_after_the_notice() {
    let mut hub = Hub::with(|rules| rules.terms.version = 1);
    let (_, alice_key, bob, bob_key, hold) = hold_for_bob(&mut hub);
    // A hub of version 1 has the state root it had before version 2: no
    // leaf or byte of it changes until a validator votes.
    let root = hub.ledger.state_root();
    assert!(!hub.ledger.has_protocol_state());
    vote(&mut hub, 0..2, 2);
    assert_ne!(hub.ledger.state_root(), root, "the votes are state");
    assert_eq!(hub.ledger.pending_upgrade(), None, "two of four");
    // A vote for the version it runs is stale.
    assert_eq!(
        vote(&mut hub, 2..3, 1),
        vec![Outcome::Failed {
            fee: 0,
            reason: Failure::Stale
        }]
    );
    vote(&mut hub, 2..3, 2);
    let upgrade = hub.ledger.pending_upgrade().unwrap();
    assert_eq!(upgrade.version, 2);
    assert_eq!(
        upgrade.at,
        hub.ledger.timestamp() + hub.ledger.terms().timers.reconfigure_notice
    );
    // Until then claims pay at once; from the block it falls due in, they
    // wait.
    let op = claim(&hub, &alice_key, hold, 1, 100);
    assert!(matches!(
        hub.run(bob, &bob_key, op),
        Outcome::Applied {
            detail: Detail::Paid(100),
            ..
        }
    ));
    let next = claim_until(&hub, &alice_key, hold, 2, 100, upgrade.at + 10_000);
    let op = hub.member(bob, &bob_key, next);
    assert_eq!(
        hub.ledger.version_at(upgrade.at),
        2,
        "a block at the notice runs version 2"
    );
    hub.block_at(upgrade.at, vec![op], &[]);
    assert_eq!(hub.ledger.version(), 2);
    assert_eq!(hub.ledger.pending_upgrade(), None);
    assert_eq!(hub.ledger.hold(hold).unwrap().pending.len(), 1);
    // The hub keeps its ID and its rules.
    assert_eq!(hub.ledger.rules().terms.version, 1);
    assert_eq!(
        hub.ledger.hub(),
        &resigned(rules(4).0, |r| r.terms.version = 1).id()
    );
    // Snapshots carry the version and what waits.
    let restored = Ledger::restore(&hub.ledger.snapshot(), &hub.ledger.state_root()).unwrap();
    assert_eq!(restored, hub.ledger);
}

// spec/decisions/batched-validator-messages.md: version 3 changes nothing a
// ledger executes, and a hub of version 1 or 2 moves to it as to any
// version, keeping its rules and its ID.
#[test]
fn hubs_of_versions_1_and_2_move_to_version_3() {
    for from in [1, 2] {
        let mut hub = Hub::with(|rules| rules.terms.version = from);
        let id = *hub.ledger.hub();
        vote(&mut hub, 0..3, crate::rules::BATCHES_VERSION);
        let at = hub.ledger.pending_upgrade().unwrap().at;
        hub.block_at(at, vec![], &[]);
        assert_eq!(hub.ledger.version(), 3);
        assert_eq!(hub.ledger.rules().terms.version, from);
        assert_eq!(hub.ledger.hub(), &id);
        let restored = Ledger::restore(&hub.ledger.snapshot(), &hub.ledger.state_root()).unwrap();
        assert_eq!(restored, hub.ledger);
    }
}

// New rules after the move name the version the hub runs.
#[test]
fn new_rules_name_the_version_the_hub_runs() {
    let mut hub = Hub::with(|rules| rules.terms.version = 1);
    vote(&mut hub, 0..3, 2);
    let at = hub.ledger.pending_upgrade().unwrap().at;
    hub.block_at(at, vec![], &[]);
    let stale = resigned(hub.ledger.rules().clone(), |r| r.terms.fees.operation = 20);
    assert_eq!(
        super::tests::reconfigure(&mut hub, &stale)[0],
        Outcome::Failed {
            fee: 0,
            reason: Failure::NotYours
        }
    );
    let current = resigned(hub.ledger.rules().clone(), |r| {
        r.terms.fees.operation = 20;
        r.terms.version = 2;
    });
    super::tests::reconfigure(&mut hub, &current);
    assert_eq!(hub.ledger.rules().terms.version, 2);
    assert_eq!(hub.ledger.version(), 2);
    // The rules name the version now, so the state needs no word of it.
    assert!(!hub.ledger.has_protocol_header());
}

// Every version counts the votes alike, so validators of different
// releases agree on them; a version this code does not run stops it at the
// block the move falls due in, as it would a validator of an older release.
#[test]
fn a_move_to_a_version_this_code_does_not_run_stops_it_there() {
    let mut hub = Hub::new();
    let later = crate::rules::HUB_VERSION + 1;
    vote(&mut hub, 0..3, later);
    let at = hub.ledger.pending_upgrade().unwrap().at;
    hub.block_at(at - 1, vec![], &[]);
    let block = Block {
        height: hub.ledger.height() + 1,
        timestamp: at,
        operations: &[],
        votes: &[],
    };
    assert_eq!(hub.ledger.check_block(&block), Err(BlockError::Version));
}

// New rules name the version the hub runs, so none are voted while a move
// waits, and no move while new rules wait.
#[test]
fn a_move_and_new_rules_never_wait_together() {
    let mut hub = Hub::with(|rules| rules.terms.version = 1);
    vote(&mut hub, 0..3, 2);
    let rules = resigned(hub.ledger.rules().clone(), |r| r.terms.fees.operation = 20);
    assert_eq!(
        super::tests::reconfigure(&mut hub, &rules)[0],
        Outcome::Failed {
            fee: 0,
            reason: Failure::NotYours
        }
    );
    let mut hub = Hub::with(|rules| rules.terms.version = 1);
    let ops = (0..3)
        .map(|i| hub.validator_op(i, ValidatorOperation::Reconfigure(rules.clone())))
        .collect();
    hub.block(ops);
    assert!(hub.ledger.pending_rules().is_some());
    assert_eq!(
        vote(&mut hub, 0..1, 2),
        vec![Outcome::Failed {
            fee: 0,
            reason: Failure::Stale
        }]
    );
}

// A change of validators starts the votes afresh, as it does every vote.
#[test]
fn votes_to_move_start_again_with_a_new_set() {
    let mut hub = Hub::with(|rules| rules.terms.version = 1);
    vote(&mut hub, 0..2, 2);
    let rules = resigned(hub.ledger.rules().clone(), |r| r.terms.fees.operation = 20);
    super::tests::reconfigure(&mut hub, &rules);
    vote(&mut hub, 2..3, 2);
    assert_eq!(hub.ledger.pending_upgrade(), None);
}
