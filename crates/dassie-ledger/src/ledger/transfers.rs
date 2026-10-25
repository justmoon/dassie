//! Creating, posting and voiding transfers.

use alloc::collections::BTreeMap;
use alloc::vec::Vec;

use super::{
    JournalEntry, Ledger, LedgerError, Transfer, TransferError, TransferId, TransferSpec,
    TransferState, account_id, usize_of,
};
use crate::MAX_TOTAL;
use crate::account::{Account, AccountId, Balance};
use crate::{AccountName, AccountPath, Name, Reference};

/// A change to an account's counters.
#[derive(Clone, Copy)]
enum Change {
    AddPending { debit: bool },
    RemovePending { debit: bool },
    AddPosted { debit: bool },
}

impl Change {
    fn apply(self, balance: &mut Balance, amount: u128) {
        match self {
            Self::AddPending { debit: true } => balance.debits_pending += amount,
            Self::AddPending { debit: false } => balance.credits_pending += amount,
            Self::RemovePending { debit: true } => balance.debits_pending -= amount,
            Self::RemovePending { debit: false } => balance.credits_pending -= amount,
            Self::AddPosted { debit: true } => balance.debits_posted += amount,
            Self::AddPosted { debit: false } => balance.credits_posted += amount,
        }
    }
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
        self.check_room(spec)?;
        let durable = spec.pending && spec.reference.is_durable_while_pending();
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
        assert_eq!(ids.len() - start, specs.len(), "one ID per linked transfer");
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

    /// Whether [`create`](Self::create) would take `spec` now, without
    /// creating it: the accounts, their limits, and room in the tables.
    /// What it takes, it takes for a smaller amount too, until the ledger
    /// changes.
    ///
    /// # Errors
    ///
    /// Why it would be refused.
    pub fn validate(&self, spec: &TransferSpec) -> Result<(), TransferError> {
        self.check(spec)?;
        self.check_room(spec)
    }

    /// Whether [`create`](Self::create) would take a posted transfer of
    /// `amount` from `debit` to the interledger account of `node` on
    /// `debit`'s ledger, as [`open_peer`](Self::open_peer) would open it
    /// with `trust` if it is not open yet, without opening it: for a caller
    /// that opens a peer's accounts only once a transfer to them is taken
    /// (spec/decisions/lazy-downlink-accounts.md). Room for the accounts is
    /// the caller's to check.
    ///
    /// # Errors
    ///
    /// Why it would be refused.
    pub fn validate_to_peer(
        &self,
        debit: AccountId,
        node: Name,
        trust: u64,
        amount: u64,
        reference: Reference,
    ) -> Result<(), TransferError> {
        let ledger = self
            .account(debit)
            .ok_or(TransferError::UnknownAccount)?
            .path
            .ledger;
        let path = AccountPath::new(ledger, AccountName::AssetsInterledger(node));
        let open = self.by_path.get(&path).copied();
        // Not open yet: as it would open, the next account, with the trust
        // posted to it.
        let fresh_id = account_id(self.accounts.len());
        let mut fresh = Account::new(path, path.group_root().map(|_| fresh_id));
        fresh.balance.credits_posted = u128::from(trust);
        let spec = TransferSpec {
            debit,
            credit: open.unwrap_or(fresh_id),
            amount,
            pending: false,
            reference,
        };
        match open {
            Some(_) => self.check(&spec)?,
            None => check(&spec, |id| {
                if id == fresh_id {
                    Some(&fresh)
                } else {
                    self.account(id)
                }
            })?,
        }
        self.check_room(&spec)
    }

