//! XRP's rate from mainnet's XRP/RLUSD pool (spec/areas/xrpl.md#xrps-rate,
//! spec/decisions/open-rate-sources.md), sans-IO: how many XRP a dollar
//! buys, as the pool prices XRP in RLUSD, RLUSD taken at par.
//!
//! - **Proven, never signed for.** A [`Verifier`] of mainnet, with no key
//!   and no account, anchors on ledgers a quorum of its UNL validated, and
//!   proves the pool's `AMM` object, its account's root and its line to
//!   RLUSD's issuer, all in one state proof at the anchor
//!   ([`crate::pool`]). Its holder carries out what it asks
//!   ([`Out`]): requests on one connection to a proof server, and the
//!   validator list publishers' sites. Nothing here signs, so a node of
//!   either realm follows mainnet for this one price.
//! - **Read often.** Every [`Timings::read_every_ms`] it anchors anew and
//!   reads the pool at the anchor it holds, more often than the minute the
//!   other rate sources are asked.
//! - **What counts.** A reading counts while the pool holds at least
//!   [`MIN_POOL_DROPS`], while its ledger closed within
//!   [`MAX_ANCHOR_AGE_MS`] of when it was read, and only once its ledger is
//!   [`Timings::min_ledgers_apart`] past the last counted one's: so however
//!   late a server serves validated ledgers, each reading stands for that
//!   many ledgers of the pool's history, and a pool pushed for a few
//!   ledgers is a few readings at most.
//! - **A median over a window of ledger time.** The rate is the median of
//!   the readings whose ledgers closed within the last
//!   [`Timings::window_ms`], once at least [`Timings::min_readings`] did,
//!   more than half of what the window holds: so with an honest server a
//!   pool pushed for less than half the window leaves it within what the
//!   pool said while not pushed, and a window the server left mostly unread
//!   gives no rate. A server that withholds ledgers can thin the window to
//!   [`Timings::min_readings`], so that a push of their larger half moves
//!   the rate: one held for half that many readings' worth of ledgers
//!   spaced [`Timings::min_ledgers_apart`], about a minute on mainnet.
//!
//! Time is an input, every table has a bound, and nothing recurses
//! (spec/decisions/tigerstyle.md).

use alloc::boxed::Box;
use alloc::collections::{BTreeMap, VecDeque};
use alloc::string::String;
use alloc::vec::Vec;

use crate::DROPS_PER_XRP;
use crate::amount::{IssuedAsset, TokenValue};
use crate::api::{self, Message};
use crate::keys::AccountId;
use crate::pool::{self, PoolSeen};
use crate::unl::ValidationSignatures;
use crate::validator_list::UnlConfig;
use crate::verifier::{self, Ask, Query, QueryId, Verdict, Verifier, Waiting};

/// The pool read: mainnet's of XRP and RLUSD.
pub const POOL: IssuedAsset = crate::rlusd::LINE;
/// The least XRP the pool must hold for a reading to count: as much as the
/// peg guard asks of it ([`crate::rlusd::PEG`]). It held about 1.86 million
/// XRP when measured; drained to a twentieth of that, its price moves too
/// cheaply to be XRP's.
pub const MIN_POOL_DROPS: u64 = crate::rlusd::PEG.min_pool_drops;
/// The oldest a reading's ledger may be, by the close time its validators
/// agreed on against the clock, when it is read: as long as a validation
/// stays fresh ([`crate::unl::FRESH_FOR_MS`]), as for the peg guard's
/// readings. The verifier takes no anchor whose validations are older, so
/// a server that stops sending new ledgers and keeps proving the one held
/// gives no reading that counts past it.
pub const MAX_ANCHOR_AGE_MS: u64 = crate::unl::FRESH_FOR_MS;
/// How often the pool is read: every fifteen seconds, four times a ledger
/// minute's worth, so a window holds twenty readings.
pub const READ_EVERY_MS: u64 = 15_000;
/// How long the window the rate is the median of is: five minutes. A pool
/// pushed off the market comes back within a few ledgers once arbitrage
/// finds it; holding it off for half of this means selling into it,
/// against every arbitrageur, for two and a half minutes.
pub const WINDOW_MS: u64 = 5 * 60_000;
/// The most readings kept: more than a window holds at the rate read.
pub const MAX_READINGS: usize = 64;
/// How many ledgers past the last counted reading's a reading's ledger must
/// be to count: three. Mainnet closes a ledger every 3.5 to 4 seconds, so
/// readings fifteen seconds apart are about four ledgers apart, and three
/// keeps each of an honest server's; a server that serves ledgers late,
/// each strictly newer, can show a push no more often than every three
/// ledgers, over ten seconds of the pool's history.
pub const MIN_LEDGERS_APART: u32 = 3;
/// The most of the verifier's asks waiting on a connection: its own bound,
/// with room for those of a round it gave up on, as a hub's link allows.
const MAX_ASKS: usize = 4 * verifier::MAX_QUERIES;

