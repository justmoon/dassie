//! When to settle with a peer, and how much (D23, spec 7 §4.7).
//!
//! A node knows only the credit it extends to a peer, never the credit the
//! peer extends to it. So the debtor cannot tell when it should settle:
//! the creditor can, from its own books, and asks. The creditor asks once
//! the peer is close to the credit it extends: its room for the peer, what
//! the peer may still owe it, has fallen to a tenth of that credit.
//!
//! A request says that the debtor stands near the asker's trigger. The
//! debtor settles half the way from where it stands to its own trigger,
//! which lands near the middle of the two, and never so far that it would
//! have to ask in turn, however far the two books have drifted apart. It
//! settles nothing less than a tenth of the credit it extends, so a peer
//! that asks again and again cannot make it pay a ledger fee for every few
//! drops. Both are simple rules, to be tuned later.

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

/// Our own trigger for a peer: the room for it, our `position`, at which
/// we ask it to settle, a tenth of the credit we extend.
fn trigger(credit_out: u64) -> u128 {
    u128::from(credit_out) / 10
}

/// Whether to ask a peer to settle: it is close to the credit we extend,
/// our room for it having fallen to our trigger. With no credit, once it
/// has nothing paid in advance left.
#[must_use]
pub fn ask(position: u128, credit_out: u64) -> bool {
    position <= trigger(credit_out)
}

/// What to settle when a peer asks: half the way from our `position` to
/// our own trigger for it, and nothing if that is less than a tenth of the
/// credit we extend. Settling lowers our position, so we never pay
/// ourselves into asking back.
#[must_use]
pub fn requested_amount(position: u128, credit_out: u64) -> u64 {
    let amount = position.saturating_sub(trigger(credit_out)) / 2;
    if amount < u128::from(credit_out) / 10 {
        return 0;
    }
    clamp(amount)
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

    // A peer is asked to settle once our room for it is a tenth of the
    // credit we extend.
    #[test]
    fn a_peer_is_asked_close_to_the_credit() {
        assert!(!ask(u128::from(C) / 10 + 1, C));
        assert!(ask(u128::from(C) / 10, C));
        // With the same credit both ways, a debtor asked owes about 0.99:
        // it stands at 2.09 and pays 0.99, back to the middle.
        assert_eq!(requested_amount(2_090_000_000, C), 990_000_000);
    }

    // Asked at a balance of 0.6 of a credit of 1.1, so at a position of
    // 0.5, a node settles half the way to its own trigger at 0.11.
    #[test]
    fn a_debtor_settles_half_the_way_to_its_own_trigger() {
        assert_eq!(requested_amount(500_000_000, C), 195_000_000);
    }

    // A senior extends a junior nothing: it asks at once, and the junior,
    // owing nothing yet, pays in advance (D20).
    #[test]
    fn without_credit_a_peer_pays_in_advance() {
        assert!(ask(0, 0));
        assert!(!ask(1, 0));
        assert_eq!(requested_amount(u128::from(C), C), 495_000_000);
    }

    // Books that drifted apart: the payer stays clear of its own trigger,
    // so it does not ask back for what it paid, and the two do not settle
    // back and forth for ever.
    #[test]
    fn a_payer_never_pays_itself_into_asking() {
        let position = 865_000_000;
        let paid = requested_amount(position, C);
        assert_eq!(paid, 377_500_000);
        assert!(!ask(position - u128::from(paid), C));
        // At its own trigger already, it pays nothing.
        assert_eq!(requested_amount(u128::from(C) / 10, C), 0);
    }

    // A peer that asks again and again gets nothing less than a tenth of
    // the credit at a time, so it cannot make us pay a fee per drop.
    #[test]
    fn nothing_less_than_a_tenth_of_the_credit_is_settled() {
        let trigger = u128::from(C) / 10;
        assert_eq!(requested_amount(trigger + 2 * trigger - 1, C), 0);
        assert_eq!(requested_amount(trigger + 2 * trigger, C), C / 10);
        // Without credit extended, any amount is settled.
        assert_eq!(requested_amount(2, 0), 1);
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
