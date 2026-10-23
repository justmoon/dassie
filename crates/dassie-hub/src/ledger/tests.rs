//! Scenarios of the hub's ledger, block by block. Every block asserts the
//! invariants, so each scenario also checks that the books add up.

use alloc::vec;
use alloc::vec::Vec;

use dassie_identity::NodeKey;
use sha2::{Digest as _, Sha256};

use super::*;
use crate::operation::{
    CloseTo, Deposit, Destination, MemberOperation, SignedMemberOperation,
    SignedValidatorOperation, ValidatorOperation,
};
use crate::promise::{Claim, Commitment, Fulfilled, Signed};
use crate::rules::Rules;
use crate::rules::tests::rules;

const START: Millis = 1_000_000;
pub(crate) const OP_FEE: Amount = 10;

/// A ledger, the validators' keys, and what the next block needs.
pub(crate) struct Hub {
    pub(crate) ledger: Ledger,
    pub(crate) validators: Vec<NodeKey>,
    sequence: u64,
    deposits: u8,
}

impl Hub {
    pub(crate) fn new() -> Self {
        Self::with(|_| {})
    }

    /// A hub whose rules `change` changed.
    pub(crate) fn with(change: impl FnOnce(&mut Rules)) -> Self {
        Self::of(4, change)
    }

    /// A hub of `n` validators whose rules `change` changed.
    fn of(n: u8, change: impl FnOnce(&mut Rules)) -> Self {
        let (rules, validators) = rules(n);
        let rules = resigned(rules, change);
        let ledger = Ledger::genesis(rules, START).unwrap();
        let mut hub = Self {
            ledger,
            validators,
            sequence: 0,
            deposits: 0,
        };
        hub.all_validators(|_| ValidatorOperation::AttestEscrow { sequence: 100 });
        assert!(hub.ledger.escrow().checked);
        hub
    }

    /// Executes a block 10 ms after the last, recording every validator's
    /// vote for the last block.
    pub(crate) fn block(&mut self, operations: Vec<Operation>) -> Vec<Outcome> {
        let height = self.ledger.height() + 1;
        let votes: Vec<(u64, Signers)> = if height > 1 {
            vec![(height - 1, Signers::from_bits(0b1111))]
        } else {
            vec![]
        };
        let timestamp = self.ledger.timestamp() + 10;
        self.block_at(timestamp, operations, &votes)
    }

    pub(crate) fn block_at(
        &mut self,
        timestamp: Millis,
        operations: Vec<Operation>,
        votes: &[(u64, Signers)],
    ) -> Vec<Outcome> {
        let block = Block {
            height: self.ledger.height() + 1,
            timestamp,
            operations: &operations,
            votes,
        };
        self.ledger.apply(&block).unwrap()
    }

    pub(crate) fn validator_op(
        &mut self,
        index: usize,
        operation: ValidatorOperation,
    ) -> Operation {
        self.sequence += 1;
        Operation::Validator(SignedValidatorOperation::sign(
            self.ledger.hub(),
            ValidatorIndex::new(index).unwrap(),
            self.sequence,
            operation,
            &self.validators[index],
        ))
    }

    fn all_validators(&mut self, operation: impl Fn(usize) -> ValidatorOperation) -> Vec<Outcome> {
        let ops = (0..self.validators.len())
            .map(|i| self.validator_op(i, operation(i)))
            .collect();
        self.block(ops)
    }

    /// Deposits `amount` with destination tag `tag` (or a new account for
    /// `key`), attested by three validators.
    pub(crate) fn deposit(&mut self, tag: u32, key: Option<&NodeKey>, amount: Amount) {
        self.deposits += 1;
        let deposit = Deposit {
            transaction: Digest([self.deposits; 32]),
            ledger: 1_000 + u32::from(self.deposits),
            amount,
            sender: [0xaa; 20],
            tag,
            key: key.map(NodeKey::public_key),
        };
        let ops = (0..self.ledger.validators().quorum())
            .map(|i| self.validator_op(i, ValidatorOperation::AttestDeposit(deposit)))
            .collect();
        self.block(ops);
    }

    pub(crate) fn member(
        &self,
        account: AccountNumber,
        key: &NodeKey,
        operation: MemberOperation,
    ) -> Operation {
        let sequence = self.ledger.account(account).unwrap().sequence;
        Operation::Member(SignedMemberOperation::sign(
            self.ledger.hub(),
            account,
            sequence,
            OP_FEE,
            operation,
            key,
        ))
    }

    pub(crate) fn run(
        &mut self,
        account: AccountNumber,
        key: &NodeKey,
        operation: MemberOperation,
    ) -> Outcome {
        let op = self.member(account, key, operation);
        self.block(vec![op])[0]
    }

    /// A member with `amount` deposited and a disbursal address set.
    pub(crate) fn join(&mut self, seed: u8, amount: Amount) -> (AccountNumber, NodeKey) {
        let key = NodeKey::from_bytes(&[seed; 32]);
        self.deposit(0, Some(&key), amount);
        let number = self.ledger.account_of(&key.public_key()).unwrap();
        let to = Destination {
            account: [seed; 20],
            tag: None,
        };
        assert!(matches!(
            self.run(number, &key, MemberOperation::SetDisbursal(to)),
            Outcome::Applied { .. }
        ));
        (number, key)
    }

    pub(crate) fn balance(&self, account: AccountNumber) -> Amount {
        self.ledger.account(account).map_or(0, |a| a.balance)
    }
}

fn commitment(
    hold: HoldNumber,
    sequence: u64,
    amount: Amount,
    expiry: Millis,
    preimage: u8,
) -> (Commitment, [u8; 32]) {
    let fulfillment = [preimage; 32];
    (
        Commitment {
            hold,
            sequence,
            amount,
            condition: Sha256::digest(fulfillment).into(),
            expiry,
        },
        fulfillment,
    )
}

const RESERVE: Amount = 2_000_000;

#[test]
fn a_deposit_opens_an_account_and_locks_its_reserve() {
    let mut hub = Hub::new();
    let (alice, _) = hub.join(1, 10_000_000);
    let account = hub.ledger.account(alice).unwrap();
    assert_eq!(account.reserve, RESERVE);
    assert_eq!(account.balance, 10_000_000 - RESERVE - OP_FEE);
    assert_eq!(hub.ledger.supply(), 10_000_000);
}

