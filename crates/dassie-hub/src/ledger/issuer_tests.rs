//! Scenarios of what a token hub's issuer does (spec/decisions/token-hubs.md):
//! freezes and transfer fees that freeze the hub, the hub's clocks standing
//! still while it is frozen, and clawbacks that end it with a payout pro
//! rata. Every block asserts the invariants.

use alloc::vec;
use alloc::vec::Vec;

use super::tests::{Hub, OP_FEE, commitment, hold_for_bob};
use super::*;
use crate::consensus::Config;
use crate::escrow::payout::{PayoutTransaction, payout_set, shares};
use crate::operation::{Destination, MemberOperation, ValidatorOperation};
use crate::promise::{Fulfilled, Signed};
use crate::rules::MAX_HOLD_MS;
use crate::rules::tests::token_terms;

fn token_hub() -> Hub {
    Hub::with(|rules| rules.terms = token_terms(rules.terms.clone()))
}

fn issuer(ledger: u32, frozen: bool, fee: bool) -> ValidatorOperation {
    ValidatorOperation::AttestIssuer(Issuer {
        ledger,
        frozen,
        fee,
    })
}

fn clawback(byte: u8, ledger: u32, amount: Amount) -> ValidatorOperation {
    ValidatorOperation::AttestClawback(Clawback {
        transaction: Digest([byte; 32]),
        ledger,
        amount,
    })
}

/// The operations of validators `who`, each attesting `operation`.
fn ops(
    hub: &mut Hub,
    who: core::ops::Range<usize>,
    operation: &ValidatorOperation,
) -> Vec<Operation> {
    who.map(|i| hub.validator_op(i, operation.clone()))
        .collect()
}

/// Validators `who` attest `operation` in a block of their own.
fn attest(
    hub: &mut Hub,
    who: core::ops::Range<usize>,
    operation: &ValidatorOperation,
) -> Vec<Outcome> {
    let ops = ops(hub, who, operation);
    hub.block(ops)
}

const APPLIED: Outcome = Outcome::Applied {
    fee: 0,
    detail: Detail::None,
};

const TO: Destination = Destination {
    account: [7; 20],
    tag: Some(1),
};

// spec/decisions/token-hubs.md: only a token's issuer can freeze or claw
// back, so an XRP hub takes neither.
#[test]
fn an_xrp_hub_takes_no_issuers_act() {
    let mut hub = Hub::new();
    for operation in [issuer(5, true, false), clawback(1, 5, 1_000)] {
        let outcomes = hub.all_validators(|_| operation.clone());
        assert!(
            outcomes
                .iter()
                .all(|o| *o == Outcome::Refused(Refusal::NotAllowed))
        );
    }
    assert_eq!(hub.ledger.frozen(), None);
    assert_eq!(hub.ledger.phase(), Phase::Running);
}

// A freeze takes effect once a quorum attested it, and the frozen hub
// takes no member's operation but claims of commitments that expire within
// the freeze's window: the rest is refused, paying nothing and using no
// sequence, and goes through after the thaw.
#[test]
fn a_frozen_hub_takes_only_claims_within_the_window() {
    let mut hub = token_hub();
    let (alice, alice_key, bob, bob_key, hold) = hold_for_bob(&mut hub);
    let id = *hub.ledger.hub();
    attest(&mut hub, 0..2, &issuer(10, true, false));
    assert_eq!(hub.ledger.frozen(), None, "two of four are no quorum");
    attest(&mut hub, 2..3, &issuer(10, true, false));
    let since = hub.ledger.timestamp();
    assert_eq!(hub.ledger.frozen(), Some(since));
    let end = hub.ledger.freeze_window_end().unwrap();
    let margin = hub.ledger.terms().timers.margin;
    assert_eq!(
        end,
        since + margin + MAX_HOLD_MS + Config::DEFAULT.precision_ms
    );
    let transfer = MemberOperation::Transfer { to: bob, amount: 1 };
    let before = hub.ledger.account(alice).cloned();
    assert_eq!(
        hub.run(alice, &alice_key, transfer.clone()),
        Outcome::Refused(Refusal::Frozen)
    );
    assert_eq!(
        hub.ledger.account(alice).cloned(),
        before,
        "no fee, no sequence"
    );
    let claim = |sequence, expiry, preimage| {
        let (c, f) = commitment(hold, sequence, 100, expiry, preimage);
        MemberOperation::Claim {
            hold,
            fulfilled: vec![Fulfilled {
                commitment: Signed::sign(c, &id, &alice_key),
                fulfillment: f,
            }],
        }
    };
    assert_eq!(
        hub.run(bob, &bob_key, claim(2, end + 1, 2)),
        Outcome::Refused(Refusal::Frozen),
        "a commitment beyond the window waits"
    );
    assert_eq!(
        hub.run(bob, &bob_key, claim(1, end, 1)),
        Outcome::Applied {
            fee: OP_FEE,
            detail: Detail::Pending(100)
        }
    );
    // The claim waits for the hold's notice by the hub's clock, which
    // stands still while frozen: the thaw moves its payout back by the
    // freeze's length (spec/decisions/signed-rejects.md).
    let notice = hub.ledger.terms().timers.hold_notice;
    let due = hub.ledger.hold(hold).unwrap().pending_due;
    assert_eq!(due, since + notice);
    attest(&mut hub, 0..3, &issuer(11, false, false));
    assert_eq!(hub.ledger.frozen(), None);
    let thawed = hub.ledger.timestamp();
    assert_eq!(
        hub.ledger.hold(hold).unwrap().pending_due,
        due + (thawed - since)
    );
    assert!(matches!(
        hub.run(alice, &alice_key, transfer),
        Outcome::Applied { .. }
    ));
    assert_eq!(
        hub.run(bob, &bob_key, claim(2, end + 1, 2)),
        Outcome::Applied {
            fee: OP_FEE,
            detail: Detail::Pending(100)
        }
    );
}

