//! Inventory pricing (spec/areas/connector.md#inventory-pricing,
//! spec/decisions/passive-rebalancing.md): what a node gives for what a peer
//! brings it from another ledger, moved from spot by how much of each ledger
//! the owner holds, so that conversions bring a skewed inventory back toward
//! its targets without the node trading.
//!
//! For each ledger that converts, `v_i` is the owner's available funds on it,
//! valued at spot; `s_i = v_i / Σv` is its share, and `t_i = 1/n` its target.
//! A unit of it is worth `w_i = (1 − k) + k · t_i / s_i` against spot, so a
//! ledger short of its target is worth more. The rate from `a` to `b` is
//! `spot(a → b) · w_a / w_b · (1 − spread)`:
//!
//! - **1 at target:** with every share at its target, the rate is spot less
//!   the spread.
//! - **The bonus is capped** at `1 + k`: bringing in what the node lacks is
//!   rewarded, but never by more than `k`, so a peer cannot drain the curve.
//! - **The penalty is not:** taking out what the node lacks gets worse
//!   without bound, and out of a ledger with nothing, nothing converts; the
//!   owner's group limit refuses it anyway.
//! - **A round trip at one state never gains:** `w_a/w_b · w_b/w_a = 1`
//!   before the cap and the spread, and both only take away.
//!
//! Everything is exact integer arithmetic, rounding down: shares are known to
//! twelve decimals, and a rate is never better for the sender than the curve
//! at them. The table is built once per change of rates, balances or
//! settings, and a packet then only multiplies.

use dassie_ledger::LedgerId;

use crate::charges::Charges;
use crate::fx::{RATE_PRECISION, Rates};

/// One unit of a rate: rates are out per in, times 10^15.
pub const RATE_ONE_E15: u64 = 1_000_000_000_000_000;

/// What a new node starts with as `k`: 5% (spec/decisions/passive-rebalancing.md),
/// a starting point.
pub const DEFAULT_REBALANCE_PPM: u32 = 50_000;

/// The most `k` may be: as the charges, 10%, so that a setting in percent
/// mistaken for parts per million cannot set the curve wild.
pub const MAX_REBALANCE_PPM: u32 = Charges::MAX_PPM;

/// Most ledgers a table prices: every ledger there is.
pub const MAX_PRICED_LEDGERS: usize = LedgerId::ALL.len();

/// Most prices in a table: one per ordered pair of ledgers.
pub const MAX_PRICES: usize = MAX_PRICED_LEDGERS * (MAX_PRICED_LEDGERS - 1);

/// Parts per million.
const MILLION: u128 = 1_000_000;

/// A whole share: each ledger's share of the owner's funds is known to
/// twelve decimal places, rounded down, which keeps every product of the
/// curve within 128 bits. A share that rounds to nothing is a ledger with
/// nothing.
const SHARE_ONE: u128 = 1_000_000_000_000;

/// The most one ledger's value counts as: so that the values of every
/// ledger add up within 128 bits. Only shares read values.
const MAX_VALUE: u128 = u128::MAX / 16;

// The bounds of `factor`'s products: 10^6 · n · 10^12 + 10^5 · 10^12 stays
// below 2^64, so a weight times a share stays below 2^104, and that times
// 1.1 · 10^6 below 2^125.
const _: () = assert!(
    MAX_PRICED_LEDGERS <= 16,
    "the curve's products fit 128 bits"
);
const _: () = assert!(
    MAX_REBALANCE_PPM <= 100_000,
    "the curve's products fit 128 bits"
);

/// What the owner sets for transit: the charges, and the curve's `k`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Pricing {
    /// The fee and the spread on what passes from one peer to another.
    pub charges: Charges,
    /// `k`, in parts per million: how far the curve moves a rate from spot
    /// for a skewed inventory. Zero converts at spot, less the spread.
    pub rebalance_ppm: u32,
}

impl Pricing {
    /// Nothing charged, and no curve.
    pub const NONE: Self = Self {
        charges: Charges::NONE,
        rebalance_ppm: 0,
    };