#[test]
fn a_deposit_needs_a_quorum_and_counts_once() {
    let mut hub = Hub::new();
    let key = NodeKey::from_bytes(&[1; 32]);
    let deposit = Deposit {
        transaction: Digest([9; 32]),
        ledger: 5,
        amount: 5_000_000,
        sender: [1; 20],
        tag: 0,
        key: Some(key.public_key()),
    };
    let two: Vec<Operation> = (0..2)
        .map(|i| hub.validator_op(i, ValidatorOperation::AttestDeposit(deposit)))
        .collect();
    hub.block(two);
    assert!(hub.ledger.account_of(&key.public_key()).is_none());
    let rest: Vec<Operation> = (2..4)
        .map(|i| hub.validator_op(i, ValidatorOperation::AttestDeposit(deposit)))
        .collect();
    hub.block(rest);
    assert_eq!(hub.ledger.supply(), 5_000_000);
    // A late attestation of a credited deposit changes nothing.
    let again = hub.validator_op(0, ValidatorOperation::AttestDeposit(deposit));
    hub.block(vec![again]);
    assert_eq!(hub.ledger.supply(), 5_000_000);
}

#[test]
fn a_deposit_to_no_account_is_paid_back() {
    let mut hub = Hub::new();
    hub.deposit(4242, None, 3_000_000);
    let (_, withdrawal) = hub.ledger.withdrawals().next().unwrap();
    assert_eq!(withdrawal.amount, 3_000_000 - OP_FEE - 12);
    assert_eq!(withdrawal.to.account, [0xaa; 20]);
}

#[test]
fn operations_wait_for_a_disbursal_address() {
    let mut hub = Hub::new();
    let key = NodeKey::from_bytes(&[1; 32]);
    hub.deposit(0, Some(&key), 10_000_000);
    let alice = hub.ledger.account_of(&key.public_key()).unwrap();
    let outcome = hub.run(
        alice,
        &key,
        MemberOperation::Transfer {
            to: AccountNumber(1),
            amount: 1,
        },
    );
    assert_eq!(
        outcome,
        Outcome::Failed {
            fee: OP_FEE,
            reason: Failure::NoDisbursal
        }
    );
}

#[test]
fn replays_and_forgeries_are_refused() {
    let mut hub = Hub::new();
    let (alice, key) = hub.join(1, 10_000_000);
    let op = hub.member(
        alice,
        &key,
        MemberOperation::Transfer {
            to: AccountNumber(1),
            amount: 5,
        },
    );
    assert!(matches!(
        hub.block(vec![op.clone()])[0],
        Outcome::Applied { .. }
    ));
    assert_eq!(hub.block(vec![op])[0], Outcome::Refused(Refusal::Sequence));
    let mallory = NodeKey::from_bytes(&[66; 32]);
    let forged = hub.member(
        alice,
        &mallory,
        MemberOperation::Transfer {
            to: AccountNumber(1),
            amount: 5,
        },
    );
    assert_eq!(
        hub.block(vec![forged])[0],
        Outcome::Refused(Refusal::Signature)
    );
}

#[test]
fn pay_opens_the_payees_account() {
    let mut hub = Hub::new();
    let (alice, key) = hub.join(1, 10_000_000);
    let bob = NodeKey::from_bytes(&[2; 32]);
    let minimum = hub.ledger.terms().new_account_minimum();
    let too_little = MemberOperation::Pay {
        key: bob.public_key(),
        amount: minimum - 1,
    };
    assert_eq!(
        hub.run(alice, &key, too_little),
        Outcome::Failed {
            fee: OP_FEE,
            reason: Failure::TooSmall
        }
    );
    let outcome = hub.run(
        alice,
        &key,
        MemberOperation::Pay {
            key: bob.public_key(),
            amount: minimum,
        },
    );
    let Outcome::Applied {
        detail: Detail::Account(number),
        ..
    } = outcome
    else {
        panic!("{outcome:?}");
    };
    assert_eq!(hub.ledger.account(number).unwrap().reserve, RESERVE);
}

// The holds of an account are found by an index, kept in step as holds
// open and close, and rebuilt from the holds when a snapshot is restored.
#[test]
fn the_holds_of_an_account_are_indexed() {
    let mut hub = Hub::new();
    let (alice, alice_key, bob, bob_key, hold) = hold_for_bob(&mut hub);
    let of = |hub: &Hub, account| {
        hub.ledger
            .holds_of(account)
            .map(|(n, _)| n)
            .collect::<Vec<_>>()
    };
    assert_eq!(of(&hub, alice), [hold]);
    assert_eq!(of(&hub, bob), [hold]);
    let snapshot = hub.ledger.snapshot();
    let restored = Ledger::restore(&snapshot, &hub.ledger.state_root()).unwrap();
    assert_eq!(restored, hub.ledger, "the index rebuilt");
    hub.run(bob, &bob_key, MemberOperation::HoldRelease { hold });
    assert!(of(&hub, alice).is_empty() && of(&hub, bob).is_empty());
    let _ = alice_key;
}

/// Alice holds 1 XRP for Bob.
fn hold_for_bob(hub: &mut Hub) -> (AccountNumber, NodeKey, AccountNumber, NodeKey, HoldNumber) {
    let (alice, alice_key) = hub.join(1, 10_000_000);
    let (bob, bob_key) = hub.join(2, 5_000_000);
    let outcome = hub.run(
        alice,
        &alice_key,
        MemberOperation::HoldOpen {
            payee: bob,
            amount: 1_000_000,
        },
    );
    let Outcome::Applied {
        detail: Detail::Hold(hold),
        ..
    } = outcome
    else {
        panic!("{outcome:?}");
    };
    (alice, alice_key, bob, bob_key, hold)
}

#[test]
fn redeem_pays_what_each_claim_adds() {
    let mut hub = Hub::new();
    let (_, alice_key, bob, bob_key, hold) = hold_for_bob(&mut hub);
    let id = *hub.ledger.hub();
    let before = hub.balance(bob);
    let first = Signed::sign(
        Claim {
            hold,
            total: 300,
            through: 3,
        },
        &id,
        &alice_key,
    );
    assert_eq!(
        hub.run(bob, &bob_key, MemberOperation::Redeem(first)),
        Outcome::Applied {
            fee: OP_FEE,
            detail: Detail::Paid(300)
        }
    );
    let second = Signed::sign(
        Claim {
            hold,
            total: 500,
            through: 5,
        },
        &id,
        &alice_key,
    );
    hub.run(bob, &bob_key, MemberOperation::Redeem(second));
    assert_eq!(hub.balance(bob), before + 500 - 2 * OP_FEE);
    // An old claim pays nothing more.
    assert_eq!(
        hub.run(bob, &bob_key, MemberOperation::Redeem(first)),
        Outcome::Failed {
            fee: OP_FEE,
            reason: Failure::Claim
        }
    );
    assert_eq!(hub.ledger.hold(hold).unwrap().amount, 1_000_000 - 500);
}

