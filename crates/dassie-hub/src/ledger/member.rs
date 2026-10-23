//! Operations members sign with their account keys
//! (spec/designs/settlement-hubs.md#3-the-ledger). Each checks everything
//! before it changes anything, so a failure leaves the state as it was but
//! for the fee and the sequence.

use dassie_identity::PublicKey;

use super::Ledger;
use super::outcome::{Detail, Failure, Outcome, Refusal};
use super::state::{Account, Hold, Phase, Reduction, Withdrawal};
use crate::operation::{
    AccountNumber, CloseTo, Destination, MemberOperation, SignedMemberOperation,
};
use crate::promise::HoldNumber;
use crate::rules::Amount;
use crate::validators::Signers;

impl Ledger {
    /// Checks a member's signature, sequence and fee, charges the fee, and
    /// executes the operation.
    pub(super) fn apply_member(&mut self, signed: &SignedMemberOperation) -> Outcome {
        let fee = self.rules.terms.fees.operation;
        // A closing account has nothing left to do: it waits for its payout.
        let Some(account) = self.accounts.get(&signed.account).filter(|a| !a.closing) else {
            return Outcome::Refused(Refusal::Unknown);
        };
        if !signed.verifies(&self.hub, &account.key) {
            return Outcome::Refused(Refusal::Signature);
        }
        if signed.sequence != account.sequence {
            return Outcome::Refused(Refusal::Sequence);
        }
        if fee > signed.max_fee || fee > account.balance {
            return Outcome::Refused(Refusal::Fee);
        }
        if let Some(account) = self.accounts.get_mut(&signed.account) {
            account.balance = account.balance.saturating_sub(fee);
            account.sequence = account.sequence.saturating_add(1);
        }
        self.collect_fee(fee);
        match self.execute_member(signed.account, &signed.operation) {
            Ok(detail) => Outcome::Applied { fee, detail },
            Err(reason) => Outcome::Failed { fee, reason },
        }
    }

    fn execute_member(
        &mut self,
        from: AccountNumber,
        operation: &MemberOperation,
    ) -> Result<Detail, Failure> {
        let account = self.accounts.get(&from).ok_or(Failure::Missing)?;
        let needs_disbursal = !account.validator && account.disbursal.is_none();
        if needs_disbursal && !matches!(operation, MemberOperation::SetDisbursal(_)) {
            return Err(Failure::NoDisbursal);
        }
        let allowed_now = matches!(
            operation,
            MemberOperation::Redeem(_)
                | MemberOperation::Claim { .. }
                | MemberOperation::SetDisbursal(_)
        );
        if self.phase != Phase::Running && !allowed_now {
            return Err(Failure::ShuttingDown);
        }
        match operation {
            MemberOperation::Transfer { to, amount } => self.transfer(from, *to, *amount),
            MemberOperation::Pay { key, amount } => self.pay(from, key, *amount),
            MemberOperation::SetDisbursal(destination) => self.set_disbursal(from, *destination),
            MemberOperation::HoldOpen { payee, amount } => self.hold_open(from, *payee, *amount),
            MemberOperation::HoldFund { hold, amount } => self.hold_fund(from, *hold, *amount),
            MemberOperation::HoldReduce { hold, amount } => self.hold_reduce(from, *hold, *amount),
            MemberOperation::HoldRelease { hold } => self.hold_release(from, *hold),
            MemberOperation::Redeem(claim) => self.redeem(from, claim),
            MemberOperation::Claim { hold, fulfilled } => self.claim(from, *hold, fulfilled),
            MemberOperation::Withdraw { amount, to } => self.withdraw(from, *amount, *to),
            MemberOperation::Close(to) => self.close(from, *to),
        }
    }

    /// The account's balance, if it has `amount`.
    fn spendable(&self, from: AccountNumber, amount: Amount) -> Result<(), Failure> {
        let account = self.accounts.get(&from).ok_or(Failure::Missing)?;
        if account.balance < amount {
            return Err(Failure::Funds);
        }
        Ok(())
    }

    /// Whether `to` can be paid: it exists and is not closing.
    fn payable(&self, to: AccountNumber) -> Result<(), Failure> {
        match self.accounts.get(&to) {
            Some(account) if !account.closing => Ok(()),
            _ => Err(Failure::Missing),
        }
    }

    pub(super) fn debit(&mut self, from: AccountNumber, amount: Amount) {
        if let Some(account) = self.accounts.get_mut(&from) {
            assert!(account.balance >= amount, "debit beyond a checked balance");
            account.balance = account.balance.saturating_sub(amount);
        }
    }

