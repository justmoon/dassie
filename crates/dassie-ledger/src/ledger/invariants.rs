//! What holds after every step. Tests and simulations check it after each
//! transition (D16).
//!
//! One pass over the pending transfers and one over the accounts, so the
//! check costs what the tables hold, once.

// Its job is to crash on a broken invariant.
#![allow(clippy::panic)]

use alloc::collections::BTreeMap;
use alloc::vec;
use alloc::vec::Vec;

use super::{Ledger, usize_of};
use crate::account::Balance;
use crate::ledger_id::LedgerId;
use crate::{AccountId, MAX_TOTAL};

/// What the pending transfers add up to: per account, its pending debits
/// and credits; per group root, the pending between its members.
struct PendingSums {
    accounts: Vec<[u128; 2]>,
    internal: BTreeMap<AccountId, u128>,
}

impl Ledger {
    /// Asserts the ledger's invariants:
    ///
    /// - every ledger balances, posted and pending;
    /// - the pending counters are exactly the pending transfers;
    /// - each group's total is the sum of its members;
    /// - no counter passes [`MAX_TOTAL`], and the bounds hold;
    /// - the path index matches the accounts.
    ///
    /// Limits are not invariants: reports may break them
    /// (spec/areas/ledger.md#what-the-ledger-refuses-and-what-it-records).
    ///
    /// # Panics
    ///
    /// If one does not hold.
    pub fn check_invariants(&self) {
        assert!(self.accounts.len() <= usize_of(self.config.max_accounts));
        assert!(self.pending.len() <= usize_of(self.config.max_pending));
        assert!(self.journal.len() <= usize_of(self.config.max_journal));
        assert_eq!(self.by_path.len(), self.accounts.len(), "path index");
        let sums = self.pending_sums();
        let groups = self.check_accounts(&sums);
        self.check_groups(&groups, &sums.internal);
    }

    fn pending_sums(&self) -> PendingSums {
        let mut sums = PendingSums {
            accounts: vec![[0; 2]; self.accounts.len()],
            internal: BTreeMap::new(),
        };
        let group = |id: AccountId| self.account(id).and_then(|account| account.group);
        for (&id, transfer) in &self.pending {
            assert_eq!(id, transfer.id);
            assert!(
                id.0 < self.next_transfer,
                "pending transfer from the future"
            );
            assert!(transfer.amount > 0);
            assert_ne!(transfer.debit, transfer.credit);
            let amount = u128::from(transfer.amount);
            for (account, side) in [(transfer.debit, 0), (transfer.credit, 1)] {
                let sum = sums.accounts.get_mut(usize_of(account.0));
                let Some(sum) = sum.and_then(|sum| sum.get_mut(side)) else {
                    panic!("pending transfer on an unknown account");
                };
                *sum += amount;
            }
            if let Some(root) = group(transfer.debit)
                && group(transfer.credit) == Some(root)
            {
                *sums.internal.entry(root).or_default() += amount;
            }
        }
        sums
    }

    /// Checks each account, and returns the sums of each group's members.
    fn check_accounts(&self, sums: &PendingSums) -> BTreeMap<AccountId, Balance> {
        let mut ledgers = [Balance::default(); LedgerId::ALL.len()];
        let mut groups = BTreeMap::<AccountId, Balance>::new();
        for ((id, account), [debits, credits]) in self.accounts().zip(&sums.accounts) {
            assert_eq!(self.by_path.get(&account.path), Some(&id), "path index");
            let balance = account.balance;
            for counter in [
                balance.debits_posted + balance.debits_pending,
                balance.credits_posted + balance.credits_pending,
            ] {
                assert!(counter <= u128::from(MAX_TOTAL), "counter past MAX_TOTAL");
            }
            assert_eq!(balance.debits_pending, *debits, "{}", account.path);
            assert_eq!(balance.credits_pending, *credits, "{}", account.path);
            let ledger = LedgerId::ALL.iter().position(|l| *l == account.path.ledger);
            if let Some(total) = ledger.and_then(|index| ledgers.get_mut(index)) {
                add(total, &balance);
            }
            if account.group != Some(id) {
                assert_eq!(
                    account.group_balance,
                    Balance::default(),
                    "{}",
                    account.path
                );
                assert_eq!(account.group_internal_pending, 0, "{}", account.path);
            }
            if let Some(root) = account.group {
                add(groups.entry(root).or_default(), &balance);
            }
        }
        for (ledger, total) in LedgerId::ALL.iter().zip(ledgers) {
            assert_eq!(total.debits_posted, total.credits_posted, "{ledger} posted");
            assert_eq!(
                total.debits_pending, total.credits_pending,
                "{ledger} pending"
            );
        }
        groups
    }

    fn check_groups(
        &self,
        groups: &BTreeMap<AccountId, Balance>,
        internal: &BTreeMap<AccountId, u128>,
    ) {
        for (&root_id, total) in groups {
            let root = self.account(root_id);
            let Some(root) = root.filter(|root| root.group == Some(root_id)) else {
                panic!("a group without its root");
            };
            assert_eq!(root.group_balance, *total, "group {}", root.path);
            let pending = internal.get(&root_id).copied().unwrap_or_default();
            assert_eq!(root.group_internal_pending, pending, "group {}", root.path);
        }
        for member in self.accounts.iter().filter(|a| a.group.is_some()) {
            let root = member.group.and_then(|root| self.account(root));
            assert_eq!(
                member.path.group_root(),
                root.map(|root| root.path),
                "{}",
                member.path
            );
        }
    }
}

fn add(total: &mut Balance, balance: &Balance) {
    total.debits_posted += balance.debits_posted;
    total.credits_posted += balance.credits_posted;
    total.debits_pending += balance.debits_pending;
    total.credits_pending += balance.credits_pending;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{AccountName, AccountPath, Config, OwnerSub, Reference, TransferSpec};

    fn ledger() -> (Ledger, AccountId, AccountId) {
        let mut ledger = Ledger::new(Config::DEFAULT);
        let mut open = |name| {
            let path = AccountPath::new(LedgerId::StubUsd, name);
            ledger.open(path).unwrap()
        };
        let settlement = open(AccountName::AssetsSettlement);
        let owner = open(AccountName::EquityOwner);
        let token = open(AccountName::EquityOwnerSub(OwnerSub::Btp(1)));
        let spec = |debit, credit, pending| TransferSpec {
            debit,
            credit,
            amount: 10,
            pending,
            reference: Reference::Deposit,
        };
        ledger.create(0, &spec(settlement, owner, false)).unwrap();
        ledger.create(0, &spec(owner, token, true)).unwrap();
        ledger.check_invariants();
        (ledger, owner, token)
    }

    #[test]
    #[should_panic(expected = "group")]
    fn a_wrong_group_total_is_found() {
        let (mut ledger, owner, _) = ledger();
        ledger.accounts[usize_of(owner.0)]
            .group_balance
            .credits_posted += 1;
        ledger.check_invariants();
    }

    #[test]
    #[should_panic(expected = "group")]
    fn wrong_pending_inside_a_group_is_found() {
        let (mut ledger, owner, _) = ledger();
        ledger.accounts[usize_of(owner.0)].group_internal_pending = 0;
        ledger.check_invariants();
    }

    #[test]
    #[should_panic(expected = "`left == right`")]
    fn a_wrong_pending_counter_is_found() {
        let (mut ledger, _, token) = ledger();
        ledger.accounts[usize_of(token.0)].balance.credits_pending = 0;
        ledger.check_invariants();
    }
}
