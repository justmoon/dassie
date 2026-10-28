//! Taking from holds (spec/designs/settlement-hubs.md#4-peering-over-a-hub):
//! `redeem` with the payer's latest claim, `claim` with fulfilled
//! commitments when the claim does not come, and the reductions a payer
//! asked for, once their notice passed.
//!
//! A payee is paid only for what its payer signed: a claim, or a
//! commitment whose fulfillment is in a block stamped before the
//! commitment's expiry. Nothing is paid twice: a hold remembers the
//! sequences claimed above the last redeemed W until a redeem passes them,
//! and a redeem pays what its claim adds less what `claim` already took up
//! to its W.
//!
//! From version 2 on, what `claim` takes waits in the hold, all of it paid
//! once the hold's notice has passed since the last claim, and a payer that
//! shows the payee's release of a commitment first puts what the claim took
//! for it back in the hold (spec/decisions/signed-rejects.md). A redeem
//! leaves what waits as it is: it subtracts what `claim` took up to its W,
//! paid or waiting, as in version 1, so the payee is paid the payer's total
//! whatever it claimed; and a dispute is only of a sequence above the last
//! redeemed W, so it never takes back what a redeem counted as paid.

use alloc::collections::BTreeSet;
use alloc::vec::Vec;

use super::Ledger;
use super::outcome::{Detail, Failure};
use crate::operation::AccountNumber;
use crate::promise::{Claim, Fulfilled, HoldNumber, Release, Signed};
use crate::rules::Amount;

impl Ledger {
    /// The hold `number` if `payee` may take from it, with its payer's key.
    fn payee_hold(
        &self,
        payee: AccountNumber,
        number: HoldNumber,
    ) -> Result<dassie_identity::PublicKey, Failure> {
        let hold = self.holds.get(&number).ok_or(Failure::Missing)?;
        if hold.payee != payee {
            return Err(Failure::NotYours);
        }
        // A payer cannot close while it has holds.
        let payer = self.accounts.get(&hold.payer);
        assert!(payer.is_some(), "a hold outlived its payer");
        payer.map(|account| account.key).ok_or(Failure::Missing)
    }

    pub(super) fn redeem(
        &mut self,
        from: AccountNumber,
        claim: &Signed<Claim>,
    ) -> Result<Detail, Failure> {
        let number = claim.value.hold;
        let payer = self.payee_hold(from, number)?;
        if !claim.verifies(&self.hub, &payer) {
            return Err(Failure::Claim);
        }
        let hold = self.holds.get_mut(&number).ok_or(Failure::Missing)?;
        if claim.value.total < hold.total || claim.value.through < hold.through {
            return Err(Failure::Claim);
        }
        let claimed: Amount = hold
            .claimed
            .range(..=claim.value.through)
            .map(|(_, amount)| *amount)
            .fold(0, Amount::saturating_add);
        let added = claim.value.total.saturating_sub(hold.total);
        let paid = added.saturating_sub(claimed).min(hold.amount);
        hold.amount = hold.amount.saturating_sub(paid);
        hold.total = claim.value.total;
        hold.through = claim.value.through;
        // Every sequence up to W is covered now, `u64::MAX` included: what
        // waits for them is the payee's, and no release takes it back.
        let through = claim.value.through;
        hold.claimed.retain(|sequence, _| *sequence > through);
        self.credit(from, paid);
        Ok(Detail::Paid(paid))
    }

    pub(super) fn claim(
        &mut self,
        from: AccountNumber,
        number: HoldNumber,
        fulfilled: &[Fulfilled],
    ) -> Result<Detail, Failure> {
        let payer = self.payee_hold(from, number)?;
        let limits = &self.rules.terms.limits;
        if fulfilled.is_empty() || fulfilled.len() > usize::from(limits.commitments_per_claim) {
            return Err(Failure::Claim);
        }
        let room = usize::from(limits.claimed_per_hold);
        let hold = self.holds.get(&number).ok_or(Failure::Missing)?;
        if hold.claimed.len().saturating_add(fulfilled.len()) > room {
            return Err(Failure::Full);
        }
        let mut sequences = BTreeSet::new();
        for item in fulfilled {
            let commitment = &item.commitment.value;
            let valid = commitment.hold == number
                && commitment.sequence > hold.through
                && !hold.claimed.contains_key(&commitment.sequence)
                && sequences.insert(commitment.sequence)
                && self.timestamp < commitment.expiry
                && commitment.fulfilled_by(&item.fulfillment)
                && item.commitment.verifies(&self.hub, &payer);
            if !valid {
                return Err(Failure::Claim);
            }
        }
        let waits = self.takes_releases();
        let paid = self.pay_claimed(number, fulfilled, waits);
        if waits {
            // Only a claim that took something moves the payout of what
            // waits.
            let due = self
                .clock()
                .saturating_add(self.rules.terms.timers.hold_notice);
            if let Some(hold) = self.holds.get_mut(&number).filter(|_| paid > 0) {
                hold.pending_due = due;
            }
            return Ok(Detail::Pending(paid));
        }
        self.credit(from, paid);
        Ok(Detail::Paid(paid))
    }

