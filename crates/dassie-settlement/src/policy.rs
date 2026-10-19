//! When to settle with a peer, and how much (D23, D25,
//! spec/areas/settlement.md#positions-and-thresholds).
//!
//! A node knows only the credit it extends to a peer, never the credit the
//! peer extends to it. So the debtor cannot tell when it should settle:
//! the creditor can, from its own books, and asks. The creditor asks once
//! its room for the peer, what the peer may still owe it, has fallen to
//! its trigger.
//!
//! A request says that the debtor stands near the asker's trigger. The
//! debtor settles half the way from where it stands to its own trigger,
//! which lands near the middle of the two, and never so far that it would
//! have to ask in turn, however far the two books have drifted apart. It
//! settles nothing less than a minimum, so a peer that asks again and
//! again cannot make it pay a ledger fee for every few drops.
//!
//! The trigger and the minimum are counted in the largest packet, not in
//! the credit: the largest packet is how much the owner is prepared to
//! lose on a link, and the credit differs on each side. Both are simple
//! rules, to be tuned later.

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

/// Our trigger for every peer: one and a half of the largest packets.
#[must_use]
pub const fn trigger(max_packet: u64) -> u64 {
    max_packet.saturating_mul(3) / 2
}

/// The least we settle when asked: one of the largest packets.
#[must_use]
pub const fn minimum(max_packet: u64) -> u64 {
    max_packet
}

/// The least credit that keeps a link settling after a crash drifted the
/// two sides' books apart by `drift`. Asked at the asker's trigger, a
/// debtor settles only if its own books leave it its trigger plus twice the
/// minimum, so a link whose two sides count with these rules takes this
/// much credit.
#[must_use]
pub const fn least_credit(max_packet: u64, drift: u64) -> u64 {
    trigger(max_packet)
        .saturating_mul(2)
        .saturating_add(minimum(max_packet).saturating_mul(2))
        .saturating_add(drift)
}

/// The share of the fees a peer earned us by which the credit we extend
/// to it grows, in millionths: 10%. A packet comes in through one peer and
/// leaves through another, so its fee counts toward both; at most 50%, so
/// that the credit a node hands out never exceeds what it earned (D20).
pub const GROWTH_PPM: u64 = 100_000;
const _: () = assert!(GROWTH_PPM <= 500_000, "growth above half the fees");

/// The most credit a peer can earn: a hundred of the largest packets.
/// Credit extended above it, as configured or from before, stays.
#[must_use]
pub const fn credit_cap(max_packet: u64) -> u64 {
    max_packet.saturating_mul(100)
}

/// The credit a peer earned `growth` more of, up to `cap`: never less than
/// it was.
#[must_use]
pub fn raised(credit: u64, growth: u128, cap: u64) -> u64 {
    if credit >= cap {
        return credit;
    }
    let raised = u128::from(credit)
        .saturating_add(growth)
        .min(u128::from(cap));
    u64::try_from(raised).unwrap_or(cap)
}

/// Whether to ask a peer to settle: our room for it, our `position`, has
/// fallen to our `trigger`.
#[must_use]
pub fn ask(position: u128, trigger: u64) -> bool {
    position <= u128::from(trigger)
}

/// What to settle when a peer asks: half the way from our `position` to
/// our own `trigger` for it, and nothing if that is less than `minimum`.
/// Settling lowers our position, so we never pay ourselves into asking
/// back.
#[must_use]
pub fn requested_amount(position: u128, trigger: u64, minimum: u64) -> u64 {
    let amount = position.saturating_sub(u128::from(trigger)) / 2;
    if amount < u128::from(minimum) {
        return 0;
    }
    clamp(amount)
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
    /// The largest packet, 0.1 units, and what follows from it.
    const P: u64 = 100_000_000;
    const T: u64 = trigger(P);
    const M: u64 = minimum(P);

    // D20: credit grows by a tenth of the fees, up to a hundred packets,
    // and never falls.
    #[test]
    fn credit_grows_up_to_the_cap() {
        let cap = credit_cap(P);
        assert_eq!(cap, 10_000_000_000);
        assert_eq!(raised(C, 5, cap), C + 5);
        assert_eq!(raised(cap - 1, 5, cap), cap);
        assert_eq!(raised(cap + 7, 5, cap), cap + 7);
    }

    #[test]
    fn the_thresholds_follow_the_largest_packet() {
        assert_eq!((T, M), (150_000_000, 100_000_000));
        // Two triggers, twice the minimum, and a crash's drift at the cap
        // of two and a half packets in flight: 0.75 units.
        assert_eq!(least_credit(P, 250_000_000), 750_000_000);
    }

    // A peer is asked to settle once our room for it has fallen to our
    // trigger, whatever the credit we extend.
    #[test]
    fn a_peer_is_asked_at_the_trigger() {
        assert!(!ask(u128::from(T) + 1, T));
        assert!(ask(u128::from(T), T));
        // With the same credit both ways, a debtor asked owes about 0.95:
        // it stands at 2.05 and pays 0.95, back to the middle.
        assert_eq!(requested_amount(2_050_000_000, T, M), 950_000_000);
    }

    // Asked at a balance of 0.6 of a credit of 1.1, so at a position of
    // 0.5, a node settles half the way to its own trigger at 0.15.
    #[test]
    fn a_debtor_settles_half_the_way_to_its_own_trigger() {
        assert_eq!(requested_amount(500_000_000, T, M), 175_000_000);
    }

    // A senior extends a junior nothing: it asks at once, and the junior,
    // owing nothing yet, pays in advance (D20).
    #[test]
    fn without_credit_a_peer_pays_in_advance() {
        assert!(ask(0, T));
        assert_eq!(requested_amount(u128::from(C), T, M), 475_000_000);
    }

    // Books that drifted apart: the payer stays clear of its own trigger,
    // so it does not ask back for what it paid, and the two do not settle
    // back and forth for ever.
    #[test]
    fn a_payer_never_pays_itself_into_asking() {
        let position = 865_000_000;
        let paid = requested_amount(position, T, M);
        assert_eq!(paid, 357_500_000);
        assert!(!ask(position - u128::from(paid), T));
        // At its own trigger already, it pays nothing.
        assert_eq!(requested_amount(u128::from(T), T, M), 0);
    }

    // A peer that asks again and again gets nothing less than the minimum
    // at a time, so it cannot make us pay a fee per drop.
    #[test]
    fn nothing_less_than_the_minimum_is_settled() {
        let least = u128::from(T) + 2 * u128::from(M);
        assert_eq!(requested_amount(least - 1, T, M), 0);
        assert_eq!(requested_amount(least, T, M), M);
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