/// How often the pool is read, and how long the window is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Timings {
    /// How often the pool is read.
    pub read_every_ms: u64,
    /// How long the window the rate is the median of is.
    pub window_ms: u64,
    /// How many readings the window must hold for a rate: more than half
    /// of what it holds at the rate read.
    pub min_readings: usize,
    /// How many ledgers past the last counted reading's a reading's ledger
    /// must be to count.
    pub min_ledgers_apart: u32,
}

impl Timings {
    /// The production values: [`READ_EVERY_MS`], [`WINDOW_MS`] and
    /// [`MIN_LEDGERS_APART`].
    pub const DEFAULT: Self = Self::reading_every(READ_EVERY_MS, MIN_LEDGERS_APART);

    /// The production rule on dev timers: a reading a second, so a window
    /// is as many readings long as in production, each a ledger apart, as
    /// a dev ledger closes about once a second.
    pub const DEV: Self = Self::reading_every(1_000, 1);

    /// Whether the rule holds on these timings: the pool read at all, a
    /// window of several readings, each kept, and more than half of them
    /// needed.
    #[must_use]
    pub const fn holds(&self) -> bool {
        let per_window = self.per_window();
        self.read_every_ms > 0
            && self.min_ledgers_apart > 0
            && per_window > 2
            && per_window < MAX_READINGS
            && 2 * self.min_readings > per_window
            && self.min_readings <= per_window
    }

    /// How many readings a window holds at the rate read.
    const fn per_window(&self) -> usize {
        if self.read_every_ms == 0 {
            return 0;
        }
        // At most MAX_READINGS for any timings that hold.
        #[allow(clippy::cast_possible_truncation)]
        let readings = (self.window_ms / self.read_every_ms) as usize;
        readings
    }

    /// The production rule with the pool read every `read_every_ms`, each
    /// reading `min_ledgers_apart`.
    const fn reading_every(read_every_ms: u64, min_ledgers_apart: u32) -> Self {
        let mut timings = Self {
            read_every_ms,
            window_ms: read_every_ms * (WINDOW_MS / READ_EVERY_MS),
            min_readings: 0,
            min_ledgers_apart,
        };
        timings.min_readings = timings.per_window() / 2 + 1;
        timings
    }
}

const _: () = assert!(Timings::DEFAULT.holds());
const _: () = assert!(Timings::DEV.holds());

/// What the reader has its holder do. Taken apart at once: boxing would
/// only allocate.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Out {
    /// Send this on the connection.
    Send(Vec<u8>),
    /// Fetch this validator list publisher's site by an HTTPS GET that
    /// follows no redirect and reads at most
    /// [`MAX_LIST_BYTES`](crate::validator_list::MAX_LIST_BYTES), and hand
    /// what came to [`PoolRate::fetched`].
    Fetch(String),
    /// Drop the connection, connect anew, and say so with
    /// [`PoolRate::connected`].
    Reconnect,
}

/// What became of the last reading.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Seen {
    /// It counted.
    Counted,
    /// There is no pool, or it holds none of the token.
    NoPool,
    /// The pool held less XRP than [`MIN_POOL_DROPS`].
    TooThin,
    /// Its ledger closed longer ago than [`MAX_ANCHOR_AGE_MS`].
    OldAnchor,
    /// Its ledger is fewer than [`Timings::min_ledgers_apart`] past the
    /// last counted reading's, or that one.
    TooClose,
    /// A proven object did not read as what its key holds.
    Unreadable,
}

/// A reading that counted.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Counted {
    ledger_index: u32,
    /// When its ledger closed, by the close time its validators agreed on.
    closed_ms: u64,
    /// XRP per dollar, at the reader's precision.
    rate: u128,
}

