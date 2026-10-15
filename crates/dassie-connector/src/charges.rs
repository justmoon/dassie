//! What the node charges for carrying a packet from one peer to another
//! (D7): a connector fee, and a spread on conversions.

/// Parts per million.
const MILLION: u128 = 1_000_000;

/// The charges on transit, in parts per million of what passes. Packets to
/// or from the node's own endpoints are never charged.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Charges {
    /// The fee, taken from the incoming amount, rounded up.
    pub fee_ppm: u32,
    /// The spread, taken from a converted amount, rounded down.
    pub spread_ppm: u32,
}

impl Charges {
    /// Nothing charged.
    pub const NONE: Self = Self {
        fee_ppm: 0,
        spread_ppm: 0,
    };

    /// The most either charge may be: 10%, so that a setting in percent
    /// mistaken for parts per million cannot take everything.
    pub const MAX_PPM: u32 = 100_000;

    /// Whether both charges are within [`MAX_PPM`](Self::MAX_PPM).
    #[must_use]
    pub const fn is_valid(&self) -> bool {
        self.fee_ppm <= Self::MAX_PPM && self.spread_ppm <= Self::MAX_PPM
    }

    /// The fee on `amount`: `ceil(amount × fee_ppm / 10^6)`, never more
    /// than `amount`.
    #[must_use]
    pub fn fee(&self, amount: u64) -> u64 {
        let product = u128::from(amount) * u128::from(self.fee_ppm);
        let fee = product.div_ceil(MILLION);
        u64::try_from(fee).unwrap_or(amount).min(amount)
    }

    /// What is left of a converted `amount` after the spread:
    /// `floor(amount × (10^6 − spread_ppm) / 10^6)`.
    #[must_use]
    pub fn after_spread(&self, amount: u64) -> u64 {
        let kept = MILLION.saturating_sub(u128::from(self.spread_ppm));
        let left = u128::from(amount) * kept / MILLION;
        u64::try_from(left).unwrap_or(amount).min(amount)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_fee_rounds_up_and_the_spread_down() {
        let charges = Charges {
            fee_ppm: 1_000,
            spread_ppm: 2_500,
        };
        assert_eq!(charges.fee(0), 0);
        assert_eq!(charges.fee(1), 1);
        assert_eq!(charges.fee(1_000), 1);
        assert_eq!(charges.fee(1_001), 2);
        assert_eq!(charges.fee(u64::MAX), u64::MAX.div_ceil(1_000));
        assert_eq!(charges.after_spread(0), 0);
        assert_eq!(charges.after_spread(1), 0);
        assert_eq!(charges.after_spread(1_000), 997);
        assert_eq!(charges.after_spread(u64::MAX), 18_400_627_213_525_277_735);
        assert_eq!(Charges::NONE.fee(u64::MAX), 0);
        assert_eq!(Charges::NONE.after_spread(u64::MAX), u64::MAX);
    }

    #[test]
    fn charges_are_bounded() {
        assert!(Charges::NONE.is_valid());
        let most = Charges {
            fee_ppm: Charges::MAX_PPM,
            spread_ppm: Charges::MAX_PPM,
        };
        assert!(most.is_valid());
        assert!(
            !Charges {
                fee_ppm: Charges::MAX_PPM + 1,
                ..most
            }
            .is_valid()
        );
        assert!(
            !Charges {
                spread_ppm: Charges::MAX_PPM + 1,
                ..most
            }
            .is_valid()
        );
    }
}
