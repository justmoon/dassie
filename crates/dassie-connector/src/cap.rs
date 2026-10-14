//! Spending caps (spec 7 §4.5): at most an amount per window, refilling
//! continuously, for what an account sends out of its owner's funds.
//!
//! The bucket is kept in exact integers. `used` is in amount·ms: a
//! fulfilled packet of `a` adds `a · window`, and `used` drains by `amount`
//! every ms. A packet in flight holds its amount in `reserved`. A Prepare of
//! `a` passes only while `used + (reserved + a) · window ≤ amount · window`,
//! so whatever an account spends in a period of `t` ms is at most
//! `amount + amount · t / window`.

/// A cap: at most `amount` per `window_ms`, refilling continuously.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SpendingCap {
    /// The most the account may spend in a window, in its ledger's units.
    pub amount: u64,
    /// The window, in ms.
    pub window_ms: u64,
}

/// Why a cap was not taken.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CapError {
    /// The amount is zero or above `i64::MAX`, or the window is out of
    /// range.
    Invalid,
    /// The connector holds as many caps as it may.
    Full,
}

impl SpendingCap {
    /// The shortest window: a second.
    pub const MIN_WINDOW_MS: u64 = 1_000;
    /// The longest window: 366 days.
    pub const MAX_WINDOW_MS: u64 = 366 * 24 * 60 * 60 * 1_000;

    /// Whether the cap is one the connector takes.
    #[must_use]
    pub const fn is_valid(&self) -> bool {
        self.amount > 0
            && self.amount <= i64::MAX.unsigned_abs()
            && self.window_ms >= Self::MIN_WINDOW_MS
            && self.window_ms <= Self::MAX_WINDOW_MS
    }

    /// The bucket's size, in amount·ms.
    const fn capacity(&self) -> u128 {
        self.amount as u128 * self.window_ms as u128
    }
}

/// What a cap's fulfilled packets use of it, as of a time. This is what is
/// persisted: the journal's recent rows alone cannot rebuild it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CapUsage {
    /// In amount·ms.
    pub used: u128,
    /// When `used` was last brought up to date.
    pub as_of_ms: u64,
}

impl CapUsage {
    /// Nothing used, as of `now_ms`.
    #[must_use]
    pub const fn new(now_ms: u64) -> Self {
        Self {
            used: 0,
            as_of_ms: now_ms,
        }
    }

    /// The usage at `now_ms`, after draining. A time before `as_of_ms`
    /// changes nothing: the bucket never fills up again backwards.
    #[must_use]
    pub fn refilled(self, cap: SpendingCap, now_ms: u64) -> Self {
        let Some(elapsed_ms) = now_ms.checked_sub(self.as_of_ms) else {
            return self;
        };
        let drained = u128::from(elapsed_ms) * u128::from(cap.amount);
        Self {
            used: self.used.saturating_sub(drained),
            as_of_ms: now_ms,
        }
    }

    /// The usage after a fulfilled packet of `amount` at `now_ms`.
    #[must_use]
    pub fn spent(self, cap: SpendingCap, now_ms: u64, amount: u64) -> Self {
        let refilled = self.refilled(cap, now_ms);
        let added = u128::from(amount) * u128::from(cap.window_ms);
        Self {
            used: refilled.used.saturating_add(added),
            ..refilled
        }
    }

    /// The usage under `to` of what was used under `from`, at `now_ms`:
    /// the amount used, rounded up, then as much of the new cap.
    #[must_use]
    pub fn rescaled(self, from: SpendingCap, to: SpendingCap, now_ms: u64) -> Self {
        let refilled = self.refilled(from, now_ms);
        let amount = refilled.used.div_ceil(u128::from(from.window_ms));
        Self {
            used: amount.saturating_mul(u128::from(to.window_ms)),
            as_of_ms: refilled.as_of_ms,
        }
    }
}

/// The usage that fulfilled packets of `(time, amount)`, in time order,
/// leave from `start`: the model the connector's bookkeeping must match.
#[must_use]
pub fn replay(
    cap: SpendingCap,
    start: CapUsage,
    spent: impl IntoIterator<Item = (u64, u64)>,
) -> CapUsage {
    spent.into_iter().fold(start, |usage, (at_ms, amount)| {
        usage.spent(cap, at_ms, amount)
    })
}

/// A cap as the owner sees it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CapView {
    /// The cap.
    pub cap: SpendingCap,
    /// What fulfilled packets use of it, as of the time asked.
    pub usage: CapUsage,
    /// What packets in flight hold.
    pub reserved: u128,
    /// What could be spent now.
    pub available: u64,
    /// When the bucket is full again, if it is not.
    pub full_at_ms: Option<u64>,
}

/// Why a Prepare does not pass a cap.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CapRefusal {
    /// The packet alone is more than the cap: it never passes (F08).
    TooLarge(u64),
    /// Not now: the bucket refills (T04).
    Exhausted,
}

/// A cap and its bucket.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct CapState {
    pub cap: SpendingCap,
    pub usage: CapUsage,
    pub reserved: u128,
    /// Set while the bucket is over the cap, which only a cap set or
    /// lowered while packets were in flight leaves it; cleared once it is
    /// within again.
    pub over: bool,
}

impl CapState {
    /// Whether a packet of `amount` may go at `now_ms`.
    pub fn admits(&self, now_ms: u64, amount: u64) -> Result<(), CapRefusal> {
        if amount > self.cap.amount {
            return Err(CapRefusal::TooLarge(self.cap.amount));
        }
        let used = self.usage.refilled(self.cap, now_ms).used;
        let held = self
            .reserved
            .checked_add(u128::from(amount))
            .and_then(|held| held.checked_mul(u128::from(self.cap.window_ms)))
            .and_then(|held| held.checked_add(used));
        match held {
            Some(held) if held <= self.cap.capacity() => Ok(()),
            _ => Err(CapRefusal::Exhausted),
        }
    }

