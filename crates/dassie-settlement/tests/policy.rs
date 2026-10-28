//! Properties of the settlement policy.

use dassie_settlement::policy::{
    MOST_DOUBLINGS, Pace, ask, least_credit, minimum, most_counted_credit, requested_amount,
    trigger,
};
use proptest::prelude::*;

/// Amounts up to the ledger's bound, so sums stay meaningful.
const MAX: u64 = 1 << 62;

proptest! {
    // What a peer settles on request is at most half its position, at
    // least the minimum, and never leaves it asking in turn: unless it was
    // asking already, as books that drifted apart may have it, when it pays
    // nothing. It lands half the way to its trigger.
    #[test]
    fn a_requested_settlement_never_asks_back(
        max_packet in 1..MAX / 4,
        position in 0..u128::from(MAX) * 2,
    ) {
        let (trigger, minimum) = (trigger(max_packet), minimum(max_packet));
        let paid = u128::from(requested_amount(position, trigger, minimum));
        prop_assert!(paid <= position / 2);
        prop_assert!(paid == 0 || paid >= u128::from(minimum));
        if paid > 0 {
            prop_assert_eq!(paid, (position - u128::from(trigger)) / 2);
        }
        if ask(position, trigger) {
            prop_assert_eq!(paid, 0);
        } else {
            prop_assert!(!ask(position - paid, trigger));
        }
    }

    // The asker asks exactly while its room is at most its trigger, one
    // and a half of the largest packets, whatever the credit.
    #[test]
    fn the_asker_asks_at_its_trigger(
        max_packet in 1..MAX / 4,
        position in 0..u128::from(MAX) * 2,
    ) {
        let trigger = trigger(max_packet);
        prop_assert_eq!(u128::from(trigger), u128::from(max_packet) * 3 / 2);
        prop_assert_eq!(ask(position, trigger), position <= u128::from(trigger));
    }
}

proptest! {
    // spec/decisions/settle-by-request-rate.md: however a payer is asked
    // and its fundings land, the credit it counts stays between the
    // default and a quarter of the ceiling, grows by one doubling at most
    // per request, never grows on a landing or with time alone, and its
    // step is never less than the rule at the default credit asks.
    #[test]
    fn a_pace_stays_within_its_bounds(
        events in proptest::collection::vec((any::<bool>(), 0..120_000u64), 1..64),
        max_packet in 1..1u64 << 40,
        held in 0..1u64 << 44,
    ) {
        let every = 60_000;
        let credit = least_credit(max_packet, max_packet * 5 / 2) + max_packet * 7 / 2;
        let most = most_counted_credit(max_packet);
        let (trigger, minimum) = (trigger(max_packet), minimum(max_packet));
        let mut pace = Pace::new(0);
        let mut now = 0u64;
        for (asked, wait) in events {
            now += wait;
            let before = pace.doublings(now, every);
            if asked {
                pace.asked(now, every);
                prop_assert!(pace.doublings(now, every) <= before.saturating_add(1));
            } else {
                pace.landed(now, every);
                prop_assert_eq!(pace.doublings(now, every), before);
            }
            pace.check(now);
            prop_assert!(pace.doublings(now, every) <= MOST_DOUBLINGS);
            let counted = pace.credit(now, every, credit, most);
            prop_assert!(counted >= credit && counted <= credit.max(most));
            // Standing as if it extended the credit, its hold paid in
            // advance.
            let standing = |counted: u64| u128::from(counted.saturating_sub(held));
            let base = requested_amount(standing(credit), trigger, minimum);
            prop_assert!(requested_amount(standing(counted), trigger, minimum) >= base);
            // Later, with no request, it only shrinks.
            prop_assert!(pace.doublings(now + wait, every) <= pace.doublings(now, every));
        }
    }

    // A payee that asks again and again, spending nothing, has its payer
    // fund its hold toward it up to the credit counted less the trigger,
    // and no further: below a quarter of the ceiling at most.
    #[test]
    fn a_hold_stays_below_the_credit_counted(
        max_packet in 1..1u64 << 40,
        doublings in 0..=MOST_DOUBLINGS,
    ) {
        let credit = least_credit(max_packet, max_packet * 5 / 2) + max_packet * 7 / 2;
        let most = most_counted_credit(max_packet);
        let counted = credit.saturating_mul(1 << doublings).min(most).max(credit);
        let (trigger, minimum) = (trigger(max_packet), minimum(max_packet));
        let mut hold = 0u64;
        for _ in 0..200 {
            let step = requested_amount(u128::from(counted - hold), trigger, minimum);
            if step == 0 {
                break;
            }
            hold += step;
        }
        prop_assert!(hold <= counted - trigger);
        prop_assert!(hold < most.max(credit));
    }
}
