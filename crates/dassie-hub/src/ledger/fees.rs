//! Who gets the fees (spec/designs/settlement-hubs.md#7-fees).
//!
//! Different validators can hold certificates of one block signed by
//! different quorums, so fees cannot follow certificates. Instead each
//! block records the votes for the last K blocks its proposer received and
//! no block recorded yet, and the fees of a block go in equal parts to the
//! validators whose votes for it were recorded by the time K more blocks
//! were made. That is in the log, the same for every validator. A share of
//! every fee goes to the reserve account instead, which pays the hub's own
//! fees on the underlying ledger.

use alloc::collections::BTreeSet;

use super::state::FeePool;
use super::{Block, BlockError, Ledger};
use crate::rules::Amount;

impl Ledger {
    /// Splits a fee between the reserve account and this block's pool.
    pub(super) fn collect_fee(&mut self, fee: Amount) {
        let share = u128::from(fee)
            .saturating_mul(u128::from(self.rules.terms.fees.reserve_share_ppm))
            / 1_000_000;
        let to_reserve = Amount::try_from(share).unwrap_or(fee).min(fee);
        self.reserve_account = self.reserve_account.saturating_add(to_reserve);
        let to_pool = fee.saturating_sub(to_reserve);
        let height = self.height;
        match self.pools.last_mut() {
            Some(pool) if pool.height == height => {
                pool.amount = pool.amount.saturating_add(to_pool)
            }
            _ => self.pools.push(FeePool {
                height,
                amount: to_pool,
                voters: crate::validators::Signers::NONE,
            }),
        }
    }

    /// The oldest height a block may still record votes for.
    fn vote_floor(&self, height: u64) -> u64 {
        height.saturating_sub(u64::from(self.rules.terms.limits.vote_window))
    }

    /// Checks the votes a block records: each for an earlier block within
    /// the window, each height once, and none recorded before.
    pub(super) fn check_votes(&self, block: &Block<'_>) -> Result<(), BlockError> {
        let floor = self.vote_floor(block.height);
        let n = self.validators.len();
        let mut seen = BTreeSet::new();
        for (height, signers) in block.votes {
            let fresh = *height >= floor.max(1)
                && *height < block.height
                && seen.insert(*height)
                && signers.count() > 0
                && signers.fits(n);
            let recorded = self
                .pools
                .iter()
                .find(|pool| pool.height == *height)
                .is_some_and(|pool| pool.voters.bits() & signers.bits() != 0);
            if !fresh || recorded {
                return Err(BlockError::Votes);
            }
        }
        Ok(())
    }

    /// Records the block's votes in the pools they are for, and when each
    /// voter was last seen taking part.
    pub(super) fn record_votes(&mut self, block: &Block<'_>) {
        for (height, signers) in block.votes {
            match self.pools.iter_mut().find(|pool| pool.height == *height) {
                Some(pool) => pool.voters = pool.voters.with_all(*signers),
                // A block that collected no fees still had voters: it gets
                // a pool, so they count as taking part.
                None => {
                    let at = self.pools.partition_point(|pool| pool.height < *height);
                    self.pools.insert(
                        at,
                        FeePool {
                            height: *height,
                            amount: 0,
                            voters: *signers,
                        },
                    );
                }
            }
            for index in signers.iter() {
                if let Some(state) = self.validator_states.get_mut(index.get()) {
                    state.last_recorded = block.timestamp;
                }
            }
        }
    }

    /// What closes a block: pools whose window ended are paid out, and
    /// deposits that waited are credited.
    pub(super) fn end_block(&mut self) {
        let floor = self.vote_floor(self.height);
        while self.pools.first().is_some_and(|pool| pool.height <= floor) {
            let pool = self.pools.remove(0);
            self.distribute(pool);
        }
        self.credit_deposits();
        self.settle_withdrawals();
    }

    /// Pays out every pool now, as when the validator set changes.
    pub(super) fn distribute_all_pools(&mut self) {
        for pool in core::mem::take(&mut self.pools) {
            self.distribute(pool);
        }
    }

    /// Pays a pool in equal parts to its voters; what does not divide, and
    /// a pool no one voted for, goes to the reserve account.
    fn distribute(&mut self, pool: FeePool) {
        let voters = u64::try_from(pool.voters.count()).unwrap_or(0);
        let part = pool.amount.checked_div(voters).unwrap_or(0);
        let mut paid: Amount = 0;
        for index in pool.voters.iter() {
            let Some(account) = self.validator_states.get(index.get()).map(|v| v.account) else {
                continue;
            };
            if self.accounts.contains_key(&account) {
                self.credit(account, part);
                paid = paid.saturating_add(part);
            }
        }
        self.reserve_account = self
            .reserve_account
            .saturating_add(pool.amount.saturating_sub(paid));
    }
}
