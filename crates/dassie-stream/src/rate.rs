use core::fmt;

/// An exchange rate: `numerator` destination units per `denominator` source
/// units, in lowest terms. Exact integer math throughout
/// (spec/decisions/exchange-rates.md); no floats.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct Rate {
    numerator: u64,
    denominator: u64,
}

const fn gcd(mut a: u128, mut b: u128) -> u128 {
    while b != 0 {
        let remainder = a % b;
        a = b;
        b = remainder;
    }
    a
}

impl Rate {
    /// One destination unit per source unit.
    pub const ONE: Self = Self {
        numerator: 1,
        denominator: 1,
    };

    /// A rate that converts everything to zero: sending without caring what
    /// arrives. Used by `dangerously_ignore_exchange_rate`.
    pub const ZERO: Self = Self {
        numerator: 0,
        denominator: 1,
    };

    /// `numerator / denominator`, reduced. `None` for a zero denominator.
    #[must_use]
    pub const fn new(numerator: u64, denominator: u64) -> Option<Self> {
        if denominator == 0 {
            return None;
        }
        Some(Self::reduce(numerator as u128, denominator as u128))
    }

    /// Reduces a ratio of 128-bit terms to lowest terms that fit 64 bits,
    /// shifting both down (losing precision, not magnitude) if they do not.
    const fn reduce(numerator: u128, denominator: u128) -> Self {
        let divisor = gcd(numerator, denominator);
        let (mut numerator, mut denominator) = if divisor > 1 {
            (numerator / divisor, denominator / divisor)
        } else {
            (numerator, denominator)
        };
        while numerator > u64::MAX as u128 || denominator > u64::MAX as u128 {
            numerator >>= 1;
            denominator = if denominator > 1 { denominator >> 1 } else { 1 };
        }
        #[allow(clippy::cast_possible_truncation)]
        // Both terms fit after the loop above.
        let rate = Self {
            numerator: numerator as u64,
            denominator: denominator as u64,
        };
        rate
    }

    /// Destination units per `denominator` source units.
    #[must_use]
    pub const fn numerator(self) -> u64 {
        self.numerator
    }

    /// Source units the numerator is relative to.
    #[must_use]
    pub const fn denominator(self) -> u64 {
        self.denominator
    }

    /// Whether the rate converts everything to zero.
    #[must_use]
    pub const fn is_zero(self) -> bool {
        self.numerator == 0
    }

    /// `floor(source * rate)`, saturating at `u64::MAX`.
    #[must_use]
    pub const fn destination_amount(self, source: u64) -> u64 {
        let product = source as u128 * self.numerator as u128 / self.denominator as u128;
        saturate(product)
    }

    /// `floor(source * rate)`, or `None` if it does not fit a `u64`: what a
    /// fixed-send quote delivers, which never delivers more than its debit
    /// buys.
    #[must_use]
    pub fn checked_destination_amount(self, source: u64) -> Option<u64> {
        let product =
            u128::from(source) * u128::from(self.numerator) / u128::from(self.denominator);
        u64::try_from(product).ok()
    }

    /// The fewest source units that convert to at least `destination`
    /// units: `ceil(destination / rate)`. `None` for a zero rate, or if it
    /// does not fit a `u64`. What a fixed-receive quote debits, which always
    /// covers its delivery.
    #[must_use]
    pub fn min_source_amount(self, destination: u64) -> Option<u64> {
        if self.numerator == 0 {
            return None;
        }
        let numerator = u128::from(destination) * u128::from(self.denominator);
        let source = u64::try_from(numerator.div_ceil(u128::from(self.numerator))).ok()?;
        assert!(
            self.checked_destination_amount(source)
                .is_none_or(|delivered| delivered >= destination),
            "rounding up covers the destination amount"
        );
        Some(source)
    }

    /// The most source units that convert to at most `destination` units:
    /// `floor(destination / rate)`. Unlimited for a zero rate.
    #[must_use]
    pub const fn max_source_amount(self, destination: u64) -> u64 {
        if self.numerator == 0 {
            return u64::MAX;
        }
        // floor((d + 1) * den / num) - 1 would be the exact maximum when the
        // division is not exact; floor(d * den / num) is the safe side.
        saturate(destination as u128 * self.denominator as u128 / self.numerator as u128)
    }

    /// The most source units that convert to at most `destination` units,
    /// exactly: `ceil((destination + 1) / rate) - 1`. Unlimited for a zero
    /// rate. Unlike [`Self::max_source_amount`], a part of a source unit
    /// left over still counts, so the last destination unit can be sent.
    #[must_use]
    pub const fn exact_max_source_amount(self, destination: u64) -> u64 {
        if self.numerator == 0 {
            return u64::MAX;
        }
        // At most (2^64)(2^64 - 1), which fits.
        let bound = (destination as u128 + 1) * self.denominator as u128;
        saturate(bound.div_ceil(self.numerator as u128) - 1)
    }

    /// The fewest source units that convert to at least one destination
    /// unit: `ceil(1 / rate)`. None for a zero rate.
    #[must_use]
    pub const fn min_source_for_one_unit(self) -> Option<u64> {
        if self.numerator == 0 {
            return None;
        }
        Some(self.denominator.div_ceil(self.numerator))
    }

    /// The rate reduced by `slippage_permille` thousandths, the least a
    /// sender accepts when its measured rate is this one.
    #[must_use]
    pub const fn with_slippage(self, slippage_permille: u16) -> Self {
        assert!(slippage_permille <= 1000, "slippage above 100%");
        let kept = 1000 - slippage_permille as u128;
        Self::reduce(
            self.numerator as u128 * kept,
            self.denominator as u128 * 1000,
        )
    }

    /// Whether `self` is at least `other`.
    #[must_use]
    pub const fn at_least(self, other: Self) -> bool {
        self.numerator as u128 * other.denominator as u128
            >= other.numerator as u128 * self.denominator as u128
    }
}

const fn saturate(value: u128) -> u64 {
    if value > u64::MAX as u128 {
        u64::MAX
    } else {
        #[allow(clippy::cast_possible_truncation)]
        // Checked by the branch.
        let value = value as u64;
        value
    }
}

impl fmt::Debug for Rate {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Rate({}/{})", self.numerator, self.denominator)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_for_one_is_exact() {
        assert_eq!(Rate::ONE.checked_destination_amount(1_234), Some(1_234));
        assert_eq!(Rate::ONE.min_source_amount(1_234), Some(1_234));
    }

    #[test]
    fn quotes_round_in_the_receivers_favor() {
        let rate = Rate::new(2, 3).unwrap();
        assert_eq!(rate.checked_destination_amount(10), Some(6));
        assert_eq!(rate.min_source_amount(7), Some(11));
        assert_eq!(rate.checked_destination_amount(11), Some(7));
        assert_eq!(Rate::ZERO.min_source_amount(1), None);
        assert_eq!(Rate::ZERO.checked_destination_amount(5), Some(0));
        let steep = Rate::new(u64::MAX, 1).unwrap();
        assert_eq!(steep.checked_destination_amount(2), None);
        assert_eq!(steep.destination_amount(2), u64::MAX);
    }
}
