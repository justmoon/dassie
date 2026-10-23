//! A hold as a member follows it: what the hub's ledger does to it
//! (`ledger/holds.rs`), replayed from the operations executed on it, in
//! the order of the log. Amounts paid come from the outcomes, so the mirror
//! never has to guess what a redeem or a claim took.

use alloc::collections::BTreeMap;
use alloc::vec::Vec;

use crate::client::HoldView;
use crate::operation::AccountNumber;
use crate::promise::{Claim, Fulfilled};
use crate::rules::{Amount, Millis};

/// A hold the member pays into or takes from, as of the last block it read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HoldMirror {
    /// Who set the funds aside.
    pub payer: AccountNumber,
    /// Who may take from them.
    pub payee: AccountNumber,
    /// What is left in it.
    pub amount: Amount,
    /// A reduction waiting for its notice: how much, and from when.
    pub reduction: Option<(Amount, Millis)>,
    /// T of the last redeemed claim.
    pub total: Amount,
    /// W of the last redeemed claim.
    pub through: u64,
    /// Sequences above `through` taken with `claim`, and what each paid.
    pub claimed: BTreeMap<u64, Amount>,
}

impl HoldMirror {
    /// A hold just opened with `amount`.
    #[must_use]
    pub const fn opened(payer: AccountNumber, payee: AccountNumber, amount: Amount) -> Self {
        Self {
            payer,
            payee,
            amount,
            reduction: None,
            total: 0,
            through: 0,
            claimed: BTreeMap::new(),
        }
    }

    /// A hold as a statement told it: what `claim` took above its W is
    /// not in the view, and starts empty.
    #[must_use]
    pub const fn of(view: &HoldView) -> Self {
        Self {
            payer: view.payer,
            payee: view.payee,
            amount: view.amount,
            reduction: view.reduction,
            total: view.total,
            through: view.through,
            claimed: BTreeMap::new(),
        }
    }

    /// Everything the payee has taken from it: the last redeemed total,
    /// and what `claim` paid above its W. A redeem pays what its claim adds
    /// less what was claimed up to its W, so this sum only grows, by what
    /// each take moves out of the hold.
    #[must_use]
    pub fn taken(&self) -> Amount {
        self.claimed
            .values()
            .fold(self.total, |sum, paid| sum.saturating_add(*paid))
    }

    /// What `claim` took above the last redeemed W, up to `through`.
    #[must_use]
    pub fn claimed_through(&self, through: u64) -> Amount {
        self.claimed
            .range(..=through)
            .fold(0, |sum, (_, paid)| sum.saturating_add(*paid))
    }

    /// A `hold-fund` of `amount` applied.
    pub const fn fund(&mut self, amount: Amount) {
        self.amount = self.amount.saturating_add(amount);
    }

    /// A `hold-reduce` applied in a block stamped `timestamp`: it takes
    /// effect after `notice`, and replaces any earlier one.
    pub const fn reduce(&mut self, amount: Amount, timestamp: Millis, notice: Millis) {
        self.reduction = if amount > 0 {
            Some((amount, timestamp.saturating_add(notice)))
        } else {
            None
        };
    }

    /// Applies a reduction due by a block stamped `timestamp`, as the
    /// ledger does before the block's operations. Returns what went back
    /// to the payer.
    pub fn apply_due(&mut self, timestamp: Millis) -> Option<Amount> {
        let (amount, at) = self.reduction?;
        if at > timestamp {
            return None;
        }
        let amount = amount.min(self.amount);
        self.amount = self.amount.saturating_sub(amount);
        self.reduction = None;
        Some(amount)
    }

    /// A `redeem` with `claim` applied, which paid `paid`.
    pub fn redeem(&mut self, claim: &Claim, paid: Amount) {
        self.amount = self.amount.saturating_sub(paid);
        self.total = claim.total;
        self.through = claim.through;
        let through = claim.through;
        self.claimed.retain(|sequence, _| *sequence > through);
    }

    /// A `claim` of `fulfilled` applied, which paid `paid`: each commitment
    /// in sequence order, as far as what was paid covers it, as the ledger
    /// pays them as far as the hold covers them.
    pub fn claim(&mut self, fulfilled: &[Fulfilled], paid: Amount) {
        let mut ordered: Vec<(u64, Amount)> = fulfilled
            .iter()
            .map(|item| (item.commitment.value.sequence, item.commitment.value.amount))
            .collect();
        ordered.sort_unstable();
        let mut left = paid;
        for (sequence, amount) in ordered {
            let pay = amount.min(left);
            left = left.saturating_sub(pay);
            self.claimed.insert(sequence, pay);
        }
        self.amount = self.amount.saturating_sub(paid);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::promise::{Commitment, HoldNumber, Signed};
    use dassie_identity::Signature;

    fn fulfilled(sequence: u64, amount: Amount) -> Fulfilled {
        Fulfilled {
            commitment: Signed {
                value: Commitment {
                    hold: HoldNumber(1),
                    sequence,
                    amount,
                    condition: [0; 32],
                    expiry: 0,
                },
                signature: Signature::from_bytes([0; 64]),
            },
            fulfillment: [0; 32],
        }
    }

    // What the payee took only grows, by what each take moves out.
    #[test]
    fn takes_move_money_out_of_the_hold_once() {
        let mut hold = HoldMirror::opened(AccountNumber(1), AccountNumber(2), 1_000);
        hold.claim(&[fulfilled(3, 100), fulfilled(2, 50)], 150);
        assert_eq!((hold.amount, hold.taken()), (850, 150));
        assert_eq!(hold.claimed_through(2), 50);
        // A redeem through 3 pays what its claim adds less what was claimed
        // up to 3: the ledger says 100.
        let claim = Claim {
            hold: HoldNumber(1),
            total: 250,
            through: 3,
        };
        hold.redeem(&claim, 100);
        assert_eq!((hold.amount, hold.taken()), (750, 250));
        assert!(hold.claimed.is_empty());
    }

    #[test]
    fn a_claim_the_hold_could_not_cover_pays_in_sequence_order() {
        let mut hold = HoldMirror::opened(AccountNumber(1), AccountNumber(2), 120);
        hold.claim(&[fulfilled(5, 100), fulfilled(4, 100)], 120);
        assert_eq!(hold.claimed.get(&4), Some(&100));
        assert_eq!(hold.claimed.get(&5), Some(&20));
        assert_eq!(hold.amount, 0);
    }

    #[test]
    fn a_reduction_waits_for_its_notice() {
        let mut hold = HoldMirror::opened(AccountNumber(1), AccountNumber(2), 100);
        hold.reduce(30, 1_000, 60_000);
        assert_eq!(hold.apply_due(60_999), None);
        assert_eq!(hold.apply_due(61_000), Some(30));
        assert_eq!((hold.amount, hold.reduction), (70, None));
        hold.reduce(500, 2_000, 10);
        assert_eq!(hold.apply_due(2_010), Some(70), "never more than is left");
    }
}