// A transfer fee the issuer sets freezes the hub as a freeze does, until
// it is removed.
#[test]
fn a_transfer_fee_freezes_the_hub() {
    let mut hub = token_hub();
    attest(&mut hub, 0..3, &issuer(5, false, true));
    assert!(hub.ledger.frozen().is_some());
    // A freeze on top keeps it frozen from the first block.
    let since = hub.ledger.frozen();
    attest(&mut hub, 0..3, &issuer(6, true, true));
    assert_eq!(hub.ledger.frozen(), since);
    attest(&mut hub, 0..3, &issuer(7, true, false));
    assert_eq!(hub.ledger.frozen(), since);
    attest(&mut hub, 0..3, &issuer(8, false, false));
    assert_eq!(hub.ledger.frozen(), None);
}

// The state in force is replaced only by a newer one: a late vote for an
// older state is stale, a vote for a state the hub moved past is dropped,
// and a faulty validator's made-up state needs a quorum.
#[test]
fn late_votes_never_take_the_hub_back() {
    let mut hub = token_hub();
    attest(&mut hub, 0..3, &issuer(10, true, false));
    attest(&mut hub, 0..3, &issuer(12, false, false));
    assert_eq!(hub.ledger.frozen(), None);
    assert_eq!(
        attest(&mut hub, 3..4, &issuer(10, true, false)),
        vec![Outcome::Failed {
            fee: 0,
            reason: Failure::Stale
        }]
    );
    assert_eq!(
        attest(&mut hub, 3..4, &issuer(99, true, false)),
        vec![APPLIED]
    );
    assert_eq!(hub.ledger.frozen(), None, "one validator is no quorum");
    attest(&mut hub, 0..1, &issuer(13, true, false));
    // A validator's vote only moves forward: its late vote for an older
    // state is stale.
    attest(&mut hub, 1..2, &issuer(15, true, false));
    assert_eq!(
        attest(&mut hub, 1..2, &issuer(14, false, false)),
        vec![Outcome::Failed {
            fee: 0,
            reason: Failure::Stale
        }]
    );
    attest(&mut hub, 0..3, &issuer(16, false, false));
    assert_eq!(hub.ledger.issuer().ledger, 16);
    assert_eq!(
        hub.ledger.issuer_votes,
        vec![(
            Issuer {
                ledger: 99,
                frozen: true,
                fee: false
            },
            Signers::NONE.with(crate::validators::ValidatorIndex::new(3).unwrap())
        )],
        "votes for older states dropped, the made-up one kept"
    );
    assert_eq!(hub.ledger.frozen(), None);
}

