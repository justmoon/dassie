//! Quote math: what a payment debits and delivers at an exchange rate, in
//! exact integers.
//!
//! A fixed-send quote delivers what the debit buys, rounded down; a
//! fixed-receive quote debits what the delivery costs, rounded up, so it
//! always covers it. Until the node has exchange rates (M7) every quote is
//! same-asset at [`Rate::ONE`].

/// Destination units per source unit, as a fraction.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rate {
    /// The destination amount.
    pub destination: u64,
    /// The source amount it is worth. Never zero.
    pub source: u64,
}

impl Rate {
    /// One for one.
    pub const ONE: Self = Self {
        destination: 1,
        source: 1,
    };

    /// What `debit` delivers, rounded down; `None` if it does not fit a
    /// `u64` or the rate is not one.
    #[must_use]
    pub fn delivered(&self, debit: u64) -> Option<u64> {
        if self.source == 0 {
            return None;
        }
        let exact = u128::from(debit) * u128::from(self.destination) / u128::from(self.source);
        u64::try_from(exact).ok()
    }

    /// What delivering `receive` costs, rounded up; `None` if it does not
    /// fit, or nothing delivers it.
    #[must_use]
    pub fn cost(&self, receive: u64) -> Option<u64> {
        if self.source == 0 || self.destination == 0 {
            return None;
        }
        let numerator = u128::from(receive) * u128::from(self.source);
        let destination = u128::from(self.destination);
        let debit = u64::try_from(numerator.div_ceil(destination)).ok()?;
        // Rounding up covers the delivery.
        assert!(
            self.delivered(debit)
                .is_none_or(|delivered| delivered >= receive),
            "a fixed-receive quote must cover its delivery"
        );
        Some(debit)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_for_one_is_exact() {
        assert_eq!(Rate::ONE.delivered(1_234), Some(1_234));
        assert_eq!(Rate::ONE.cost(1_234), Some(1_234));
    }

    #[test]
    fn rounding_favours_the_receiver() {
        let rate = Rate {
            destination: 2,
            source: 3,
        };
        assert_eq!(rate.delivered(10), Some(6));
        assert_eq!(rate.cost(7), Some(11));
        assert_eq!(rate.delivered(11), Some(7));
        let zero = Rate {
            destination: 0,
            source: 1,
        };
        assert_eq!(zero.cost(1), None);
        assert_eq!(zero.delivered(5), Some(0));
    }
}
