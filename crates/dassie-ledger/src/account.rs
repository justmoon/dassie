//! Accounts and their counters.

use crate::path::AccountPath;

/// An account's index in its ledger. Dense, assigned in opening order, and
/// persisted, so that journal rows can refer to accounts by number.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct AccountId(pub u32);

/// What an account's (or a group's) counters may never do.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Limit {
    /// Nothing.
    None,
    /// Posted and pending debits stay within posted credits: an asset that
    /// may not go below zero, counting money in flight out but not in.
    DebitsMustNotExceedCredits,
    /// Posted and pending credits stay within posted debits.
    CreditsMustNotExceedDebits,
}

/// The four counters of an account or a group. Wide enough for a group's
/// sum; an account's own counters never pass [`crate::MAX_TOTAL`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Balance {
    /// Posted debits.
    pub debits_posted: u128,
    /// Posted credits.
    pub credits_posted: u128,
    /// Debits of pending transfers.
    pub debits_pending: u128,
    /// Credits of pending transfers.
    pub credits_pending: u128,
}

impl Balance {
    /// Posted credits minus posted debits: the balance of an equity,
    /// revenue or liability-like account.
    #[must_use]
    pub fn net_credit(&self) -> i128 {
        signed(self.credits_posted) - signed(self.debits_posted)
    }

    /// Whether `limit` allows `debit` more debits and `credit` more credits,
    /// pending.
    pub(crate) fn allows(&self, limit: Limit, debit: u128, credit: u128) -> bool {
        match limit {
            Limit::None => true,
            Limit::DebitsMustNotExceedCredits => {
                debit == 0
                    || self.debits_posted + self.debits_pending + debit <= self.credits_posted
            }
            Limit::CreditsMustNotExceedDebits => {
                credit == 0
                    || self.credits_posted + self.credits_pending + credit <= self.debits_posted
            }
        }
    }

    /// Whether the counters keep to `limit` now.
    #[must_use]
    pub fn within(&self, limit: Limit) -> bool {
        match limit {
            Limit::None => true,
            Limit::DebitsMustNotExceedCredits => {
                self.debits_posted + self.debits_pending <= self.credits_posted
            }
            Limit::CreditsMustNotExceedDebits => {
                self.credits_posted + self.credits_pending <= self.debits_posted
            }
        }
    }
}

/// Counters never pass `i64::MAX`, so this never saturates.
fn signed(value: u128) -> i128 {
    i128::try_from(value).unwrap_or(i128::MAX)
}

/// An account.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Account {
    pub(crate) path: AccountPath,
    pub(crate) balance: Balance,
    /// The root of the account's group, which may be the account itself.
    pub(crate) group: Option<AccountId>,
    /// For a group root, the group's total, itself included.
    pub(crate) group_balance: Balance,
    /// For a group root, the amount of pending transfers between members.
    /// They count as pending debits and credits of the group alike; the
    /// limit looks past them, since they cannot change the group's total.
    pub(crate) group_internal_pending: u128,
}

impl Account {
    pub(crate) fn new(path: AccountPath, group: Option<AccountId>) -> Self {
        Self {
            path,
            balance: Balance::default(),
            group,
            group_balance: Balance::default(),
            group_internal_pending: 0,
        }
    }

    /// For a group root, the group's total as the outside sees it: without
    /// pending transfers between members.
    pub(crate) fn group_view(&self) -> Balance {
        Balance {
            debits_pending: self.group_balance.debits_pending - self.group_internal_pending,
            credits_pending: self.group_balance.credits_pending - self.group_internal_pending,
            ..self.group_balance
        }
    }

    /// The account's path.
    #[must_use]
    pub const fn path(&self) -> &AccountPath {
        &self.path
    }

    /// The account's counters.
    #[must_use]
    pub const fn balance(&self) -> &Balance {
        &self.balance
    }

    /// The root of the account's group.
    #[must_use]
    pub const fn group(&self) -> Option<AccountId> {
        self.group
    }
}
