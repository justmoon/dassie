//! Time and randomness: where everything the node decides gets them
//! (D16), so tests and simulations can substitute their own. Pacing I/O on
//! the monotonic clock and generating TLS and ACME keys decide nothing, and
//! read the OS directly.

use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use rand_core::{CryptoRng, UnwrapErr};
use tokio::sync::watch;

/// Wall-clock time in ms since the Unix epoch.
pub trait Clock: Send + Sync + 'static {
    /// Now.
    fn now_ms(&self) -> u64;

    /// Tells when the clock is moved by hand, for clocks that are. Tasks
    /// waiting for a deadline wake up then, instead of after real time.
    fn subscribe(&self) -> Option<watch::Receiver<u64>> {
        None
    }
}

/// The system clock.
#[derive(Clone, Copy, Debug, Default)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now_ms(&self) -> u64 {
        // Before 1970 or past the year 584 million, both of which mean a
        // broken clock, time stands still at the edge rather than panicking.
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |since| {
                u64::try_from(since.as_millis()).unwrap_or(u64::MAX)
            })
    }
}

/// A clock that moves only when told to.
#[derive(Clone, Debug, Default)]
pub struct ManualClock(Arc<watch::Sender<u64>>);

impl ManualClock {
    /// A clock showing `now_ms`.
    #[must_use]
    pub fn new(now_ms: u64) -> Self {
        Self(Arc::new(watch::Sender::new(now_ms)))
    }

    /// Moves the clock forward.
    pub fn advance(&self, by_ms: u64) {
        self.0.send_modify(|now| *now = now.saturating_add(by_ms));
    }
}

impl Clock for ManualClock {
    fn now_ms(&self) -> u64 {
        *self.0.borrow()
    }

    fn subscribe(&self) -> Option<watch::Receiver<u64>> {
        Some(self.0.subscribe())
    }
}

/// A cryptographic RNG the node can share between tasks.
pub trait SharedRng: CryptoRng + Send + 'static {}

impl<T: CryptoRng + Send + 'static> SharedRng for T {}

/// The OS RNG. A failure of the OS RNG is not something to carry on from,
/// so it panics.
#[must_use]
pub fn os_rng() -> Box<dyn SharedRng> {
    Box::new(UnwrapErr(getrandom::SysRng))
}