/// The reader of XRP's rate from the pool.
pub struct PoolRate {
    verifier: Verifier,
    timings: Timings,
    /// The decimals of a rate.
    precision: u32,
    /// The pool's account, once its `AMM` object named it.
    account: Option<AccountId>,
    /// The state proof of the pool asked for, if one is.
    query: Option<QueryId>,
    /// When the next reading is due.
    read_due_ms: u64,
    readings: VecDeque<Counted>,
    last: Option<Seen>,
    /// The verifier's asks on this connection, by their request's id.
    asks: BTreeMap<u64, Ask>,
    next_id: u64,
    now_ms: u64,
    out: VecDeque<Out>,
}

impl core::fmt::Debug for PoolRate {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("PoolRate")
            .field("verifier", &self.verifier)
            .field("readings", &self.readings.len())
            .field("last", &self.last)
            .finish_non_exhaustive()
    }
}

impl PoolRate {
    /// A reader of the pool on the network `network_id`, mainnet in
    /// production, trusting the validators the publishers of `config`
    /// list, whose validations `signatures` checks, reading on `timings`,
    /// with rates of `precision` decimals.
    ///
    /// # Panics
    ///
    /// If `timings` break the rule ([`Timings::holds`]), or `precision`
    /// leaves no room for a rate's arithmetic.
    #[must_use]
    pub fn new(
        network_id: u32,
        config: UnlConfig,
        signatures: Box<dyn ValidationSignatures + Send>,
        timings: Timings,
        precision: u32,
    ) -> Self {
        assert!(timings.holds(), "the reading rule holds");
        assert!(precision <= MAX_PRECISION, "a rate's arithmetic fits");
        Self {
            verifier: Verifier::new(network_id, config, signatures),
            timings,
            precision,
            account: None,
            query: None,
            read_due_ms: 0,
            readings: VecDeque::new(),
            last: None,
            asks: BTreeMap::new(),
            next_id: 1,
            now_ms: 0,
            out: VecDeque::new(),
        }
    }

    /// The connection is up.
    pub fn connected(&mut self, now_ms: u64) {
        self.now(now_ms);
        self.asks.clear();
        self.verifier.connected(now_ms);
        self.drain();
    }

    /// The connection is down: what was asked on it is never answered.
    pub fn disconnected(&mut self) {
        self.asks.clear();
        self.verifier.disconnected();
        self.drain();
    }

    /// Does what is due at `now_ms`: a reading, and the verifier's timers.
    pub fn tick(&mut self, now_ms: u64) {
        self.now(now_ms);
        self.verifier.tick(now_ms);
        self.read_if_due();
        self.drain();
    }

    /// A message from the server.
    pub fn message(&mut self, now_ms: u64, bytes: &[u8]) {
        self.now(now_ms);
        if let Message::Response { id, outcome } = api::parse(bytes)
            && let Some(ask) = self.asks.remove(&id)
        {
            self.verifier.answered(&ask, outcome, now_ms);
        }
        self.read_if_due();
        self.drain();
    }

    /// What a publisher's site gave for `url`, if anything.
    pub fn fetched(&mut self, now_ms: u64, url: &str, body: Option<&[u8]>) {
        self.now(now_ms);
        self.verifier.fetched(url, body, now_ms);
        self.drain();
    }

    /// What to do next, if anything.
    pub fn poll(&mut self) -> Option<Out> {
        self.out.pop_front()
    }

    /// When [`Self::tick`] next has something to do. Never in the past of
    /// the last input, so a holder that sleeps until it never spins.
    #[must_use]
    pub fn deadline(&self) -> u64 {
        let due = if self.query.is_some() {
            u64::MAX
        } else {
            self.read_due_ms
        };
        let deadline = self.verifier.deadline().map_or(due, |at| at.min(due));
        deadline.max(self.now_ms.saturating_add(1))
    }

    /// XRP's rate at `now_ms`, in XRP per dollar at the reader's
    /// precision: the median of the readings that counted whose ledgers
    /// closed within the window, once enough did. With an even number, the
    /// lower of the middle two: a rate the pool had.
    #[must_use]
    pub fn rate(&self, now_ms: u64) -> Option<u128> {
        let mut rates: Vec<u128> = (self.in_window(now_ms))
            .map(|reading| reading.rate)
            .collect();
        if rates.len() < self.timings.min_readings {
            return None;
        }
        rates.sort_unstable();
        rates.get((rates.len() - 1) / 2).copied()
    }

    /// What became of the last reading, if one was taken.
    #[must_use]
    pub const fn last(&self) -> Option<Seen> {
        self.last
    }

