//! When to settle with a peer, and how much (spec 5 §4.6, spec 7 §4.7).
//!
//! The original's policy is kept: settle once what we owe passes a
//! threshold above the middle of the credit range, back down to that
//! middle. The middle now uses the credit the peer extends to us, as it
//! announced when peering, instead of assuming it equals ours (ACC-24).

use dassie_ledger::Balance;

/// Parts per million.
const MILLION: u128 = 1_000_000;

/// What a peering's settlement depends on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Terms {
    /// The credit the peer extends to us.
    pub credit_in: u64,
    /// The credit we extend to the peer, as posted to its trust account.
    pub credit_out: u64,
    /// How far above the middle the threshold sits, in parts per million
    /// of the middle, doubled: 200 000 puts it at 1.1 times the middle, as
    /// in the original.
    pub ratio_ppm: u32,
}

/// The original's ratio: settle at 1.1 times the middle.
pub const DEFAULT_RATIO_PPM: u32 = 200_000;

/// Our position with a peer, from its `assets/interledger` account: the
/// credit we extend plus what we owe it, net of settlements in flight.
/// The account's limit keeps it at or above zero.
#[must_use]
pub fn position(interledger: &Balance) -> u128 {
    interledger
        .credits_posted
        .saturating_sub(interledger.debits_posted)
        .saturating_sub(interledger.debits_pending)
}

/// The credit we extend to a peer, from its `contra/trust` account: what
/// was posted, not what is configured now.
#[must_use]
pub fn trust(contra: &Balance) -> u64 {
    clamp(contra.debits_posted.saturating_sub(contra.credits_posted))
}

/// What to settle now, given our `position` with the peer: everything
/// above the middle once the position reaches the threshold, else
/// nothing. Never more than `position`.
#[must_use]
pub fn amount(position: u128, terms: Terms) -> u64 {
    let middle = (u128::from(terms.credit_in) + u128::from(terms.credit_out)) / 2;
    let threshold = middle + middle * u128::from(terms.ratio_ppm) / (2 * MILLION);
    if position < threshold || position <= middle {
        return 0;
    }
    clamp(position - middle)
}

/// What to settle when a peering ends: everything we owe, so the
/// position comes back to the credit we extend.
#[must_use]
pub fn final_amount(position: u128, credit_out: u64) -> u64 {
    clamp(position.saturating_sub(u128::from(credit_out)))
}

fn clamp(value: u128) -> u64 {
    u64::try_from(value).unwrap_or(u64::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;

    const UNIT: u64 = 1_000_000_000;
    const U: u128 = 1_000_000_000;

    fn terms(credit_in: u64, credit_out: u64) -> Terms {
        Terms {
            credit_in,
            credit_out,
            ratio_ppm: DEFAULT_RATIO_PPM,
        }
    }

    // The original's numbers, with the default credit both ways: the
    // middle is 1.1e9, the threshold 1.21e9 (spec 5 §4.6).
    #[test]
    fn symmetric_credit_settles_as_the_original() {
        let credit = 1_100_000_000;
        let terms = terms(credit, credit);
        assert_eq!(amount(1_209_999_999, terms), 0);
        assert_eq!(amount(1_210_000_000, terms), 110_000_000);
        assert_eq!(amount(2_000_000_000, terms), 900_000_000);
        assert_eq!(final_amount(1_500_000_000, credit), 400_000_000);
        assert_eq!(final_amount(1_000_000_000, credit), 0);
    }

    // ACC-24: the middle uses the credit the peer extends, not ours.
    #[test]
    fn the_middle_uses_both_credits() {
        // The peer extends 3 units, we extend 1: the position runs from 0
        // to 4, its middle is 2 (we owe 1), and we settle from 2.2 on.
        let terms = terms(3 * UNIT, UNIT);
        assert_eq!(amount(2 * U + U / 5 - 1, terms), 0);
        assert_eq!(amount(2 * U + U / 5, terms), UNIT / 5);
        assert_eq!(amount(3 * U, terms), UNIT);
        // A peer that extends nothing is paid in advance, to the middle
        // of what we may hold.
        let prepaid = Terms {
            credit_in: 0,
            ..terms
        };
        assert_eq!(amount(U, prepaid), UNIT / 2);
    }

    #[test]
    fn positions_and_trust_come_from_the_accounts() {
        let interledger = Balance {
            credits_posted: 10,
            debits_posted: 3,
            debits_pending: 2,
            credits_pending: 100,
        };
        assert_eq!(position(&interledger), 5);
        let contra = Balance {
            debits_posted: 9,
            credits_posted: 4,
            ..Balance::default()
        };
        assert_eq!(trust(&contra), 5);
        assert_eq!(trust(&Balance::default()), 0);
    }

    #[test]
    fn nothing_without_credit() {
        assert_eq!(amount(0, terms(0, 0)), 0);
        assert_eq!(final_amount(0, 0), 0);
    }
}
