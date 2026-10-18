//! When to settle with a peer, and how much (D23, spec 7 §4.7).
//!
//! A node knows only the credit it extends to a peer, never the credit the
//! peer extends to it. So the debtor cannot tell when it should settle:
//! the creditor can, from its own books, and asks. The creditor asks once
//! its room for the peer, what the peer may still owe it, has fallen to
//! half the credit it extends. The debtor then settles half its own
//! position, what it may pay without the creditor owing it past the credit
//! it extends in turn, but never so much that it would have to ask in
//! turn: when two books have drifted apart, settling back and forth would
//! not end. Both are simple rules, to be tuned later.

use dassie_ledger::{Balance, MAX_TOTAL};

/// Our position with a peer, from its `assets/interledger` account: the
/// credit we extend plus what we owe it, net of settlements in flight.
/// It is also what the peer may still owe us. The account's limit keeps
/// it at or above zero.
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

/// Whether to ask a peer to settle: our room for it, our `position`, has
/// fallen to half the credit we extend. With no credit, once it has
/// nothing paid in advance left.
#[must_use]
pub fn ask(position: u128, credit_out: u64) -> bool {
    position <= u128::from(credit_out) / 2
}

/// What to settle when a peer asks: half our `position`, but no more than
/// keeps us short of our own ask point for it, given the credit we extend.
#[must_use]
pub fn requested_amount(position: u128, credit_out: u64) -> u64 {
    // The least position at which we would not ask in turn.
    let floor = u128::from(credit_out) / 2 + 1;
    clamp((position / 2).min(position.saturating_sub(floor)))
}

/// What to settle when a peering ends: everything we owe, so the
/// position comes back to the credit we extend.
#[must_use]
pub fn final_amount(position: u128, credit_out: u64) -> u64 {
    clamp(position.saturating_sub(u128::from(credit_out)))
}

/// An amount from an account's counters, which never pass `MAX_TOTAL`.
fn clamp(value: u128) -> u64 {
    assert!(value <= u128::from(MAX_TOTAL), "an amount past MAX_TOTAL");
    u64::try_from(value).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    const C: u64 = 1_100_000_000;

    // A peer is asked to settle once our room for it is half the credit we
    // extend, and pays half its own position.
    #[test]
    fn a_peer_is_asked_at_half_the_credit() {
        assert!(!ask(u128::from(C) / 2 + 1, C));
        assert!(ask(u128::from(C) / 2, C));
        // Owing us half our credit, with the same credit both ways, it
        // stands at one and a half times the credit, and pays 0.75 of it.
        assert_eq!(requested_amount(u128::from(C) * 3 / 2, C), C * 3 / 4);
    }

    // A senior extends a junior nothing: it asks at once, and the junior,
    // owing nothing yet, pays half the credit it extends, in advance (D20).
    #[test]
    fn without_credit_a_peer_pays_in_advance() {
        assert!(ask(0, 0));
        assert!(!ask(1, 0));
        // Short of its own ask point by a unit.
        assert_eq!(requested_amount(u128::from(C), C), C / 2 - 1);
    }

    // Books that drifted apart: the payer stops short of its own ask
    // point, so it does not ask back for what it paid, and the two do not
    // settle back and forth for ever.
    #[test]
    fn a_payer_never_pays_itself_into_asking() {
        let position = 865_000_000;
        let paid = requested_amount(position, C);
        assert_eq!(paid, 315_000_000 - 1);
        assert!(!ask(position - u128::from(paid), C));
        // At its own ask point already, it pays nothing.
        assert_eq!(requested_amount(u128::from(C) / 2, C), 0);
    }

    #[test]
    fn the_final_settlement_pays_what_is_owed() {
        assert_eq!(final_amount(1_500_000_000, C), 400_000_000);
        assert_eq!(final_amount(1_000_000_000, C), 0);
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
}
