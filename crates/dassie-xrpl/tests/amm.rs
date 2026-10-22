//! An AMM pool's price (spec/areas/xrpl.md#amm-pools), against its own
//! inverse: paying what a swap costs yields at least what it takes, a unit
//! less yields less, and the pool's product never falls. Nothing panics,
//! however far apart the amounts.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use dassie_xrpl::amm::{MAX_TRADING_FEE, SwapError, keeps_product, swap_in, swap_out};
use dassie_xrpl::amount::{MAX_EXPONENT, MAX_MANTISSA, MIN_EXPONENT, MIN_MANTISSA};
use dassie_xrpl::codec::{Amount, Currency, TokenAmount, TokenValue};
use dassie_xrpl::{AccountId, MAX_DROPS};
use proptest::prelude::*;

fn usd(value: TokenValue) -> Amount {
    Amount::Token(TokenAmount {
        value,
        currency: Currency::parse("USD").unwrap(),
        issuer: AccountId([7; 20]),
    })
}

fn value(mantissa: u64, exponent: i32) -> TokenValue {
    TokenValue::new(false, mantissa, exponent).unwrap()
}

/// A token value above zero, within `exponents`.
fn token_value(exponents: core::ops::RangeInclusive<i32>) -> impl Strategy<Value = TokenValue> {
    (MIN_MANTISSA..=MAX_MANTISSA, exponents)
        .prop_map(|(mantissa, exponent)| value(mantissa, exponent))
}

/// The value one unit of the sixteenth digit below `value`.
fn below(value: TokenValue) -> TokenValue {
    if value.mantissa() > MIN_MANTISSA {
        TokenValue::new(false, value.mantissa() - 1, value.exponent()).unwrap()
    } else {
        TokenValue::new(false, MAX_MANTISSA, value.exponent() - 1).unwrap()
    }
}

fn token(amount: Amount) -> TokenValue {
    amount.token().unwrap().value
}

proptest! {
    // XRP in, the token out: the price in whole drops.
    #[test]
    fn xrp_buys_at_the_least_price_that_keeps_the_pool(
        drops in 1..=MAX_DROPS / 1_000,
        held in token_value(-15..=5),
        share_ppm in 1..1_000_000_u64,
        fee in 0..=MAX_TRADING_FEE,
    ) {
        let (pool_in, pool_out) = (Amount::Xrp(drops), usd(held));
        // A share of what the pool holds, below all of it.
        let out = value(held.mantissa() / 1_000_000 * share_ppm, held.exponent());
        let Ok(cost) = swap_in(pool_in, pool_out, fee, usd(out)) else {
            return Ok(());
        };
        let paid = cost.drops().unwrap();
        prop_assert!(paid >= 1);
        // What the price buys is at least what was asked for.
        let bought = swap_out(pool_in, pool_out, fee, cost).unwrap();
        prop_assert!(token(bought) >= out);
        // A drop less buys less.
        let less = swap_out(pool_in, pool_out, fee, Amount::Xrp(paid - 1)).unwrap();
        prop_assert!(token(less) < out);
        if let Some(left) = held.checked_sub(out) {
            prop_assert_eq!(
                keeps_product([pool_in, pool_out], [Amount::Xrp(drops + paid), usd(left)]),
                Some(true)
            );
        }
    }

    // The token in, XRP out: the price in sixteen digits.
    #[test]
    fn the_token_buys_at_the_least_price_that_keeps_the_pool(
        drops in 2..=MAX_DROPS / 1_000,
        held in token_value(-15..=5),
        share_ppm in 1..1_000_000_u64,
        fee in 0..=MAX_TRADING_FEE,
    ) {
        let (pool_in, pool_out) = (usd(held), Amount::Xrp(drops));
        let out = (u128::from(drops) * u128::from(share_ppm) / 1_000_000).max(1);
        let out = Amount::Xrp(u64::try_from(out).unwrap());
        let Ok(cost) = swap_in(pool_in, pool_out, fee, out) else {
            return Ok(());
        };
        let paid = token(cost);
        let bought = swap_out(pool_in, pool_out, fee, cost).unwrap();
        prop_assert!(bought.drops() >= out.drops());
        if let Ok(less) = swap_out(pool_in, pool_out, fee, usd(below(paid))) {
            prop_assert!(less.drops() < out.drops());
        }
        if let Some(more) = held.checked_add(paid) {
            prop_assert_eq!(
                keeps_product([pool_in, pool_out], [usd(more), Amount::Xrp(drops - out.drops().unwrap())]),
                Some(true)
            );
        }
    }

    // Whatever the amounts, a price is a value or an error, never a panic;
    // and what it says of the pool's assets holds.
    #[test]
    fn prices_never_panic(
        drops in 0..=MAX_DROPS,
        held in token_value(MIN_EXPONENT..=MAX_EXPONENT),
        asked in token_value(MIN_EXPONENT..=MAX_EXPONENT),
        drops_asked in 0..=MAX_DROPS,
        fee in 0..=MAX_TRADING_FEE + 1,
    ) {
        for (pool_in, pool_out, amount) in [
            (Amount::Xrp(drops), usd(held), usd(asked)),
            (usd(held), Amount::Xrp(drops), Amount::Xrp(drops_asked)),
        ] {
            if let Ok(cost) = swap_in(pool_in, pool_out, fee, amount) {
                prop_assert_eq!(cost.asset(), pool_in.asset());
                prop_assert!(fee <= MAX_TRADING_FEE);
            }
            if let Ok(yielded) = swap_out(pool_out, pool_in, fee, amount) {
                prop_assert_eq!(yielded.asset(), pool_in.asset());
            }
        }
        prop_assert_eq!(
            swap_in(Amount::Xrp(drops.max(1)), usd(held), MAX_TRADING_FEE + 1, usd(asked)),
            Err(SwapError::Invalid)
        );
    }
}
