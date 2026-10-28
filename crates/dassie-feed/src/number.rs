//! The numbers of a snapshot (spec/areas/rate-feed.md#snapshots): quotes as
//! 32-bit floats, and ladders of amounts as a 16-bit float and 8-bit steps.
//!
//! Every number here is defined by integer arithmetic alone, so every
//! reader decodes a snapshot to the same values, and each has exactly one
//! encoding: a decoder refuses any other, and every encoder rounds the way
//! that never says more than the node gives.
//!
//! - **A quote** ([`Quote`]) is `m · 2^(e − 150)`, the exponent `e` in the
//!   high 8 bits and the mantissa `m` in the low 24, normalized
//!   (`m ≥ 2^23`); the word zero is the quote zero, a side the node does not
//!   convert. A bid rounds down ([`Quote::floor`]), an ask up
//!   ([`Quote::ceil`]).
//! - **A ladder's first amount** is `m · 2^e`, the exponent `e` in the high 6
//!   bits and the mantissa `m` in the low 10, with `m ≥ 512` unless `e = 0`
//!   and the value within a `u64`, rounded down ([`amount16_floor`]).
//! - **Each later amount** is a step `b` up from the amount before it:
//!   `floor(prev · 2^(b/16))`, computed as
//!   `floor(prev · F[b mod 16] / 2^(62 − ⌊b/16⌋))` with
//!   `F[k] = floor(2^(62 + k/16))` ([`step_up`]). The step is the smallest
//!   that gives its amount, so the amounts say what the bytes are.

/// Drops in a grid, and amounts in a ladder.
pub const LEVELS: usize = 8;

/// Bytes of a ladder: the first amount and a step for each later one.
pub const LADDER_BYTES: usize = 2 + (LEVELS - 1);

/// The exponent bias of a quote: its value is `mantissa · 2^(exponent −
/// QUOTE_BIAS)`, so a quote spans about `2^−127` to `2^129`.
pub const QUOTE_BIAS: i32 = 150;

const QUOTE_MANTISSA_BITS: u32 = 24;
const QUOTE_MANTISSA_MIN: u32 = 1 << (QUOTE_MANTISSA_BITS - 1);
const QUOTE_MANTISSA_MASK: u32 = (1 << QUOTE_MANTISSA_BITS) - 1;
const QUOTE_EXPONENT_MAX: i32 = 255;

const AMOUNT_MANTISSA_BITS: u32 = 10;
const AMOUNT_MANTISSA_MIN: u16 = 1 << (AMOUNT_MANTISSA_BITS - 1);
const AMOUNT_MANTISSA_MASK: u16 = (1 << AMOUNT_MANTISSA_BITS) - 1;
/// The largest exponent of a first amount whose value fits a `u64`:
/// `1023 · 2^54 < 2^64 ≤ 512 · 2^55`.
const AMOUNT_EXPONENT_MAX: u16 = 54;

/// The fixed-point bits of [`STEP_FRACTIONS`].
const STEP_FRACTION_BITS: u32 = 62;

/// `F[k] = floor(2^(62 + k/16))`, for `k` in `0..16`: the fractions of a
/// step, in fixed point. A test checks each against its definition.
pub const STEP_FRACTIONS: [u64; 16] = [
    0x4000_0000_0000_0000,
    0x42d5_61b3_e624_3d8a,
    0x45ca_e0f1_f545_eb73,
    0x48e1_e9b9_d588_e19b,
    0x4c1b_f828_c6dc_54b7,
    0x4f7a_9930_48d0_88d6,
    0x52ff_6b54_d8a8_9c75,
    0x56ac_1f75_2150_a563,
    0x5a82_7999_fcef_3242,
    0x5e84_51cf_ac06_1b5f,
    0x62b3_9508_aa83_6d6e,
    0x6712_460a_8fc2_4071,
    0x6ba2_7e65_6b4e_b57a,
    0x7066_6f76_154a_7088,
    0x7560_6373_ee92_1c97,
    0x7a92_be8a_9243_6616,
];