// Deposits and withdrawal outcomes a quorum attested during a freeze are
// kept, and take effect on the thaw.
#[test]
fn deposits_and_withdrawals_wait_for_the_thaw() {
    let mut hub = token_hub();
    let (alice, key) = hub.join(1, 10_000_000);
    let Outcome::Applied {
        detail: Detail::Withdrawal(sequence),
        ..
    } = hub.run(
        alice,
        &key,
        MemberOperation::Withdraw {
            amount: 1_000_000,
            to: TO,
        },
    )
    else {
        panic!("a withdrawal");
    };
    attest(&mut hub, 0..3, &issuer(10, true, false));
    let deposited = hub.ledger.account(alice).unwrap().deposited;
    hub.deposit(alice.0, None, 3_000_000);
    attest(
        &mut hub,
        0..4,
        &ValidatorOperation::AttestWithdrawal {
            escrow_sequence: sequence,
            delivered: true,
        },
    );
    assert_eq!(hub.ledger.account(alice).unwrap().deposited, deposited);
    assert_eq!(hub.ledger.pending_deposits.len(), 1);
    assert_eq!(hub.ledger.withdrawals().count(), 1);
    attest(&mut hub, 0..3, &issuer(11, false, false));
    assert_eq!(
        hub.ledger.account(alice).unwrap().deposited,
        deposited + 3_000_000
    );
    assert_eq!(hub.ledger.withdrawals().count(), 0);
}

// No notice runs while the hub is frozen: a reduction pending at a freeze
// falls due only after the thaw, its notice moved by the freeze's length.
#[test]
fn a_reduction_pending_at_a_freeze_falls_due_after_the_thaw() {
    let mut hub = token_hub();
    let (alice, alice_key, _, _, hold) = hold_for_bob(&mut hub);
    hub.run(
        alice,
        &alice_key,
        MemberOperation::HoldReduce {
            hold,
            amount: 400_000,
        },
    );
    let at = hub.ledger.hold(hold).unwrap().reduction.unwrap().at;
    let since = at - 1_000;
    let freeze = ops(&mut hub, 0..3, &issuer(10, true, false));
    hub.block_at(since, freeze, &[]);
    hub.block_at(at + 50_000, vec![], &[]);
    assert_eq!(hub.ledger.hold(hold).unwrap().amount, 1_000_000);
    let thawed = at + 100_000;
    let thaw = ops(&mut hub, 0..3, &issuer(11, false, false));
    hub.block_at(thawed, thaw, &[]);
    let moved = hub.ledger.hold(hold).unwrap().reduction.unwrap().at;
    assert_eq!(moved, at + (thawed - since));
    assert_eq!(moved - thawed, 1_000, "what was left of the notice");
    hub.block_at(moved - 1, vec![], &[]);
    assert_eq!(hub.ledger.hold(hold).unwrap().amount, 1_000_000);
    hub.block_at(moved, vec![], &[]);
    assert_eq!(hub.ledger.hold(hold).unwrap().amount, 600_000);
}

// A validator's absence does not count while the hub is frozen, however
// long the freeze: after the thaw it runs for what was left of it.
#[test]
fn absence_does_not_count_while_frozen() {
    let mut hub = token_hub();
    let absence = hub.ledger.terms().timers.absence;
    hub.block(vec![]);
    let recorded = hub.ledger.timestamp();
    let since = recorded + 1_000;
    let freeze = ops(&mut hub, 0..3, &issuer(10, true, false));
    hub.block_at(since, freeze, &[]);
    hub.block_at(since + 4 * absence, vec![], &[]);
    assert_eq!(hub.ledger.phase(), Phase::Running);
    let thawed = since + 4 * absence + 10;
    let thaw = ops(&mut hub, 0..3, &issuer(11, false, false));
    hub.block_at(thawed, thaw, &[]);
    let due = thawed + absence - (since - recorded);
    hub.block_at(due, vec![], &[]);
    assert_eq!(hub.ledger.phase(), Phase::Running);
    hub.block_at(due + 1, vec![], &[]);
    assert!(matches!(hub.ledger.phase(), Phase::ShuttingDown { .. }));
}