    /// A packet of `amount` that [`admits`](Self::admits) let through is
    /// in flight from `now_ms`. The usage is brought up to date with it, so
    /// that what is stored stays within the cap.
    pub fn reserve(&mut self, now_ms: u64, amount: u64) {
        self.usage = self.usage.refilled(self.cap, now_ms);
        self.reserved += u128::from(amount);
    }

    /// A packet of `amount` was fulfilled at `now_ms`.
    pub fn post(&mut self, now_ms: u64, amount: u64) {
        self.unreserve(amount);
        self.usage = self.usage.spent(self.cap, now_ms, amount);
        self.over = self.over && !self.within();
    }

    /// A packet of `amount` is no longer in flight.
    pub fn release(&mut self, amount: u64) {
        self.unreserve(amount);
        self.over = self.over && !self.within();
    }

    fn unreserve(&mut self, amount: u64) {
        assert!(
            self.reserved >= u128::from(amount),
            "a cap released more than it held"
        );
        self.reserved -= u128::from(amount);
    }

    /// The bucket as of `now_ms`.
    pub fn view(&self, now_ms: u64) -> CapView {
        let usage = self.usage.refilled(self.cap, now_ms);
        let window = u128::from(self.cap.window_ms);
        let held = usage
            .used
            .saturating_add(self.reserved.saturating_mul(window));
        let free = self.cap.capacity().saturating_sub(held) / window;
        let full_at_ms = (usage.used > 0).then(|| {
            let drain_ms = usage.used.div_ceil(u128::from(self.cap.amount));
            usage
                .as_of_ms
                .saturating_add(u64::try_from(drain_ms).unwrap_or(u64::MAX))
        });
        CapView {
            cap: self.cap,
            usage,
            reserved: self.reserved,
            available: u64::try_from(free).unwrap_or(u64::MAX),
            full_at_ms,
        }
    }

    /// Whether nothing is over the cap: true unless the cap was lowered,
    /// or set, while packets were in flight.
    pub fn within(&self) -> bool {
        self.reserved
            .checked_mul(u128::from(self.cap.window_ms))
            .and_then(|held| held.checked_add(self.usage.used))
            .is_some_and(|held| held <= self.cap.capacity())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const CAP: SpendingCap = SpendingCap {
        amount: 1_000,
        window_ms: 10_000,
    };

    fn state() -> CapState {
        CapState {
            cap: CAP,
            usage: CapUsage::new(0),
            reserved: 0,
            over: false,
        }
    }

    #[test]
    fn a_full_bucket_then_a_refill_per_ms() {
        let mut cap = state();
        assert_eq!(cap.admits(0, 1_000), Ok(()));
        cap.reserved += 1_000;
        cap.post(0, 1_000);
        assert_eq!(cap.admits(0, 1), Err(CapRefusal::Exhausted));
        // 1 unit per 10 ms.
        assert_eq!(cap.admits(9, 1), Err(CapRefusal::Exhausted));
        assert_eq!(cap.admits(10, 1), Ok(()));
        assert_eq!(cap.view(10).available, 1);
        assert_eq!(cap.view(5_000).available, 500);
        assert_eq!(cap.view(0).full_at_ms, Some(10_000));
        assert_eq!(cap.view(20_000).available, 1_000);
    }

    #[test]
    fn more_than_the_cap_never_passes() {
        assert_eq!(state().admits(0, 1_001), Err(CapRefusal::TooLarge(1_000)));
    }

    #[test]
    fn packets_in_flight_hold_their_amount() {
        let mut cap = state();
        cap.reserved += 600;
        assert_eq!(cap.admits(0, 600), Err(CapRefusal::Exhausted));
        cap.release(600);
        assert_eq!(cap.admits(0, 600), Ok(()));
    }

    #[test]
    fn rescaling_rounds_up() {
        let usage = CapUsage {
            used: 1,
            as_of_ms: 0,
        };
        let to = SpendingCap {
            amount: 5,
            window_ms: 1_000,
        };
        assert_eq!(usage.rescaled(CAP, to, 0).used, 1_000);
    }

    #[test]
    fn the_largest_caps_do_not_overflow() {
        let cap = SpendingCap {
            amount: i64::MAX.unsigned_abs(),
            window_ms: SpendingCap::MAX_WINDOW_MS,
        };
        assert!(cap.is_valid());
        let mut state = CapState {
            cap,
            usage: CapUsage::new(0),
            reserved: 0,
            over: false,
        };
        assert_eq!(state.admits(0, cap.amount), Ok(()));
        state.reserved += u128::from(cap.amount);
        state.post(0, cap.amount);
        assert_eq!(state.view(u64::MAX).available, cap.amount);
        assert!(state.view(0).full_at_ms.is_some());
    }

    #[test]
    fn invalid_caps() {
        let invalid = [
            SpendingCap {
                amount: 0,
                window_ms: 1_000,
            },
            SpendingCap {
                amount: 1,
                window_ms: 999,
            },
            SpendingCap {
                amount: u64::MAX,
                window_ms: 1_000,
            },
            SpendingCap {
                amount: 1,
                window_ms: SpendingCap::MAX_WINDOW_MS + 1,
            },
        ];
        assert!(invalid.iter().all(|cap| !cap.is_valid()));
    }
}
