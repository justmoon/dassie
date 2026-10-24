//! A dollar token's peg guard (spec/areas/xrpl.md#the-peg-guard,
//! spec/decisions/issued-assets.md), sans-IO: it pauses the token's scheme
//! while the token's AMM pool of XRP prices it off a dollar.
//!
//! A reading is the pool's two balances at one validated ledger, which the
//! account reads for the scheme ([`crate::XrplAccount::read_pool`]), and
//! the node's own XRP/USD rate. The token's price is their ratio times the
//! rate, exact, in millionths of a dollar. A reading counts only if the pool
//! holds at least its token's floor of XRP and the rate is fresh.
//!
//! - **Pegged,** the scheme runs. Once every reading that counted has been
//!   outside [`OUTER_BAND_PPM`] for [`TRIP_HOLD_MS`], it is off its peg.
//!   Once no reading has counted for [`MAX_READING_AGE_MS`], it is unread.
//! - **Off its peg,** the scheme is paused. Once every reading that counted
//!   has been inside [`INNER_BAND_PPM`] for [`RECOVER_HOLD_MS`], with none
//!   missing for longer than [`MAX_READING_AGE_MS`], it is pegged again.
//! - **Unread,** the scheme is paused too: the guard fails closed. The
//!   first reading that counts inside the outer band makes it pegged again,
//!   as it was before; once every reading has been outside it for the trip
//!   hold, it is off its peg.
//!
//! Off upward or downward is the same pause. A guard new to a scheme starts
//! pegged, as if read in the band when it started: a de-peg already under
//! way trips it as one that starts then would, and without a reading it
//! pauses as any guard does.

use dassie_settlement::{DollarRate, PegPause, PegView};
use serde_json::{Value, json};

use crate::amount::TokenValue;
use crate::keys::AccountId;

/// A dollar, in millionths: the peg.
pub const PAR_PPM: u64 = 1_000_000;
/// How far off a dollar a reading may be before it counts toward tripping
/// the guard: 3%. Over 56 hours the mainnet RLUSD/XRP pool, against the
/// median of three exchanges' XRP/USD, never moved more than 1% off a dollar,
/// and its price moved at most 1.65% in an hour; the node's rate lags the
/// pool by up to a minute. Dollar tokens that lost their peg moved several
/// percent to tens of percent.
pub const OUTER_BAND_PPM: u64 = 30_000;
/// How close to a dollar every reading must be for the guard to recover:
/// 1%, where the mainnet pool spent all of the 56 hours measured. Narrower
/// than [`OUTER_BAND_PPM`], so a price that hovers at the band's edge does
/// not pause and resume the scheme in turn.
pub const INNER_BAND_PPM: u64 = 10_000;
/// How long every reading must be outside the band before the guard trips:
/// ten minutes. The testnet DSUSD pool's excursions, up to 9.8%, were
/// corrected within two to five minutes by its keeper or a trade, and
/// holding a deep pool 3% off for ten minutes means selling into it over
/// and over as arbitrageurs take the price back.
pub const TRIP_HOLD_MS: u64 = 10 * 60_000;
/// How long every reading must be back inside the inner band before the
/// guard recovers: an hour, longer than it takes to trip, since de-pegs
/// last hours to days and a token that just came back has to stay back.
pub const RECOVER_HOLD_MS: u64 = 60 * 60_000;
/// How long the guard goes without a reading that counts before it pauses
/// the scheme: as long as a de-peg must last to trip it, so a de-peg that
/// hides behind readings that fail costs no more than one in plain view.
pub const MAX_READING_AGE_MS: u64 = TRIP_HOLD_MS;
/// The oldest the node's XRP/USD rate may be for a reading to count: ten
/// minutes. The node fetches rates every minute; the connector takes them
/// for an hour, but in an hour XRP moved the mainnet pool's price by up to
/// 1.65%, half the band.
pub const MAX_RATE_AGE_MS: u64 = 10 * 60_000;
/// How often the guard reads the pool: once a minute, as the node fetches
/// rates, so a hold sees ten readings or more before it ends.
pub const READ_EVERY_MS: u64 = 60_000;
/// The highest price the guard tells apart, in millionths of a dollar: a
/// million dollars. Any price above is as far off its peg.
pub const MAX_PRICE_PPM: u64 = 1_000_000_000_000;
/// The largest rate the guard takes, in units per dollar at its precision:
/// so the arithmetic of a price stays within 128 bits. At the node's
/// precision of 15, a million XRP to the dollar.
const MAX_RATE: u128 = 1_000_000_000_000_000_000_000;
/// The most decimals a rate it takes may have.
const MAX_RATE_PRECISION: u32 = 18;

