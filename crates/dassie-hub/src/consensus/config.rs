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
}