    /// Whether the charges and `k` are within their bounds.
    #[must_use]
    pub const fn is_valid(&self) -> bool {
        self.charges.is_valid() && self.rebalance_ppm <= MAX_REBALANCE_PPM
    }
}

/// A ledger the table prices: its spot rate, and what the owner holds on it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LedgerPrice {
    /// The ledger.
    pub ledger: LedgerId,
    /// How many units of its asset the common base buys, times
    /// 10^[`RATE_PRECISION`], as [`Rates`] has it.
    pub rate: u128,
    /// When that rate was learned.
    pub as_of_ms: u64,
    /// What the owner may spend on it, in its units.
    pub available: u64,
}

/// What the node gives, on transit from one peer to another, for a unit
/// that comes in on `from`, in units of `to`, after the fee.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Price {
    /// The ledger the packet comes in on.
    pub from: LedgerId,
    /// The ledger it goes out on.
    pub to: LedgerId,
    /// Out per in, times [`RATE_ONE_E15`]: spot, moved by the curve, less
    /// the spread, rounded down. Zero when the owner holds nothing on `to`.
    pub rate_e15: u64,
    /// When the older of its two rates was learned: it is good while that is.
    pub as_of_ms: u64,
}

impl Price {
    /// What `net` converts to: `floor(net × rate_e15 / 10^15)`, or `None`
    /// when that does not fit.
    #[must_use]
    pub fn convert(&self, net: u64) -> Option<u64> {
        let out = u128::from(net) * u128::from(self.rate_e15) / u128::from(RATE_ONE_E15);
        u64::try_from(out).ok()
    }
}

/// Why a packet could not be converted.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PriceError {
    /// A rate it needs is missing or stale.
    Stale,
    /// The owner holds nothing on the ledger it would go out on.
    NoFunds,
    /// What it converts to does not fit.
    TooLarge,
}

/// Every price the connector converts transit at, and the spot rates it
/// converts into the owner's own endpoints at, with what they were built
/// from. Fixed in size and copied whole, so that a rate feed can publish it
/// as it is: what a packet converts at.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PriceTable {
    /// When it was built.
    built_at_ms: u64,
    /// The earliest time a rate it uses ages out or comes of age: by then
    /// it must be built again.
    valid_until_ms: u64,
    pricing: Pricing,
    ledgers: [Option<LedgerPrice>; MAX_PRICED_LEDGERS],
    prices: [Option<Price>; MAX_PRICES],
}

/// A table never built: nothing charged, nothing converts, and it is due
/// to be built at once.
impl Default for PriceTable {
    fn default() -> Self {
        Self {
            valid_until_ms: 0,
            ..Self::without_rates(Pricing::NONE)
        }
    }
}

impl PriceTable {
    /// A table for `pricing` without rates: only what stays on one ledger
    /// passes, charged its fee.
    #[must_use]
    pub const fn without_rates(pricing: Pricing) -> Self {
        Self {
            built_at_ms: 0,
            valid_until_ms: u64::MAX,
            pricing,
            ledgers: [None; MAX_PRICED_LEDGERS],
            prices: [None; MAX_PRICES],
        }
    }

    /// Prices every ordered pair of the ledgers in `inventories` that have
    /// a rate learned at most `max_rate_age_ms` before `now_ms`, and not
    /// after: the others are absent, and their packets get T00. Each
    /// inventory is what the owner may spend on that ledger now. A ledger
    /// named twice counts once, at its first.
    ///
    /// # Panics
    ///
    /// If `pricing` is not valid: settings are checked before they are used.
    #[must_use]
    pub fn build(
        rates: &Rates,
        inventories: &[(LedgerId, u64)],
        pricing: Pricing,
        now_ms: u64,
        max_rate_age_ms: u64,
    ) -> Self {
        assert!(pricing.is_valid(), "pricing is checked before it is used");
        let mut table = Self::without_rates(pricing);
        table.built_at_ms = now_ms;
        let mut values = [0u128; MAX_PRICED_LEDGERS];
        let mut count = 0;
        for &(ledger, available) in inventories {
            if count == MAX_PRICED_LEDGERS || table.ledger(ledger).is_some() {
                continue;
            }
            let code = ledger.asset().code;
            let (Some(rate), Some(as_of_ms)) = (rates.get(code), rates.as_of(code)) else {
                continue;
            };
            if as_of_ms > now_ms {
                table.valid_until_ms = table.valid_until_ms.min(as_of_ms);
                continue;
            }
            if now_ms - as_of_ms > max_rate_age_ms {
                continue;
            }
            let stale_at_ms = as_of_ms.saturating_add(max_rate_age_ms).saturating_add(1);
            table.valid_until_ms = table.valid_until_ms.min(stale_at_ms);
            let Some(value) = value(available, rate, ledger.asset().scale) else {
                continue;
            };
            if let (Some(slot), Some(slot_value)) =
                (table.ledgers.get_mut(count), values.get_mut(count))
            {
                *slot = Some(LedgerPrice {
                    ledger,
                    rate,
                    as_of_ms,
                    available,
                });
                *slot_value = value;
                count += 1;
            }
        }
        table.price_pairs(&shares(&values));
        table
    }

