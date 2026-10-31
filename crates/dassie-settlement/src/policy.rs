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

/// The least a node holds toward a peer over a settlement hub when its hub
/// cannot give every peer the default credit: what the peer may have in
/// flight, `max_in_flight`, and what it lets claims add before it redeems
/// them ([`redeem_threshold`]). So the hold carries a link filled to the
/// cap on money in flight while the payee waits to redeem
/// (spec/decisions/hold-allotment.md).
#[must_use]
pub const fn least_hold(max_packet: u64, max_in_flight: u64) -> u64 {
    max_in_flight.saturating_add(redeem_threshold(max_packet))
}

/// What a node keeps in its settlement hub unless its owner sets another
/// target for it: a thousand of the largest packets, more than its holds
/// toward its peers are likely ever to need, so that the owner's funds stay
/// on the owner's own account only when they are truly in excess. What
/// counts toward it is what the node put in: the balance, its reserves and
/// its holds toward peers, never its peers' holds toward it, which are
/// theirs until it takes them (spec/decisions/generous-hub-target.md).
#[must_use]
pub const fn hub_target(max_packet: u64) -> u64 {
    max_packet.saturating_mul(1_000)
}

/// The least target an owner may set for a hub: ten of the largest
/// packets, so that a [`hub_step`] is at least a largest packet, the least
/// a deposit moves.
#[must_use]
pub const fn least_hub_target(max_packet: u64) -> u64 {
    max_packet.saturating_mul(10)
}

/// The most target an owner may set for a hub: an eighth of what an
/// account's counters hold, so that the ceiling, a step above it, and what
/// the owner's payments need on top never come near them. Any target past
/// what the owner holds has the node deposit everything, as this one does.
pub const MOST_HUB_TARGET: u64 = MAX_TOTAL / 8;

/// How far what the node keeps in its hub moves from its `target` before
/// the node moves funds: a tenth of it, so that each move between the hub
/// and the node's own account carries many packets' worth.
#[must_use]
pub const fn hub_step(target: u64) -> u64 {
    target / 10
}

/// The floor and the ceiling of a hub whose target is `target`: below the
/// floor, a [`hub_step`] under the target, the node deposits from its own
/// account back up to it; above the ceiling, a step over it, it withdraws
/// back down to it.
#[must_use]
pub const fn hub_limits(target: u64) -> (u64, u64) {
    let step = hub_step(target);
    (target.saturating_sub(step), target.saturating_add(step))
}

/// The least a node keeps spendable in its settlement hub, whatever it
/// keeps there in all: for each of its peerings over the hub, and for one
/// while it has none, the most one request to settle has it fund the hold
/// toward the peer. A payer over a hub settles as if it extended the
/// default `credit`, with its hold counted as paid in advance, so a request
/// takes at most what [`requested_amount`] gives at that credit, and never
/// less than the least settlement. A peer's allotment of the hub's funds
/// only lowers what a request takes (spec/decisions/hold-allotment.md), so
/// this stays the most requests may ask. At most half a [`hub_step`] of the hub's
/// `target`, the default [`hub_target`] or the owner's, so that a
/// withdrawal down to it still carries many packets' worth. At a target
/// the owner set below ninety-five packets, half a step is less than one
/// request at the default credit takes, 4.75 packets, so one request may
/// spend it all: the owner's choice. Its holds toward peers count toward
/// the target, so without it a node whose holds
/// alone reach the target could withdraw what it can spend and never
/// deposit again (spec/decisions/generous-hub-target.md).
#[must_use]
pub fn hub_spendable(max_packet: u64, credit: u64, peerings: u64, target: u64) -> u64 {
    let each = one_request(max_packet, credit);
    let peerings = if peerings == 0 { 1 } else { peerings };
    let least = each.saturating_mul(peerings);
    let most = hub_step(target) / 2;
    if least < most { least } else { most }
}

/// The most one request to settle has a payer over a hub fund its hold
/// with: what it would settle at the default `credit`, its hold counted as
/// paid in advance, and never less than the least settlement.
#[must_use]
pub fn one_request(max_packet: u64, credit: u64) -> u64 {
    let asked = requested_amount(u128::from(credit), trigger(max_packet), minimum(max_packet));
    asked.max(minimum(max_packet))
}

/// What a downlink's own account in a settlement hub must be able to spend
/// for its uplink to promote it: one request to settle at the default
/// `credit` for each of the `edges` joining the core asks, what funding a
/// hold toward each of its first core peers takes
/// (spec/decisions/promotion-to-core.md).
#[must_use]
pub fn promotion_funds(max_packet: u64, credit: u64, edges: u64) -> u64 {
    one_request(max_packet, credit).saturating_mul(edges)
}