#[test]
fn claim_pays_fulfilled_commitments_once_and_redeem_does_not_pay_them_again() {
    let mut hub = Hub::new();
    let (_, alice_key, bob, bob_key, hold) = hold_for_bob(&mut hub);
    let id = *hub.ledger.hub();
    let expiry = hub.ledger.timestamp() + 10_000;
    let (c1, f1) = commitment(hold, 1, 100, expiry, 1);
    let (c2, f2) = commitment(hold, 2, 200, expiry, 2);
    let fulfilled = vec![
        Fulfilled {
            commitment: Signed::sign(c1, &id, &alice_key),
            fulfillment: f1,
        },
        Fulfilled {
            commitment: Signed::sign(c2, &id, &alice_key),
            fulfillment: f2,
        },
    ];
    let claim = MemberOperation::Claim {
        hold,
        fulfilled: fulfilled.clone(),
    };
    assert_eq!(
        hub.run(bob, &bob_key, claim.clone()),
        Outcome::Applied {
            fee: OP_FEE,
            detail: Detail::Paid(300)
        }
    );
    // Claiming again pays nothing.
    assert_eq!(
        hub.run(bob, &bob_key, claim),
        Outcome::Failed {
            fee: OP_FEE,
            reason: Failure::Claim
        }
    );
    // Alice's claim covers both packets and a third; the redeem pays only
    // the third.
    let total = Signed::sign(
        Claim {
            hold,
            total: 350,
            through: 3,
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
    assert!(hub.ledger.hold(hold).unwrap().claimed.is_empty());
    assert_eq!(hub.ledger.hold(hold).unwrap().amount, 1_000_000 - 350);
}

#[test]
fn a_commitment_cannot_be_claimed_once_it_expired_or_without_its_fulfillment() {
    let mut hub = Hub::new();
    let (_, alice_key, bob, bob_key, hold) = hold_for_bob(&mut hub);
    let id = *hub.ledger.hub();
    let now = hub.ledger.timestamp();
    let (late, f) = commitment(hold, 1, 100, now + 5, 1);
    let expired = MemberOperation::Claim {
        hold,
        fulfilled: vec![Fulfilled {
            commitment: Signed::sign(late, &id, &alice_key),
            fulfillment: f,
        }],
    };
    assert_eq!(
        hub.run(bob, &bob_key, expired),
        Outcome::Failed {
            fee: OP_FEE,
            reason: Failure::Claim
        }
    );
    let (fine, _) = commitment(hold, 2, 100, now + 10_000, 2);
    let wrong = MemberOperation::Claim {
        hold,
        fulfilled: vec![Fulfilled {
            commitment: Signed::sign(fine, &id, &alice_key),
            fulfillment: [3; 32],
        }],
    };
    assert_eq!(
        hub.run(bob, &bob_key, wrong),
        Outcome::Failed {
            fee: OP_FEE,
            reason: Failure::Claim
        }
    );
    // Bob cannot sign Alice's commitments.
    let forged = MemberOperation::Claim {
        hold,
        fulfilled: vec![Fulfilled {
            commitment: Signed::sign(fine, &id, &bob_key),
            fulfillment: [2; 32],
        }],
    };
    assert_eq!(
        hub.run(bob, &bob_key, forged),
        Outcome::Failed {
            fee: OP_FEE,
            reason: Failure::Claim
        }
    );
}

#[test]
fn a_reduction_waits_for_the_notice_and_closes_an_empty_hold() {
    let mut hub = Hub::new();
    let (alice, alice_key, bob, _, hold) = hold_for_bob(&mut hub);
    let before = hub.balance(alice);
    hub.run(
        alice,
        &alice_key,
        MemberOperation::HoldReduce {
            hold,
            amount: 1_000_000,
        },
    );
    hub.block(vec![]);
    assert!(hub.ledger.hold(hold).is_some(), "reduced before its notice");
    let notice = hub.ledger.terms().timers.hold_notice;
    let at = hub.ledger.timestamp() + notice;
    hub.block_at(at, vec![], &[]);
    assert!(hub.ledger.hold(hold).is_none());
    assert_eq!(hub.balance(alice), before - OP_FEE + 1_000_000 + 200_000);
    assert_eq!(hub.ledger.account(bob).unwrap().holds, 0);
}

#[test]
fn a_withdrawal_settles_by_quorum_and_a_failed_one_comes_back() {
    let mut hub = Hub::new();
    let (alice, key) = hub.join(1, 10_000_000);
    let to = Destination {
        account: [7; 20],
        tag: Some(1),
    };
    let outcome = hub.run(
        alice,
        &key,
        MemberOperation::Withdraw {
            amount: 3_000_000,
            to,
        },
    );
    let Outcome::Applied {
        detail: Detail::Withdrawal(sequence),
        ..
    } = outcome
    else {
        panic!("{outcome:?}");
    };
    assert_eq!(sequence, 100);
    let before = hub.balance(alice);
    hub.all_validators(|_| ValidatorOperation::AttestWithdrawal {
        escrow_sequence: sequence,
        delivered: false,
    });
    assert_eq!(hub.balance(alice), before + 3_000_000);
    assert_eq!(hub.ledger.supply(), 10_000_000 - 12);
    let outcome = hub.run(
        alice,
        &key,
        MemberOperation::Withdraw {
            amount: 3_000_000,
            to,
        },
    );
    let Outcome::Applied {
        detail: Detail::Withdrawal(sequence),
        ..
    } = outcome
    else {
        panic!("{outcome:?}");
    };
    hub.all_validators(|_| ValidatorOperation::AttestWithdrawal {
        escrow_sequence: sequence,
        delivered: true,
    });
    assert_eq!(hub.ledger.supply(), 10_000_000 - 12 - 3_000_012);
    assert_eq!(hub.ledger.withdrawals().count(), 0);
}

#[test]
fn close_pays_out_the_reserve_and_ends_the_account() {
    let mut hub = Hub::new();
    let (alice, key) = hub.join(1, 10_000_000);
    let to = Destination {
        account: [7; 20],
        tag: None,
    };
    let outcome = hub.run(alice, &key, MemberOperation::Close(CloseTo::Ledger(to)));
    let Outcome::Applied {
        detail: Detail::Withdrawal(sequence),
        ..
    } = outcome
    else {
        panic!("{outcome:?}");
    };
    assert!(hub.ledger.account(alice).unwrap().closing);
    hub.all_validators(|_| ValidatorOperation::AttestWithdrawal {
        escrow_sequence: sequence,
        delivered: true,
    });
    assert!(hub.ledger.account(alice).is_none());
    assert!(hub.ledger.account_of(&key.public_key()).is_none());
}

#[test]
fn fees_go_to_the_validators_whose_votes_were_recorded() {
    let mut hub = Hub::new();
    let (alice, key) = hub.join(1, 10_000_000);
    let validator_balance = |hub: &Hub, i: u32| hub.balance(AccountNumber(i));
    let before: Vec<Amount> = (1..=4).map(|i| validator_balance(&hub, i)).collect();
    // A block with eight operations' fees, whose votes only 0 and 1 have
    // recorded.
    let ops: Vec<Operation> = (0..8)
        .map(|i| {
            let mut op = hub.member(
                alice,
                &key,
                MemberOperation::Transfer {
                    to: AccountNumber(1),
                    amount: 1,
                },
            );
            if let Operation::Member(signed) = &mut op {
                *signed = SignedMemberOperation::sign(
                    hub.ledger.hub(),
                    alice,
                    signed.sequence + i,
                    OP_FEE,
                    signed.operation.clone(),
                    &key,
                );
            }
            op
        })
        .collect();
    hub.block(ops);
    let height = hub.ledger.height();
    let t = hub.ledger.timestamp() + 10;
    hub.block_at(t, vec![], &[(height, Signers::from_bits(0b0011))]);
    for _ in 0..40 {
        let t = hub.ledger.timestamp() + 10;
        hub.block_at(t, vec![], &[]);
    }
    // 80 drops of fees, 10% to the reserve account: 36 each to 0 and 1.
    // Every validator also gets 2 of the 9 the join's last block left for
    // them, whose votes all four recorded.
    let after: Vec<Amount> = (1..=4).map(|i| validator_balance(&hub, i)).collect();
    assert_eq!(after[0] - before[0], 8 + 36 + 2);
    assert_eq!(after[1] - before[1], 36 + 2);
    assert_eq!(after[2] - before[2], 2);
    assert_eq!(after[3] - before[3], 2);
}

#[test]
fn a_vote_cannot_be_recorded_twice_or_outside_the_window() {
    let mut hub = Hub::new();
    hub.block(vec![]);
    let height = hub.ledger.height();
    hub.block_at(
        hub.ledger.timestamp() + 10,
        vec![],
        &[(height, Signers::from_bits(1))],
    );
    let check = |hub: &Hub, votes: &[(u64, Signers)]| {
        hub.ledger.check_block(&Block {
            height: hub.ledger.height() + 1,
            timestamp: hub.ledger.timestamp() + 10,
            operations: &[],
            votes,
        })
    };
    assert_eq!(
        check(&hub, &[(height, Signers::from_bits(1))]),
        Err(BlockError::Votes)
    );
    assert_eq!(
        check(&hub, &[(1_000, Signers::from_bits(1))]),
        Err(BlockError::Votes)
    );
    assert_eq!(
        check(&hub, &[(height, Signers::from_bits(1 << 5))]),
        Err(BlockError::Votes)
    );
    assert_eq!(check(&hub, &[(height, Signers::from_bits(2))]), Ok(()));
}

#[test]
fn blocks_keep_deadlines_first_and_within_bounds() {
    let mut hub = Hub::new();
    let (_, alice_key, bob, bob_key, hold) = hold_for_bob(&mut hub);
    let id = *hub.ledger.hub();
    let redeem = hub.member(
        bob,
        &bob_key,
        MemberOperation::Redeem(Signed::sign(
            Claim {
                hold,
                total: 1,
                through: 1,
            },
            &id,
            &alice_key,
        )),
    );
    let transfer = hub.member(
        bob,
        &bob_key,
        MemberOperation::Transfer {
            to: AccountNumber(1),
            amount: 1,
        },
    );
    let operations = [transfer, redeem];
    let block = Block {
        height: hub.ledger.height() + 1,
        timestamp: hub.ledger.timestamp() + 10,
        operations: &operations,
        votes: &[],
    };
    assert_eq!(hub.ledger.check_block(&block), Err(BlockError::Order));
}

#[test]
fn shutdown_needs_a_quorum_and_leaves_only_redeems_and_claims() {
    let mut hub = Hub::new();
    let (_, alice_key, bob, bob_key, hold) = hold_for_bob(&mut hub);
    let ops: Vec<Operation> = (0..2)
        .map(|i| hub.validator_op(i, ValidatorOperation::Shutdown))
        .collect();
    hub.block(ops);
    assert_eq!(hub.ledger.phase(), Phase::Running);
    let third = hub.validator_op(2, ValidatorOperation::Shutdown);
    hub.block(vec![third]);
    let Phase::ShuttingDown { ends } = hub.ledger.phase() else {
        panic!("not shutting down");
    };
    let transfer = MemberOperation::Transfer {
        to: AccountNumber(1),
        amount: 1,
    };
    assert_eq!(
        hub.run(bob, &bob_key, transfer),
        Outcome::Failed {
            fee: OP_FEE,
            reason: Failure::ShuttingDown
        }
    );
    let id = *hub.ledger.hub();
    let claim = Signed::sign(
        Claim {
            hold,
            total: 10,
            through: 1,
        },
        &id,
        &alice_key,
    );
    assert!(matches!(
        hub.run(bob, &bob_key, MemberOperation::Redeem(claim)),
        Outcome::Applied { .. }
    ));
    hub.block_at(ends, vec![], &[]);
    assert_eq!(hub.ledger.phase(), Phase::Ended);
}

#[test]
fn an_absent_validator_shuts_the_hub_down() {
    let mut hub = Hub::new();
    let absence = hub.ledger.terms().timers.absence;
    // Validators 0–2 keep voting; 3 never does.
    let mut t = hub.ledger.timestamp();
    while t < START + absence + 1_000 && hub.ledger.phase() == Phase::Running {
        t += 3_600_000;
        let height = hub.ledger.height();
        hub.block_at(t, vec![], &[(height, Signers::from_bits(0b0111))]);
    }
    assert!(matches!(hub.ledger.phase(), Phase::ShuttingDown { .. }));
}

#[test]
fn a_reset_needs_a_quorum_and_writes_everything_off() {
    let mut hub = Hub::new();
    let (alice, _, _, _, _) = hold_for_bob(&mut hub);
    let one = hub.validator_op(0, ValidatorOperation::AttestReset { ledger: 1 });
    hub.block(vec![one]);
    assert!(hub.ledger.supply() > 0, "one validator reset the hub");
    let ops: Vec<Operation> = (1..3)
        .map(|i| hub.validator_op(i, ValidatorOperation::AttestReset { ledger: 1 }))
        .collect();
    hub.block(ops);
    assert_eq!(hub.ledger.supply(), 0);
    assert!(!hub.ledger.escrow().checked);
    assert_eq!(hub.ledger.holds().count(), 0);
    // Alice's account stays, and locks its reserve again from her next
    // deposit, which waits for the escrow's check.
    hub.deposit(alice.0, None, 5_000_000);
    assert_eq!(hub.balance(alice), 0);
    hub.all_validators(|_| ValidatorOperation::AttestEscrow { sequence: 7 });
    let account = hub.ledger.account(alice).unwrap();
    assert_eq!((account.reserve, account.balance), (RESERVE, 3_000_000));
}

#[test]
fn reconfigure_takes_effect_after_its_notice() {
    let mut hub = Hub::new();
    let (mut rules, _) = rules(4);
    rules.terms.fees.operation = 20;
    let terms = rules.terms.clone();
    for (i, acceptance) in rules.validators.iter_mut().enumerate() {
        let seed = u8::try_from(i + 1).unwrap();
        *acceptance = acceptance
            .clone()
            .sign(&NodeKey::from_bytes(&[seed; 32]), &terms);
    }
    let ops: Vec<Operation> = (0..3)
        .map(|i| hub.validator_op(i, ValidatorOperation::Reconfigure(rules.clone())))
        .collect();
    hub.block(ops);
    assert_eq!(hub.ledger.terms().fees.operation, 10);
    let notice = hub.ledger.terms().timers.reconfigure_notice;
    let at = hub.ledger.timestamp() + notice;
    hub.block_at(at, vec![], &[]);
    // The new set takes over two blocks after its notice passed.
    hub.block(vec![]);
    hub.block(vec![]);
    assert_eq!(hub.ledger.terms().fees.operation, 20);
    assert_eq!(hub.ledger.hub(), &rules_id_of_genesis());
}

fn rules_id_of_genesis() -> HubId {
    rules(4).0.id()
}

#[test]
fn the_state_root_follows_the_state() {
    let mut hub = Hub::new();
    let before = hub.ledger.state_root();
    hub.join(1, 10_000_000);
    assert_ne!(hub.ledger.state_root(), before);
    let copy = hub.ledger.clone();
    assert_eq!(copy.state_root(), hub.ledger.state_root());
}

/// `rules`, changed by `change` and signed again by the validators' node
/// keys (seeds 1..=4), so that they pass `Rules::check`.
pub(crate) fn resigned(mut rules: Rules, change: impl FnOnce(&mut Rules)) -> Rules {
    change(&mut rules);
    let terms = rules.terms.clone();
    for (i, acceptance) in rules.validators.iter_mut().enumerate() {
        let seed = u8::try_from(i + 1).unwrap();
        *acceptance = acceptance
            .clone()
            .sign(&NodeKey::from_bytes(&[seed; 32]), &terms);
    }
    rules
}

/// Votes `rules` in with three validators and lets their notice pass.
pub(crate) fn reconfigure(hub: &mut Hub, rules: &Rules) -> Vec<Outcome> {
    let ops: Vec<Operation> = (0..3)
        .map(|i| hub.validator_op(i, ValidatorOperation::Reconfigure(rules.clone())))
        .collect();
    let outcomes = hub.block(ops);
    let at = hub.ledger.timestamp() + hub.ledger.terms().timers.reconfigure_notice;
    hub.block_at(at, vec![], &[]);
    // The new set takes over two blocks after its notice passed.
    hub.block(vec![]);
    hub.block(vec![]);
    outcomes
}

#[test]
fn a_claim_through_the_last_sequence_leaves_nothing_behind() {
    let mut hub = Hub::new();
    let (_, alice_key, bob, bob_key, hold) = hold_for_bob(&mut hub);
    let id = *hub.ledger.hub();
    let expiry = hub.ledger.timestamp() + 10_000;
    let (last, f) = commitment(hold, u64::MAX, 100, expiry, 1);
    let claim = MemberOperation::Claim {
        hold,
        fulfilled: vec![Fulfilled {
            commitment: Signed::sign(last, &id, &alice_key),
            fulfillment: f,
        }],
    };
    assert!(matches!(
        hub.run(bob, &bob_key, claim),
        Outcome::Applied { .. }
    ));
    let through = Signed::sign(
        Claim {
            hold,
            total: 100,
            through: u64::MAX,
        },
        &id,
        &alice_key,
    );
    assert_eq!(
        hub.run(bob, &bob_key, MemberOperation::Redeem(through)),
        Outcome::Applied {
            fee: OP_FEE,
            detail: Detail::Paid(0)
        }
    );
    assert!(hub.ledger.hold(hold).unwrap().claimed.is_empty());
}

#[test]
fn a_member_account_for_a_new_validators_key_becomes_its_account() {
    let mut hub = Hub::new();
    let newcomer = NodeKey::from_bytes(&[77; 32]);
    let (rules, _) = rules(4);
    let rules = resigned(rules, |rules| {
        rules.validators[3].consensus = newcomer.public_key();
    });
    // Someone opens a member account for the newcomer's key meanwhile.
    hub.deposit(0, Some(&newcomer), 5_000_000);
    let number = hub.ledger.account_of(&newcomer.public_key()).unwrap();
    reconfigure(&mut hub, &rules);
    let account = hub.ledger.account(number).unwrap();
    assert!(account.validator);
    assert_eq!((account.reserve, account.balance), (0, 5_000_000));
    // The validator it replaced is a member now.
    assert!(!hub.ledger.account(AccountNumber(4)).unwrap().validator);
    // The old set keeps an escrow sequence for its change of signers.
    assert_eq!(hub.ledger.escrow().signer_list, Some(100));
    assert_eq!(hub.ledger.escrow().next_sequence, 101);
}

#[test]
fn new_rules_may_not_shrink_a_table() {
    let mut hub = Hub::new();
    let (rules, _) = rules(4);
    let smaller = resigned(rules, |rules| rules.terms.limits.holds -= 1);
    let outcomes = reconfigure(&mut hub, &smaller);
    assert!(outcomes.iter().all(|o| matches!(o, Outcome::Failed { .. })));
    assert_eq!(hub.ledger.terms().limits.holds, 4_000);
}

#[test]
fn a_closing_account_does_nothing_more() {
    let mut hub = Hub::new();
    let (alice, key) = hub.join(1, 10_000_000);
    let to = Destination {
        account: [7; 20],
        tag: None,
    };
    hub.run(alice, &key, MemberOperation::Close(CloseTo::Ledger(to)));
    let open = MemberOperation::HoldOpen {
        payee: AccountNumber(1),
        amount: 0,
    };
    assert_eq!(
        hub.run(alice, &key, open),
        Outcome::Refused(Refusal::Unknown)
    );
}

#[test]
fn one_validators_made_up_deposits_fill_only_its_own_share() {
    let mut hub = Hub::new();
    let share = hub.ledger.terms().limits.pending_deposits as usize / 4;
    let made_up: Vec<Operation> = (0..share + 1)
        .map(|i| {
            let mut transaction = [0xfe; 32];
            transaction[..8].copy_from_slice(&u64::try_from(i).unwrap().to_be_bytes());
            let deposit = Deposit {
                transaction: Digest(transaction),
                ledger: 9_999_999,
                amount: 1,
                sender: [1; 20],
                tag: 1,
                key: None,
            };
            hub.validator_op(3, ValidatorOperation::AttestDeposit(deposit))
        })
        .collect();
    for chunk in made_up.chunks(100) {
        hub.block(chunk.to_vec());
    }
    assert_eq!(hub.ledger.pending_deposits.len(), share);
    // An honest quorum still credits a real deposit.
    let (alice, _) = hub.join(1, 10_000_000);
    assert!(hub.balance(alice) > 0);
}

#[test]
fn a_payee_can_release_holds_toward_it() {
    let mut hub = Hub::new();
    let (alice, _, bob, bob_key, hold) = hold_for_bob(&mut hub);
    let before = hub.balance(alice);
    hub.run(bob, &bob_key, MemberOperation::HoldRelease { hold });
    assert!(hub.ledger.hold(hold).is_none());
    assert_eq!(hub.balance(alice), before + 1_000_000 + 200_000);
    assert_eq!(hub.ledger.account(bob).unwrap().holds, 0);
    // Only payers count toward the limit per account.
    assert_eq!(hub.ledger.account(alice).unwrap().paying, 0);
}

#[test]
fn reset_votes_count_validators_whatever_ledger_each_saw() {
    let mut hub = Hub::new();
    hub.join(1, 10_000_000);
    // Honest validators see the new chain at different ledgers.
    let ops: Vec<Operation> = (0..3)
        .map(|i| {
            let ledger = u32::try_from(i).unwrap() + 1;
            hub.validator_op(i, ValidatorOperation::AttestReset { ledger })
        })
        .collect();
    hub.block(ops);
    assert_eq!(hub.ledger.supply(), 0);
    assert_eq!(hub.ledger.written_off_at(), Some(hub.ledger.timestamp()));
}

#[test]
fn terms_that_would_trap_the_ledger_are_refused() {
    let sponsor = NodeKey::from_bytes(&[1; 32]);
    let base = crate::rules::tests::terms(&sponsor);
    let changes: [fn(&mut crate::rules::Terms); 9] = [
        // Blocks or claims a follower could not decode.
        |t| t.limits.block_operations = 4097,
        |t| t.limits.vote_window = 257,
        |t| t.limits.commitments_per_claim = 1025,
        |t| t.fees.operation = 0,
        |t| t.fees.hold_reserve = 0,
        |t| t.limits.recent_deposits = 0,
        |t| t.limits.deadline_operations = t.limits.block_operations,
        |t| t.fees.least_payout = 999_999,
        |t| t.timers.reconfigure_notice = t.timers.hold_notice,
    ];
    for change in changes {
        let mut terms = base.clone();
        change(&mut terms);
        assert!(terms.check().is_err(), "{terms:?}");
    }
}

#[test]
fn a_full_accounts_table_still_takes_a_new_validator() {
    let mut hub = Hub::with(|rules| rules.terms.limits.accounts = 2);
    hub.join(1, 10_000_000);
    hub.join(2, 10_000_000);
    let newcomer = NodeKey::from_bytes(&[77; 32]);
    let (rules, _) = rules(4);
    let rules = resigned(rules, |rules| {
        rules.terms.limits.accounts = 2;
        rules.validators[3].consensus = newcomer.public_key();
    });
    reconfigure(&mut hub, &rules);
    let number = hub.ledger.account_of(&newcomer.public_key()).unwrap();
    assert!(hub.ledger.account(number).unwrap().validator);
    // The validator that left keeps its account, outside the bound.
    assert!(!hub.ledger.account(AccountNumber(4)).unwrap().validator);
    assert_eq!(hub.ledger.members(), 2);
}

#[test]
fn a_closing_account_that_becomes_a_validators_stays_open() {
    let mut hub = Hub::new();
    let newcomer = NodeKey::from_bytes(&[77; 32]);
    let (rules, _) = rules(4);
    let rules = resigned(rules, |rules| {
        rules.validators[3].consensus = newcomer.public_key();
    });
    hub.deposit(0, Some(&newcomer), 5_000_000);
    let number = hub.ledger.account_of(&newcomer.public_key()).unwrap();
    let to = Destination {
        account: [7; 20],
        tag: None,
    };
    hub.run(number, &newcomer, MemberOperation::SetDisbursal(to));
    hub.run(
        number,
        &newcomer,
        MemberOperation::Close(CloseTo::Ledger(to)),
    );
    assert!(hub.ledger.account(number).unwrap().closing);
    reconfigure(&mut hub, &rules);
    assert!(!hub.ledger.account(number).unwrap().closing);
    assert!(hub.ledger.withdrawals().all(|(_, w)| !w.closes));
    // Fees reach it while its payout is still out.
    for _ in 0..40 {
        hub.block(vec![]);
    }
    let (sequence, _) = hub.ledger.withdrawals().next().unwrap();
    hub.all_validators(|_| ValidatorOperation::AttestWithdrawal {
        escrow_sequence: sequence,
        delivered: true,
    });
    assert!(hub.ledger.account(number).is_some());
}

#[test]
fn attestations_follow_their_validators_through_a_change_of_set() {
    let mut hub = Hub::new();
    let key = NodeKey::from_bytes(&[5; 32]);
    let deposit = Deposit {
        transaction: Digest([42; 32]),
        ledger: 3,
        amount: 5_000_000,
        sender: [1; 20],
        tag: 0,
        key: Some(key.public_key()),
    };
    let ops: Vec<Operation> = (0..2)
        .map(|i| hub.validator_op(i, ValidatorOperation::AttestDeposit(deposit)))
        .collect();
    hub.block(ops);
    let (rules, _) = rules(4);
    let rules = resigned(rules, |rules| {
        rules.validators[3].consensus = NodeKey::from_bytes(&[78; 32]).public_key();
    });
    reconfigure(&mut hub, &rules);
    // Validators 0 and 1 stayed: one more attestation makes a quorum.
    let third = hub.validator_op(2, ValidatorOperation::AttestDeposit(deposit));
    hub.block(vec![third]);
    assert!(hub.ledger.account_of(&key.public_key()).is_some());
}

#[test]
fn a_validator_votes_for_a_reset_once() {
    let mut hub = Hub::new();
    hub.join(1, 10_000_000);
    let first = hub.validator_op(0, ValidatorOperation::AttestReset { ledger: 1 });
    let again = hub.validator_op(0, ValidatorOperation::AttestReset { ledger: 2 });
    let other = hub.validator_op(1, ValidatorOperation::AttestReset { ledger: 1 });
    let outcomes = hub.block(vec![first, again, other]);
    assert_eq!(outcomes[1], Outcome::Refused(Refusal::NotAllowed));
    assert!(hub.ledger.supply() > 0, "two validators are no quorum");
}

/// The hub of `reset_votes_count_validators_whatever_ledger_each_saw`
/// after its write-off, with its escrow checked again and a member funded.
fn written_off() -> Hub {
    let mut hub = Hub::new();
    hub.join(1, 10_000_000);
    let ops: Vec<Operation> = (0..3)
        .map(|i| hub.validator_op(i, ValidatorOperation::AttestReset { ledger: 1 }))
        .collect();
    hub.block(ops);
    assert_eq!(hub.ledger.supply(), 0);
    hub.all_validators(|_| ValidatorOperation::AttestEscrow { sequence: 7 });
    hub.join(2, 10_000_000);
    assert!(hub.ledger.supply() > 0);
    hub
}

// spec/areas/hubs.md#what-validators-attest: votes count validators, so a
// vote must not outlive its reset. The honest validator that saw the reset
// late, with f faulty ones and one a lying server fooled, would otherwise
// be a quorum for a second write-off.
#[test]
fn late_reset_votes_never_make_a_second_write_off() {
    let mut hub = written_off();
    let late = hub.validator_op(3, ValidatorOperation::AttestReset { ledger: 5 });
    let faulty = hub.validator_op(0, ValidatorOperation::AttestReset { ledger: 9 });
    let fooled = hub.validator_op(1, ValidatorOperation::AttestReset { ledger: 9 });
    let outcomes = hub.block(vec![late, faulty, fooled]);
    assert!(
        outcomes
            .iter()
            .all(|o| *o == Outcome::Refused(Refusal::NotAllowed)),
        "{outcomes:?}"
    );
    assert!(hub.ledger.supply() > 0, "no second write-off");
}

#[test]
fn a_reset_vote_counts_again_after_the_quiet_period() {
    let mut hub = written_off();
    let at = hub.ledger.written_off_at().unwrap();
    let ops: Vec<Operation> = [0, 1]
        .into_iter()
        .map(|i| hub.validator_op(i, ValidatorOperation::AttestReset { ledger: 3 }))
        .collect();
    let votes = [(hub.ledger.height(), Signers::from_bits(0b1111))];
    let quiet_end = at + crate::ledger::RESET_QUIET_MS;
    let outcomes = hub.block_at(quiet_end - 1, ops, &votes);
    assert!(outcomes.iter().all(|o| matches!(o, Outcome::Refused(_))));
    let ops: Vec<Operation> = [0, 1, 3]
        .into_iter()
        .map(|i| hub.validator_op(i, ValidatorOperation::AttestReset { ledger: 3 }))
        .collect();
    let votes = [(hub.ledger.height(), Signers::from_bits(0b1111))];
    hub.block_at(quiet_end, ops, &votes);
    assert_eq!(hub.ledger.supply(), 0, "a new reset is written off");
    assert_eq!(hub.ledger.written_off_at(), Some(quiet_end));
}

#[test]
fn new_rules_wait_for_the_old_sets_change_of_signers() {
    let mut hub = Hub::new();
    let (rules, _) = rules(4);
    let first = resigned(rules.clone(), |rules| {
        rules.validators[3].escrow = crate::rules::tests::escrow_key(200);
    });
    reconfigure(&mut hub, &first);
    // Only escrow keys changed, and the old set still owes the signer list.
    assert_eq!(hub.ledger.escrow().signer_list, Some(100));
    let second = resigned(rules, |rules| rules.terms.fees.operation = 20);
    let outcomes = reconfigure(&mut hub, &second);
    assert!(outcomes.iter().all(|o| matches!(o, Outcome::Failed { .. })));
}

#[test]
fn a_withdrawal_with_a_quorum_after_a_change_of_set_settles() {
    let mut hub = Hub::of(7, |_| {});
    let (alice, key) = hub.join(1, 10_000_000);
    let to = Destination {
        account: [7; 20],
        tag: None,
    };
    let outcome = hub.run(
        alice,
        &key,
        MemberOperation::Withdraw {
            amount: 3_000_000,
            to,
        },
    );
    let Outcome::Applied {
        detail: Detail::Withdrawal(sequence),
        ..
    } = outcome
    else {
        panic!("{outcome:?}");
    };
    // Three of seven saw it deliver: short of the quorum of five.
    let ops: Vec<Operation> = (0..3)
        .map(|i| {
            hub.validator_op(
                i,
                ValidatorOperation::AttestWithdrawal {
                    escrow_sequence: sequence,
                    delivered: true,
                },
            )
        })
        .collect();
    hub.block(ops);
    assert_eq!(hub.ledger.withdrawals().count(), 1);
    // The first four stay; a quorum of four is three.
    let (four, _) = rules(4);
    let four = resigned(four, |_| {});
    let ops: Vec<Operation> = (0..5)
        .map(|i| hub.validator_op(i, ValidatorOperation::Reconfigure(four.clone())))
        .collect();
    hub.block(ops);
    let at = hub.ledger.timestamp() + hub.ledger.terms().timers.reconfigure_notice;
    hub.block_at(at, vec![], &[]);
    // The new set takes over two blocks after its notice passed.
    hub.block(vec![]);
    hub.block(vec![]);
    assert_eq!(hub.ledger.validators().len(), 4);
    assert_eq!(
        hub.ledger.withdrawals().count(),
        0,
        "settled by the attestations it had"
    );
}

mod properties {
    use super::*;
    use proptest::prelude::*;

    /// One step of a random run: who acts, toward whom, what it does, and
    /// an amount.
    #[derive(Clone, Debug)]
    struct Step {
        actor: usize,
        other: usize,
        action: u8,
        amount: Amount,
    }

    fn step() -> impl Strategy<Value = Step> {
        (0usize..3, 0usize..3, 0u8..11, 0u64..3_000_000).prop_map(
            |(actor, other, action, amount)| Step {
                actor,
                other,
                action,
                amount,
            },
        )
    }

    /// Three members acting at random, signing claims for each other as
    /// honest payers would.
    struct Run {
        hub: Hub,
        members: Vec<(AccountNumber, [u8; 32])>,
        sequence: u64,
        totals: alloc::collections::BTreeMap<HoldNumber, Amount>,
    }

    impl Run {
        fn new() -> Self {
            let mut hub = Hub::new();
            let members = (1..=3)
                .map(|seed| {
                    let (number, key) = hub.join(seed, 20_000_000);
                    (number, *key.as_bytes())
                })
                .collect();
            Self {
                hub,
                members,
                sequence: 0,
                totals: alloc::collections::BTreeMap::new(),
            }
        }

        fn hold_between(&self, payer: AccountNumber, payee: AccountNumber) -> Option<HoldNumber> {
            self.hub
                .ledger
                .holds()
                .find(|(_, h)| h.payer == payer && h.payee == payee)
                .map(|(n, _)| n)
        }

        fn operation(&mut self, step: &Step) -> Option<MemberOperation> {
            let me = self.members[step.actor].0;
            let (other, other_seed) = self.members[step.other];
            let id = *self.hub.ledger.hub();
            let payer = NodeKey::from_bytes(&other_seed);
            Some(match step.action {
                0 => MemberOperation::Transfer {
                    to: other,
                    amount: step.amount,
                },
                1 => MemberOperation::HoldOpen {
                    payee: other,
                    amount: step.amount,
                },
                2 => MemberOperation::HoldFund {
                    hold: self.hold_between(me, other)?,
                    amount: step.amount,
                },
                3 => MemberOperation::HoldReduce {
                    hold: self.hold_between(me, other)?,
                    amount: step.amount,
                },
                4 | 5 => {
                    let hold = self.hold_between(other, me)?;
                    let total = self.totals.entry(hold).or_default();
                    *total += step.amount % 1_000;
                    self.sequence += 1;
                    let claim = Claim {
                        hold,
                        total: *total,
                        through: self.sequence,
                    };
                    MemberOperation::Redeem(Signed::sign(claim, &id, &payer))
                }
                6 => {
                    let hold = self.hold_between(other, me)?;
                    self.sequence += 1;
                    let expiry = self.hub.ledger.timestamp() + 1_000;
                    let (c, f) = commitment(hold, self.sequence, step.amount % 5_000, expiry, 9);
                    let fulfilled = vec![Fulfilled {
                        commitment: Signed::sign(c, &id, &payer),
                        fulfillment: f,
                    }];
                    MemberOperation::Claim { hold, fulfilled }
                }
                7 => MemberOperation::Withdraw {
                    amount: step.amount.max(1_000_000),
                    to: Destination {
                        account: [3; 20],
                        tag: None,
                    },
                },
                8 => MemberOperation::Close(CloseTo::Account(other)),
                9 => MemberOperation::HoldRelease {
                    hold: self.hold_between(other, me)?,
                },
                _ => return None,
            })
        }

        fn step(&mut self, step: &Step) {
            if step.action == 10 {
                // The escrow settles the oldest withdrawal, one way or the
                // other.
                let oldest = self
                    .hub
                    .ledger
                    .withdrawals()
                    .next()
                    .map(|(sequence, _)| sequence);
                if let Some(sequence) = oldest {
                    let delivered = step.amount.is_multiple_of(2);
                    self.hub
                        .all_validators(|_| ValidatorOperation::AttestWithdrawal {
                            escrow_sequence: sequence,
                            delivered,
                        });
                }
                return;
            }
            let (me, seed) = self.members[step.actor];
            if self.hub.ledger.account(me).is_none() {
                return;
            }
            if let Some(operation) = self.operation(step) {
                self.hub.run(me, &NodeKey::from_bytes(&seed), operation);
            }
        }
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(256))]

        /// Whatever members do, the books add up after every block (the
        /// ledger asserts it), and two validators executing the same steps
        /// reach the same state.
        #[test]
        fn random_runs_keep_the_books_and_agree(steps in proptest::collection::vec(step(), 1..60)) {
            let mut first = Run::new();
            let mut second = Run::new();
            for step in &steps {
                first.step(step);
                second.step(step);
            }
            prop_assert_eq!(first.hub.ledger.state_root(), second.hub.ledger.state_root());
            prop_assert_eq!(first.hub.ledger.liabilities(), first.hub.ledger.supply());
            let restored = Ledger::restore(&first.hub.ledger.snapshot(), &first.hub.ledger.state_root()).unwrap();
            prop_assert_eq!(&restored, &first.hub.ledger);
        }
    }
}