/// What prices a dollar token: its pool of XRP and the token, and how much
/// XRP that pool must hold to be a price.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Peg {
    /// The least XRP the pool must hold for a reading to count, in drops:
    /// a pool thinner than that moves too cheaply to say what the token is
    /// worth.
    pub min_pool_drops: u64,
}

/// The pool of XRP and a token, as of one validated ledger: both balances
/// at that ledger, never at two, which could pair an old XRP balance with a
/// newer token balance, a ratio the pool never had.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PoolReading {
    /// The pool's account, which holds both.
    pub account: AccountId,
    /// Its XRP, in drops.
    pub drops: u64,
    /// Its token.
    pub value: TokenValue,
    /// The validated ledger both are as of.
    pub ledger_index: u32,
}

/// What became of a reading.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Reading {
    /// It counted, at this price, in millionths of a dollar.
    Counted(u64),
    /// There is no pool, or it holds none of the token.
    NoPool,
    /// The pool holds less XRP than its floor.
    TooThin,
    /// The node has no XRP rate, or one older than [`MAX_RATE_AGE_MS`], or
    /// one past the arithmetic's bounds.
    NoRate,
}

/// Where the guard stands.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Standing {
    /// On its peg: the scheme runs.
    Pegged,
    /// No reading counted for too long while pegged: paused.
    Unread,
    /// Off its peg: paused.
    Off,
}

impl Standing {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Pegged => "pegged",
            Self::Unread => "unread",
            Self::Off => "off",
        }
    }

    fn parse(text: &str) -> Option<Self> {
        [Self::Pegged, Self::Unread, Self::Off]
            .into_iter()
            .find(|standing| standing.as_str() == text)
    }
}

/// The guard of one dollar token's scheme.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Guard {
    peg: Peg,
    // Kept with the scheme's state.
    standing: Standing,
    /// Since when every reading that counted has been on the other side:
    /// outside the outer band while pegged or unread, inside the inner band
    /// while off.
    crossing_ms: Option<u64>,
    /// When the last reading counted, or the guard started; `None` until it
    /// knows the time.
    read_ms: Option<u64>,
    /// The last price that counted, and its ledger.
    last: Option<(u64, u32)>,
    // Not kept.
    /// The node's XRP rate, as last told.
    rate: Option<DollarRate>,
    /// When the pool was last asked for.
    asked_ms: Option<u64>,
}

impl Guard {
    /// A guard new to a scheme: pegged, as if read in the band when it
    /// first learns the time.
    #[must_use]
    pub const fn new(peg: Peg) -> Self {
        Self {
            peg,
            standing: Standing::Pegged,
            crossing_ms: None,
            read_ms: None,
            last: None,
            rate: None,
            asked_ms: None,
        }
    }

    /// Takes the time. The first time is when the guard started. With no
    /// reading that counted for [`MAX_READING_AGE_MS`], a pegged guard is
    /// unread, and one off its peg starts its recovery hold again.
    pub fn tick(&mut self, now_ms: u64) {
        let read_ms = *self.read_ms.get_or_insert(now_ms);
        if now_ms.saturating_sub(read_ms) <= MAX_READING_AGE_MS {
            return;
        }
        match self.standing {
            Standing::Pegged => {
                self.standing = Standing::Unread;
                self.crossing_ms = None;
            }
            Standing::Off => self.crossing_ms = None,
            Standing::Unread => {}
        }
    }