    /// Credits an account, filling its account reserve first if a reset of
    /// the test network emptied it (spec/designs/settlement-hubs.md#6-deposits-and-withdrawals).
    pub(super) fn credit(&mut self, to: AccountNumber, amount: Amount) {
        let reserve = self.rules.terms.fees.account_reserve;
        let account = self.accounts.get_mut(&to);
        assert!(account.is_some(), "credit to a missing account");
        if let Some(account) = account {
            let missing = if account.validator {
                0
            } else {
                reserve.saturating_sub(account.reserve)
            };
            let to_reserve = missing.min(amount);
            account.reserve = account.reserve.saturating_add(to_reserve);
            account.balance = account
                .balance
                .saturating_add(amount.saturating_sub(to_reserve));
        }
    }

    fn transfer(
        &mut self,
        from: AccountNumber,
        to: AccountNumber,
        amount: Amount,
    ) -> Result<Detail, Failure> {
        self.payable(to)?;
        self.spendable(from, amount)?;
        self.debit(from, amount);
        self.credit(to, amount);
        Ok(Detail::None)
    }

    fn pay(
        &mut self,
        from: AccountNumber,
        key: &PublicKey,
        amount: Amount,
    ) -> Result<Detail, Failure> {
        if self.keys.contains_key(key.as_bytes()) {
            return Err(Failure::Exists);
        }
        if amount < self.rules.terms.new_account_minimum() {
            return Err(Failure::TooSmall);
        }
        self.spendable(from, amount)?;
        let number = self.open_account(*key).ok_or(Failure::Full)?;
        self.debit(from, amount);
        self.credit(number, amount);
        Ok(Detail::Account(number))
    }

    /// Opens an account for `key`, with nothing in it yet, if the table has
    /// room.
    pub(super) fn open_account(&mut self, key: PublicKey) -> Option<AccountNumber> {
        let limit = usize::try_from(self.rules.terms.limits.accounts).unwrap_or(usize::MAX);
        if self.members() >= limit || key.is_weak() || self.keys.contains_key(key.as_bytes()) {
            return None;
        }
        // Account numbers fit a destination tag, and 0 is kept for new
        // accounts.
        let number = AccountNumber(self.next_account);
        self.next_account = self.next_account.checked_add(1)?;
        self.keys.insert(*key.as_bytes(), number);
        self.accounts.insert(
            number,
            Account {
                key,
                balance: 0,
                reserve: 0,
                sequence: 0,
                disbursal: None,
                closing: false,
                holds: 0,
                paying: 0,
                withdrawals: 0,
                validator: false,
                member_slot: true,
            },
        );
        Some(number)
    }

    /// Deletes an account; its number is never used again.
    pub(super) fn delete_account(&mut self, number: AccountNumber) {
        if let Some(account) = self.accounts.remove(&number) {
            assert!(
                account.balance == 0 && account.reserve == 0 && account.holds == 0,
                "deleting an account that still holds something"
            );
            self.keys.remove(account.key.as_bytes());
        }
    }

    fn set_disbursal(
        &mut self,
        from: AccountNumber,
        destination: Destination,
    ) -> Result<Detail, Failure> {
        let account = self.accounts.get_mut(&from).ok_or(Failure::Missing)?;
        account.disbursal = Some(destination);
        Ok(Detail::None)
    }

    /// Opens a hold toward `payee`. Only the holds an account pays into
    /// count toward its limit: anyone can open one toward it, and it can
    /// release those ([`MemberOperation::HoldRelease`]), so no one can use
    /// up its room.
    fn hold_open(
        &mut self,
        from: AccountNumber,
        payee: AccountNumber,
        amount: Amount,
    ) -> Result<Detail, Failure> {
        if payee == from {
            return Err(Failure::NotYours);
        }
        self.payable(payee)?;
        let limits = &self.rules.terms.limits;
        let full = self.holds.len() >= usize::try_from(limits.holds).unwrap_or(usize::MAX)
            || self
                .accounts
                .get(&from)
                .is_none_or(|a| a.paying >= limits.holds_per_account)
            || self
                .accounts
                .get(&payee)
                .is_none_or(|a| a.holds == u16::MAX);
        if full {
            return Err(Failure::Full);
        }
        let reserve = self.rules.terms.fees.hold_reserve;
        self.spendable(from, amount.saturating_add(reserve))?;
        self.debit(from, amount.saturating_add(reserve));
        let number = HoldNumber(self.next_hold);
        self.next_hold = self.next_hold.saturating_add(1);
        self.holds.insert(
            number,
            Hold {
                payer: from,
                payee,
                amount,
                reserve,
                reduction: None,
                total: 0,
                through: 0,
                claimed: alloc::collections::BTreeMap::new(),
            },
        );
        for party in [from, payee] {
            if let Some(account) = self.accounts.get_mut(&party) {
                account.holds = account.holds.saturating_add(1);
                if party == from {
                    account.paying = account.paying.saturating_add(1);
                }
            }
        }
        Ok(Detail::Hold(number))
    }

    /// The payee gives up a hold toward it: what is left goes back to the
    /// payer at once, with its reserve. What the payee has not redeemed is
    /// lost to it.
    fn hold_release(&mut self, from: AccountNumber, hold: HoldNumber) -> Result<Detail, Failure> {
        let found = self.holds.get(&hold).ok_or(Failure::Missing)?;
        if found.payee != from {
            return Err(Failure::NotYours);
        }
        self.close_hold(hold);
        Ok(Detail::None)
    }