    /// Fills the prices of every ordered pair of the table's ledgers, from
    /// their shares of the owner's funds.
    fn price_pairs(&mut self, shares: &[u128; MAX_PRICED_LEDGERS]) {
        let ledgers = self.ledgers;
        let count = ledgers.iter().flatten().count();
        let mut slots = self.prices.iter_mut();
        for (from, &share_in) in ledgers.iter().flatten().zip(shares) {
            for (to, &share_out) in ledgers.iter().flatten().zip(shares) {
                if from.ledger == to.ledger {
                    continue;
                }
                let multiplier = factor(share_in, share_out, count, self.pricing).map_or(
                    Some(0),
                    |(num, den)| {
                        let spread = MILLION - u128::from(self.pricing.charges.spread_ppm);
                        mul_div(num, spread * 10u128.pow(12), den)
                    },
                );
                // A rate that does not fit leaves the pair out.
                let Some(rate_e15) =
                    multiplier.and_then(|multiplier| spot_times(from, to, multiplier))
                else {
                    continue;
                };
                // There is a slot for every ordered pair.
                let Some(slot) = slots.next() else {
                    return;
                };
                *slot = Some(Price {
                    from: from.ledger,
                    to: to.ledger,
                    rate_e15,
                    as_of_ms: from.as_of_ms.min(to.as_of_ms),
                });
            }
        }
    }

    /// What the owner set for transit.
    #[must_use]
    pub const fn pricing(&self) -> Pricing {
        self.pricing
    }

    /// The charges on transit.
    #[must_use]
    pub const fn charges(&self) -> Charges {
        self.pricing.charges
    }

    /// When the table was built.
    #[must_use]
    pub const fn built_at_ms(&self) -> u64 {
        self.built_at_ms
    }

    /// The earliest time a rate the table uses ages out, or one it left
    /// out comes of age: build it again by then.
    #[must_use]
    pub const fn valid_until_ms(&self) -> u64 {
        self.valid_until_ms
    }