    /// Why the verifier waits, if it does.
    #[must_use]
    pub const fn waiting(&self) -> Option<Waiting> {
        self.verifier.waiting()
    }

    /// Whether the verifier stopped: the UNL validated two chains of a
    /// network that never resets.
    #[must_use]
    pub const fn failed(&self) -> bool {
        self.verifier.failed()
    }

    /// How many readings counted whose ledgers closed within the window at
    /// `now_ms`.
    #[must_use]
    pub fn readings(&self, now_ms: u64) -> usize {
        self.in_window(now_ms).count()
    }

    /// The readings that counted whose ledgers closed within the window at
    /// `now_ms`: one a clock behind sees close after it is in it too.
    fn in_window(&self, now_ms: u64) -> impl Iterator<Item = &Counted> {
        let since = now_ms.saturating_sub(self.timings.window_ms);
        (self.readings.iter()).filter(move |reading| reading.closed_ms >= since)
    }

    /// Checks what must always hold.
    pub fn check_invariants(&self) {
        assert!(self.readings.len() <= MAX_READINGS);
        assert!(self.asks.len() <= MAX_ASKS);
        assert!(
            (self.readings.iter().zip(self.readings.iter().skip(1))).all(|(a, b)| b.ledger_index
                >= a.ledger_index
                    .saturating_add(self.timings.min_ledgers_apart)
                && a.closed_ms <= b.closed_ms),
            "readings go forward, the least ledgers apart"
        );
        self.verifier.check_invariants();
    }

    fn now(&mut self, now_ms: u64) {
        self.now_ms = self.now_ms.max(now_ms);
    }

    /// Asks for a reading, once one is due and none is asked: a new
    /// anchor, and the pool at the one held.
    fn read_if_due(&mut self) {
        if self.query.is_some() || self.now_ms < self.read_due_ms {
            return;
        }
        self.read_due_ms = self.now_ms.saturating_add(self.timings.read_every_ms);
        self.verifier.anchor_now(self.now_ms);
        let mut keys = Vec::new();
        pool::push_keys(&mut keys, &POOL, self.account);
        let asked = self
            .verifier
            .query(Query::State { keys, at: None }, self.now_ms);
        assert!(asked.is_ok(), "the only query, of three distinct keys");
        self.query = asked.ok();
    }

    /// Carries out what the verifier decided.
    fn drain(&mut self) {
        for out in self.verifier.take_out() {
            match out {
                verifier::Out::Ask(Ask::Fetch(url)) => self.out.push_back(Out::Fetch(url)),
                verifier::Out::Ask(ask) => {
                    let id = self.next_id;
                    self.next_id = self.next_id.wrapping_add(1).max(1);
                    if let Some(request) = ask.request(id) {
                        if self.asks.len() >= MAX_ASKS {
                            self.asks.pop_first();
                        }
                        self.asks.insert(id, ask);
                        self.out.push_back(Out::Send(request));
                    }
                }
                verifier::Out::Verdict(id, verdict) if self.query == Some(id) => {
                    self.query = None;
                    self.verdict(&verdict);
                }
                verifier::Out::Verdict(..)
                | verifier::Out::Anchored(_)
                | verifier::Out::Reset(_) => {}
                verifier::Out::Reconnect => {
                    self.asks.clear();
                    self.out.push_back(Out::Reconnect);
                }
            }
        }
    }

    /// Takes what the state proof of the pool proved.
    fn verdict(&mut self, verdict: &Verdict) {
        let Verdict::State(proven) = verdict else {
            return;
        };
        let header = &proven.header;
        let seen = match pool::seen(
            proven,
            &POOL,
            self.account,
            header.sequence,
            header.close_time,
        ) {
            Ok(Some(seen)) => seen,
            Ok(None) => return,
            Err(_) => {
                self.last = Some(Seen::Unreadable);
                return;
            }
        };
        let reading = match seen {
            PoolSeen::Gone => {
                self.account = None;
                self.last = Some(Seen::NoPool);
                return;
            }
            PoolSeen::Named(account) => {
                // Read again at once, with the account's root and line.
                self.account = Some(account);
                self.read_due_ms = self.now_ms;
                self.read_if_due();
                return;
            }
            PoolSeen::Read(reading) => reading,
        };
        self.last = Some(self.counts(&reading));
        if self.last != Some(Seen::Counted) {
            return;
        }
        let Some(rate) = xrp_per_dollar(reading.drops, reading.value, self.precision) else {
            self.last = Some(Seen::NoPool);
            return;
        };
        if self.readings.len() >= MAX_READINGS {
            self.readings.pop_front();
        }
        self.readings.push_back(Counted {
            ledger_index: reading.ledger_index,
            closed_ms: reading.closed_ms,
            rate,
        });
        let since = self.now_ms.saturating_sub(self.timings.window_ms);
        while self
            .readings
            .front()
            .is_some_and(|oldest| oldest.closed_ms < since)
        {
            self.readings.pop_front();
        }
    }