// A step at 0 is the amount before it; a fraction is below 2^63, so its
// product with a `u64` fits a `u128`; and a step's whole part, at most 15,
// shifts right by at least 47.
const _: () = assert!(STEP_FRACTIONS[0] == 1 << STEP_FRACTION_BITS);
const _: () = assert!(STEP_FRACTIONS[15] < 1 << (STEP_FRACTION_BITS + 1));
const _: () = assert!(((u8::MAX >> 4) as u32) < STEP_FRACTION_BITS);

/// A quote: what the node gives (bid) or asks (ask) in its virtual unit per
/// unit of a method's ledger, as a 32-bit float. Only a canonical word is
/// ever held, so two equal quotes have equal bits, and quotes order by
/// value as their bits do.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Quote(u32);

impl Quote {
    /// Zero: a side the node does not convert.
    pub const ZERO: Self = Self(0);

    /// The quote of a word, if it is canonical: zero, or normalized.
    #[must_use]
    pub const fn from_bits(bits: u32) -> Option<Self> {
        if bits == 0 || bits & QUOTE_MANTISSA_MASK >= QUOTE_MANTISSA_MIN {
            Some(Self(bits))
        } else {
            None
        }
    }

    /// The word.
    #[must_use]
    pub const fn to_bits(self) -> u32 {
        self.0
    }

    /// Whether it is zero.
    #[must_use]
    pub const fn is_zero(self) -> bool {
        self.0 == 0
    }

    /// The mantissa `m`: the value is `m · 2^`[`exponent`](Self::exponent).
    #[must_use]
    pub const fn mantissa(self) -> u32 {
        self.0 & QUOTE_MANTISSA_MASK
    }

    /// The unbiased exponent, `e − QUOTE_BIAS`.
    #[must_use]
    pub const fn exponent(self) -> i32 {
        // The word's top byte, which fits an `i32`.
        let biased = (self.0 >> QUOTE_MANTISSA_BITS).cast_signed();
        biased - QUOTE_BIAS
    }

    /// The largest quote at most `numerator / denominator`: zero below the
    /// smallest quote.
    ///
    /// # Panics
    ///
    /// If `denominator` is zero.
    #[must_use]
    pub fn floor(numerator: u128, denominator: u128) -> Self {
        assert!(denominator > 0, "a quote of a ratio needs a denominator");
        Self::round(numerator, denominator, false)
    }

    /// The smallest quote at least `numerator / denominator`: zero only for
    /// zero.
    ///
    /// # Panics
    ///
    /// If `denominator` is zero.
    #[must_use]
    pub fn ceil(numerator: u128, denominator: u128) -> Self {
        assert!(denominator > 0, "a quote of a ratio needs a denominator");
        Self::round(numerator, denominator, true)
    }

    fn round(numerator: u128, denominator: u128, up: bool) -> Self {
        if numerator == 0 {
            return Self::ZERO;
        }
        // numerator / denominator is in [2^(k−1), 2^(k+1)), so scaled by
        // 2^(24 − k) its floor is in [2^23, 2^25): one bit less if need be.
        let k = bit_length(numerator) - bit_length(denominator);
        let mut shift = 24 - k;
        let (mut mantissa, mut exact) = scaled_div(numerator, denominator, shift);
        if mantissa >> QUOTE_MANTISSA_BITS != 0 {
            shift -= 1;
            (mantissa, exact) = scaled_div(numerator, denominator, shift);
        }
        assert!(mantissa >> (QUOTE_MANTISSA_BITS - 1) == 1, "normalized");
        // The value is mantissa · 2^(−shift).
        let mut biased = QUOTE_BIAS - shift;
        if biased < 0 {
            // Below the smallest quote.
            return if up {
                Self(QUOTE_MANTISSA_MIN)
            } else {
                Self::ZERO
            };
        }
        if up && !exact {
            mantissa += 1;
            if mantissa >> QUOTE_MANTISSA_BITS != 0 {
                mantissa >>= 1;
                biased += 1;
            }
        }
        // A ratio of two `u128`s is below 2^128, below the largest quote.
        assert!(
            biased <= QUOTE_EXPONENT_MAX,
            "a ratio of u128s fits a quote"
        );
        let biased = biased.cast_unsigned();
        let mantissa = u32::try_from(mantissa).unwrap_or(QUOTE_MANTISSA_MASK);
        let quote = Self((biased << QUOTE_MANTISSA_BITS) | mantissa);
        assert!(
            Self::from_bits(quote.0).is_some(),
            "rounding gives a canonical quote"
        );
        quote
    }
}

