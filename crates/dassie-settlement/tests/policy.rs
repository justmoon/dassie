//! Properties of the settlement policy.

use dassie_settlement::policy::{ask, final_amount, requested_amount};
use proptest::prelude::*;

/// Amounts up to the ledger's bound, so sums stay meaningful.
const MAX: u64 = 1 << 62;

proptest! {
    // What a peer settles on request is at most half its position, at
    // least a tenth of its credit, and never leaves it asking in turn:
    // unless it was asking already, as books that drifted apart may have
    // it, when it pays nothing.
    #[test]
    fn a_requested_settlement_never_asks_back(
        credit_out in 0..MAX,
        position in 0..u128::from(MAX) * 2,
    ) {
        let paid = u128::from(requested_amount(position, credit_out));
        prop_assert!(paid <= position / 2);
        prop_assert!(paid == 0 || paid >= u128::from(credit_out) / 10);
        if ask(position, credit_out) {
            prop_assert_eq!(paid, 0);
        } else {
            prop_assert!(!ask(position - paid, credit_out));
        }
    }

    // The asker asks exactly while its room is at most half its credit.
    #[test]
    fn the_asker_asks_at_half_its_credit(
        credit_out in 0..MAX,
        position in 0..u128::from(MAX) * 2,
    ) {
        prop_assert_eq!(ask(position, credit_out), position * 2 <= u128::from(credit_out));
    }

    #[test]
    fn the_final_settlement_pays_what_is_owed(
        credit_out in 0..MAX,
        position in 0..u128::from(MAX) * 2,
    ) {
        let paid = u128::from(final_amount(position, credit_out));
        prop_assert!(paid <= position);
        prop_assert_eq!(position - paid, position.min(u128::from(credit_out)));
    }
}
