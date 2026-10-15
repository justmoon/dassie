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
/// one unit of a common base buys, times 10^[`RATE_PRECISION`], and when
/// that was learned.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Rates {
    /// When [`set`](Self::set) says its rates were learned.
    pub as_of_ms: u64,
    rates: Vec<(&'static str, u128, u64)>,
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

    /// Sets the rate of `code`, as of [`as_of_ms`](Self::as_of_ms).
    /// Returns false when the snapshot is full or the rate is zero.
    pub fn set(&mut self, code: &'static str, rate: u128) -> bool {
        self.set_at(code, rate, self.as_of_ms)
    }

    /// Sets the rate of `code`, learned at `as_of_ms`. Returns false when
    /// the snapshot is full or the rate is zero.
    pub fn set_at(&mut self, code: &'static str, rate: u128, as_of_ms: u64) -> bool {
        if rate == 0 {
            return false;
        }
        if let Some(entry) = self.rates.iter_mut().find(|(c, _, _)| *c == code) {
            *entry = (code, rate, as_of_ms);
            return true;
        }
        if self.rates.len() >= MAX_RATES {
            return false;
        }
        self.rates.push((code, rate, as_of_ms));
        true
    }

    /// The rate of `code`.
    #[must_use]
    pub fn get(&self, code: &str) -> Option<u128> {
        self.entry(code).map(|(rate, _)| rate)
    }

    /// When the rate of `code` was learned.
    #[must_use]
    pub fn as_of(&self, code: &str) -> Option<u64> {
        self.entry(code).map(|(_, as_of_ms)| as_of_ms)
    }

    /// Every rate: its currency, the rate, and when it was learned.
    pub fn iter(&self) -> impl Iterator<Item = (&'static str, u128, u64)> + '_ {
        self.rates.iter().copied()
    }

    /// Whether converting between `from` and `to` uses only rates learned
    /// at most `max_age_ms` before `now_ms`, and not after: a clock that
    /// went back cannot make a rate young. The same currency needs none.
    #[must_use]
    pub fn fresh(&self, from: &str, to: &str, now_ms: u64, max_age_ms: u64) -> bool {
        if from == to {
            return true;
        }
        let young = |code| {
            self.as_of(code)
                .is_some_and(|at| at <= now_ms && now_ms - at <= max_age_ms)
        };
        young(from) && young(to)
    }

    fn entry(&self, code: &str) -> Option<(u128, u64)> {
        self.rates
            .iter()
            .find(|(c, _, _)| *c == code)
            .map(|&(_, rate, as_of_ms)| (rate, as_of_ms))
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
    fn each_rate_ages_on_its_own() {
        let mut rates = rates();
        rates.set_at("EUR", 9 * 10u128.pow(RATE_PRECISION - 1), 5_000);
        assert!(rates.fresh("USD", "XRP", 1_000, 1_000));
        assert!(!rates.fresh("USD", "XRP", 1_001, 1_000));
        assert!(rates.fresh("EUR", "EUR", u64::MAX, 0));
        assert!(!rates.fresh("EUR", "XRP", 1_000, 1_000), "from the future");
        rates.set_at("USD", 10u128.pow(RATE_PRECISION), 5_500);
        assert!(rates.fresh("EUR", "USD", 6_000, 1_000));
        assert!(!rates.fresh("EUR", "USD", 6_001, 1_000));
        assert!(!rates.fresh("EUR", "GBP", 5_000, 1_000), "never learned");
        assert_eq!(rates.as_of("EUR"), Some(5_000));
        assert_eq!(rates.iter().count(), 3);
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
