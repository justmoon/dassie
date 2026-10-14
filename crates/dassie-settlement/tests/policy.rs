//! Properties of the settlement policy.

use dassie_settlement::policy::{Terms, amount, final_amount};
use proptest::prelude::*;

/// Amounts up to the ledger's bound, so sums stay meaningful.
const MAX: u64 = 1 << 62;

proptest! {
    #[test]
    fn settling_lands_on_the_middle(
        credit_in in 0..MAX,
        credit_out in 0..MAX,
        ratio_ppm in 0..2_000_000u32,
        position in 0..u128::from(MAX) * 2,
    ) {
        let terms = Terms { credit_in, credit_out, ratio_ppm };
        let middle = (u128::from(credit_in) + u128::from(credit_out)) / 2;
        let threshold = middle + middle * u128::from(ratio_ppm) / 2_000_000;
        let paid = u128::from(amount(position, terms));
        prop_assert!(paid <= position);
        if position < threshold {
            prop_assert_eq!(paid, 0);
        } else if paid > 0 {
            prop_assert_eq!(position - paid, middle);
        }
        // Once settled, nothing more is due until the position moves.
        prop_assert_eq!(amount(position - paid, terms), 0);
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
