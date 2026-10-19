//! Properties of the settlement policy.

use dassie_settlement::policy::{ask, minimum, requested_amount, trigger};
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