    fn own_hold(&self, from: AccountNumber, hold: HoldNumber) -> Result<&Hold, Failure> {
        let found = self.holds.get(&hold).ok_or(Failure::Missing)?;
        if found.payer != from {
            return Err(Failure::NotYours);
        }
        Ok(found)
    }

    fn hold_fund(
        &mut self,
        from: AccountNumber,
        hold: HoldNumber,
        amount: Amount,
    ) -> Result<Detail, Failure> {
        self.own_hold(from, hold)?;
        self.spendable(from, amount)?;
        self.debit(from, amount);
        if let Some(found) = self.holds.get_mut(&hold) {
            found.amount = found.amount.saturating_add(amount);
        }
        Ok(Detail::None)
    }

    /// Asks for `amount` back, after the hold's notice: the payee learns of
    /// it before it matters, and takes no Prepare that expires after it
    /// (spec/designs/settlement-hubs.md#4-peering-over-a-hub). A new request
    /// replaces the last, and starts the notice again.
    fn hold_reduce(
        &mut self,
        from: AccountNumber,
        hold: HoldNumber,
        amount: Amount,
    ) -> Result<Detail, Failure> {
        self.own_hold(from, hold)?;
        let at = self
            .timestamp
            .saturating_add(self.rules.terms.timers.hold_notice);
        if let Some(found) = self.holds.get_mut(&hold) {
            found.reduction = (amount > 0).then_some(Reduction { amount, at });
        }
        Ok(Detail::None)
    }

    fn withdraw(
        &mut self,
        from: AccountNumber,
        amount: Amount,
        to: Destination,
    ) -> Result<Detail, Failure> {
        if amount < self.rules.terms.fees.least_payout {
            return Err(Failure::TooSmall);
        }
        let ledger_fee = self.rules.terms.fees.ledger;
        self.spendable(from, amount.saturating_add(ledger_fee))?;
        let sequence = self.queue_withdrawal(Some(from), amount, to, false)?;
        self.debit(from, amount.saturating_add(ledger_fee));
        Ok(Detail::Withdrawal(sequence))
    }

    /// Gives a payment out of the escrow its escrow sequence, if the table
    /// has room. The caller has the funds.
    pub(super) fn queue_withdrawal(
        &mut self,
        account: Option<AccountNumber>,
        amount: Amount,
        to: Destination,
        closes: bool,
    ) -> Result<u32, Failure> {
        let limit = usize::try_from(self.rules.terms.limits.withdrawals).unwrap_or(usize::MAX);
        if self.withdrawals.len() >= limit {
            return Err(Failure::Full);
        }
        let sequence = self.escrow.next_sequence;
        self.escrow.next_sequence = sequence.checked_add(1).ok_or(Failure::Full)?;
        self.withdrawals.insert(
            sequence,
            Withdrawal {
                account,
                amount,
                ledger_fee: self.rules.terms.fees.ledger,
                to,
                closes,
                delivered: Signers::NONE,
                failed: Signers::NONE,
            },
        );
        if let Some(number) = account
            && let Some(owner) = self.accounts.get_mut(&number)
        {
            owner.withdrawals = owner.withdrawals.saturating_add(1);
        }
        Ok(sequence)
    }

    /// Pays out everything the account holds, reserve included, and ends
    /// it: at once when paid to another account, once the payout settled
    /// when paid on the underlying ledger.
    fn close(&mut self, from: AccountNumber, to: CloseTo) -> Result<Detail, Failure> {
        let account = self.accounts.get(&from).ok_or(Failure::Missing)?;
        if account.validator {
            return Err(Failure::NotYours);
        }
        if account.holds > 0 || account.withdrawals > 0 {
            return Err(Failure::Busy);
        }
        let total = account.balance.saturating_add(account.reserve);
        match to {
            CloseTo::Account(other) => {
                if other == from {
                    return Err(Failure::NotYours);
                }
                self.payable(other)?;
                self.empty_account(from);
                self.credit(other, total);
                self.delete_account(from);
                Ok(Detail::None)
            }
            CloseTo::Ledger(destination) => {
                let fees = &self.rules.terms.fees;
                let amount = total.saturating_sub(fees.ledger);
                if amount < fees.least_payout {
                    return Err(Failure::TooSmall);
                }
                let sequence = self.queue_withdrawal(Some(from), amount, destination, true)?;
                self.empty_account(from);
                if let Some(account) = self.accounts.get_mut(&from) {
                    account.closing = true;
                }
                Ok(Detail::Withdrawal(sequence))
            }
        }
    }

    fn empty_account(&mut self, number: AccountNumber) {
        if let Some(account) = self.accounts.get_mut(&number) {
            account.balance = 0;
            account.reserve = 0;
        }
    }
}