    /// Whether `reading` counts, and if not, why.
    fn counts(&self, reading: &pool::PoolReading) -> Seen {
        if reading.value.is_zero() || reading.value.is_negative() {
            Seen::NoPool
        } else if reading.drops < MIN_POOL_DROPS {
            Seen::TooThin
        } else if self.now_ms.saturating_sub(reading.closed_ms) > MAX_ANCHOR_AGE_MS {
            Seen::OldAnchor
        } else if (self.readings.back()).is_some_and(|newest| {
            // Close times grow along a chain, but by a second at least: a
            // later ledger said to close earlier is none the verifier
            // takes, and counts for nothing here either.
            reading.ledger_index
                < newest
                    .ledger_index
                    .saturating_add(self.timings.min_ledgers_apart)
                || reading.closed_ms < newest.closed_ms
        }) {
            Seen::TooClose
        } else {
            Seen::Counted
        }
    }
}

/// The most decimals a rate may have: so that drops times ten to the
/// precision, less a token value's exponent, stays within 128 bits for any
/// pool that counts.
pub const MAX_PRECISION: u32 = 18;

/// How many XRP a dollar buys in a pool of `drops` and `value` of a dollar
/// token at par, times ten to `precision`, rounded down: `None` for a pool
/// with no token, a price past the arithmetic's bounds, or one that rounds
/// to nothing.
#[must_use]
pub fn xrp_per_dollar(drops: u64, value: TokenValue, precision: u32) -> Option<u128> {
    if value.is_zero() || value.is_negative() || precision > MAX_PRECISION {
        return None;
    }
    // XRP per dollar = (drops / 10^6) / (mantissa · 10^exponent).
    let mantissa = u128::from(value.mantissa());
    let places =
        i64::from(precision) - i64::from(DROPS_PER_XRP.ilog10()) - i64::from(value.exponent());
    let rate = if places >= 0 {
        let scale = 10_u128.checked_pow(u32::try_from(places).ok()?)?;
        u128::from(drops).checked_mul(scale)? / mantissa
    } else {
        let scale = 10_u128.checked_pow(u32::try_from(-places).ok()?)?;
        u128::from(drops) / mantissa.checked_mul(scale)?
    };
    (rate > 0).then_some(rate)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    #[test]
    fn rates_are_exact() {
        // 1.86 million XRP beside 4.65 million RLUSD: $2.50 an XRP, 0.4 XRP
        // to the dollar.
        let value = TokenValue::parse("4650000").unwrap();
        let rate = xrp_per_dollar(1_860_000 * DROPS_PER_XRP, value, 15).unwrap();
        assert_eq!(rate, 400_000_000_000_000);
        // A fractional token value.
        let value = TokenValue::parse("0.5").unwrap();
        assert_eq!(xrp_per_dollar(2_000_000, value, 6), Some(4_000_000));
        // Nothing to the dollar.
        assert_eq!(
            xrp_per_dollar(1, TokenValue::parse("1000000000").unwrap(), 6),
            None
        );
        assert_eq!(xrp_per_dollar(1, TokenValue::ZERO, 15), None);
        assert_eq!(xrp_per_dollar(1, value, MAX_PRECISION + 1), None);
        // Past the arithmetic's bounds, none, never an overflow.
        let tiny = TokenValue::parse("0.000000000000001").unwrap();
        assert!(xrp_per_dollar(100_000_000_000 * DROPS_PER_XRP, tiny, MAX_PRECISION).is_none());
    }

    #[test]
    fn timings_hold() {
        assert_eq!(Timings::DEFAULT.min_readings, 11);
        assert_eq!(Timings::DEFAULT.min_ledgers_apart, MIN_LEDGERS_APART);
        assert_eq!(Timings::DEV.window_ms, 20_000);
        let broken = Timings {
            min_readings: 10,
            ..Timings::DEFAULT
        };
        assert!(!broken.holds());
    }
}
