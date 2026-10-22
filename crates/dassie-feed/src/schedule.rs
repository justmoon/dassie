//! When to publish (spec/areas/rate-feed.md#publishing).
//!
//! A node's rates move with every conversion, but a feed costs a signature
//! and a datagram per subscriber, so a node publishes at most once per
//! [`FEED_MIN_INTERVAL_MS`] while its rates change, and otherwise a
//! heartbeat every [`FEED_HEARTBEAT_MS`], so subscribers know it is alive
//! and its rates still hold. A change that comes within the interval is
//! published when the interval ends, never lost.

use dassie_node_core::SECOND_MS;

/// The least time between two feeds.
pub const FEED_MIN_INTERVAL_MS: u64 = SECOND_MS;

/// The most time between two feeds.
pub const FEED_HEARTBEAT_MS: u64 = 30 * SECOND_MS;

/// How long a feed counts after it was made: a few heartbeats, so that a
/// lost one or two do not leave a subscriber without rates.
pub const FEED_VALIDITY_MS: u64 = 3 * FEED_HEARTBEAT_MS;

/// What to do now.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Poll {
    /// Publish a feed with this sequence and validity, and poll again at
    /// `next_ms`.
    Publish {
        /// The feed's sequence: larger than any before.
        sequence_ms: u64,
        /// Until when it counts.
        valid_until_ms: u64,
        /// When to poll next, at the latest.
        next_ms: u64,
    },
    /// Nothing yet; poll again at `next_ms`, or sooner when the content
    /// changes.
    Wait {
        /// When to poll next, at the latest.
        next_ms: u64,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Published {
    at_ms: u64,
    sequence_ms: u64,
}

/// The schedule of one node's feed.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Schedule {
    last: Option<Published>,
    /// Whether the content changed since the last feed.
    pending: bool,
}

impl Schedule {
    /// A schedule that has published nothing yet.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            last: None,
            pending: false,
        }
    }

    /// Whether to publish at `now_ms`, given whether the content changed
    /// since the last poll. The first poll always publishes.
    ///
    /// A clock that went back is taken as the time of the last feed, so the
    /// heartbeat is not held up by the jump; the sequence still grows.
    pub fn poll(&mut self, now_ms: u64, changed: bool) -> Poll {
        self.pending |= changed;
        let Some(mut last) = self.last else {
            return self.publish(now_ms, 0);
        };
        last.at_ms = last.at_ms.min(now_ms);
        self.last = Some(last);
        let elapsed_ms = now_ms - last.at_ms;
        let due = if self.pending {
            FEED_MIN_INTERVAL_MS
        } else {
            FEED_HEARTBEAT_MS
        };
        if elapsed_ms >= due {
            return self.publish(now_ms, last.sequence_ms.saturating_add(1));
        }
        Poll::Wait {
            next_ms: last.at_ms.saturating_add(due),
        }
    }

    fn publish(&mut self, now_ms: u64, least_sequence_ms: u64) -> Poll {
        let sequence_ms = now_ms.max(least_sequence_ms);
        if let Some(last) = self.last {
            assert!(sequence_ms > last.sequence_ms, "the sequence only grows");
        }
        self.last = Some(Published {
            at_ms: now_ms,
            sequence_ms,
        });
        self.pending = false;
        Poll::Publish {
            sequence_ms,
            valid_until_ms: sequence_ms.saturating_add(FEED_VALIDITY_MS),
            next_ms: now_ms.saturating_add(FEED_HEARTBEAT_MS),
        }
    }
}
