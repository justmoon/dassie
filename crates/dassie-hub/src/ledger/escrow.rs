//! What validators see on the underlying ledger
//! (spec/designs/settlement-hubs.md#6-deposits-and-withdrawals): deposits
//! to the escrow, the outcomes of its withdrawals, and resets of the test
//! network.
//!
//! Each validator follows the escrow on its own servers and attests what it
//! sees in validated ledgers. The hub acts on a deposit or an outcome once
//! a quorum attested the same one, so f faulty validators can neither make
//! one up nor hold one back.

use alloc::vec::Vec;

use super::Ledger;
use super::outcome::{Detail, Failure, Refusal};
use super::state::PendingDeposit;
use crate::digest::Digest;
use crate::operation::{AccountNumber, Deposit, Destination};
use crate::validators::{Signers, ValidatorIndex};

type Attested = Result<Detail, Result<Failure, Refusal>>;

impl Ledger {
    /// Records a validator's attestation of a deposit. A deposit becomes
    /// pending with its first attestation, and below the floor it does not
    /// (it may have been credited and forgotten); one already pending is
    /// attested whatever its ledger, since it was never credited. Each
    /// validator has its own share of the pending table, so a faulty one
    /// that attests made-up deposits fills only its own share, and honest
    /// attestations still fit.
    pub(super) fn attest_deposit(&mut self, index: ValidatorIndex, deposit: &Deposit) -> Attested {
        let hash = deposit.transaction;
        if self.recent_deposits.contains_key(&hash) {
            return Ok(Detail::None);
        }
        let pending = self.pending_deposits.get(&hash);
        if pending.is_some_and(|pending| pending.attested_by(index)) {
            return Err(Err(Refusal::NotAllowed));
        }
        if pending.is_none() && deposit.ledger < self.escrow.deposit_floor {
            return Err(Ok(Failure::Stale));
        }
        if self.attested_pending(index) >= self.deposit_share() {
            return Err(Ok(Failure::Full));
        }
        let pending = self.pending_deposits.entry(hash).or_default();
        // One validator's attestation per transaction; at most n distinct
        // ones.
        match pending.attestations.iter_mut().find(|(d, _)| d == deposit) {
            Some((_, signers)) => *signers = signers.with(index),
            None => pending
                .attestations
                .push((*deposit, Signers::NONE.with(index))),
        }
        self.credit_deposits();
        Ok(Detail::None)
    }

    /// How many pending deposits each validator may have attested: its
    /// share of the table.
    fn deposit_share(&self) -> usize {
        let table = usize::try_from(self.rules.terms.limits.pending_deposits).unwrap_or(usize::MAX);
        table.checked_div(self.validators.len()).unwrap_or(0)
    }

    /// How many pending deposits validator `index` attested.
    fn attested_pending(&self, index: ValidatorIndex) -> usize {
        self.pending_deposits
            .values()
            .filter(|pending| pending.attested_by(index))
            .count()
    }

    /// Credits every deposit a quorum agrees on, once the escrow is
    /// checked. One whose payback finds the withdrawals full waits for the
    /// next block.
    pub(super) fn credit_deposits(&mut self) {
        if !self.escrow.checked {
            return;
        }
        let quorum = self.validators.quorum();
        let agreed: Vec<Deposit> = self
            .pending_deposits
            .values()
            .filter_map(|pending| {
                pending
                    .attestations
                    .iter()
                    .find(|(_, signers)| signers.count() >= quorum)
                    .map(|(deposit, _)| *deposit)
            })
            .collect();
        for deposit in agreed {
            if self.credit_deposit(&deposit) {
                self.pending_deposits.remove(&deposit.transaction);
                self.remember_deposit(deposit.transaction, deposit.ledger);
            }
        }
    }

    /// Credits one deposit: to the account its tag names, to a new account
    /// for tag 0 with a key, or back to its sender less the fees. Whether
    /// it was credited.
    fn credit_deposit(&mut self, deposit: &Deposit) -> bool {
        if let Some(number) = self.deposit_account(deposit) {
            self.escrow.balance = self.escrow.balance.saturating_add(deposit.amount);
            self.credit(number, deposit.amount);
            return true;
        }
        let fees = self.rules.terms.fees;
        let cost = fees.operation.saturating_add(fees.ledger);
        let back = deposit.amount.saturating_sub(cost);
        if back == 0 {
            self.escrow.balance = self.escrow.balance.saturating_add(deposit.amount);
            self.reserve_account = self.reserve_account.saturating_add(deposit.amount);
            return true;
        }
        let to = Destination {
            account: deposit.sender,
            tag: None,
        };
        if self.queue_withdrawal(None, back, to, false).is_err() {
            return false;
        }
        self.escrow.balance = self.escrow.balance.saturating_add(deposit.amount);
        let kept = deposit
            .amount
            .saturating_sub(back)
            .saturating_sub(fees.ledger);
        self.reserve_account = self.reserve_account.saturating_add(kept);
        true
    }

