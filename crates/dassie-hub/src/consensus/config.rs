//! The consensus timings, derived from the regional assumption
//! (spec/designs/settlement-hubs.md, "A hub is regional"): validators within
//! a few tens of milliseconds of each other, and a block final in well under
//! a quarter of a second.
//!
//! Everything follows from three numbers:
//!
//! - **Δ = 50 ms,** the longest a message takes between two validators of a
//!   region ([`Config::max_delay_ms`]).
//! - **The precision, 2Δ = 100 ms:** NTP's error is at most half the round
//!   trip to its server, so a validator synced to a server in its region is
//!   within Δ of true time, and two validators within 2Δ of each other
//!   ([`Config::precision_ms`]).
//! - **The block interval, 100 ms:** an operation waits at most one interval
//!   for the next block, then three message delays, 3Δ = 150 ms, for it to
//!   be final (Simplex: proposal, notarize votes, finalize votes), 250 ms in
//!   all ([`Config::block_interval_ms`]).
//!
//! The timeouts are Simplex's, with the interval added: validators enter a
//! view within Δ of each other, since each multicasts the certificate that
//! took it there; the leader waits at most an interval after its parent's
//! stamp, and its proposal takes Δ (`leader_timeout_ms`, interval + 2Δ); the
//! notarize votes take another Δ (`advance_timeout_ms`, interval + 3Δ).
//!
//! Views that fail in a row wait longer and longer
//! ([`Config::backoff_after_views`]), but never longer than
//! [`MAX_VIEW_TIMEOUT_MS`], and the stall a hub's rules set must fit around
//! that ([`MIN_STALL_MS`], [`MAX_STALL_MS`]).

use super::engine::{MAX_ROUNDS, MAX_UNFINALIZED_HEIGHTS, MAX_VIEWS_AHEAD};
use crate::validators::{MAX_VALIDATORS, MIN_VALIDATORS};

/// The longest any view waits for its proposal or its notarization,
/// however many views failed before it. Half a minute: long enough that a
/// hub in a long outage enters a view led by a live validator only twice a
/// minute (the views of up to f faulty leaders end at once, and its
/// rebroadcasts go every `max_retry_ms`), so it sends a trickle and holds
/// few views; short enough that once the network heals, the view in
/// progress ends within it, so recovery takes about this long, not as long
/// as the outage was. Against the views of a working hub (a quarter of a
/// second) it is 120 of them; against the stall (an hour in the test terms)
/// it is small.
pub const MAX_VIEW_TIMEOUT_MS: u64 = 30_000;

/// The most faulty validators of the largest set.
const MAX_FAULTS: u64 = (MAX_VALIDATORS as u64 - 1) / 3;

/// The shortest stall a hub's rules may set: twice what recovery takes
/// after the network heals from the longest outage. The view in progress
/// ends within [`MAX_VIEW_TIMEOUT_MS`], and so may each of the views of up
/// to f faulty leaders in a row after it, before an honest leader's block
/// is notarized and views are short again: f + 1 of the longest views. A
/// stall any shorter could end a hub that was about to recover.
pub const MIN_STALL_MS: u64 = 2 * (MAX_FAULTS + 1) * MAX_VIEW_TIMEOUT_MS;

/// The views an engine holds besides those an outage makes: the views
/// ahead of its own, those that certified the chain waiting to be final,
/// the views that fail before views wait longer (at most
/// [`MAX_BACKOFF_AFTER_VIEWS`]), and the views that double up to the
/// longest (fewer than 16, as 2^15 ms is longer than it).
const RESERVED_ROUNDS: u64 =
    MAX_VIEWS_AHEAD + MAX_UNFINALIZED_HEIGHTS + MAX_BACKOFF_AFTER_VIEWS + 16 + 1;

/// The most [`Config::backoff_after_views`] may be.
pub const MAX_BACKOFF_AFTER_VIEWS: u64 = 64;

/// The smallest share of a set's validators that are not faulty, n − f of
/// n, over every size a set may have: 11 of 16.
const LIVE_SHARE: (u64, u64) = live_share();

const fn live_share() -> (u64, u64) {
    let mut worst = (1, 1);
    let mut n = MIN_VALIDATORS as u64;
    while n <= MAX_VALIDATORS as u64 {
        let live = n - (n - 1) / 3;
        if live * worst.1 < worst.0 * n {
            worst = (live, n);
        }
        n += 1;
    }
    worst
}

/// The longest stall a hub's rules may set: an outage that long, in which
/// a quorum keeps nullifying views and nothing is notarized, must make no
/// more views than an engine holds (`MAX_ROUNDS`), or the engine could not
/// enter the next one; a longer outage is a stall anyway. A view led by a
/// live validator lasts [`MAX_VIEW_TIMEOUT_MS`] then, but those of up to f
/// faulty leaders (silent ones skipped, ones that give up their view or
/// propose what is invalid) end at once: f views of each rotation of n
/// take no time, so there are up to n / (n − f) views per longest view,
/// 16 / 11 for the largest set.
pub const MAX_STALL_MS: u64 =
    (MAX_ROUNDS as u64 - RESERVED_ROUNDS) * MAX_VIEW_TIMEOUT_MS * LIVE_SHARE.0 / LIVE_SHARE.1;

const _: () = assert!(
    MIN_STALL_MS < MAX_STALL_MS,
    "a stall fits between the bounds"
);

