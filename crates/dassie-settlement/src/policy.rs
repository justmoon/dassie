//! When to settle with a peer, and how much (spec/decisions/own-fulfills.md,
//! spec/decisions/thresholds-in-packets.md,
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

/// The least default credit (`peer_credit`) a node starts with: what keeps
/// a link settling after a crash drifted the two sides' books apart by
/// `drift`, at most what was in flight. Asked at the asker's trigger, a
/// debtor settles only if its own books leave it its trigger plus twice
/// the minimum, and a payer over a hub counts the default credit when
/// asked, whatever its hold: so with this much, a hub link survives that
/// drift even where the asker holds nothing toward it
/// (spec/areas/settlement.md#credit). It is above the least that has a
/// request to a payer with nothing held fund the minimum, by the drift and
/// a trigger.
#[must_use]
pub const fn least_credit(max_packet: u64, drift: u64) -> u64 {
    trigger(max_packet)
        .saturating_mul(2)
        .saturating_add(minimum(max_packet).saturating_mul(2))
        .saturating_add(drift)
}

/// What a payee over a settlement hub lets its payer's claims add before
/// it redeems them: two of the largest packets. Each redeem costs the
/// hub's fee, so it waits for more than a packet; what it has not taken is
/// lost if the hub fails, so it waits for little more
/// (spec/designs/settlement-hubs.md#4-peering-over-a-hub).
#[must_use]
pub const fn redeem_threshold(max_packet: u64) -> u64 {
    max_packet.saturating_mul(2)
}

/// What a node keeps in its settlement hub: a thousand of the largest
/// packets, more than its holds toward its peers are likely ever to need,
/// so that the owner's funds stay on the owner's own account only when they
/// are truly in excess. What counts toward it is what the node put in: the
/// balance, its reserves and its holds toward peers, never its peers'
/// holds toward it, which are theirs until it takes them
/// (spec/decisions/generous-hub-target.md).
#[must_use]
pub const fn hub_target(max_packet: u64) -> u64 {
    max_packet.saturating_mul(1_000)
}

/// How far what the node keeps in its hub moves from the [`hub_target`]
/// before the node moves funds: a tenth of it, so that each move between
/// the hub and the node's own account carries many packets' worth.
#[must_use]
pub const fn hub_step(max_packet: u64) -> u64 {
    hub_target(max_packet) / 10
}

/// Below this, the node deposits from its own account back up to the
/// [`hub_target`]: the target less a [`hub_step`].
#[must_use]
pub const fn hub_floor(max_packet: u64) -> u64 {
    hub_target(max_packet).saturating_sub(hub_step(max_packet))
}

/// Above this, the node withdraws to its own account back down to the
/// [`hub_target`]: the target and a [`hub_step`]. Only what the node takes
/// from its peers' holds toward it takes it there.
#[must_use]
pub const fn hub_ceiling(max_packet: u64) -> u64 {
    hub_target(max_packet).saturating_add(hub_step(max_packet))
}

/// The credit a node extends to every peer on a scheme with unlimited trust
/// ([`SettlementScheme::unlimited_trust`](crate::SettlementScheme::unlimited_trust)),
/// whose tests are not about settlement: a billion dollars in the stub's
/// units, so no test runs out of room. At most an eighth of what an
/// account's counters hold, so that a peer's account, whose posted credits
/// are the trust and all the peer ever paid us, takes seven times as much
/// again.
pub const UNLIMITED_CREDIT: u64 = 1_000_000_000_000_000_000;
const _: () = assert!(
    UNLIMITED_CREDIT <= MAX_TOTAL / 8,
    "unlimited credit past its share"
);

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

    // Unlimited trust is ten million times what a node keeps in its hub
    // at the default largest packet: no test runs out of room.
    #[test]
    fn unlimited_credit_dwarfs_any_hold() {
        assert_eq!(UNLIMITED_CREDIT / hub_target(P), 10_000_000);
    }

    #[test]
    fn the_thresholds_follow_the_largest_packet() {
        assert_eq!((T, M), (150_000_000, 100_000_000));
        assert_eq!(redeem_threshold(P), 200_000_000);
        // Two triggers, twice the minimum, and a crash's drift at the cap
        // of two and a half packets in flight: 7.5 packets.
        assert_eq!(least_credit(P, 250_000_000), 750_000_000);
    }

    // The least credit a node starts with keeps a hub link settling after
    // a crash's drift, with no hold of the asker's, and so a request to a
    // payer with nothing held funds at least the minimum; the default is
    // above it (spec/areas/settlement.md#credit).
    #[test]
    fn the_least_credit_survives_a_crash_and_funds_a_request() {
        let drift = 250_000_000;
        let least = least_credit(P, drift);
        assert!(C >= least);
        // Asked at the asker's trigger, with no hold of the asker's, a payer
        // stands at the credit less that trigger, and less the drift: its
        // books leave it its own trigger and twice the minimum, and it
        // funds the minimum. A little less, and it would fund nothing.
        let standing = u128::from(least - T - drift);
        assert_eq!(standing, u128::from(T + 2 * M));
        assert_eq!(requested_amount(standing, T, M), M);
        assert_eq!(requested_amount(standing - 1, T, M), 0);
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

    // A payee whose hold is empty asks at once, and the payer, owing
    // nothing yet, funds it in advance as if it extended the default credit
    // (spec/designs/settlement-hubs.md#4-peering-over-a-hub).
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

    // spec/decisions/generous-hub-target.md: a node keeps a thousand of
    // the largest packets in its hub, and moves funds once that is a tenth
    // off, whatever its peerings: its holds share it out.
    #[test]
    fn the_hub_keeps_near_its_target() {
        assert_eq!(hub_target(P), 1_000 * P);
        assert_eq!(hub_step(P), 100 * P);
        assert_eq!((hub_floor(P), hub_ceiling(P)), (900 * P, 1_100 * P));
        assert_eq!(hub_floor(P).midpoint(hub_ceiling(P)), hub_target(P));
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
