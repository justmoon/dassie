//! Creating, posting and voiding transfers.

use alloc::vec::Vec;

use super::{
    JournalEntry, Ledger, LedgerError, Transfer, TransferError, TransferId, TransferSpec,
    TransferState, usize_of,
};
use crate::MAX_TOTAL;
use crate::account::{AccountId, Balance};

/// A change to an account's counters.
#[derive(Clone, Copy)]
enum Change {
    AddPending { debit: bool },
    RemovePending { debit: bool },
    AddPosted { debit: bool },
}

impl Ledger {
    /// Creates a transfer, checking limits unless it reports something that
    /// already happened ([`crate::Reference::is_report`]).
    ///
    /// # Errors
    ///
    /// If the transfer is invalid or a limit or bound refuses it. Nothing
    /// changes then.
    pub fn create(
        &mut self,
        now_ms: u64,
        spec: &TransferSpec,
    ) -> Result<TransferId, TransferError> {
        let result = self.check(spec);
        if matches!(
            result,
            Err(TransferError::ExceedsDebits | TransferError::ExceedsCredits)
        ) {
            self.stats.refused += 1;
        }
        result?;
        let durable = spec.pending && spec.reference.is_durable_while_pending();
        if spec.pending && self.pending.len() >= usize_of(self.config.max_pending) {
            return Err(TransferError::PendingFull);
        }
        if (durable || !spec.pending) && self.journal.len() >= usize_of(self.config.max_journal) {
            return Err(TransferError::JournalFull);
        }
        let transfer = Transfer {
            id: TransferId(self.next_transfer),
            debit: spec.debit,
            credit: spec.credit,
            amount: spec.amount,
            reference: spec.reference,
            created_at_ms: now_ms,
        };
        self.next_transfer += 1;
        self.stats.created += 1;
        if spec.pending {
            self.change(
                transfer.debit,
                transfer.amount,
                Change::AddPending { debit: true },
            );
            self.change(
                transfer.credit,
                transfer.amount,
                Change::AddPending { debit: false },
            );
            self.internal_pending(&transfer, true);
            self.pending.insert(transfer.id, transfer);
            if durable {
                self.record(transfer, TransferState::Pending, now_ms);
            }
        } else {
            self.change(
                transfer.debit,
                transfer.amount,
                Change::AddPosted { debit: true },
            );
            self.change(
                transfer.credit,
                transfer.amount,
                Change::AddPosted { debit: false },
            );
            self.stats.posted += 1;
            self.record(transfer, TransferState::Posted, now_ms);
        }
        Ok(transfer.id)
    }

    /// Creates pending transfers all or none (ACC-7): if one is refused, the
    /// ones before it are voided, and the error says which.
    ///
    /// # Errors
    ///
    /// The index of the refused transfer, and why.
    ///
    /// # Panics
    ///
    /// If a transfer is not pending, or must be durable: those could not be
    /// undone without a trace.
    pub fn create_linked(
        &mut self,
        now_ms: u64,
        specs: &[TransferSpec],
        ids: &mut Vec<TransferId>,
    ) -> Result<(), (usize, TransferError)> {
        assert!(
            specs
                .iter()
                .all(|spec| spec.pending && !spec.reference.is_durable_while_pending()),
            "linked transfers are pending and not durable"
        );
        let start = ids.len();
        for (index, spec) in specs.iter().enumerate() {
            match self.create(now_ms, spec) {
                Ok(id) => ids.push(id),
                Err(error) => {
                    for id in ids.drain(start..) {
                        // Pending and not durable: voiding writes nothing
                        // and cannot fail.
                        let voided = self.void(now_ms, id);
                        assert!(voided.is_ok(), "voiding a linked transfer failed");
                    }
                    return Err((index, error));
                }
            }
        }
        Ok(())
    }

    /// Posts a pending transfer: its amount moves. No limit is checked
    /// again; the pending amount already held its place.
    ///
    /// # Errors
    ///
    /// If no transfer with that ID is pending, or the journal is full.
    pub fn post(&mut self, now_ms: u64, id: TransferId) -> Result<(), LedgerError> {
        if !self.pending.contains_key(&id) {
            return Err(LedgerError::UnknownTransfer);
        }
        if self.journal.len() >= usize_of(self.config.max_journal) {
            return Err(LedgerError::JournalFull);
        }
        let transfer = self
            .pending
            .remove(&id)
            .ok_or(LedgerError::UnknownTransfer)?;
        self.release(&transfer);
        self.change(
            transfer.debit,
            transfer.amount,
            Change::AddPosted { debit: true },
        );
        self.change(
            transfer.credit,
            transfer.amount,
            Change::AddPosted { debit: false },
        );
        self.stats.posted += 1;
        self.record(transfer, TransferState::Posted, now_ms);
        Ok(())
    }

