//! A hold as a member follows it: what the hub's ledger does to it
//! (`ledger/holds.rs`), replayed from the operations executed on it, in
//! the order of the log. Amounts paid come from the outcomes, so the mirror
//! never has to guess what a redeem or a claim took.
//!
//! From version 2 on, what a claim took waits in the hold until the hold's
//! notice has passed since the last claim, and the payer may put some of it
//! back with a dispute (spec/decisions/signed-rejects.md): the mirror pays
//! it out by the hub's clock, as it applies a reduction.

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
    /// When the reduction waiting was made: the stamp of the block whose
    /// `hold-reduce` made it, or, for one read anew from the hub's holds,
    /// the stamp up to which they reflect the hub's thaws. A thaw moves
    /// only a reduction made at or before its freeze's stamp, as the hub's
    /// does: a frozen hub takes no `hold-reduce`, so its thaw finds only
    /// those.
    pub reduced_at: Millis,
    /// T of the last redeemed claim.
    pub total: Amount,
    /// W of the last redeemed claim.
    pub through: u64,
    /// Sequences above `through` taken with `claim`, and what each paid.
    pub claimed: BTreeMap<u64, Amount>,
    /// What claims took that waits to pay the payee, by sequence, with
    /// the claimed commitment's expiry, as read in order.
    pub pending: BTreeMap<u64, (Amount, Millis)>,
    /// What waits that the hub's holds told without its sequences, when
    /// the account read its state anew: paid out with the rest.
    pub pending_unlisted: Amount,
    /// When what waits is paid, by the hub's clock; 0 while nothing waits.
    pub pending_due: Millis,
    /// The hub's clock at the last claim that added to what waits, as
    /// `reduced_at` is for a reduction: a thaw moves the payout only if
    /// that claim was at or before its freeze's stamp.
    pub pending_at: Millis,
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
            reduced_at: 0,
            total: 0,
            through: 0,
            claimed: BTreeMap::new(),
            pending: BTreeMap::new(),
            pending_unlisted: 0,
            pending_due: 0,
            pending_at: 0,
        }
    }

    /// A hold as a statement told it, its reduction and what waits counted
    /// as made at `as_of`: what `claim` took above its W is not in the
    /// view, and starts empty, and what waits is told without its
    /// sequences.
    #[must_use]
    pub const fn of(view: &HoldView, as_of: Millis) -> Self {
        Self {
            payer: view.payer,
            payee: view.payee,
            amount: view.amount,
            reduction: view.reduction,
            reduced_at: as_of,
            total: view.total,
            through: view.through,
            claimed: BTreeMap::new(),
            pending: BTreeMap::new(),
            pending_unlisted: view.pending.0,
            pending_due: view.pending.1,
            pending_at: as_of,
        }
    }

    /// What claims took that waits to pay the payee.
    #[must_use]
    pub fn waiting(&self) -> Amount {
        self.pending
            .values()
            .fold(self.pending_unlisted, |sum, (amount, _)| {
                sum.saturating_add(*amount)
            })
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
        self.reduced_at = timestamp;
        self.reduction = if amount > 0 {
            Some((amount, timestamp.saturating_add(notice)))
        } else {
            None
        };
    }

    /// Pays out what waits, if it is due by the hub's clock `clock`, as
    /// the ledger does before a block's operations, and before any
    /// reduction. Returns the sequences it paid above the last redeemed W,
    /// and whether it paid any it did not know the sequences of.
    pub fn pay_due(&mut self, clock: Millis) -> Option<(Vec<u64>, bool)> {
        if self.waiting() == 0 && self.pending_due == 0 {
            return None;
        }
        if self.pending_due > clock {
            return None;
        }
        let through = self.through;
        let sequences = self
            .pending
            .keys()
            .copied()
            .filter(|sequence| *sequence > through)
            .collect();
        let unlisted = self.pending_unlisted > 0;
        self.pending.clear();
        self.pending_unlisted = 0;
        self.pending_due = 0;
        Some((sequences, unlisted))
    }

    /// Applies a reduction due by a block stamped `timestamp`, as the
    /// ledger does before the block's operations; not while claims wait.
    /// Returns what went back to the payer.
    pub fn apply_due(&mut self, timestamp: Millis) -> Option<Amount> {
        let (amount, at) = self.reduction?;
        if at > timestamp || self.waiting() > 0 {
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

    /// A `claim` of `fulfilled` applied in version 2 or later, which took
    /// `taken` to wait in the hold until `due`, by the hub's clock, which
    /// stood at `clock`.
    pub fn claim_waiting(
        &mut self,
        fulfilled: &[Fulfilled],
        taken: Amount,
        (clock, due): (Millis, Millis),
    ) {
        self.claim(fulfilled, taken);
        // One that took nothing leaves nothing to wait, as the ledger has it.
        if taken == 0 {
            return;
        }
        let expiries: BTreeMap<u64, Millis> = fulfilled
            .iter()
            .map(|item| (item.commitment.value.sequence, item.commitment.value.expiry))
            .collect();
        for (sequence, amount) in paid_in_order(fulfilled, taken) {
            if amount > 0 {
                let expiry = expiries.get(&sequence).copied().unwrap_or(0);
                self.pending.insert(sequence, (amount, expiry));
            }
        }
        self.pending_due = due;
        self.pending_at = clock;
    }

    /// The payer's dispute with `releases`, of a sequence and the
    /// commitment's expiry each, which put `returned` back in the hold: what
    /// waits for each that names its commitment, above the last redeemed W,
    /// and, for what it does not know the sequences of, from what waits
    /// unlisted.
    pub fn dispute(&mut self, releases: &[(u64, Millis)], returned: Amount) {
        let mut left = returned;
        for (sequence, expiry) in releases {
            let named = self
                .pending
                .get(sequence)
                .is_some_and(|(_, waits)| waits == expiry);
            if *sequence <= self.through || !named {
                continue;
            }
            if let Some((amount, _)) = self.pending.remove(sequence) {
                left = left.saturating_sub(amount);
                self.claimed.insert(*sequence, 0);
            }
        }
        if left > 0 {
            self.pending_unlisted = self.pending_unlisted.saturating_sub(left);
            let through = self.through;
            for (sequence, _) in releases.iter().filter(|(s, _)| *s > through) {
                if !self.pending.contains_key(sequence) {
                    self.claimed.insert(*sequence, 0);
                }
            }
        }
        self.amount = self.amount.saturating_add(returned);
        if self.waiting() == 0 {
            self.pending_due = 0;
        }
    }

    /// A `claim` of `fulfilled` applied, which paid `paid`: each commitment
    /// in sequence order, as far as what was paid covers it, as the ledger
    /// pays them as far as the hold covers them.
    pub fn claim(&mut self, fulfilled: &[Fulfilled], paid: Amount) {
        for (sequence, pay) in paid_in_order(fulfilled, paid) {
            self.claimed.insert(sequence, pay);
        }
        self.amount = self.amount.saturating_sub(paid);
    }
}

/// What a claim of `fulfilled` took for each of its commitments: in
/// sequence order, each as far as `paid` covers it, as the ledger takes
/// them as far as the hold covers them.
fn paid_in_order(fulfilled: &[Fulfilled], paid: Amount) -> Vec<(u64, Amount)> {
    let mut ordered: Vec<(u64, Amount)> = fulfilled
        .iter()
        .map(|item| (item.commitment.value.sequence, item.commitment.value.amount))
        .collect();
    ordered.sort_unstable();
    let mut left = paid;
    for (_, amount) in &mut ordered {
        let pay = (*amount).min(left);
        left = left.saturating_sub(pay);
        *amount = pay;
    }
    ordered
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

    // spec/decisions/signed-rejects.md: what a claim took waits in the hold,
    // a dispute puts it back, and the rest pays out at once when due,
    // before any reduction.
    #[test]
    fn what_a_claim_took_waits_until_it_is_paid_or_disputed() {
        let mut hold = HoldMirror::opened(AccountNumber(1), AccountNumber(2), 1_000);
        hold.claim_waiting(&[fulfilled(3, 100), fulfilled(4, 50)], 150, (1_000, 61_000));
        assert_eq!((hold.amount, hold.waiting(), hold.taken()), (850, 150, 150));
        hold.dispute(&[(3, 0)], 100);
        assert_eq!((hold.amount, hold.waiting(), hold.taken()), (950, 50, 50));
        assert_eq!(hold.claimed.get(&3), Some(&0));
        hold.reduce(950, 2_000, 10);
        assert_eq!(hold.apply_due(2_010), None, "claims wait");
        assert_eq!(hold.pay_due(60_999), None);
        assert_eq!(hold.pay_due(61_000), Some((alloc::vec![4], false)));
        assert_eq!(hold.waiting(), 0);
        assert_eq!(hold.apply_due(61_000), Some(950));
    }
}