    /// Takes the node's XRP rate, as it has it now.
    pub const fn rate(&mut self, rate: Option<DollarRate>) {
        self.rate = rate;
    }

    /// Whether the pool is due to be read at `now_ms`: never read yet, or
    /// last asked for [`READ_EVERY_MS`] ago. Once due, it counts as asked.
    pub fn ask(&mut self, now_ms: u64) -> bool {
        let due = self
            .asked_ms
            .is_none_or(|at| now_ms < at || now_ms.saturating_sub(at) >= READ_EVERY_MS);
        if due {
            self.asked_ms = Some(now_ms);
        }
        due
    }

    /// Forgets when the pool was last asked for, so the next check asks
    /// again: after a reconnect, when the answer may never come.
    pub const fn ask_again(&mut self) {
        self.asked_ms = None;
    }

    /// Takes a reading of the pool at `now_ms`, `None` if there is none,
    /// with the rate as last told.
    pub fn reading(&mut self, now_ms: u64, pool: Option<&PoolReading>) -> Reading {
        self.tick(now_ms);
        let Some(pool) = pool.filter(|pool| !pool.value.is_zero() && !pool.value.is_negative())
        else {
            return Reading::NoPool;
        };
        if pool.drops < self.peg.min_pool_drops {
            return Reading::TooThin;
        }
        let Some(rate) = self
            .rate
            .filter(|rate| rate.as_of_ms <= now_ms && now_ms - rate.as_of_ms <= MAX_RATE_AGE_MS)
        else {
            return Reading::NoRate;
        };
        let Some(price_ppm) = price_ppm(pool.drops, pool.value, rate) else {
            return Reading::NoRate;
        };
        self.counted(now_ms, price_ppm, pool.ledger_index);
        Reading::Counted(price_ppm)
    }

    /// A reading that counted, at `price_ppm`.
    fn counted(&mut self, now_ms: u64, price_ppm: u64, ledger_index: u32) {
        let off = price_ppm.abs_diff(PAR_PPM);
        self.read_ms = Some(now_ms);
        self.last = Some((price_ppm, ledger_index));
        let (crossing, hold, across) = match self.standing {
            Standing::Pegged | Standing::Unread => {
                (off > OUTER_BAND_PPM, TRIP_HOLD_MS, Standing::Off)
            }
            Standing::Off => (off <= INNER_BAND_PPM, RECOVER_HOLD_MS, Standing::Pegged),
        };
        if !crossing {
            // Unread, and back in the band: pegged, as it was.
            if self.standing == Standing::Unread {
                self.standing = Standing::Pegged;
            }
            self.crossing_ms = None;
            return;
        }
        let since = *self.crossing_ms.get_or_insert(now_ms);
        if now_ms.saturating_sub(since) >= hold {
            self.standing = across;
            self.crossing_ms = None;
        }
    }

    /// Why the scheme is paused now, if it is: off its peg also while
    /// unread with every reading since off.
    #[must_use]
    pub const fn paused(&self) -> Option<PegPause> {
        match (self.standing, self.crossing_ms) {
            (Standing::Pegged, _) => None,
            (Standing::Unread, None) => Some(PegPause::Stale),
            (Standing::Unread, Some(_)) | (Standing::Off, _) => Some(PegPause::OffPeg),
        }
    }

    /// What it says, for the owner.
    #[must_use]
    pub const fn view(&self) -> PegView {
        PegView {
            paused: self.paused(),
            last: self.last,
        }
    }

    /// A number that changes with what it keeps.
    #[must_use]
    pub fn fingerprint(&self) -> u64 {
        let (price, ledger) = self.last.unwrap_or((u64::MAX, u32::MAX));
        [
            self.standing as u64,
            self.crossing_ms.unwrap_or(u64::MAX),
            self.read_ms.unwrap_or(u64::MAX),
            price,
            u64::from(ledger),
        ]
        .iter()
        .fold(0xcbf2_9ce4_8422_2325_u64, |mix, value| {
            (mix ^ value).wrapping_mul(0x0100_0000_01b3)
        })
    }