/// Bits in `value`, signed for exponent arithmetic.
fn bit_length(value: u128) -> i32 {
    // At most 128, so it fits.
    (u128::BITS - value.leading_zeros()).cast_signed()
}

/// `floor(numerator · 2^shift / denominator)`, and whether it is exact, for
/// a result below 2^26 and a non-zero denominator.
fn scaled_div(numerator: u128, denominator: u128, shift: i32) -> (u128, bool) {
    let mut quotient = numerator / denominator;
    let mut remainder = numerator % denominator;
    if shift <= 0 {
        let down = shift.unsigned_abs();
        if down >= u128::BITS {
            return (0, numerator == 0);
        }
        let lost = quotient & ((1u128 << down) - 1);
        return (quotient >> down, remainder == 0 && lost == 0);
    }
    // Long division, one bit at a time: 2·remainder may not fit, so it is
    // compared with the denominator as remainder against what is left.
    for _ in 0..shift {
        assert!(quotient >> 100 == 0, "the scaled quotient stays small");
        quotient <<= 1;
        let rest = denominator - remainder;
        if remainder >= rest {
            remainder -= rest;
            quotient += 1;
        } else {
            remainder <<= 1;
        }
    }
    (quotient, remainder == 0)
}

/// The largest first amount of a ladder at most `amount`, as its 16 bits.
#[must_use]
pub fn amount16_floor(amount: u64) -> u16 {
    let bits = u64::BITS - amount.leading_zeros();
    if bits <= AMOUNT_MANTISSA_BITS {
        // Below 1024: exponent zero, exact.
        return u16::try_from(amount).unwrap_or(AMOUNT_MANTISSA_MASK);
    }
    let exponent = bits - AMOUNT_MANTISSA_BITS;
    let mantissa = u16::try_from(amount >> exponent).unwrap_or(AMOUNT_MANTISSA_MASK);
    let exponent = u16::try_from(exponent).unwrap_or(AMOUNT_EXPONENT_MAX);
    assert!(exponent <= AMOUNT_EXPONENT_MAX && mantissa >= AMOUNT_MANTISSA_MIN);
    (exponent << AMOUNT_MANTISSA_BITS) | mantissa
}

/// The value of a first amount's 16 bits, if they are canonical: `m ≥ 512`
/// unless the exponent is zero, and the value within a `u64`.
#[must_use]
pub const fn amount16_value(bits: u16) -> Option<u64> {
    let exponent = bits >> AMOUNT_MANTISSA_BITS;
    let mantissa = bits & AMOUNT_MANTISSA_MASK;
    if exponent == 0 {
        return Some(mantissa as u64);
    }
    if mantissa < AMOUNT_MANTISSA_MIN || exponent > AMOUNT_EXPONENT_MAX {
        return None;
    }
    Some((mantissa as u64) << exponent)
}

/// `previous · 2^(step/16)`, rounded down, or `None` when that exceeds a
/// `u64`. It never decreases as `step` grows, and is `previous` at step 0.
#[must_use]
pub fn step_up(previous: u64, step: u8) -> Option<u64> {
    let fraction = *STEP_FRACTIONS.get(usize::from(step & 0x0f))?;
    let whole = u32::from(step >> 4);
    // Below 2^64 · 2^63.
    let product = u128::from(previous) * u128::from(fraction);
    u64::try_from(product >> (STEP_FRACTION_BITS - whole)).ok()
}

/// A ladder: [`LEVELS`] amounts of a method's ledger, each for a drop of the
/// grid, non-decreasing, and encoded as a 16-bit float and 8-bit steps.
/// Only a canonical ladder is ever held.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Ladder {
    bytes: [u8; LADDER_BYTES],
    amounts: [u64; LEVELS],
}

impl Ladder {
    /// All zero: the ladder of a closed side.
    pub const ZERO: Self = Self {
        bytes: [0; LADDER_BYTES],
        amounts: [0; LEVELS],
    };