    /// Whether [`create`](Self::create) would take each of `specs`, posted
    /// one after the other, every one with the ones before it booked,
    /// without creating any: the accounts, their limits and bounds. So a
    /// batch can be taken whole or not at all. Room in the journal is the
    /// caller's to keep, by draining it between them.
    ///
    /// # Errors
    ///
    /// The index of the first transfer it would refuse, and why.
    ///
    /// # Panics
    ///
    /// If a transfer is pending.
    pub fn validate_posted(&self, specs: &[TransferSpec]) -> Result<(), (usize, TransferError)> {
        assert!(
            specs.iter().all(|spec| !spec.pending),
            "a sequence is checked for posted transfers"
        );
        // The accounts the transfers checked so far changed, as booking
        // them would leave them: at most four per transfer.
        let mut changed: BTreeMap<AccountId, Account> = BTreeMap::new();
        for (index, spec) in specs.iter().enumerate() {
            check(spec, |id| changed.get(&id).or_else(|| self.account(id)))
                .map_err(|error| (index, error))?;
            for (id, debit) in [(spec.debit, true), (spec.credit, false)] {
                let change = Change::AddPosted { debit };
                let amount = u128::from(spec.amount);
                // The check found both accounts, and a group's root exists.
                let account = changed.get(&id).or_else(|| self.account(id)).copied();
                assert!(account.is_some(), "a checked account is missing");
                let Some(mut account) = account else {
                    continue;
                };
                change.apply(&mut account.balance, amount);
                let group = account.group;
                changed.insert(id, account);
                let Some(root) = group else {
                    continue;
                };
                let root_account = changed.get(&root).or_else(|| self.account(root)).copied();
                assert!(root_account.is_some(), "a group without its root");
                if let Some(mut root_account) = root_account {
                    change.apply(&mut root_account.group_balance, amount);
                    changed.insert(root, root_account);
                }
            }
            assert!(
                changed.len() <= 4 * (index + 1),
                "at most four accounts a transfer"
            );
        }
        Ok(())
    }

    /// Whether the pending table and the journal have room for `spec`.
    fn check_room(&self, spec: &TransferSpec) -> Result<(), TransferError> {
        let durable = spec.pending && spec.reference.is_durable_while_pending();
        if spec.pending && self.pending.len() >= usize_of(self.config.max_pending) {
            return Err(TransferError::PendingFull);
        }
        if (durable || !spec.pending) && self.journal.len() >= usize_of(self.config.max_journal) {
            return Err(TransferError::JournalFull);
        }
        Ok(())
    }

    /// Validates a transfer against the accounts, their limits and bounds.
    fn check(&self, spec: &TransferSpec) -> Result<(), TransferError> {
        check(spec, |id| self.account(id))
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
        let apply = |balance: &mut Balance| change.apply(balance, amount);
        // Callers pass accounts they looked up: skipping a change would
        // leave the books unbalanced.
        let account = self.accounts.get_mut(usize_of(id.0));
        assert!(account.is_some(), "a change to an unknown account");
        let group = account.and_then(|account| {
            apply(&mut account.balance);
            account.group
        });
        if let Some(root) = group {
            let root = self.accounts.get_mut(usize_of(root.0));
            assert!(root.is_some(), "a group without its root");
            if let Some(root) = root {
                apply(&mut root.group_balance);
            }
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

/// Validates a transfer against the accounts, their limits and bounds,
/// as `account` shows them.
fn check<'a>(
    spec: &TransferSpec,
    account: impl Fn(AccountId) -> Option<&'a Account>,
) -> Result<(), TransferError> {
    if spec.amount == 0 {
        return Err(TransferError::ZeroAmount);
    }
    if spec.debit == spec.credit {
        return Err(TransferError::SameAccount);
    }
    let debit = account(spec.debit).ok_or(TransferError::UnknownAccount)?;
    let credit = account(spec.credit).ok_or(TransferError::UnknownAccount)?;
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
    // no free funds (spec/areas/ledger.md#limits-and-groups).
    if debit.group == credit.group {
        return Ok(());
    }
    if let Some(root) = debit.group.and_then(&account)
        && !root.group_view().allows(root.path.group_limit(), amount, 0)
    {
        return Err(TransferError::ExceedsDebits);
    }
    if let Some(root) = credit.group.and_then(&account)
        && !root.group_view().allows(root.path.group_limit(), 0, amount)
    {
        return Err(TransferError::ExceedsCredits);
    }
    Ok(())
}
