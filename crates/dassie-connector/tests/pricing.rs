//! The inventory curve's properties (spec/decisions/passive-rebalancing.md):
//! spot at target, monotonic in each ledger's holding, a bonus of at most
//! `k`, no gain from a round trip at one state, and no overflow for any rates
//! and holdings.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use dassie_connector::pricing::{MAX_REBALANCE_PPM, RATE_ONE_E15};
use dassie_connector::{Charges, PriceTable, Pricing, RATE_PRECISION, Rates};
use dassie_ledger::LedgerId;
use num_bigint::BigUint;
use proptest::prelude::*;

const USD: LedgerId = LedgerId::StubUsd;
const XRP: LedgerId = LedgerId::XrplTestnetXrp;
const RLUSD: LedgerId = LedgerId::XrplTestnetRlusdUsd;
const NOW: u64 = 1_700_000_000_000;
const AGE: u64 = 3_600_000;
const MILLION: u32 = 1_000_000;

fn rates(xrp_rate: u128) -> Rates {
    let mut rates = Rates::new(NOW);
    rates.set("USD", 10u128.pow(RATE_PRECISION));
    rates.set("XRP", xrp_rate);
    rates
}

fn pricing(rebalance_ppm: u32, spread_ppm: u32) -> Pricing {
    Pricing {
        charges: Charges {
            fee_ppm: 0,
            spread_ppm,
        },
        rebalance_ppm,
    }
}

fn table(xrp_rate: u128, usd: u64, xrp: u64, pricing: Pricing) -> PriceTable {
    PriceTable::build(
        &rates(xrp_rate),
        &[(USD, usd), (XRP, xrp)],
        pricing,
        NOW,
        AGE,
    )
}

/// The rate from `from` to `to`, or 0 for a pair without a price.
fn rate(table: &PriceTable, from: LedgerId, to: LedgerId) -> u64 {
    table.price(from, to).map_or(0, |price| price.rate_e15)
}

/// The most a price may be: spot from `from_rate` to `to_rate`, times
/// `1 + k`, less the spread, times 10^15, in arbitrary precision.
fn most(from_rate: u128, to_rate: u128, pricing: Pricing) -> BigUint {
    let million = BigUint::from(MILLION);
    BigUint::from(to_rate)
        * BigUint::from(RATE_ONE_E15)
        * BigUint::from(MILLION + pricing.rebalance_ppm)
        * BigUint::from(MILLION - pricing.charges.spread_ppm)
        / (BigUint::from(from_rate) * &million * &million)
}

proptest! {
    // With every ledger at its target share, the curve is 1, whatever k:
    // spot less the spread, exactly.
    #[test]
    fn at_target_the_price_is_spot(
        xrp_per_dollar in 1u128..=1_000,
        dollars in 1u64..=1_000_000_000_000_000,
        rebalance_ppm in 0..=MAX_REBALANCE_PPM,
        spread_ppm in 0..=Charges::MAX_PPM,
    ) {
        let xrp_rate = xrp_per_dollar * 10u128.pow(RATE_PRECISION);
        let xrp = dollars.checked_mul(u64::try_from(xrp_per_dollar).unwrap());
        prop_assume!(xrp.is_some());
        let inventories = [(USD, dollars), (XRP, xrp.unwrap()), (RLUSD, dollars)];
        let table = PriceTable::build(
            &rates(xrp_rate),
            &inventories,
            pricing(rebalance_ppm, spread_ppm),
            NOW,
            AGE,
        );
        let spread = u64::from(MILLION - spread_ppm);
        let per_dollar = u64::try_from(xrp_per_dollar).unwrap();
        prop_assert_eq!(rate(&table, USD, XRP), per_dollar * spread * 1_000_000_000);
        prop_assert_eq!(rate(&table, USD, RLUSD), spread * 1_000_000_000);
        let flat = PriceTable::build(
            &rates(xrp_rate),
            &inventories,
            pricing(0, spread_ppm),
            NOW,
            AGE,
        );
        for price in table.prices() {
            prop_assert_eq!(price.rate_e15, rate(&flat, price.from, price.to));
        }
    }

    // More of a ledger never makes bringing it in better, nor taking it out
    // worse.
    #[test]
    fn the_price_is_monotonic_in_the_holding(
        xrp_rate in 1u128..=1_000 * 10u128.pow(RATE_PRECISION),
        usd in 0u64..=u64::MAX / 2,
        more in 0u64..=u64::MAX / 2,
        xrp in 0u64..=u64::MAX,
        rebalance_ppm in 0..=MAX_REBALANCE_PPM,
        spread_ppm in 0..=Charges::MAX_PPM,
    ) {
        let pricing = pricing(rebalance_ppm, spread_ppm);
        let less = table(xrp_rate, usd, xrp, pricing);
        let richer = table(xrp_rate, usd + more, xrp, pricing);
        prop_assume!(less.ledgers().count() == 2 && richer.ledgers().count() == 2);
        prop_assert!(rate(&richer, USD, XRP) <= rate(&less, USD, XRP));
        prop_assert!(rate(&richer, XRP, USD) >= rate(&less, XRP, USD));
    }

    // No price is above spot by more than k, less the spread; a round trip
    // at one state never gains, in rates or in what a packet converts to;
    // and no rates or holdings overflow.
    #[test]
    fn the_bonus_is_bounded_and_round_trips_lose(
        xrp_rate in 1u128..,
        usd in any::<u64>(),
        xrp in any::<u64>(),
        rlusd in any::<u64>(),
        amount in any::<u64>(),
        rebalance_ppm in 0..=MAX_REBALANCE_PPM,
        spread_ppm in 0..=Charges::MAX_PPM,
    ) {
        let pricing = pricing(rebalance_ppm, spread_ppm);
        let inventories = [(USD, usd), (XRP, xrp), (RLUSD, rlusd)];
        let rates = rates(xrp_rate);
        let table = PriceTable::build(&rates, &inventories, pricing, NOW, AGE);
        let rate_of = |ledger: LedgerId| rates.get(ledger.asset().code).unwrap();
        for price in table.prices() {
            let bound = most(rate_of(price.from), rate_of(price.to), pricing);
            prop_assert!(BigUint::from(price.rate_e15) <= bound);
            let Some(back) = table.price(price.to, price.from) else {
                continue;
            };
            let product = u128::from(price.rate_e15) * u128::from(back.rate_e15);
            prop_assert!(product <= u128::from(RATE_ONE_E15) * u128::from(RATE_ONE_E15));
            let there_and_back = price.convert(amount).and_then(|out| back.convert(out));
            prop_assert!(there_and_back.is_none_or(|back| back <= amount));
        }
    }
}

// The curve's numbers at a few states: k = 5%, two ledgers held evenly by
// value or not.
#[test]
fn a_skew_costs_what_the_curve_says() {
    let pricing = pricing(50_000, 0);
    let two = 2 * 10u128.pow(RATE_PRECISION);
    let dollar = 1_000_000_000;
    // A quarter in dollars: about 6% worse out of dollars, 5% better in.
    let quarter = table(two, 25 * dollar, 150 * dollar, pricing);
    assert_eq!(rate(&quarter, XRP, USD), 468_253_968_253_968);
    assert_eq!(rate(&quarter, USD, XRP), 2_100_000_000_000_000);
    // A twentieth: about 33% worse.
    let twentieth = table(two, 5 * dollar, 190 * dollar, pricing);
    assert_eq!(rate(&twentieth, XRP, USD) / 1_000_000_000_000, 336);
}