/// The consensus settings. [`Config::DEFAULT`] holds the derived values;
/// the hub's rules may set others.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Config {
    /// Δ: the longest a message takes between two validators.
    pub max_delay_ms: u64,
    /// How far apart two validators' clocks may be. A validator notarizes a
    /// block only if its stamp is within this of its own clock, allowing Δ
    /// for the message (proposer-based timestamps).
    pub precision_ms: u64,
    /// The least time between a block's stamp and its child's proposal: a
    /// block at most this often.
    pub block_interval_ms: u64,
    /// How long after entering a view a validator waits for the leader's
    /// proposal before it nullifies the view.
    pub leader_timeout_ms: u64,
    /// How long after entering a view a validator waits for its
    /// notarization before it nullifies the view.
    pub advance_timeout_ms: u64,
    /// The longest between two rebroadcasts of a nullify, with the
    /// certificate that entered its view, while a view is stuck. Retries
    /// start after `advance_timeout_ms` and double up to this: eight
    /// timeouts, so a partition costs each pair a message every two
    /// seconds, and a healed network is back in step within two seconds.
    pub max_retry_ms: u64,
    /// How many views a leader may have been silent in, while a quorum
    /// voted, before its views are nullified at once instead of after the
    /// timeouts. A live validator votes in every view it is in; ten views
    /// are at least a second.
    pub skip_views: u64,
    /// How many views in a row may be nullified, since the last that was
    /// notarized, before each further view waits twice as long as the one
    /// before. The views after the last notarized one are what the engine
    /// must keep to build on it, so this bounds them, however long the
    /// network stays unable to notarize: some 40 doublings make a view
    /// longer than any outage. Sixteen is more than the faulty leaders of
    /// three rotations of the largest set, so a hub that is merely missing
    /// validators never waits longer.
    pub backoff_after_views: u64,
    /// The least time between two catch-up requests: a round trip.
    pub fetch_interval_ms: u64,
}

const DELAY_MS: u64 = 50;
const INTERVAL_MS: u64 = 100;
const ADVANCE_MS: u64 = INTERVAL_MS + 3 * DELAY_MS;

impl Config {
    /// The timings derived from the regional assumption (the module's
    /// documentation says how).
    pub const DEFAULT: Self = Self {
        max_delay_ms: DELAY_MS,
        precision_ms: 2 * DELAY_MS,
        block_interval_ms: INTERVAL_MS,
        leader_timeout_ms: INTERVAL_MS + 2 * DELAY_MS,
        advance_timeout_ms: ADVANCE_MS,
        max_retry_ms: 8 * ADVANCE_MS,
        skip_views: 10,
        backoff_after_views: 16,
        fetch_interval_ms: 2 * DELAY_MS,
    };

    /// The hub's margin M with `faults` validators faulty
    /// (spec/designs/settlement-hubs.md#4-peering-over-a-hub): the
    /// precision, plus the longest it takes to finalize a block when the
    /// next `faults` leaders are faulty: each of their views skipped after
    /// its timeout, then an interval and three message delays for an honest
    /// leader's block.
    #[must_use]
    pub const fn margin_ms(&self, faults: u64) -> u64 {
        self.precision_ms
            .saturating_add(faults.saturating_mul(self.advance_timeout_ms))
            .saturating_add(self.block_interval_ms)
            .saturating_add(self.max_delay_ms.saturating_mul(3))
    }

    /// Checks that the settings fit together.
    ///
    /// # Panics
    ///
    /// If they do not: a configuration error, not input.
    pub fn validate(&self) {
        assert!(self.max_delay_ms > 0, "a message takes some time");
        assert!(
            self.leader_timeout_ms > self.block_interval_ms,
            "a leader has time to propose"
        );
        assert!(
            self.advance_timeout_ms >= self.leader_timeout_ms,
            "a view waits for its notarization at least as long as for its proposal"
        );
        assert!(
            self.max_retry_ms >= self.advance_timeout_ms,
            "retries back off from the timeout"
        );
        assert!(self.skip_views > 0, "a leader may miss a view");
        assert!(
            self.backoff_after_views > 0 && self.backoff_after_views <= MAX_BACKOFF_AFTER_VIEWS,
            "a view may fail before views wait longer, but not too many"
        );
        assert!(
            self.advance_timeout_ms <= MAX_VIEW_TIMEOUT_MS,
            "no view waits longer than the longest"
        );
    }
}

impl Default for Config {
    fn default() -> Self {
        Self::DEFAULT
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_block_is_final_within_a_quarter_of_a_second() {
        let config = Config::DEFAULT;
        config.validate();
        // One interval to the next block, then three message delays.
        assert_eq!(config.block_interval_ms + 3 * config.max_delay_ms, 250);
        // With one faulty leader before an honest one: its timeout too.
        assert_eq!(config.margin_ms(0), 350);
        assert_eq!(config.margin_ms(1), 600);
    }

    #[test]
    fn the_stall_fits_around_the_longest_view() {
        // Recovery from the longest outage takes f + 1 of the longest views
        // at worst: six for the largest set, three minutes.
        assert_eq!(MIN_STALL_MS, 6 * 60_000);
        // 384 longest views, of which 5 in 16 take no time: 2.2 hours.
        assert_eq!(LIVE_SHARE, (11, 16));
        assert_eq!(MAX_STALL_MS, 384 * 30_000 * 11 / 16);
        // The test terms' hour fits.
        assert!((MIN_STALL_MS..=MAX_STALL_MS).contains(&3_600_000));
    }
}
