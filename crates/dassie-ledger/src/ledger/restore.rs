//! Rebuilding a ledger from what the journal persisted.

use core::fmt;

use super::{Config, Ledger, Transfer, TransferId, usize_of};
use crate::MAX_TOTAL;
use crate::account::{Account, AccountId};
use crate::ledger_id::LedgerId;
use crate::path::AccountPath;

/// Persisted state that is not a ledger. Each means a damaged or foreign
/// database, never a state the ledger can produce.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RestoreError {
    /// More accounts than the config allows.
    TooManyAccounts,
    /// Account IDs are not 0, 1, 2, … in order.
    NotDense,
    /// Two accounts share a path.
    DuplicatePath,
    /// A group member was opened before its root.
    MissingGroupRoot,
    /// A counter passes [`MAX_TOTAL`].
    TotalTooLarge,
    /// A ledger's posted debits and credits differ.
    Unbalanced(LedgerId),
    /// A pending transfer is not one that survives restarts, refers to an
    /// unknown account, or is otherwise invalid.
    InvalidPending(TransferId),
    /// More pending transfers than the config allows.
    TooManyPending,
    /// The next transfer ID is not past every pending one.
    StaleNextTransfer,
}

impl fmt::Display for RestoreError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TooManyAccounts => f.write_str("more accounts than the ledger allows"),
            Self::NotDense => f.write_str("account IDs are not consecutive"),
            Self::DuplicatePath => f.write_str("two accounts share a path"),
            Self::MissingGroupRoot => f.write_str("an account was opened before its group"),
            Self::TotalTooLarge => f.write_str("an account total is too large"),
            Self::Unbalanced(ledger) => write!(f, "ledger {ledger} does not balance"),
            Self::InvalidPending(id) => write!(f, "pending transfer {} is invalid", id.0),
            Self::TooManyPending => f.write_str("more pending transfers than the ledger allows"),
            Self::StaleNextTransfer => f.write_str("the next transfer ID is behind"),
        }
    }
}

/// A persisted account: its ID, path and posted totals.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StoredAccount {
    /// Its ID.
    pub id: AccountId,
    /// Its path.
    pub path: AccountPath,
    /// Posted debits.
    pub debits_posted: u64,
    /// Posted credits.
    pub credits_posted: u64,
}

impl Ledger {
    /// Rebuilds a ledger from persisted accounts (in ID order), the pending
    /// transfers that survive restarts (settlements), and the next transfer
    /// ID. Packet transfers that were pending are gone: their packets died
    /// with the process (spec 7 §4.4).
    ///
    /// # Errors
    ///
    /// If the input is not a state a ledger can be in.
    pub fn restore(
        config: Config,
        accounts: impl IntoIterator<Item = StoredAccount>,
        pending: impl IntoIterator<Item = Transfer>,
        next_transfer: TransferId,
    ) -> Result<Self, RestoreError> {
        let mut ledger = Self::new(config);
        ledger.next_transfer = next_transfer.0;
        for stored in accounts {
            ledger.restore_account(stored)?;
        }
        for ledger_id in LedgerId::ALL {
            let (debits, credits) = ledger
                .accounts
                .iter()
                .filter(|account| account.path.ledger == ledger_id)
                .fold((0u128, 0u128), |(debits, credits), account| {
                    (
                        debits + account.balance.debits_posted,
                        credits + account.balance.credits_posted,
                    )
                });
            if debits != credits {
                return Err(RestoreError::Unbalanced(ledger_id));
            }
        }
        for transfer in pending {
            ledger.restore_pending(transfer)?;
        }
        // Restoring is not a step: nothing waits to be persisted.
        ledger.journal.clear();
        Ok(ledger)
    }

    fn restore_account(&mut self, stored: StoredAccount) -> Result<(), RestoreError> {
        if self.accounts.len() >= usize_of(self.config.max_accounts) {
            return Err(RestoreError::TooManyAccounts);
        }
        if usize_of(stored.id.0) != self.accounts.len() {
            return Err(RestoreError::NotDense);
        }
        if self.by_path.contains_key(&stored.path) {
            return Err(RestoreError::DuplicatePath);
        }
        if stored.debits_posted > MAX_TOTAL || stored.credits_posted > MAX_TOTAL {
            return Err(RestoreError::TotalTooLarge);
        }
        let group = match stored.path.group_root() {
            Some(root) if root == stored.path => Some(stored.id),
            Some(root) => Some(
                *self
                    .by_path
                    .get(&root)
                    .ok_or(RestoreError::MissingGroupRoot)?,
            ),
            None => None,
        };
        let mut account = Account::new(stored.path, group);
        account.balance.debits_posted = u128::from(stored.debits_posted);
        account.balance.credits_posted = u128::from(stored.credits_posted);
        self.accounts.push(account);
        self.by_path.insert(stored.path, stored.id);
        if let Some(root) = group.and_then(|root| self.accounts.get_mut(usize_of(root.0))) {
            root.group_balance.debits_posted += u128::from(stored.debits_posted);
            root.group_balance.credits_posted += u128::from(stored.credits_posted);
        }
        Ok(())
    }

    fn restore_pending(&mut self, transfer: Transfer) -> Result<(), RestoreError> {
        let invalid = RestoreError::InvalidPending(transfer.id);
        if self.pending.len() >= usize_of(self.config.max_pending) {
            return Err(RestoreError::TooManyPending);
        }
        if transfer.id.0 >= self.next_transfer {
            return Err(RestoreError::StaleNextTransfer);
        }
        if !transfer.reference.is_durable_while_pending()
            || transfer.amount == 0
            || transfer.debit == transfer.credit
            || self.pending.contains_key(&transfer.id)
        {
            return Err(invalid);
        }
        let debit = self.account(transfer.debit).ok_or(invalid)?;
        let credit = self.account(transfer.credit).ok_or(invalid)?;
        if debit.path.ledger != credit.path.ledger {
            return Err(invalid);
        }
        let amount = u128::from(transfer.amount);
        let max = u128::from(MAX_TOTAL);
        if debit.balance.debits_posted + debit.balance.debits_pending + amount > max
            || credit.balance.credits_posted + credit.balance.credits_pending + amount > max
        {
            return Err(RestoreError::TotalTooLarge);
        }
        self.change_pending(transfer.debit, amount, true);
        self.change_pending(transfer.credit, amount, false);
        self.internal_pending(&transfer, true);
        self.pending.insert(transfer.id, transfer);
        Ok(())
    }

    fn change_pending(&mut self, id: AccountId, amount: u128, debit: bool) {
        let add = |balance: &mut crate::account::Balance| {
            if debit {
                balance.debits_pending += amount;
            } else {
                balance.credits_pending += amount;
            }
        };
        let group = match self.accounts.get_mut(usize_of(id.0)) {
            Some(account) => {
                add(&mut account.balance);
                account.group
            }
            None => None,
        };
        if let Some(root) = group.and_then(|root| self.accounts.get_mut(usize_of(root.0))) {
            add(&mut root.group_balance);
        }
    }
}