// A clawback ends the hub: it shuts down, what was taken is counted once
// and kept apart from what the hub owes, and the payout set pays the
// withdrawals already sequenced in full and every account pro rata to what
// the hub owes it, rounded down, out of what the escrow still holds.
#[test]
fn a_clawback_ends_the_hub_with_a_payout_pro_rata() {
    let mut hub = token_hub();
    let (alice, alice_key) = hub.join(1, 10_000_000);
    let (bob, _) = hub.join(2, 30_000_003);
    hub.run(
        alice,
        &alice_key,
        MemberOperation::Withdraw {
            amount: 2_000_000,
            to: TO,
        },
    );
    let supply = hub.ledger.supply();
    let taken = 19_000_001;
    attest(&mut hub, 0..2, &clawback(0xcb, 50, taken));
    assert_eq!(hub.ledger.phase(), Phase::Running);
    attest(&mut hub, 2..3, &clawback(0xcb, 50, taken));
    let Phase::ShuttingDown { ends } = hub.ledger.phase() else {
        panic!("a clawback shuts the hub down");
    };
    assert_eq!(
        ends,
        hub.ledger.timestamp() + hub.ledger.terms().timers.shutdown_notice
    );
    assert_eq!(hub.ledger.escrow().clawed_back, taken);
    assert_eq!(hub.ledger.supply(), supply, "the hub owes what it owed");
    // Counted once.
    assert_eq!(
        attest(&mut hub, 3..4, &clawback(0xcb, 50, taken)),
        vec![APPLIED]
    );
    assert_eq!(hub.ledger.escrow().clawed_back, taken);
    hub.block_at(ends, vec![], &[]);
    assert_eq!(hub.ledger.phase(), Phase::Ended);

    let (have, due) = shares(&hub.ledger);
    assert_eq!(due, hub.ledger.escrow().balance - 2_000_000);
    assert_eq!(have, hub.ledger.escrow().balance - taken - 2_000_000);
    let set = payout_set(&hub.ledger).unwrap();
    let terms = hub.ledger.terms().clone();
    let PayoutTransaction::Withdrawal(first) = &set.transactions[0] else {
        panic!("the sequenced withdrawal first");
    };
    assert_eq!(terms.hub_amount(&first.amount), Some(2_000_000));
    let paid: Vec<Amount> = set
        .transactions
        .iter()
        .filter_map(|t| match t {
            PayoutTransaction::Payout(p) => terms.hub_amount(&p.amount),
            _ => None,
        })
        .collect();
    let owed = |number| {
        let a = hub.ledger.account(number).unwrap();
        u128::from(a.balance + a.reserve)
    };
    let share =
        |number| Amount::try_from(owed(number) * u128::from(have) / u128::from(due)).unwrap();
    // The validators' accounts, which hold their fees, have no disbursal
    // address, and get no payout: their shares stay in the escrow.
    assert_eq!(paid, vec![share(alice), share(bob)]);
    assert!(paid.iter().sum::<Amount>() <= have);
    assert!(u128::from(share(bob)) < owed(bob));
}

// A clawback during a freeze shuts the hub down and keeps it frozen: the
// shutdown's notice runs only after the thaw, so the hub never ends while
// frozen, and no payout set is signed until then.
#[test]
fn a_clawback_during_a_freeze_ends_the_hub_after_the_thaw() {
    let mut hub = token_hub();
    hub.join(1, 10_000_000);
    attest(&mut hub, 0..3, &issuer(10, true, false));
    let since = hub.ledger.frozen().unwrap();
    let claw = ops(&mut hub, 0..3, &clawback(1, 11, 5_000_000));
    hub.block_at(since + 1_000, claw, &[]);
    let notice = hub.ledger.terms().timers.shutdown_notice;
    assert_eq!(
        hub.ledger.phase(),
        Phase::ShuttingDown {
            ends: since + notice
        }
    );
    hub.block_at(since + 10 * notice, vec![], &[]);
    assert!(matches!(hub.ledger.phase(), Phase::ShuttingDown { .. }));
    assert_eq!(hub.ledger.frozen(), Some(since));
    let thawed = since + 10 * notice + 7;
    let thaw = ops(&mut hub, 0..3, &issuer(12, false, false));
    hub.block_at(thawed, thaw, &[]);
    assert_eq!(
        hub.ledger.phase(),
        Phase::ShuttingDown {
            ends: thawed + notice
        }
    );
    hub.block_at(thawed + notice - 1, vec![], &[]);
    assert!(matches!(hub.ledger.phase(), Phase::ShuttingDown { .. }));
    hub.block_at(thawed + notice, vec![], &[]);
    assert_eq!(hub.ledger.phase(), Phase::Ended);
}

