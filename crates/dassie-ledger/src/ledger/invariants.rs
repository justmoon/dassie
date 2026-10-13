//! What holds after every step. Tests and simulations check it after each
//! transition (D16).

use alloc::vec;

use super::{Ledger, usize_of};
use crate::MAX_TOTAL;
use crate::account::Balance;
use crate::ledger_id::LedgerId;

impl Ledger {
    /// Asserts the ledger's invariants:
    ///
    /// - every ledger balances, posted and pending;
    /// - the pending counters are exactly the pending transfers;
    /// - each group's total is the sum of its members;
    /// - no counter passes [`MAX_TOTAL`], and the bounds hold;
    /// - the path index matches the accounts.
    ///
    /// Limits are not invariants: reports may break them (spec 7 §4.5).
    ///
    /// # Panics
    ///
    /// If one does not hold.
    pub fn check_invariants(&self) {
        assert!(self.accounts.len() <= usize_of(self.config.max_accounts));
        assert!(self.pending.len() <= usize_of(self.config.max_pending));
        assert!(self.journal.len() <= usize_of(self.config.max_journal));
        assert_eq!(self.by_path.len(), self.accounts.len(), "path index");
        for (id, account) in self.accounts() {
            assert_eq!(self.by_path.get(&account.path), Some(&id), "path index");
        }
        self.check_balanced();
        self.check_pending();
        self.check_groups();
    }

    fn check_balanced(&self) {
        for ledger in LedgerId::ALL {
            let mut total = Balance::default();
            for account in self.accounts.iter().filter(|a| a.path.ledger == ledger) {
                let balance = account.balance;
                for counter in [
                    balance.debits_posted + balance.debits_pending,
                    balance.credits_posted + balance.credits_pending,
                ] {
                    assert!(counter <= u128::from(MAX_TOTAL), "counter past MAX_TOTAL");
                }
                total.debits_posted += balance.debits_posted;
                total.credits_posted += balance.credits_posted;
                total.debits_pending += balance.debits_pending;
                total.credits_pending += balance.credits_pending;
            }
            assert_eq!(total.debits_posted, total.credits_posted, "{ledger} posted");
            assert_eq!(
                total.debits_pending, total.credits_pending,
                "{ledger} pending"
            );
        }
    }

    fn check_pending(&self) {
        let mut debits = vec![0u128; self.accounts.len()];
        let mut credits = vec![0u128; self.accounts.len()];
        for (&id, transfer) in &self.pending {
            assert_eq!(id, transfer.id);
            assert!(
                id.0 < self.next_transfer,
                "pending transfer from the future"
            );
            assert!(transfer.amount > 0);
            assert_ne!(transfer.debit, transfer.credit);
            if let Some(debit) = debits.get_mut(usize_of(transfer.debit.0)) {
                *debit += u128::from(transfer.amount);
            } else {
                panic!("pending transfer on an unknown account");
            }
            if let Some(credit) = credits.get_mut(usize_of(transfer.credit.0)) {
                *credit += u128::from(transfer.amount);
            } else {
                panic!("pending transfer on an unknown account");
            }
        }
        for ((account, debit), credit) in self.accounts.iter().zip(debits).zip(credits) {
            assert_eq!(account.balance.debits_pending, debit, "{}", account.path);
            assert_eq!(account.balance.credits_pending, credit, "{}", account.path);
        }
    }

    fn check_groups(&self) {
        for (id, root) in self.accounts() {
            let is_root = root.group == Some(id);
            if !is_root {
                assert_eq!(root.group_balance, Balance::default(), "{}", root.path);
                assert_eq!(root.group_internal_pending, 0, "{}", root.path);
                if let Some(group) = root.group {
                    let parent = self.account(group);
                    assert!(
                        parent.is_some_and(|parent| parent.group == Some(group)),
                        "{} is in a group without a root",
                        root.path
                    );
                }
                continue;
            }
            let mut total = Balance::default();
            for member in self.accounts.iter().filter(|a| a.group == Some(id)) {
                assert_eq!(member.path.group_root(), Some(root.path), "{}", member.path);
                total.debits_posted += member.balance.debits_posted;
                total.credits_posted += member.balance.credits_posted;
                total.debits_pending += member.balance.debits_pending;
                total.credits_pending += member.balance.credits_pending;
            }
            assert_eq!(root.group_balance, total, "group {}", root.path);
            let group = |id| self.account(id).and_then(|account| account.group);
            let internal: u128 = self
                .pending
                .values()
                .filter(|t| group(t.debit) == Some(id) && group(t.credit) == Some(id))
                .map(|t| u128::from(t.amount))
                .sum();
            assert_eq!(root.group_internal_pending, internal, "group {}", root.path);
        }
    }
}
