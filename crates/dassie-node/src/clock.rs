//! Time and randomness: the only places the node reads the real clock or
//! the OS RNG (D16). Everything else gets them from here, so tests and
//! simulations can substitute their own.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use rand_core::{CryptoRng, UnwrapErr};

/// Wall-clock time in ms since the Unix epoch.
pub trait Clock: Send + Sync + 'static {
    /// Now.
    fn now_ms(&self) -> u64;
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
pub struct ManualClock(Arc<AtomicU64>);

impl ManualClock {
    /// A clock showing `now_ms`.
    #[must_use]
    pub fn new(now_ms: u64) -> Self {
        Self(Arc::new(AtomicU64::new(now_ms)))
    }

    /// Moves the clock forward.
    pub fn advance(&self, by_ms: u64) {
        self.0.fetch_add(by_ms, Ordering::SeqCst);
    }
}

impl Clock for ManualClock {
    fn now_ms(&self) -> u64 {
        self.0.load(Ordering::SeqCst)
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