    /// The ledgers it prices.
    pub fn ledgers(&self) -> impl Iterator<Item = &LedgerPrice> + '_ {
        self.ledgers.iter().flatten()
    }

    /// Every price, by ledger pair.
    pub fn prices(&self) -> impl Iterator<Item = &Price> + '_ {
        self.prices.iter().flatten()
    }

    /// The ledger `ledger`, if it is priced.
    #[must_use]
    pub fn ledger(&self, ledger: LedgerId) -> Option<&LedgerPrice> {
        self.ledgers().find(|entry| entry.ledger == ledger)
    }

    /// The price from `from` to `to`, if both are priced.
    #[must_use]
    pub fn price(&self, from: LedgerId, to: LedgerId) -> Option<&Price> {
        self.prices()
            .find(|price| price.from == from && price.to == to)
    }

    /// What `net`, the incoming amount less the fee, converts to on
    /// transit from `from` to `to`, at `now_ms`: exactly
    /// [`Price::convert`] at the table's price.
    ///
    /// # Errors
    ///
    /// No price, or one whose rates are older than `max_rate_age_ms`; no
    /// funds on `to`; or a result that does not fit.
    pub fn convert(
        &self,
        from: LedgerId,
        to: LedgerId,
        net: u64,
        now_ms: u64,
        max_rate_age_ms: u64,
    ) -> Result<u64, PriceError> {
        let price = self
            .price(from, to)
            .filter(|price| fresh(price.as_of_ms, now_ms, max_rate_age_ms))
            .ok_or(PriceError::Stale)?;
        if price.rate_e15 == 0 {
            return Err(PriceError::NoFunds);
        }
        price.convert(net).ok_or(PriceError::TooLarge)
    }

    /// What `amount` on `from` is worth on `to` at spot, uncharged: what a
    /// peer's packet into the owner's own endpoints converts at. The same
    /// currency converts one to one without a rate.
    ///
    /// # Errors
    ///
    /// A rate it needs is missing or older than `max_rate_age_ms`, or the
    /// result does not fit.
    pub fn convert_at_spot(
        &self,
        from: LedgerId,
        to: LedgerId,
        amount: u64,
        now_ms: u64,
        max_rate_age_ms: u64,
    ) -> Result<u64, PriceError> {
        let (from_asset, to_asset) = (from.asset(), to.asset());
        let (from_rate, to_rate) = if from_asset.code == to_asset.code {
            (1, 1)
        } else {
            let rate = |ledger| {
                self.ledger(ledger)
                    .filter(|entry| fresh(entry.as_of_ms, now_ms, max_rate_age_ms))
                    .map(|entry| entry.rate)
                    .ok_or(PriceError::Stale)
            };
            (rate(from)?, rate(to)?)
        };
        crate::fx::convert_at(from_rate, to_rate, from_asset, to_asset, amount)
            .ok_or(PriceError::TooLarge)
    }
}

/// Whether a rate learned at `as_of_ms` is at most `max_age_ms` old at
/// `now_ms`, and not from after it.
const fn fresh(as_of_ms: u64, now_ms: u64, max_age_ms: u64) -> bool {
    as_of_ms <= now_ms && now_ms - as_of_ms <= max_age_ms
}

/// What `available` units of an asset at `scale` are worth in the common
/// base, at scale 9 and the base's rate of 10^[`RATE_PRECISION`], at most
/// [`MAX_VALUE`]. `None` for a rate too large for its scale.
fn value(available: u64, rate: u128, scale: u8) -> Option<u128> {
    let base = 10u128.pow(RATE_PRECISION);
    let denominator = rate.checked_mul(10u128.checked_pow(u32::from(scale))?)?;
    let numerator = u128::from(available) * base;
    let value = mul_div(numerator, 10u128.pow(9), denominator).unwrap_or(MAX_VALUE);
    Some(value.min(MAX_VALUE))
}

/// Each value's share of their sum, in [`SHARE_ONE`]s, rounded down. A
/// share only grows with its value, and only shrinks with another's.
fn shares(values: &[u128; MAX_PRICED_LEDGERS]) -> [u128; MAX_PRICED_LEDGERS] {
    let total: u128 = values.iter().sum();
    let mut shares = [0u128; MAX_PRICED_LEDGERS];
    for (share, &value) in shares.iter_mut().zip(values) {
        *share = mul_div(value, SHARE_ONE, total).unwrap_or(0);
    }
    shares
}

/// The curve's `w_in / w_out`, as a fraction, capped at `1 + k`; `None`
/// when the owner holds nothing on the way out. Of `count` ledgers, each
/// with a target of `1/count`, these hold `share_in` and `share_out`.
fn factor(share_in: u128, share_out: u128, count: usize, pricing: Pricing) -> Option<(u128, u128)> {
    if share_out == 0 {
        return None;
    }
    let k = u128::from(pricing.rebalance_ppm);
    let cap = (MILLION + k, MILLION);
    if share_in == 0 {
        return Some(cap);
    }
    let count = u128::try_from(count).unwrap_or(u128::MAX);
    assert!(share_in <= SHARE_ONE && share_out <= SHARE_ONE && count <= 16);
    // `10^6 · n · s · w = (10^6 − k) · n · s + k`, for a ledger of share `s`,
    // in shares.
    let weight = |share: u128| (MILLION - k) * count * share + k * SHARE_ONE;
    let numerator = weight(share_in) * share_out;
    let denominator = share_in * weight(share_out);
    if numerator * MILLION > denominator * (MILLION + k) {
        Some(cap)
    } else {
        Some((numerator, denominator))
    }
}

