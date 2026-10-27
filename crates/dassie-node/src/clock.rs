//! Time and randomness: where everything the node decides gets them
//! (spec/decisions/tigerstyle.md), so tests and simulations can substitute
//! their own. Pacing I/O on the monotonic clock and generating TLS and ACME
//! keys decide nothing, and read the OS directly.

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

/// Resolves when a clock that can be moved by hand moves, so tests need not
/// wait for real time to pass: `wake` is what [`Clock::subscribe`] gave.
/// Never resolves for the system clock.
pub async fn clock_moved(wake: &mut Option<watch::Receiver<u64>>) {
    match wake {
        Some(receiver) => {
            if receiver.changed().await.is_err() {
                *wake = None;
            }
        }
        None => std::future::pending().await,
    }
}

/// Resolves once `interval_ms` from its first poll has passed on `clock`:
/// on a clock moved by hand, when the clock reaches the deadline read then,
/// woken by its moves; on the system clock, when the interval's real time
/// has passed, a sleep that never reads the wall clock, so no step of it,
/// back or forward, at any moment, changes the wait. A periodic task paced
/// by this one comes every interval of real time in production, and in a
/// test whenever the test moves the clock past its deadline. `moves` is
/// what [`Clock::subscribe`] gave.
pub async fn interval_passed(
    clock: &dyn Clock,
    moves: &mut Option<watch::Receiver<u64>>,
    interval_ms: u64,
) {
    if moves.is_none() {
        tokio::time::sleep(std::time::Duration::from_millis(interval_ms)).await;
        return;
    }
    let deadline_ms = clock.now_ms().saturating_add(interval_ms);
    loop {
        let wait_ms = deadline_ms.saturating_sub(clock.now_ms());
        if wait_ms == 0 {
            return;
        }
        tokio::select! {
            () = tokio::time::sleep(std::time::Duration::from_millis(wait_ms)) => {
                if moves.is_none() {
                    return;
                }
            }
            () = clock_moved(moves) => {}
        }
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
