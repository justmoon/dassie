//! The snapshot's numbers (spec/areas/rate-feed.md#numbers): each value
//! has one encoding, quotes round the way they are meant to, and a ladder
//! never says more than the amounts it was made from. Fixed seed in CI
//! (.cargo/config.toml).

#![allow(
    clippy::unwrap_used,
    clippy::indexing_slicing,
    clippy::cast_possible_truncation
)]

use dassie_feed::number::{
    LADDER_BYTES, LEVELS, Ladder, QUOTE_BIAS, Quote, STEP_FRACTIONS, amount16_floor,
    amount16_value, step_up,
};
use num_bigint::BigUint;
use proptest::prelude::*;

/// `2^exponent`, exactly.
fn power_of_two(exponent: usize) -> BigUint {
    BigUint::from(1u8) << exponent
}

#[test]
fn step_fractions_are_floor_of_their_definition() {
    // F[k] = floor(2^(62 + k/16)) ⇔ F^16 ≤ 2^(992 + k) < (F + 1)^16.
    for (k, &fraction) in STEP_FRACTIONS.iter().enumerate() {
        let bound = power_of_two(62 * 16 + k);
        assert!(BigUint::from(fraction).pow(16) <= bound, "F[{k}]");
        assert!(BigUint::from(fraction + 1).pow(16) > bound, "F[{k}]");
    }
}

#[test]
fn steps_grow_with_the_step() {
    let mut previous = 0u128;
    for step in 0..=u8::MAX {
        let multiplier = u128::from(STEP_FRACTIONS[usize::from(step & 15)]) << u32::from(step >> 4);
        assert!(multiplier > previous, "step {step}");
        previous = multiplier;
    }
    assert_eq!(step_up(1_000_000, 0), Some(1_000_000));
    assert_eq!(step_up(1_000_000, 16), Some(2_000_000));
    assert_eq!(step_up(1_000_000, 8), Some(1_414_213));
    assert_eq!(step_up(u64::MAX, 1), None);
    assert_eq!(step_up(0, u8::MAX), Some(0));
}

#[test]
fn quotes_round_down_and_up() {
    let one = Quote::floor(1, 1);
    assert_eq!(one.mantissa(), 1 << 23);
    assert_eq!(one.exponent(), -23);
    assert_eq!(Quote::ceil(1, 1), one);
    assert_eq!(Quote::floor(0, 7), Quote::ZERO);
    assert_eq!(Quote::ceil(0, 7), Quote::ZERO);
    // A third is not exact: floor and ceil are one apart.
    let (down, up) = (Quote::floor(1, 3), Quote::ceil(1, 3));
    assert_eq!(down.mantissa() + 1, up.mantissa());
    assert_eq!(down.exponent(), up.exponent());
    // Below the smallest quote: floor is zero, ceil the smallest.
    let tiny = (1u128, u128::MAX);
    assert_eq!(Quote::floor(tiny.0, tiny.1), Quote::ZERO);
    let smallest = Quote::ceil(tiny.0, tiny.1);
    assert_eq!(smallest.to_bits(), 1 << 23);
    assert_eq!(smallest.exponent(), -QUOTE_BIAS);
    // The largest ratio fits.
    let largest = Quote::ceil(u128::MAX, 1);
    assert_eq!(largest.exponent(), 128 - 24 + 1);
    assert_eq!(largest.mantissa(), 1 << 23);
    // Rounding up across a power of two moves the exponent.
    let below_two = Quote::ceil((1 << 25) - 1, 1 << 24);
    assert_eq!(below_two, Quote::floor(2, 1));
}

#[test]
fn only_canonical_quotes() {
    assert_eq!(Quote::from_bits(0), Some(Quote::ZERO));
    assert_eq!(Quote::from_bits(1), None);
    assert_eq!(Quote::from_bits(0x0100_0000), None);
    assert_eq!(Quote::from_bits(0x007f_ffff), None);
    assert!(Quote::from_bits(0x0080_0000).is_some());
    assert!(Quote::from_bits(u32::MAX).is_some());
}