    /// What it keeps, with the scheme's state.
    #[must_use]
    pub fn state(&self) -> Value {
        json!({
            "standing": self.standing.as_str(),
            "crossing_ms": self.crossing_ms,
            "read_ms": self.read_ms,
            "last": self.last.map(|(price, ledger)| json!([price, ledger])),
        })
    }

    /// Takes back what it kept, `None` for a state it never wrote.
    #[must_use]
    pub fn restore(peg: Peg, state: &Value) -> Option<Self> {
        let time = |key: &str| match state.get(key)? {
            Value::Null => Some(None),
            value => value.as_u64().map(Some),
        };
        let last = match state.get("last")? {
            Value::Null => None,
            last => Some((
                last.get(0)?.as_u64()?,
                u32::try_from(last.get(1)?.as_u64()?).ok()?,
            )),
        };
        let guard = Self {
            standing: Standing::parse(state.get("standing")?.as_str()?)?,
            crossing_ms: time("crossing_ms")?,
            read_ms: time("read_ms")?,
            last,
            ..Self::new(peg)
        };
        guard.holds().then_some(guard)
    }

    /// Whether what it keeps fits together: a reading counted, or a hold
    /// started, only once it knew the time, and a price within bounds.
    fn holds(&self) -> bool {
        (self.read_ms.is_some() || (self.crossing_ms.is_none() && self.last.is_none()))
            && self.last.is_none_or(|(price, _)| price <= MAX_PRICE_PPM)
    }

    /// Asserts what holds between calls (spec/decisions/tigerstyle.md).
    ///
    /// # Panics
    ///
    /// If its state is inconsistent.
    pub fn check_invariants(&self) {
        assert!(self.holds(), "the guard's state fits together");
    }
}