/// Spot from `from` to `to`, in units out per unit in, times
/// `multiplier / 10^18`, times 10^15, rounded down: a price's `rate_e15`,
/// if it fits.
fn spot_times(from: &LedgerPrice, to: &LedgerPrice, multiplier: u128) -> Option<u64> {
    let (scale_in, scale_out) = (from.ledger.asset().scale, to.ledger.asset().scale);
    // A unit in is worth `rate_out / rate_in · 10^(scale_out − scale_in)`
    // units out; 10^18 for the multiplier over 10^15 for the rate leaves
    // 10^3 below.
    let (numerator, denominator) = if scale_out >= scale_in {
        let up = 10u128.checked_pow(u32::from(scale_out - scale_in))?;
        (to.rate.checked_mul(up)?, from.rate.checked_mul(1_000)?)
    } else {
        let down = 10u128.checked_pow(u32::from(scale_in - scale_out) + 3)?;
        (to.rate, from.rate.checked_mul(down)?)
    };
    u64::try_from(mul_div(numerator, multiplier, denominator)?).ok()
}

/// The low 64 bits.
const LOW: u128 = (1 << 64) - 1;

/// `floor(a × b / c)`, exact through a 256-bit product; `None` when `c` is
/// zero or the quotient does not fit 128 bits.
#[must_use]
pub(crate) fn mul_div(a: u128, b: u128, c: u128) -> Option<u128> {
    if c == 0 {
        return None;
    }
    if let Some(product) = a.checked_mul(b) {
        return Some(product / c);
    }
    let (high, low) = wide_mul(a, b);
    if high >= c {
        return None;
    }
    // Long division a bit at a time; the remainder stays below `c`, and a
    // bit shifted out of it means it passed `c`.
    let mut remainder = high;
    let mut quotient = 0u128;
    for bit in (0..u128::BITS).rev() {
        let carry = remainder >> 127;
        remainder = (remainder << 1) | ((low >> bit) & 1);
        quotient <<= 1;
        if carry == 1 || remainder >= c {
            remainder = remainder.wrapping_sub(c);
            quotient |= 1;
        }
    }
    Some(quotient)
}

/// `a × b` as its high and low 128 bits.
const fn wide_mul(a: u128, b: u128) -> (u128, u128) {
    let (a_high, a_low) = (a >> 64, a & LOW);
    let (b_high, b_low) = (b >> 64, b & LOW);
    let low_low = a_low * b_low;
    let low_high = a_low * b_high;
    let high_low = a_high * b_low;
    let high_high = a_high * b_high;
    // At most three times 2^64: no overflow.
    let middle = (low_low >> 64) + (low_high & LOW) + (high_low & LOW);
    let low = (low_low & LOW) | ((middle & LOW) << 64);
    let high = high_high + (low_high >> 64) + (high_low >> 64) + (middle >> 64);
    (high, low)
}

#[cfg(test)]
mod tests {
    use super::*;

    const USD: LedgerId = LedgerId::StubUsd;
    const XRP: LedgerId = LedgerId::XrplTestnetXrp;
    const RLUSD: LedgerId = LedgerId::XrplTestnetRlusdUsd;
    const DOLLAR: u64 = 1_000_000_000;
    const NOW: u64 = 1_000_000;
    const AGE: u64 = 3_600_000;

    fn rates() -> Rates {
        let mut rates = Rates::new(NOW);
        rates.set("USD", 10u128.pow(RATE_PRECISION));
        rates.set("XRP", 2 * 10u128.pow(RATE_PRECISION));
        rates
    }

    fn pricing(k: u32, spread_ppm: u32) -> Pricing {
        Pricing {
            charges: Charges {
                fee_ppm: 0,
                spread_ppm,
            },
            rebalance_ppm: k,
        }
    }

    fn rate(table: &PriceTable, from: LedgerId, to: LedgerId) -> u64 {
        table.price(from, to).unwrap().rate_e15
    }