    /// The payer's dispute of a claim with the payee's releases: what the
    /// claim took for each released commitment that still waits goes back
    /// to the hold. The sequence stays claimed, for nothing, so it is never
    /// claimed again (spec/decisions/signed-rejects.md). Every release must
    /// be the payee's, of a commitment on the hold; one of a sequence up to
    /// the last redeemed W finds nothing, since the redeem counted what the
    /// claim took as paid.
    pub(super) fn dispute(
        &mut self,
        from: AccountNumber,
        number: HoldNumber,
        releases: &[Signed<Release>],
    ) -> Result<Detail, Failure> {
        let limit = usize::from(self.rules.terms.limits.commitments_per_claim);
        if releases.is_empty() || releases.len() > limit {
            return Err(Failure::Claim);
        }
        let hold = self.holds.get(&number).ok_or(Failure::Missing)?;
        if hold.payer != from {
            return Err(Failure::NotYours);
        }
        let payee = self.accounts.get(&hold.payee);
        assert!(payee.is_some(), "a hold outlived its payee");
        let key = payee.map(|account| account.key).ok_or(Failure::Missing)?;
        let valid = releases
            .iter()
            .all(|release| release.value.hold == number && release.verifies(&self.hub, &key));
        if !valid {
            return Err(Failure::Claim);
        }
        let hold = self.holds.get_mut(&number).ok_or(Failure::Missing)?;
        let mut returned: Amount = 0;
        let mut found = false;
        for release in releases {
            let sequence = release.value.sequence;
            if sequence <= hold.through {
                continue;
            }
            if let Some(amount) = hold.pending.remove(&sequence) {
                hold.amount = hold.amount.saturating_add(amount);
                hold.claimed.insert(sequence, 0);
                returned = returned.saturating_add(amount);
                found = true;
            }
        }
        if hold.pending.is_empty() {
            hold.pending_due = 0;
        }
        if !found {
            return Err(Failure::Missing);
        }
        Ok(Detail::Returned(returned))
    }

    /// Pays what waits in each hold once the hold's notice has passed
    /// since its last claim, by the hub's clock, which stands still while
    /// the hub is frozen.
    pub(super) fn pay_pending_claims(&mut self) {
        let clock = self.clock();
        let mut paid: Vec<(AccountNumber, Amount)> = Vec::new();
        for hold in self.holds.values_mut() {
            if hold.pending.is_empty() || hold.pending_due > clock {
                continue;
            }
            paid.push((hold.payee, hold.pending_amount()));
            hold.pending.clear();
            hold.pending_due = 0;
        }
        for (payee, amount) in paid {
            self.credit(payee, amount);
        }
    }

    /// Takes the commitments from the hold in sequence order, as far as the
    /// hold covers them, and remembers each sequence with what it took:
    /// waiting in the hold if `waits`, paid at once if not.
    fn pay_claimed(&mut self, number: HoldNumber, fulfilled: &[Fulfilled], waits: bool) -> Amount {
        let mut ordered: Vec<(u64, Amount)> = fulfilled
            .iter()
            .map(|item| (item.commitment.value.sequence, item.commitment.value.amount))
            .collect();
        ordered.sort_unstable();
        let Some(hold) = self.holds.get_mut(&number) else {
            return 0;
        };
        let mut paid: Amount = 0;
        for (sequence, amount) in ordered {
            let pay = amount.min(hold.amount);
            hold.amount = hold.amount.saturating_sub(pay);
            hold.claimed.insert(sequence, pay);
            if waits && pay > 0 {
                hold.pending.insert(sequence, pay);
            }
            paid = paid.saturating_add(pay);
        }
        paid
    }

    /// Applies every reduction whose notice passed by the hub's clock, which
    /// stands still while the hub is frozen. A hold reduced to nothing
    /// closes, and frees its reserve. A reduction waits while claims of the
    /// hold wait for their payout, so a hold never closes from under what
    /// its payer may still dispute.
    pub(super) fn apply_reductions(&mut self) {
        let clock = self.clock();
        let due: Vec<HoldNumber> = self
            .holds
            .iter()
            .filter(|(_, hold)| hold.pending.is_empty())
            .filter(|(_, hold)| hold.reduction.is_some_and(|r| r.at <= clock))
            .map(|(number, _)| *number)
            .collect();
        for number in due {
            let Some(hold) = self.holds.get_mut(&number) else {
                continue;
            };
            let amount = hold.reduction.map_or(0, |r| r.amount).min(hold.amount);
            hold.amount = hold.amount.saturating_sub(amount);
            hold.reduction = None;
            let payer = hold.payer;
            let empty = hold.amount == 0;
            self.credit(payer, amount);
            if empty {
                self.close_hold(number);
            }
        }
    }

    /// Closes a hold: what is left in it and its reserve go back to the
    /// payer.
    pub(super) fn close_hold(&mut self, number: HoldNumber) {
        let Some(hold) = self.holds.remove(&number) else {
            return;
        };
        if let Some(payer) = self.accounts.get_mut(&hold.payer) {
            payer.balance = payer
                .balance
                .saturating_add(hold.amount)
                .saturating_add(hold.reserve);
        }
        self.hold_index.remove(&(hold.payer, number));
        self.hold_index.remove(&(hold.payee, number));
        for party in [hold.payer, hold.payee] {
            if let Some(account) = self.accounts.get_mut(&party) {
                account.holds = account.holds.saturating_sub(1);
                if party == hold.payer {
                    account.paying = account.paying.saturating_sub(1);
                }
            }
        }
    }
}