    /// The account a deposit goes to, opening it for tag 0, if it can be
    /// credited at all.
    fn deposit_account(&mut self, deposit: &Deposit) -> Option<AccountNumber> {
        if deposit.tag != 0 {
            let number = AccountNumber(deposit.tag);
            return self
                .accounts
                .get(&number)
                .is_some_and(|account| !account.closing)
                .then_some(number);
        }
        let key = deposit.key?;
        if deposit.amount < self.rules.terms.new_account_minimum() {
            return None;
        }
        self.open_account(key)
    }

    /// Remembers a credited deposit against crediting it twice. When the
    /// table is full, the deposits of its lowest ledger go, and the floor
    /// rises past it: the hub takes no attestation below the floor.
    fn remember_deposit(&mut self, hash: Digest, ledger: u32) {
        let limit = usize::try_from(self.rules.terms.limits.recent_deposits).unwrap_or(usize::MAX);
        while self.recent_deposits.len() >= limit {
            let Some(lowest) = self.recent_deposits.values().min().copied() else {
                break;
            };
            self.recent_deposits.retain(|_, l| *l != lowest);
            self.escrow.deposit_floor = self.escrow.deposit_floor.max(lowest.saturating_add(1));
        }
        self.recent_deposits.insert(hash, ledger);
    }

    pub(super) fn attest_withdrawal(
        &mut self,
        index: ValidatorIndex,
        sequence: u32,
        delivered: bool,
    ) -> Attested {
        let quorum = self.validators.quorum();
        let withdrawal = self
            .withdrawals
            .get_mut(&sequence)
            .ok_or(Ok(Failure::Missing))?;
        if withdrawal.delivered.contains(index) || withdrawal.failed.contains(index) {
            return Err(Err(Refusal::NotAllowed));
        }
        if delivered {
            withdrawal.delivered = withdrawal.delivered.with(index);
        } else {
            withdrawal.failed = withdrawal.failed.with(index);
        }
        if withdrawal.delivered.count() >= quorum {
            self.settle_withdrawal(sequence, true);
        } else if withdrawal.failed.count() >= quorum {
            self.settle_withdrawal(sequence, false);
        }
        Ok(Detail::None)
    }

    /// Settles a withdrawal. Delivered, the hub burns it and its fee; failed
    /// (a `tec` result, which still costs the escrow its fee), it returns
    /// the amount to the owner. A closing account is deleted once paid out,
    /// or open again, its reserve locked anew, if the payout failed.
    fn settle_withdrawal(&mut self, sequence: u32, delivered: bool) {
        let Some(withdrawal) = self.withdrawals.remove(&sequence) else {
            return;
        };
        let burnt = if delivered {
            withdrawal.amount.saturating_add(withdrawal.ledger_fee)
        } else {
            withdrawal.ledger_fee
        };
        assert!(
            self.escrow.balance >= burnt,
            "escrow pays out more than it holds"
        );
        self.escrow.balance = self.escrow.balance.saturating_sub(burnt);
        let Some(number) = withdrawal.account else {
            // A deposit paid back that failed stays with the hub.
            if !delivered {
                self.reserve_account = self.reserve_account.saturating_add(withdrawal.amount);
            }
            return;
        };
        if let Some(account) = self.accounts.get_mut(&number) {
            account.withdrawals = account.withdrawals.saturating_sub(1);
            if withdrawal.closes && !delivered {
                account.closing = false;
            }
        }
        // An account that became a validator's while it closed stays.
        let validator = self.accounts.get(&number).is_some_and(|a| a.validator);
        if delivered && withdrawal.closes && !validator {
            self.delete_account(number);
        } else if delivered && withdrawal.closes {
            if let Some(account) = self.accounts.get_mut(&number) {
                account.closing = false;
            }
        } else if !delivered {
            self.credit(number, withdrawal.amount);
        }
    }

    /// Writes off what a reset of the test network took: every balance,
    /// reserve, hold and pending payment. Accounts stay, and lock their
    /// reserve again from what they next receive. Deposits wait until the
    /// validators checked the escrow made again.
    pub(super) fn write_off(&mut self) {
        let closing: Vec<AccountNumber> = self
            .accounts
            .iter()
            .filter(|(_, account)| account.closing)
            .map(|(number, _)| *number)
            .collect();
        for account in self.accounts.values_mut() {
            account.balance = 0;
            account.reserve = 0;
            account.holds = 0;
            account.paying = 0;
            account.withdrawals = 0;
        }
        for number in closing {
            self.delete_account(number);
        }
        self.holds.clear();
        self.withdrawals.clear();
        self.pending_deposits = alloc::collections::BTreeMap::new();
        self.recent_deposits.clear();
        self.pools.clear();
        self.reserve_account = 0;
        self.escrow.balance = 0;
        self.escrow.checked = false;
        self.escrow.next_sequence = 0;
        self.escrow.deposit_floor = 0;
        self.escrow.signer_list = None;
        self.reset_votes.clear();
        self.escrow_votes.clear();
    }
}

impl PendingDeposit {
    /// Whether validator `index` attested this deposit already.
    #[must_use]
    pub fn attested_by(&self, index: ValidatorIndex) -> bool {
        self.attestations
            .iter()
            .any(|(_, signers)| signers.contains(index))
    }
}