    #[test]
    fn at_target_the_rate_is_spot_less_the_spread() {
        // 100 dollars, and 200 XRP worth 100 dollars.
        let inventories = [(USD, 100 * DOLLAR), (XRP, 200 * DOLLAR)];
        let table = PriceTable::build(&rates(), &inventories, pricing(50_000, 0), NOW, AGE);
        assert_eq!(rate(&table, USD, XRP), 2 * RATE_ONE_E15);
        assert_eq!(rate(&table, XRP, USD), RATE_ONE_E15 / 2);
        let spread = PriceTable::build(&rates(), &inventories, pricing(50_000, 5_000), NOW, AGE);
        assert_eq!(rate(&spread, USD, XRP), 1_990_000_000_000_000);
        assert_eq!(table.ledgers().count(), 2);
        assert_eq!(table.prices().count(), 2);
    }

    // The plan's worked examples, k = 5% and two ledgers: 25/75 costs about
    // 6%, 5/95 about 33% on the way out of the scarce ledger.
    #[test]
    fn a_skew_moves_the_rate_by_the_curve() {
        let skewed = |usd: u64, xrp_dollars: u64| {
            let inventories = [(USD, usd * DOLLAR), (XRP, 2 * xrp_dollars * DOLLAR)];
            PriceTable::build(&rates(), &inventories, pricing(50_000, 0), NOW, AGE)
        };
        let quarter = skewed(25, 75);
        // w_USD = 0.95 + 0.05 · 2 = 1.05, w_XRP = 0.95 + 0.05 · 2/3: XRP to
        // dollars gives 59/63 of spot, 6.3% less.
        assert_eq!(rate(&quarter, XRP, USD), 468_253_968_253_968);
        // Dollars in get the capped bonus.
        assert_eq!(rate(&quarter, USD, XRP), 2_100_000_000_000_000);
        let twentieth = skewed(5, 95);
        let out = u128::from(rate(&twentieth, XRP, USD));
        // w_USD = 1.45, w_XRP ≈ 0.9763: about 67.3% of spot.
        assert!(out * 1_000 / u128::from(RATE_ONE_E15 / 2) == 673, "{out}");
    }

    #[test]
    fn nothing_converts_out_of_a_ledger_with_nothing() {
        let inventories = [(USD, 0), (XRP, 2 * DOLLAR)];
        let table = PriceTable::build(&rates(), &inventories, pricing(50_000, 0), NOW, AGE);
        assert_eq!(rate(&table, XRP, USD), 0);
        assert_eq!(
            table.convert(XRP, USD, 1_000, NOW, AGE),
            Err(PriceError::NoFunds)
        );
        assert_eq!(rate(&table, USD, XRP), 2_100_000_000_000_000);
        // With nothing anywhere, nothing converts either way.
        let empty = PriceTable::build(&rates(), &[(USD, 0), (XRP, 0)], pricing(0, 0), NOW, AGE);
        assert_eq!(rate(&empty, USD, XRP), 0);
        assert_eq!(rate(&empty, XRP, USD), 0);
    }

    #[test]
    fn without_k_the_rate_is_spot() {
        let inventories = [(USD, DOLLAR), (XRP, 1_000 * DOLLAR)];
        let table = PriceTable::build(&rates(), &inventories, pricing(0, 0), NOW, AGE);
        assert_eq!(rate(&table, USD, XRP), 2 * RATE_ONE_E15);
        assert_eq!(rate(&table, XRP, USD), RATE_ONE_E15 / 2);
    }

