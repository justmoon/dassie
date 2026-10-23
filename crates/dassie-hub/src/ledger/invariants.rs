//! The ledger's invariants, asserted after every block
//! (spec/decisions/tigerstyle.md). The first is the one the hub exists for
//! (spec/designs/settlement-hubs.md#3-the-ledger): every drop the escrow
//! holds for the hub is in exactly one place, so the hub never owes more
//! than the escrow holds.

use super::Ledger;
use super::state::Phase;
use crate::rules::Amount;

impl Ledger {
    /// Everything the hub owes: balances and reserves of accounts, holds
    /// and their reserves, pending withdrawals with their fees, fees not
    /// yet paid out, and the reserve account.
    #[must_use]
    pub fn liabilities(&self) -> Amount {
        let accounts = self
            .accounts
            .values()
            .map(|a| a.balance.saturating_add(a.reserve))
            .fold(0, Amount::saturating_add);
        let holds = self
            .holds
            .values()
            .map(|h| h.amount.saturating_add(h.reserve))
            .fold(0, Amount::saturating_add);
        let withdrawals = self
            .withdrawals
            .values()
            .map(|w| w.amount.saturating_add(w.ledger_fee))
            .fold(0, Amount::saturating_add);
        let pools = self
            .pools
            .iter()
            .map(|p| p.amount)
            .fold(0, Amount::saturating_add);
        accounts
            .saturating_add(holds)
            .saturating_add(withdrawals)
            .saturating_add(pools)
            .saturating_add(self.reserve_account)
    }

    /// Asserts every invariant; a failure is a bug, never an input.
    pub fn check_invariants(&self) {
        assert_eq!(
            self.liabilities(),
            self.escrow.balance,
            "the hub's books do not add up"
        );
        self.check_tables();
        self.check_accounts();
        self.check_holds();
        let window = usize::from(self.rules.terms.limits.vote_window);
        assert!(
            self.pools.len() <= window.saturating_add(1),
            "fee pools beyond the window"
        );
        assert!(
            self.pools
                .windows(2)
                .all(|w| matches!(w, [a, b] if a.height < b.height)),
            "fee pools out of order"
        );
        assert_eq!(
            self.validator_states.len(),
            self.validators.len(),
            "validator states"
        );
        if self.phase == Phase::Ended {
            assert!(
                self.pending_rules.is_none(),
                "an ended hub changes its rules"
            );
        }
    }

    fn check_tables(&self) {
        let limits = &self.rules.terms.limits;
        let bound = |limit: u32| usize::try_from(limit).unwrap_or(usize::MAX);
        // New rules never shrink a table (`Terms::keeps_tables_of`).
        assert!(
            self.members() <= bound(limits.accounts),
            "accounts beyond their bound"
        );
        assert!(
            self.holds.len() <= bound(limits.holds),
            "holds beyond their bound"
        );
        assert!(
            self.withdrawals.len() <= bound(limits.withdrawals),
            "withdrawals beyond their bound"
        );
        assert!(
            self.pending_deposits.len() <= bound(limits.pending_deposits),
            "pending deposits beyond their bound"
        );
        assert!(
            self.recent_deposits.len() <= bound(limits.recent_deposits),
            "recent deposits beyond their bound"
        );
        assert!(
            self.escrow
                .signer_list
                .is_none_or(|s| s < self.escrow.next_sequence),
            "signer list without an escrow sequence"
        );
        assert!(
            self.hold_index == self.derived_hold_index(),
            "hold index out of step"
        );
        assert_eq!(
            self.keys.len(),
            self.accounts.len(),
            "key index out of step"
        );
        for (key, number) in &self.keys {
            assert!(
                self.accounts
                    .get(number)
                    .is_some_and(|a| a.key.as_bytes() == key),
                "key index names the wrong account"
            );
        }
        assert!(
            self.accounts
                .keys()
                .all(|n| n.0 >= 1 && n.0 < self.next_account),
            "account number outside the assigned range"
        );
        assert!(
            self.withdrawals
                .keys()
                .all(|s| *s < self.escrow.next_sequence),
            "withdrawal without an escrow sequence"
        );
        for (hash, ledger) in &self.recent_deposits {
            assert!(
                *ledger >= self.escrow.deposit_floor,
                "remembered deposit below the floor"
            );
            assert!(
                !self.pending_deposits.contains_key(hash),
                "deposit credited and pending"
            );
        }
    }

    fn check_accounts(&self) {
        for (number, account) in &self.accounts {
            let holds = self
                .holds
                .values()
                .filter(|h| h.payer == *number || h.payee == *number)
                .count();
            assert_eq!(usize::from(account.holds), holds, "account's hold count");
            let paying = self.holds.values().filter(|h| h.payer == *number).count();
            assert_eq!(
                usize::from(account.paying),
                paying,
                "account's holds as payer"
            );
            assert!(
                account.paying <= self.rules.terms.limits.holds_per_account,
                "holds as payer beyond their bound"
            );
            let withdrawals = self
                .withdrawals
                .values()
                .filter(|w| w.account == Some(*number))
                .count();
            assert_eq!(
                usize::from(account.withdrawals),
                withdrawals,
                "account's withdrawals"
            );
            if account.validator {
                assert_eq!(account.reserve, 0, "a validator's account locks a reserve");
            }
            if account.closing {
                assert!(
                    account.balance == 0 && account.reserve == 0 && account.withdrawals == 1,
                    "a closing account holds something or has no payout"
                );
            }
        }
    }

    fn check_holds(&self) {
        let room = usize::from(self.rules.terms.limits.claimed_per_hold);
        for hold in self.holds.values() {
            assert_ne!(hold.payer, hold.payee, "a hold pays its own payer");
            assert!(
                self.accounts.contains_key(&hold.payer),
                "a hold outlived its payer"
            );
            assert!(
                self.accounts.contains_key(&hold.payee),
                "a hold outlived its payee"
            );
            assert!(
                hold.claimed.len() <= room,
                "claimed sequences beyond their bound"
            );
            assert!(
                hold.claimed.keys().all(|s| *s > hold.through),
                "a claimed sequence a redeem passed"
            );
        }
    }
}