#[test]
fn first_amounts() {
    assert_eq!(amount16_floor(0), 0);
    assert_eq!(amount16_floor(1023), 1023);
    assert_eq!(amount16_value(amount16_floor(1024)), Some(1024));
    assert_eq!(amount16_value(amount16_floor(1025)), Some(1024));
    assert_eq!(amount16_value(amount16_floor(2047)), Some(2046));
    assert_eq!(amount16_value(amount16_floor(u64::MAX)), Some(1023 << 54));
    // Not normalized above exponent zero, or beyond a u64.
    assert_eq!(amount16_value((1 << 10) | 511), None);
    assert_eq!(amount16_value((55 << 10) | 512), None);
    assert_eq!(amount16_value((54 << 10) | 1023), Some(1023 << 54));
}

#[test]
fn ladders() {
    assert_eq!(Ladder::floor([0; LEVELS]), Ladder::ZERO);
    assert_eq!(Ladder::ZERO.to_bytes(), [0; LADDER_BYTES]);
    // Doubling is exactly 16 sixteenths.
    let doubling = Ladder::floor(core::array::from_fn(|level| 1_000 << level));
    assert_eq!(
        doubling.to_bytes(),
        [0x03, 0xe8, 16, 16, 16, 16, 16, 16, 16]
    );
    assert_eq!(
        doubling.amounts(),
        core::array::from_fn(|level| 1_000 << level)
    );
    // An amount above a later one is taken as the later one.
    let dip = Ladder::floor([500, 400, 600, 600, 600, 600, 600, 600]);
    assert_eq!(dip.amounts()[0], 400);
    assert_eq!(dip.amounts()[1], 400);
    // Beyond what one step reaches, the step is the largest.
    let leap = Ladder::floor([1, u64::MAX, u64::MAX, 0, 0, 0, 0, 0]);
    assert_eq!(leap, Ladder::ZERO);
    let leap = Ladder::floor([1; LEVELS].map(|one: u64| one << 63));
    assert!(leap.amounts().iter().all(|&amount| amount <= 1 << 63));
    // A step that is not the smallest for its amount is refused.
    assert_eq!(Ladder::from_bytes(&[0, 1, 1, 0, 0, 0, 0, 0, 0]), None);
    assert!(Ladder::from_bytes(&[0, 1, 0, 0, 0, 0, 0, 0, 0]).is_some());
    // An amount beyond a u64 is refused.
    assert_eq!(
        Ladder::from_bytes(&[0xdb, 0xff, 255, 0, 0, 0, 0, 0, 0]),
        None
    );
}

/// How a quote's value compares with `numerator / denominator`, exactly:
/// `m · 2^e` against it is `m · denominator · 2^e` against `numerator`.
fn quote_vs(quote: Quote, numerator: u128, denominator: u128) -> core::cmp::Ordering {
    let left = BigUint::from(quote.mantissa()) * BigUint::from(denominator);
    let right = BigUint::from(numerator);
    let shift = quote.exponent().unsigned_abs() as usize;
    if quote.exponent() >= 0 {
        (left << shift).cmp(&right)
    } else {
        left.cmp(&(right << shift))
    }
}