    #[test]
    fn stale_and_missing_rates_leave_their_ledger_out() {
        let mut rates = rates();
        let inventories = [(USD, DOLLAR), (XRP, DOLLAR), (RLUSD, DOLLAR)];
        let table = PriceTable::build(&rates, &inventories, pricing(50_000, 0), NOW + AGE, AGE);
        assert_eq!(table.ledgers().count(), 3);
        assert_eq!(table.prices().count(), 6);
        assert_eq!(table.valid_until_ms(), NOW + AGE + 1);
        let late = PriceTable::build(&rates, &inventories, pricing(50_000, 0), NOW + AGE + 1, AGE);
        assert_eq!(late.prices().count(), 0);
        // A rate from the future is left out until its time.
        rates.set_at("XRP", 2 * 10u128.pow(RATE_PRECISION), NOW + 10);
        let early = PriceTable::build(&rates, &inventories, pricing(50_000, 0), NOW, AGE);
        assert!(early.ledger(XRP).is_none());
        assert_eq!(early.valid_until_ms(), NOW + 10);
        // A packet checks the age at its own time.
        assert_eq!(
            table.convert(USD, XRP, 1, NOW + AGE + 1, AGE),
            Err(PriceError::Stale)
        );
        // Dollars on two ledgers held alike convert one to one.
        assert_eq!(rate(&table, USD, RLUSD), RATE_ONE_E15);
        let only = PriceTable::build(&Rates::new(0), &inventories, pricing(0, 0), NOW, AGE);
        assert!(only.price(USD, RLUSD).is_none());
    }

    #[test]
    fn spot_conversion_is_exact_and_uncharged() {
        let inventories = [(USD, 0), (XRP, 0)];
        let table = PriceTable::build(&rates(), &inventories, pricing(50_000, 5_000), NOW, AGE);
        assert_eq!(table.convert_at_spot(USD, XRP, 1_000, NOW, AGE), Ok(2_000));
        assert_eq!(table.convert_at_spot(XRP, USD, 3, NOW, AGE), Ok(1));
        assert_eq!(
            table.convert_at_spot(USD, XRP, 1, NOW + AGE + 1, AGE),
            Err(PriceError::Stale)
        );
        // The same currency needs no rate.
        let none = PriceTable::without_rates(Pricing::NONE);
        assert_eq!(none.convert_at_spot(USD, RLUSD, 7, NOW, AGE), Ok(7));
    }

    proptest::proptest! {
        // Against arbitrary precision: exact, rounded down, and `None` only
        // when the quotient does not fit.
        #[test]
        fn mul_div_matches_arbitrary_precision(a: u128, b: u128, c in 1u128..) {
            use num_bigint::BigUint;
            let exact = BigUint::from(a) * BigUint::from(b) / BigUint::from(c);
            let fits = u128::try_from(&exact).ok();
            proptest::prop_assert_eq!(mul_div(a, b, c), fits);
        }
    }

    #[test]
    fn mul_div_is_exact() {
        assert_eq!(mul_div(6, 7, 4), Some(10));
        assert_eq!(mul_div(u128::MAX, u128::MAX, u128::MAX), Some(u128::MAX));
        assert_eq!(mul_div(u128::MAX, 2, 3), Some(u128::MAX / 3 * 2));
        assert_eq!(mul_div(u128::MAX, 3, 2), None);
        assert_eq!(mul_div(u128::MAX, 4, 6), Some(u128::MAX / 3 * 2));
        assert_eq!(mul_div(u128::MAX, 2, 1), None);
        assert_eq!(mul_div(1, 1, 0), None);
        assert_eq!(wide_mul(u128::MAX, u128::MAX), (u128::MAX - 1, 1));
    }

    /// The curve's `w_in / w_out` for shares from `values`, capped.
    fn exact_factor(values: &[u128], from: usize, to: usize, k: u32) -> Option<(u128, u128)> {
        let mut all = [0; MAX_PRICED_LEDGERS];
        all[..values.len()].copy_from_slice(values);
        let shares = shares(&all);
        factor(shares[from], shares[to], values.len(), pricing(k, 0))
    }

    #[test]
    fn the_factor_is_one_at_target_and_capped() {
        let (num, den) = exact_factor(&[5, 5], 0, 1, 50_000).unwrap();
        assert_eq!(num, den);
        let (num, den) = exact_factor(&[3, 3, 3], 2, 0, 100_000).unwrap();
        assert_eq!(num, den);
        assert_eq!(
            exact_factor(&[0, 5], 0, 1, 50_000),
            Some((1_050_000, 1_000_000))
        );
        assert_eq!(
            exact_factor(&[1, 99], 0, 1, 50_000),
            Some((1_050_000, 1_000_000))
        );
        assert_eq!(exact_factor(&[1, 0], 0, 1, 50_000), None);
    }
}