// A quorum's freeze that lands in the block in which the shutdown's notice
// ends finds the hub ended: nothing a block holds stops the ledger, the
// hold is recorded, so the payout's signers see the escrow held, and the
// hub, which has no clock left to stop and takes no further block, is not
// frozen. A fee and a clawback in that block do the same.
#[test]
fn a_hold_in_the_block_the_hub_ends_in_is_recorded_but_freezes_nothing() {
    let mut hub = token_hub();
    hub.join(1, 10_000_000);
    hub.all_validators(|_| ValidatorOperation::Shutdown);
    let Phase::ShuttingDown { ends } = hub.ledger.phase() else {
        panic!("a quorum's votes shut the hub down");
    };
    let mut last = ops(&mut hub, 0..3, &issuer(10, true, true));
    last.extend(ops(&mut hub, 0..3, &clawback(1, 11, 1_000_000)));
    let outcomes = hub.block_at(ends, last, &[]);
    assert!(outcomes.iter().all(|o| *o == APPLIED));
    assert_eq!(hub.ledger.phase(), Phase::Ended);
    assert_eq!(hub.ledger.frozen(), None);
    assert!(hub.ledger.issuer().holds());
    assert_eq!(hub.ledger.escrow().clawed_back, 1_000_000);
}

// A faulty validator's made-up clawbacks fill only its own share of the
// pending ones, and a quorum's clawback still applies.
#[test]
fn one_validators_made_up_clawbacks_fill_only_its_share() {
    let mut hub = token_hub();
    for byte in 0..CLAWBACK_SHARE {
        let made_up = clawback(u8::try_from(byte).unwrap() + 100, 5, 1);
        assert_eq!(attest(&mut hub, 3..4, &made_up), vec![APPLIED]);
    }
    assert_eq!(
        attest(&mut hub, 3..4, &clawback(99, 5, 1)),
        vec![Outcome::Failed {
            fee: 0,
            reason: Failure::Full
        }]
    );
    attest(&mut hub, 0..3, &clawback(1, 6, 7));
    assert_eq!(hub.ledger.escrow().clawed_back, 7);
    assert!(matches!(hub.ledger.phase(), Phase::ShuttingDown { .. }));
}

// The frozen state, the votes and the clawbacks are in the state root and
// the snapshot.
#[test]
fn a_frozen_hub_restores_from_its_snapshot() {
    let mut hub = token_hub();
    hub.join(1, 10_000_000);
    let root = hub.ledger.state_root();
    attest(&mut hub, 0..3, &issuer(10, true, false));
    assert_ne!(hub.ledger.state_root(), root);
    attest(&mut hub, 0..1, &issuer(11, false, false));
    attest(&mut hub, 0..3, &clawback(1, 12, 5));
    attest(&mut hub, 1..2, &clawback(2, 13, 6));
    let ledger = &hub.ledger;
    let restored = Ledger::restore(&ledger.snapshot(), &ledger.state_root()).unwrap();
    assert_eq!(&restored, ledger);
    assert!(restored.frozen().is_some());
    assert_eq!(restored.escrow().clawed_back, 5);
}

// A reconfiguration's notice stands still during a freeze too, and new
// rules voted in while frozen wait for their whole notice after the thaw.
#[test]
fn a_reconfiguration_waits_out_a_freeze() {
    let mut hub = token_hub();
    let mut rules = hub.ledger.rules().clone();
    rules.terms.fees.operation = 20;
    let rules = super::tests::resigned(rules, |_| {});
    let votes = ops(&mut hub, 0..3, &ValidatorOperation::Reconfigure(rules));
    hub.block(votes);
    let at = hub.ledger.pending_rules().unwrap().at;
    let since = at - 5_000;
    let freeze = ops(&mut hub, 0..3, &issuer(10, true, false));
    hub.block_at(since, freeze, &[]);
    hub.block_at(at + 1_000, vec![], &[]);
    assert_eq!(hub.ledger.pending_rules().unwrap().from_height, None);
    let thawed = at + 2 * 86_400_000;
    let thaw = ops(&mut hub, 0..3, &issuer(11, false, false));
    hub.block_at(thawed, thaw, &[]);
    assert_eq!(hub.ledger.pending_rules().unwrap().at, thawed + 5_000);
    hub.block_at(thawed + 4_999, vec![], &[]);
    assert_eq!(hub.ledger.pending_rules().unwrap().from_height, None);
    hub.block_at(thawed + 5_000, vec![], &[]);
    assert!(hub.ledger.pending_rules().unwrap().from_height.is_some());
}

// A receipt says an operation waits for the thaw, and a member reads it
// back as such.
#[test]
fn a_waiting_operations_refusal_reads_back() {
    use dassie_oer::{Encode, Reader};
    let outcome = Outcome::Refused(Refusal::Frozen);
    let bytes = outcome.to_vec();
    let mut reader = Reader::new(&bytes);
    assert_eq!(Outcome::decode(&mut reader).unwrap(), outcome);
    reader.finish().unwrap();
}