/// The token's price in millionths of a dollar: what `value` of it is
/// worth in `drops` of XRP, at `rate`, XRP per dollar. Exact, rounded down,
/// and at most [`MAX_PRICE_PPM`]; `None` for a rate past the arithmetic's
/// bounds.
///
/// A token is worth `drops / 10^6 / value` XRP, and an XRP `10^p / rate`
/// dollars, so a million times its price is `drops · 10^(p − e) / (m ·
/// rate)` for a value of `m · 10^e`: a long division, a digit at a time.
#[must_use]
pub fn price_ppm(drops: u64, value: TokenValue, rate: DollarRate) -> Option<u64> {
    if rate.per_dollar == 0 || rate.per_dollar > MAX_RATE || rate.precision > MAX_RATE_PRECISION {
        return None;
    }
    if value.is_zero() || value.is_negative() {
        return Some(MAX_PRICE_PPM);
    }
    // At most 10^16 · 10^21: within 128 bits, and ten times it too.
    let mut divisor = u128::from(value.mantissa()) * rate.per_dollar;
    let digits = i64::from(rate.precision) - i64::from(value.exponent());
    if digits < 0 {
        // Worth less than a millionth once the divisor no longer fits.
        let scale = 10_u128.checked_pow(u32::try_from(-digits).ok()?);
        match scale.and_then(|scale| divisor.checked_mul(scale)) {
            Some(scaled) => divisor = scaled,
            None => return Some(0),
        }
    }
    let dividend = u128::from(drops);
    let mut quotient = dividend / divisor;
    let mut remainder = dividend % divisor;
    for _ in 0..digits.max(0) {
        if quotient > u128::from(MAX_PRICE_PPM) {
            break;
        }
        remainder *= 10;
        quotient = quotient * 10 + remainder / divisor;
        remainder %= divisor;
    }
    Some(u64::try_from(quotient.min(u128::from(MAX_PRICE_PPM))).unwrap_or(MAX_PRICE_PPM))
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    fn value(text: &str) -> TokenValue {
        TokenValue::parse(text).unwrap()
    }

    const PEG: Peg = Peg {
        min_pool_drops: 100_000_000,
    };

    /// 2 XRP to the dollar, as of `as_of_ms`.
    const fn two_per_dollar(as_of_ms: u64) -> DollarRate {
        DollarRate {
            per_dollar: 2_000_000_000_000_000,
            precision: 15,
            as_of_ms,
        }
    }

    /// A pool of 1,000 XRP priced at `ppm` millionths of a dollar a token,
    /// at 2 XRP to the dollar: 500 · 10^6 / ppm tokens.
    fn pool(ppm: u64) -> PoolReading {
        let micro_tokens = 500_000_000_000_000 / ppm;
        PoolReading {
            account: AccountId([7; 20]),
            drops: 1_000_000_000,
            value: TokenValue::from_units(micro_tokens, 6).unwrap(),
            ledger_index: 10,
        }
    }

    /// A guard told the rate at `now_ms`, given a reading at `ppm`.
    fn read(guard: &mut Guard, now_ms: u64, ppm: u64) -> Reading {
        guard.rate(Some(two_per_dollar(now_ms)));
        let reading = guard.reading(now_ms, Some(&pool(ppm)));
        guard.check_invariants();
        reading
    }

    /// Readings at `ppm` every minute from `from_ms` to `to_ms`.
    fn hold(guard: &mut Guard, from_ms: u64, to_ms: u64, ppm: u64) {
        let mut at = from_ms;
        while at <= to_ms {
            assert!(matches!(read(guard, at, ppm), Reading::Counted(_)));
            at += READ_EVERY_MS;
        }
    }

    #[test]
    fn prices_are_exact() {
        let rate = two_per_dollar(0);
        // 2 XRP for one token, at 2 XRP to the dollar: a dollar.
        assert_eq!(price_ppm(2_000_000, value("1"), rate), Some(PAR_PPM));
        assert_eq!(price_ppm(1_000_000, value("1"), rate), Some(500_000));
        // Rounded down.
        assert_eq!(price_ppm(2_000_000, value("3"), rate), Some(333_333));
        // The mainnet pool as measured, 1,857,021.585932 XRP for
        // 2,396,529.5597 RLUSD, with XRP at $1.2905.
        let mainnet = DollarRate {
            per_dollar: 774_893_452_150_329,
            precision: 15,
            as_of_ms: 0,
        };
        assert_eq!(
            price_ppm(1_857_021_585_932, value("2396529.5597"), mainnet),
            Some(999_981)
        );
        // Far apart in size: saturated, or nothing, never an overflow.
        assert_eq!(
            price_ppm(crate::MAX_DROPS, value("1e-80"), rate),
            Some(MAX_PRICE_PPM)
        );
        assert_eq!(price_ppm(1, value("9999999999999999e80"), rate), Some(0));
        assert_eq!(price_ppm(1, value("0"), rate), Some(MAX_PRICE_PPM));
        let too_large = DollarRate {
            per_dollar: MAX_RATE + 1,
            ..rate
        };
        assert_eq!(price_ppm(1, value("1"), too_large), None);
        assert_eq!(
            price_ppm(
                1,
                value("1"),
                DollarRate {
                    per_dollar: 0,
                    ..rate
                }
            ),
            None
        );
    }

    #[test]
    fn the_band_is_exact_at_its_edges() {
        for (ppm, trips) in [
            (PAR_PPM + OUTER_BAND_PPM, false),
            (PAR_PPM + OUTER_BAND_PPM + 1, true),
            (PAR_PPM - OUTER_BAND_PPM, false),
            (PAR_PPM - OUTER_BAND_PPM - 1, true),
        ] {
            let mut guard = Guard::new(PEG);
            hold(&mut guard, 0, TRIP_HOLD_MS, ppm);
            assert_eq!(guard.paused().is_some(), trips, "{ppm}");
        }
    }

    #[test]
    fn it_trips_only_once_every_reading_was_off_for_the_hold() {
        let mut guard = Guard::new(PEG);
        hold(&mut guard, 0, TRIP_HOLD_MS - READ_EVERY_MS, 900_000);
        assert_eq!(guard.paused(), None, "one reading, or a few, never trip it");
        // One reading back in the band starts the hold again.
        read(&mut guard, TRIP_HOLD_MS, PAR_PPM);
        hold(
            &mut guard,
            TRIP_HOLD_MS + READ_EVERY_MS,
            2 * TRIP_HOLD_MS,
            900_000,
        );
        assert_eq!(guard.paused(), None);
        read(&mut guard, 2 * TRIP_HOLD_MS + READ_EVERY_MS, 900_000);
        assert_eq!(guard.paused(), Some(PegPause::OffPeg));
        assert_eq!(guard.view().last, Some((900_000, 10)));
    }

    #[test]
    fn off_upward_is_off_too() {
        let mut guard = Guard::new(PEG);
        hold(&mut guard, 0, TRIP_HOLD_MS, 1_100_000);
        assert_eq!(guard.paused(), Some(PegPause::OffPeg));
    }

    #[test]
    fn it_recovers_only_inside_the_inner_band_for_longer() {
        let mut guard = Guard::new(PEG);
        hold(&mut guard, 0, TRIP_HOLD_MS, 900_000);
        assert_eq!(guard.paused(), Some(PegPause::OffPeg));
        // Inside the outer band, outside the inner: still off.
        let start = TRIP_HOLD_MS + READ_EVERY_MS;
        hold(&mut guard, start, start + 2 * RECOVER_HOLD_MS, 980_000);
        assert_eq!(guard.paused(), Some(PegPause::OffPeg));
        let start = start + 3 * RECOVER_HOLD_MS;
        hold(
            &mut guard,
            start,
            start + RECOVER_HOLD_MS - READ_EVERY_MS,
            995_000,
        );
        assert_eq!(guard.paused(), Some(PegPause::OffPeg));
        read(&mut guard, start + RECOVER_HOLD_MS, 995_000);
        assert_eq!(guard.paused(), None);
    }

    #[test]
    fn without_a_reading_that_counts_it_fails_closed() {
        let mut guard = Guard::new(PEG);
        guard.tick(1_000);
        guard.tick(1_000 + MAX_READING_AGE_MS);
        assert_eq!(guard.paused(), None);
        guard.tick(1_001 + MAX_READING_AGE_MS);
        assert_eq!(guard.paused(), Some(PegPause::Stale));
        // No pool, a thin one, no rate or a stale one: none count.
        let now = 2 * MAX_READING_AGE_MS;
        assert_eq!(guard.reading(now, None), Reading::NoPool);
        let thin = PoolReading {
            drops: PEG.min_pool_drops - 1,
            ..pool(PAR_PPM)
        };
        guard.rate(Some(two_per_dollar(now)));
        assert_eq!(guard.reading(now, Some(&thin)), Reading::TooThin);
        guard.rate(None);
        assert_eq!(guard.reading(now, Some(&pool(PAR_PPM))), Reading::NoRate);
        guard.rate(Some(two_per_dollar(now - MAX_RATE_AGE_MS - 1)));
        assert_eq!(guard.reading(now, Some(&pool(PAR_PPM))), Reading::NoRate);
        // A rate from the future, as a clock gone back makes it: none.
        guard.rate(Some(two_per_dollar(now + 1)));
        assert_eq!(guard.reading(now, Some(&pool(PAR_PPM))), Reading::NoRate);
        assert_eq!(guard.paused(), Some(PegPause::Stale));
        // The next reading that counts ends it, if in the band.
        read(&mut guard, now, 990_000);
        assert_eq!(guard.paused(), None);
    }

    #[test]
    fn unread_it_resumes_only_in_the_band() {
        let mut guard = Guard::new(PEG);
        guard.tick(0);
        let start = MAX_READING_AGE_MS + 1;
        guard.tick(start);
        assert_eq!(guard.paused(), Some(PegPause::Stale));
        // Read off the peg: still paused, and off once the hold is over.
        hold(
            &mut guard,
            start,
            start + TRIP_HOLD_MS - READ_EVERY_MS,
            900_000,
        );
        assert_eq!(guard.paused(), Some(PegPause::OffPeg));
        let mut tripped = guard.clone();
        read(&mut tripped, start + TRIP_HOLD_MS, 900_000);
        read(&mut tripped, start + TRIP_HOLD_MS + READ_EVERY_MS, PAR_PPM);
        assert_eq!(tripped.paused(), Some(PegPause::OffPeg));
        // Back in the outer band before then: pegged, as it was.
        read(&mut guard, start + TRIP_HOLD_MS, 980_000);
        assert_eq!(guard.paused(), None);
    }

    #[test]
    fn a_gap_in_the_readings_starts_the_recovery_again() {
        let mut guard = Guard::new(PEG);
        hold(&mut guard, 0, TRIP_HOLD_MS, 900_000);
        let start = TRIP_HOLD_MS + READ_EVERY_MS;
        let gap = start + RECOVER_HOLD_MS / 2;
        hold(&mut guard, start, gap, PAR_PPM);
        let back = gap + MAX_READING_AGE_MS + 1;
        guard.tick(back);
        hold(
            &mut guard,
            back,
            back + RECOVER_HOLD_MS - READ_EVERY_MS,
            PAR_PPM,
        );
        assert_eq!(guard.paused(), Some(PegPause::OffPeg));
        read(&mut guard, back + RECOVER_HOLD_MS, PAR_PPM);
        assert_eq!(guard.paused(), None);
    }

    #[test]
    fn stale_while_off_stays_off() {
        let mut guard = Guard::new(PEG);
        hold(&mut guard, 0, TRIP_HOLD_MS, 900_000);
        guard.tick(10 * MAX_READING_AGE_MS);
        assert_eq!(guard.paused(), Some(PegPause::OffPeg));
        read(&mut guard, 10 * MAX_READING_AGE_MS, PAR_PPM);
        assert_eq!(guard.paused(), Some(PegPause::OffPeg));
    }

    #[test]
    fn what_it_keeps_comes_back() {
        let mut guard = Guard::new(PEG);
        hold(&mut guard, 0, TRIP_HOLD_MS, 900_000);
        read(&mut guard, TRIP_HOLD_MS + READ_EVERY_MS, PAR_PPM);
        let restored = Guard::restore(PEG, &guard.state()).unwrap();
        assert_eq!(restored.fingerprint(), guard.fingerprint());
        assert_eq!(restored.paused(), Some(PegPause::OffPeg));
        // The recovery hold goes on from where it was.
        let mut restored = restored;
        let start = TRIP_HOLD_MS + READ_EVERY_MS;
        hold(
            &mut restored,
            start + READ_EVERY_MS,
            start + RECOVER_HOLD_MS,
            PAR_PPM,
        );
        assert_eq!(restored.paused(), None);
        // A state it never wrote is refused.
        let mut state = guard.state();
        state["read_ms"] = Value::Null;
        assert!(Guard::restore(PEG, &state).is_none());
        let mut state = guard.state();
        state["last"] = json!([MAX_PRICE_PPM + 1, 3]);
        assert!(Guard::restore(PEG, &state).is_none());
        assert!(Guard::restore(PEG, &json!({})).is_none());
    }

    #[test]
    fn it_reads_once_a_minute() {
        let mut guard = Guard::new(PEG);
        assert!(guard.ask(0));
        assert!(!guard.ask(READ_EVERY_MS - 1));
        assert!(guard.ask(READ_EVERY_MS));
        guard.ask_again();
        assert!(guard.ask(READ_EVERY_MS + 1));
        // A clock gone back asks at once.
        assert!(guard.ask(0));
    }
}