    /// Voids a pending transfer: its amount stays where it was. Only a
    /// durable transfer's void is journaled; the others never reached the
    /// database.
    ///
    /// # Errors
    ///
    /// If no transfer with that ID is pending, or a durable one's void finds
    /// the journal full.
    pub fn void(&mut self, now_ms: u64, id: TransferId) -> Result<(), LedgerError> {
        let transfer = self
            .pending
            .get(&id)
            .copied()
            .ok_or(LedgerError::UnknownTransfer)?;
        let durable = transfer.reference.is_durable_while_pending();
        if durable && self.journal.len() >= usize_of(self.config.max_journal) {
            return Err(LedgerError::JournalFull);
        }
        self.pending.remove(&id);
        self.release(&transfer);
        self.stats.voided += 1;
        if durable {
            self.record(transfer, TransferState::Voided, now_ms);
        }
        Ok(())
    }

    /// Whether [`create`](Self::create) would take `spec` now, as far as
    /// the accounts and their limits go, without creating it.
    ///
    /// # Errors
    ///
    /// Why it would be refused.
    pub fn validate(&self, spec: &TransferSpec) -> Result<(), TransferError> {
        self.check(spec)
    }

    /// Validates a transfer against the accounts, their limits and bounds.
    fn check(&self, spec: &TransferSpec) -> Result<(), TransferError> {
        if spec.amount == 0 {
            return Err(TransferError::ZeroAmount);
        }
        if spec.debit == spec.credit {
            return Err(TransferError::SameAccount);
        }
        let debit = self
            .account(spec.debit)
            .ok_or(TransferError::UnknownAccount)?;
        let credit = self
            .account(spec.credit)
            .ok_or(TransferError::UnknownAccount)?;
        if debit.path.ledger != credit.path.ledger {
            return Err(TransferError::DifferentLedgers);
        }
        let amount = spec.amount;
        let debit_total = debit.balance.debits_posted + debit.balance.debits_pending;
        let credit_total = credit.balance.credits_posted + credit.balance.credits_pending;
        if debit_total + u128::from(amount) > u128::from(MAX_TOTAL)
            || credit_total + u128::from(amount) > u128::from(MAX_TOTAL)
        {
            return Err(TransferError::Overflow);
        }
        if spec.reference.is_report() {
            return Ok(());
        }
        let amount = u128::from(amount);
        if !debit.balance.allows(debit.path.limit(), amount, 0) {
            return Err(TransferError::ExceedsDebits);
        }
        if !credit.balance.allows(credit.path.limit(), 0, amount) {
            return Err(TransferError::ExceedsCredits);
        }
        // Within one group, a transfer leaves the total unchanged: it needs
        // no free funds (spec 7 §4.5).
        if debit.group == credit.group {
            return Ok(());
        }
        if let Some(root) = debit.group.and_then(|root| self.account(root))
            && !root.group_view().allows(root.path.group_limit(), amount, 0)
        {
            return Err(TransferError::ExceedsDebits);
        }
        if let Some(root) = credit.group.and_then(|root| self.account(root))
            && !root.group_view().allows(root.path.group_limit(), 0, amount)
        {
            return Err(TransferError::ExceedsCredits);
        }
        Ok(())
    }

    /// Takes a transfer that left the pending table out of the pending
    /// counters.
    fn release(&mut self, transfer: &Transfer) {
        self.internal_pending(transfer, false);
        self.change(
            transfer.debit,
            transfer.amount,
            Change::RemovePending { debit: true },
        );
        self.change(
            transfer.credit,
            transfer.amount,
            Change::RemovePending { debit: false },
        );
    }

    /// Applies a change to an account and to its group's total.
    fn change(&mut self, id: AccountId, amount: u64, change: Change) {
        let amount = u128::from(amount);
        let apply = |balance: &mut Balance| match change {
            Change::AddPending { debit: true } => balance.debits_pending += amount,
            Change::AddPending { debit: false } => balance.credits_pending += amount,
            Change::RemovePending { debit: true } => balance.debits_pending -= amount,
            Change::RemovePending { debit: false } => balance.credits_pending -= amount,
            Change::AddPosted { debit: true } => balance.debits_posted += amount,
            Change::AddPosted { debit: false } => balance.credits_posted += amount,
        };
        let group = match self.accounts.get_mut(usize_of(id.0)) {
            Some(account) => {
                apply(&mut account.balance);
                account.group
            }
            None => None,
        };
        if let Some(root) = group.and_then(|root| self.accounts.get_mut(usize_of(root.0))) {
            apply(&mut root.group_balance);
        }
    }

    fn record(&mut self, transfer: Transfer, state: TransferState, at_ms: u64) {
        self.journal.push_back(JournalEntry::Transfer {
            transfer,
            state,
            at_ms,
        });
    }
}
