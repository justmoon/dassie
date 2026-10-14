//! Exchange rates and amount conversion (spec 4 §5.3, D7), in exact
//! integer arithmetic.

use alloc::vec::Vec;

use dassie_ledger::Asset;

/// Rates are fixed-point with this many decimal places, as the original's
/// `INTERNAL_PRECISION`.
pub const RATE_PRECISION: u32 = 15;

/// Most currencies in a snapshot.
const MAX_RATES: usize = 64;

/// A snapshot of exchange rates: for each currency, how many units of it
/// one unit of a common base buys, times 10^[`RATE_PRECISION`].
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Rates {
    /// When the snapshot was taken.
    pub as_of_ms: u64,
    rates: Vec<(&'static str, u128)>,
}

impl Rates {
    /// An empty snapshot taken at `as_of_ms`.
    #[must_use]
    pub const fn new(as_of_ms: u64) -> Self {
        Self {
            as_of_ms,
            rates: Vec::new(),
        }
    }

    /// Sets the rate of `code`. Returns false when the snapshot is full or
    /// the rate is zero.
    pub fn set(&mut self, code: &'static str, rate: u128) -> bool {
        if rate == 0 {
            return false;
        }
        if let Some(entry) = self.rates.iter_mut().find(|(c, _)| *c == code) {
            entry.1 = rate;
            return true;
        }
        if self.rates.len() >= MAX_RATES {
            return false;
        }
        self.rates.push((code, rate));
        true
    }

    /// The rate of `code`.
    #[must_use]
    pub fn get(&self, code: &str) -> Option<u128> {
        self.rates
            .iter()
            .find(|(c, _)| *c == code)
            .map(|(_, rate)| *rate)
    }
}

/// Converts `amount` of `from` into `to` at `rates`, rounding down. The
/// same currency converts one to one at any scale difference; the original
/// went through a JavaScript double for `10^k` (ILP-6). Returns `None` for a
/// missing rate or a result that does not fit.
#[must_use]
pub fn convert(rates: &Rates, from: Asset, to: Asset, amount: u64) -> Option<u64> {
    let (from_rate, to_rate) = if from.code == to.code {
        (1, 1)
    } else {
        (rates.get(from.code)?, rates.get(to.code)?)
    };
    // amount / 10^from.scale units of `from` are worth
    // amount * to_rate / from_rate / 10^from.scale units of `to`, which is
    // that times 10^to.scale in `to`'s smallest unit.
    let mut numerator = u128::from(amount).checked_mul(to_rate)?;
    let mut denominator = from_rate;
    if to.scale >= from.scale {
        numerator = numerator.checked_mul(10u128.checked_pow(u32::from(to.scale - from.scale))?)?;
    } else {
        denominator =
            denominator.checked_mul(10u128.checked_pow(u32::from(from.scale - to.scale))?)?;
    }
    u64::try_from(numerator / denominator).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    const USD: Asset = Asset {
        code: "USD",
        scale: 9,
    };
    const XRP: Asset = Asset {
        code: "XRP",
        scale: 9,
    };

    fn rates() -> Rates {
        let mut rates = Rates::new(0);
        // 1 USD buys 1 USD, and 2.5 XRP.
        rates.set("USD", 10u128.pow(RATE_PRECISION));
        rates.set("XRP", 25 * 10u128.pow(RATE_PRECISION - 1));
        rates
    }

    #[test]
    fn converts_exactly_and_rounds_down() {
        assert_eq!(
            convert(&rates(), USD, XRP, 1_000_000_000),
            Some(2_500_000_000)
        );
        assert_eq!(
            convert(&rates(), XRP, USD, 2_500_000_000),
            Some(1_000_000_000)
        );
        assert_eq!(convert(&rates(), XRP, USD, 3), Some(1));
        assert_eq!(convert(&rates(), XRP, USD, 2), Some(0));
    }

    // ILP-6: 10^k went through a double, exact only for k <= 22.
    #[test]
    fn scales_are_exact_powers_of_ten() {
        let cents = Asset {
            code: "USD",
            scale: 2,
        };
        assert_eq!(
            convert(&Rates::new(0), cents, USD, 123),
            Some(1_230_000_000)
        );
        assert_eq!(
            convert(&Rates::new(0), USD, cents, 1_239_999_999),
            Some(123)
        );
        let drops = Asset {
            code: "XRP",
            scale: 6,
        };
        assert_eq!(convert(&rates(), cents, drops, 100), Some(2_500_000));
    }

    #[test]
    fn missing_rates_and_overflow_are_none() {
        let eur = Asset {
            code: "EUR",
            scale: 9,
        };
        assert_eq!(convert(&rates(), USD, eur, 1), None);
        let tiny = Asset {
            code: "XRP",
            scale: 0,
        };
        let huge = Asset {
            code: "USD",
            scale: 255,
        };
        assert_eq!(convert(&rates(), tiny, huge, u64::MAX), None);
        assert!(!Rates::new(0).set("USD", 0));
    }
}