    /// The ladder that says the most it can without saying more than
    /// `amounts`: each amount the largest it can be at most the one given,
    /// stepped from the amount before as decoded. An amount above a later
    /// one is taken as the later one, so the ladder is non-decreasing and
    /// never above `amounts`.
    #[must_use]
    pub fn floor(amounts: [u64; LEVELS]) -> Self {
        let mut bounded = amounts;
        let mut above = u64::MAX;
        for amount in bounded.iter_mut().rev() {
            above = above.min(*amount);
            *amount = above;
        }
        let first = amount16_floor(bounded[0]);
        let mut previous = amount16_value(first).unwrap_or(0);
        let mut steps = [0u8; LEVELS - 1];
        for (slot, &target) in steps.iter_mut().zip(bounded.iter().skip(1)) {
            assert!(previous <= target, "the amounts never decrease");
            let largest = largest_step_within(previous, target);
            let value = step_up(previous, largest).unwrap_or(previous);
            *slot = smallest_step_reaching(previous, value);
            previous = value;
        }
        let mut bytes = [0u8; LADDER_BYTES];
        let encoded = first.to_be_bytes().into_iter().chain(steps);
        for (slot, byte) in bytes.iter_mut().zip(encoded) {
            *slot = byte;
        }
        let ladder = Self::from_bytes(&bytes);
        assert!(ladder.is_some(), "the encoding is canonical");
        let ladder = ladder.unwrap_or(Self::ZERO);
        assert!(
            ladder
                .amounts
                .iter()
                .zip(bounded)
                .all(|(decoded, given)| *decoded <= given),
            "a ladder never says more than the amounts"
        );
        ladder
    }

    /// The ladder of `bytes`, if they are canonical: the first amount's
    /// float canonical, every amount within a `u64`, and each step the
    /// smallest that gives its amount.
    #[must_use]
    pub fn from_bytes(bytes: &[u8; LADDER_BYTES]) -> Option<Self> {
        let [high, low, steps @ ..] = *bytes;
        let mut previous = amount16_value(u16::from_be_bytes([high, low]))?;
        let mut amounts = [previous; LEVELS];
        for (slot, step) in amounts.iter_mut().skip(1).zip(steps) {
            let value = step_up(previous, step)?;
            if step > 0 && step_up(previous, step - 1) == Some(value) {
                return None;
            }
            *slot = value;
            previous = value;
        }
        assert!(amounts.is_sorted(), "a step never goes down");
        Some(Self {
            bytes: *bytes,
            amounts,
        })
    }

    /// The bytes.
    #[must_use]
    pub const fn to_bytes(&self) -> [u8; LADDER_BYTES] {
        self.bytes
    }

    /// The amounts, non-decreasing.
    #[must_use]
    pub const fn amounts(&self) -> [u64; LEVELS] {
        self.amounts
    }

    /// Whether every amount is zero.
    #[must_use]
    pub const fn is_zero(&self) -> bool {
        self.amounts[LEVELS - 1] == 0
    }
}

/// The largest step from `previous` whose amount is at most `target`, which
/// is at least `previous`.
fn largest_step_within(previous: u64, target: u64) -> u8 {
    let within = |step: u8| step_up(previous, step).is_some_and(|value| value <= target);
    // Invariant: `within(low)`, and no step above `high` is.
    let (mut low, mut high) = (0u8, u8::MAX);
    while low < high {
        let middle = low + (high - low).div_ceil(2);
        if within(middle) {
            low = middle;
        } else {
            high = middle - 1;
        }
    }
    low
}

/// The smallest step from `previous` whose amount is `value`, which some
/// step gives.
fn smallest_step_reaching(previous: u64, value: u64) -> u8 {
    let reaches = |step: u8| step_up(previous, step).is_none_or(|amount| amount >= value);
    // Invariant: `reaches(high)`, and no step below `low` does.
    let (mut low, mut high) = (0u8, u8::MAX);
    while low < high {
        let middle = low + (high - low) / 2;
        if reaches(middle) {
            high = middle;
        } else {
            low = middle + 1;
        }
    }
    low
}