/// The floor of the default [`hub_target`] ([`hub_limits`]).
#[must_use]
pub const fn hub_floor(max_packet: u64) -> u64 {
    hub_limits(hub_target(max_packet)).0
}

/// The ceiling of the default [`hub_target`] ([`hub_limits`]).
#[must_use]
pub const fn hub_ceiling(max_packet: u64) -> u64 {
    hub_limits(hub_target(max_packet)).1
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
const _: () = assert!(
    hub_limits(MOST_HUB_TARGET).1 <= MAX_TOTAL / 4,
    "the most target's ceiling within an account's counters"
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
        // What may be in flight and what claims add before a redeem: 4.5
        // packets. Below the least credit by a trigger and more whatever
        // the largest packet and the drift, so the default credit always
        // covers it (spec/decisions/hold-allotment.md).
        assert_eq!(least_hold(P, 250_000_000), 450_000_000);
        for (max_packet, drift) in [(1, 0), (P, 0), (P, 250_000_000), (7, 1_000)] {
            let gap = least_credit(max_packet, drift) - least_hold(max_packet, drift);
            assert!(gap >= trigger(max_packet));
        }
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
        assert_eq!(hub_step(hub_target(P)), 100 * P);
        assert_eq!((hub_floor(P), hub_ceiling(P)), (900 * P, 1_100 * P));
        assert_eq!(hub_limits(hub_target(P)), (hub_floor(P), hub_ceiling(P)));
        assert_eq!(hub_floor(P).midpoint(hub_ceiling(P)), hub_target(P));
    }

    // spec/decisions/promotion-to-core.md: a candidate's hub account must
    // spend one request at the default credit per edge, 4.75 packets each.
    #[test]
    fn promotion_asks_one_request_per_edge() {
        assert_eq!(one_request(P, C), 475 * P / 100);
        assert_eq!(one_request(P, 0), P, "never less than the least settlement");
        assert_eq!(promotion_funds(P, C, 1), 475 * P / 100);
        assert_eq!(promotion_funds(P, C, 2), 950 * P / 100);
    }

    // spec/decisions/generous-hub-target.md: whatever it keeps in its hub,
    // a node keeps spendable, per peering over it, what one request to
    // settle has a payer fund at the default credit of eleven packets: half
    // the way down to the trigger, 4.75 packets; one peering's worth with
    // none, never less than the least settlement, and at most half a step:
    // ten peerings are covered fully at the default target, and one at a
    // target an owner set as low as a hundred packets.
    #[test]
    fn the_hub_keeps_what_requests_need_spendable() {
        let target = hub_target(P);
        assert_eq!(hub_spendable(P, C, 0, target), 475 * P / 100);
        assert_eq!(hub_spendable(P, C, 1, target), 475 * P / 100);
        assert_eq!(hub_spendable(P, C, 10, target), 475 * P / 10);
        assert_eq!(hub_spendable(P, C, 11, target), 50 * P);
        assert_eq!(
            hub_spendable(P, 0, 2, target),
            2 * P,
            "at least the least settlement"
        );
        assert_eq!(hub_spendable(P, C, 1, 100 * P), 475 * P / 100);
        assert_eq!(hub_spendable(P, C, 3, 100 * P), 5 * P, "half a step");
        assert_eq!(hub_spendable(P, C, 1, least_hub_target(P)), P / 2);
    }

    // spec/decisions/generous-hub-target.md: a target the owner sets moves
    // the floor and the ceiling with it, a tenth either side. At the least
    // target a step is a largest packet, the least deposit; at the most,
    // nothing overflows.
    #[test]
    fn an_owners_target_moves_its_limits() {
        assert_eq!(hub_limits(100 * P), (90 * P, 110 * P));
        let least = least_hub_target(P);
        assert_eq!(least, 10 * P);
        assert_eq!(hub_step(least), P);
        assert_eq!(hub_limits(least), (9 * P, 11 * P));
        let (floor, ceiling) = hub_limits(MOST_HUB_TARGET);
        assert!(floor < MOST_HUB_TARGET && MOST_HUB_TARGET < ceiling);
        assert!(ceiling < MAX_TOTAL / 4);
        // Not a multiple of ten: the step rounds down, and the floor and
        // the ceiling stay as far either side.
        assert_eq!(hub_limits(10 * P + 9), (9 * P + 9, 11 * P + 9));
        assert_eq!(hub_limits(0), (0, 0));
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