proptest! {
    #[test]
    fn floor_and_ceil_bracket_the_ratio(numerator in any::<u128>(), denominator in 1..=u128::MAX) {
        use core::cmp::Ordering::{Greater, Less};
        let (down, up) = (Quote::floor(numerator, denominator), Quote::ceil(numerator, denominator));
        prop_assert!(Quote::from_bits(down.to_bits()) == Some(down));
        prop_assert!(Quote::from_bits(up.to_bits()) == Some(up));
        prop_assert!(down <= up);
        prop_assert!(quote_vs(down, numerator, denominator) != Greater);
        prop_assert!(quote_vs(up, numerator, denominator) != Less);
        // The next quote up from floor is above the ratio, and the next
        // down from ceil below: each is the closest on its side.
        if let Some(next) = next_quote(down) {
            prop_assert_eq!(quote_vs(next, numerator, denominator), Greater);
        }
        if !up.is_zero() && up.to_bits() != 1 << 23 {
            prop_assert_eq!(quote_vs(previous_quote(up), numerator, denominator), Less);
        }
    }

    #[test]
    fn quote_bits_round_trip(bits in any::<u32>()) {
        if let Some(quote) = Quote::from_bits(bits) {
            prop_assert_eq!(quote.to_bits(), bits);
        }
    }

    #[test]
    fn first_amounts_round_down_and_round_trip(amount in any::<u64>(), bits in any::<u16>()) {
        let value = amount16_value(amount16_floor(amount)).unwrap();
        prop_assert!(value <= amount);
        prop_assert!(amount - value <= amount >> 9);
        if let Some(value) = amount16_value(bits) {
            prop_assert_eq!(amount16_floor(value), bits);
        }
    }

    #[test]
    fn ladders_never_say_more(amounts in proptest::array::uniform8(any::<u64>())) {
        let ladder = Ladder::floor(amounts);
        let decoded = ladder.amounts();
        let mut above = u64::MAX;
        for level in (0..LEVELS).rev() {
            above = above.min(amounts[level]);
            prop_assert!(decoded[level] <= above);
        }
        prop_assert!(decoded.is_sorted());
        prop_assert_eq!(Ladder::from_bytes(&ladder.to_bytes()), Some(ladder));
    }

    #[test]
    fn ladders_say_as_much_as_a_step_can(start in 1_000u64..1 << 40, growth in proptest::array::uniform7(0u64..1 << 20)) {
        let mut amounts = [start; LEVELS];
        for level in 1..LEVELS {
            amounts[level] = amounts[level - 1].saturating_add(growth[level - 1]);
        }
        let ladder = Ladder::floor(amounts);
        let decoded = ladder.amounts();
        // Each amount is the largest a step reaches from the one before:
        // the next step would be above the amount given.
        for level in 1..LEVELS {
            let step = ladder.to_bytes()[1 + level];
            prop_assert_eq!(step_up(decoded[level - 1], step), Some(decoded[level]));
            if step < u8::MAX {
                let mut next = step + 1;
                while next < u8::MAX && step_up(decoded[level - 1], next) == Some(decoded[level]) {
                    next += 1;
                }
                if let Some(value) = step_up(decoded[level - 1], next) {
                    prop_assert!(value > amounts[level] || value == decoded[level]);
                }
            }
        }
    }

    #[test]
    fn ladder_bytes_have_one_meaning(bytes in proptest::array::uniform9(any::<u8>())) {
        if let Some(ladder) = Ladder::from_bytes(&bytes) {
            prop_assert_eq!(ladder.to_bytes(), bytes);
            prop_assert_eq!(Ladder::floor(ladder.amounts()), ladder);
        }
    }
}

/// The next quote above `quote`, if any.
fn next_quote(quote: Quote) -> Option<Quote> {
    if quote.is_zero() {
        return Some(Quote::from_bits(1 << 23).unwrap());
    }
    let bits = quote.to_bits();
    if bits & 0x00ff_ffff == 0x00ff_ffff {
        let exponent = bits >> 24;
        (exponent < 255).then(|| Quote::from_bits(((exponent + 1) << 24) | (1 << 23)).unwrap())
    } else {
        Quote::from_bits(bits + 1)
    }
}

/// The quote below `quote`, which is above the smallest.
fn previous_quote(quote: Quote) -> Quote {
    let bits = quote.to_bits();
    if bits & 0x00ff_ffff == 1 << 23 {
        Quote::from_bits((((bits >> 24) - 1) << 24) | 0x00ff_ffff).unwrap()
    } else {
        Quote::from_bits(bits - 1).unwrap()
    }
}
