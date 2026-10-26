//! A bounded token-bucket table for login and setup attempts.
//!
//! The original had no rate limiting at all
//! (spec/original/02-node-runtime.md#84-post-apilogin, RT-10). The login
//! secret is 256 bits, so guessing is hopeless anyway; the limit is there so
//! that one client cannot make the node do work as fast as it can send.
//!
//! Each client (an IPv4 address, or an IPv6 /56) has a bucket of its own.
//! There is deliberately no bucket shared by all clients: a handful of
//! addresses could keep it empty and so lock the owner out of logging in,
//! or out of setting the node up at all. The table has a fixed number of
//! client slots; when it is full, the client seen least recently loses its
//! slot, which at worst hands someone a fresh bucket.

use alloc::vec::Vec;

use dassie_kit::{ClientKey, MINUTE_MS};

/// Bucket sizes and refill rates.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RateLimitPolicy {
    /// Attempts a client may make at once.
    pub client_burst: u32,
    /// One more attempt per client every this long.
    pub client_refill_ms: u64,
    /// Client slots in the table.
    pub client_slots: usize,
}

impl RateLimitPolicy {
    /// 5 per client, then one every 12 s; 256 clients tracked.
    pub const DEFAULT: Self = Self {
        client_burst: 5,
        client_refill_ms: MINUTE_MS / 5,
        client_slots: 256,
    };
}

#[derive(Clone, Copy, Debug)]
struct Bucket {
    tokens: u32,
    /// When `tokens` was last brought up to date; the remainder of an
    /// unfinished refill interval is kept by not moving it all the way to now.
    updated_at_ms: u64,
}

impl Bucket {
    const fn full(burst: u32, now_ms: u64) -> Self {
        Self {
            tokens: burst,
            updated_at_ms: now_ms,
        }
    }

    fn refill(&mut self, burst: u32, refill_ms: u64, now_ms: u64) {
        let elapsed = now_ms.saturating_sub(self.updated_at_ms);
        let earned = elapsed.checked_div(refill_ms).unwrap_or(u64::MAX);
        let tokens = u64::from(self.tokens).saturating_add(earned);
        if tokens >= u64::from(burst) {
            self.tokens = burst;
            self.updated_at_ms = now_ms;
        } else {
            self.tokens = u32::try_from(tokens).unwrap_or(burst);
            self.updated_at_ms = self
                .updated_at_ms
                .saturating_add(earned.saturating_mul(refill_ms));
        }
    }

    const fn wait_ms(&self, refill_ms: u64, now_ms: u64) -> u64 {
        let next = self.updated_at_ms.saturating_add(refill_ms);
        next.saturating_sub(now_ms)
    }
}

/// An attempt was refused; try again after `retry_after_ms`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Limited {
    /// How long until an attempt would be allowed.
    pub retry_after_ms: u64,
}

/// What the limiter has done, for the node's counters.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RateLimitStats {
    /// Attempts allowed.
    pub allowed: u64,
    /// Attempts refused.
    pub limited: u64,
    /// Clients that lost their slot to a new one.
    pub evicted: u64,
}

/// The table.
#[derive(Clone, Debug)]
pub struct RateLimiter {
    policy: RateLimitPolicy,
    /// Sorted by key, so a client is found by binary search.
    clients: Vec<(ClientKey, Bucket)>,
    stats: RateLimitStats,
}

impl RateLimiter {
    /// An empty table, with all its memory allocated now.
    #[must_use]
    pub fn new(policy: RateLimitPolicy) -> Self {
        assert!(policy.client_slots > 0 && policy.client_burst > 0);
        Self {
            policy,
            clients: Vec::with_capacity(policy.client_slots),
            stats: RateLimitStats::default(),
        }
    }

    /// Takes one attempt for `client`, or says how long to wait.
    // Panics only on a broken invariant: `slot` returns an index into the table.
    #[allow(clippy::unreachable)]
    pub fn check(&mut self, client: ClientKey, now_ms: u64) -> Result<(), Limited> {
        let policy = self.policy;
        let index = self.slot(client, now_ms);
        let Some((_, bucket)) = self.clients.get_mut(index) else {
            unreachable!("slot returns an index into the table");
        };
        bucket.refill(policy.client_burst, policy.client_refill_ms, now_ms);
        let result = if bucket.tokens == 0 {
            Err(Limited {
                retry_after_ms: bucket.wait_ms(policy.client_refill_ms, now_ms),
            })
        } else {
            bucket.tokens -= 1;
            Ok(())
        };
        match result {
            Ok(()) => self.stats.allowed = self.stats.allowed.saturating_add(1),
            Err(_) => self.stats.limited = self.stats.limited.saturating_add(1),
        }
        self.check_invariants();
        result
    }

    /// Whether `client` has an attempt left, without taking it: for limits
    /// that count only failures, checked up front and charged with
    /// [`check`](Self::check) when an attempt fails.
    pub fn peek(&mut self, client: ClientKey, now_ms: u64) -> Result<(), Limited> {
        let policy = self.policy;
        let Some((_, bucket)) = self
            .find(client)
            .ok()
            .and_then(|index| self.clients.get_mut(index))
        else {
            return Ok(());
        };
        bucket.refill(policy.client_burst, policy.client_refill_ms, now_ms);
        if bucket.tokens == 0 {
            return Err(Limited {
                retry_after_ms: bucket.wait_ms(policy.client_refill_ms, now_ms),
            });
        }
        Ok(())
    }

    /// Where `client` is, or where it would go.
    fn find(&self, client: ClientKey) -> Result<usize, usize> {
        self.clients.binary_search_by_key(&client, |(key, _)| *key)
    }

    /// The slot of `client`, making one if needed.
    fn slot(&mut self, client: ClientKey, now_ms: u64) -> usize {
        let mut place = self.find(client);
        if place.is_err() && self.clients.len() >= self.policy.client_slots {
            self.evict_least_recent();
            place = self.find(client);
        }
        match place {
            Ok(index) => index,
            Err(index) => {
                let fresh = Bucket::full(self.policy.client_burst, now_ms);
                self.clients.insert(index, (client, fresh));
                index
            }
        }
    }

    /// Frees the slot of the client seen least recently.
    fn evict_least_recent(&mut self) {
        let oldest = self
            .clients
            .iter()
            .enumerate()
            .min_by_key(|(_, (_, bucket))| bucket.updated_at_ms)
            .map(|(index, _)| index);
        if let Some(oldest) = oldest {
            self.clients.remove(oldest);
            self.stats.evicted = self.stats.evicted.saturating_add(1);
        }
    }

    /// The counters so far.
    #[must_use]
    pub const fn stats(&self) -> RateLimitStats {
        self.stats
    }

    /// Panics if the table is inconsistent: more clients than slots, a client
    /// twice or out of order, or a bucket over its size. One pass, so it
    /// runs after every attempt.
    pub fn check_invariants(&self) {
        assert!(self.clients.len() <= self.policy.client_slots);
        let sorted = self.clients.windows(2).all(|pair| match pair {
            [(a, _), (b, _)] => a < b,
            _ => true,
        });
        assert!(sorted, "clients are sorted, each once");
        let within = |(_, bucket): &(ClientKey, Bucket)| bucket.tokens <= self.policy.client_burst;
        assert!(self.clients.iter().all(within));
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::indexing_slicing)]

    use super::*;
    use core::net::{IpAddr, Ipv4Addr};
    use proptest::prelude::*;

    fn v4(last: u8) -> ClientKey {
        ClientKey::from_ip(IpAddr::V4(Ipv4Addr::new(10, 0, 0, last)))
    }

    // RT-10: the original allowed unlimited attempts.
    #[test]
    fn a_client_gets_its_burst_then_waits() {
        let mut limiter = RateLimiter::new(RateLimitPolicy::DEFAULT);
        for _ in 0..5 {
            assert_eq!(limiter.check(v4(1), 0), Ok(()));
        }
        assert_eq!(
            limiter.check(v4(1), 0),
            Err(Limited {
                retry_after_ms: 12_000
            })
        );
        assert_eq!(
            limiter.check(v4(1), 11_999),
            Err(Limited { retry_after_ms: 1 })
        );
        assert_eq!(limiter.check(v4(1), 12_000), Ok(()));
        assert_eq!(limiter.check(v4(2), 12_000), Ok(()));
        assert_eq!(
            limiter.stats(),
            RateLimitStats {
                allowed: 7,
                limited: 2,
                evicted: 0
            }
        );
    }

    // Other clients, however many and however busy, never use up the
    // owner's attempts.
    #[test]
    fn other_clients_cannot_lock_one_out() {
        let mut limiter = RateLimiter::new(RateLimitPolicy::DEFAULT);
        for now in (0..120_000).step_by(1_000) {
            for client in 2..=40 {
                let _ = limiter.check(v4(client), now);
            }
        }
        assert_eq!(limiter.check(v4(1), 120_000), Ok(()));
    }

    #[test]
    fn peeking_takes_no_attempt() {
        let mut limiter = RateLimiter::new(RateLimitPolicy::DEFAULT);
        for _ in 0..100 {
            assert_eq!(limiter.peek(v4(1), 0), Ok(()));
        }
        for _ in 0..RateLimitPolicy::DEFAULT.client_burst {
            assert_eq!(limiter.check(v4(1), 0), Ok(()));
        }
        assert!(limiter.peek(v4(1), 0).is_err());
        assert_eq!(
            limiter.peek(v4(1), RateLimitPolicy::DEFAULT.client_refill_ms),
            Ok(())
        );
    }

    #[test]
    fn a_full_table_evicts_the_least_recent_client() {
        let policy = RateLimitPolicy {
            client_slots: 2,
            ..RateLimitPolicy::DEFAULT
        };
        let mut limiter = RateLimiter::new(policy);
        assert_eq!(limiter.check(v4(1), 0), Ok(()));
        assert_eq!(limiter.check(v4(2), 100), Ok(()));
        assert_eq!(limiter.check(v4(3), 200), Ok(()));
        assert_eq!(limiter.stats().evicted, 1);
        assert!(limiter.clients.iter().all(|(key, _)| *key != v4(1)));
    }

    proptest! {
        #[test]
        fn stays_within_bounds(attempts in proptest::collection::vec((0_u8..40, 0_u64..5_000), 1..400)) {
            let policy = RateLimitPolicy { client_slots: 64, ..RateLimitPolicy::DEFAULT };
            let mut limiter = RateLimiter::new(policy);
            let mut now = 0;
            let mut allowed = [0_u64; 40];
            for (client, step) in attempts {
                now += step;
                if limiter.check(v4(client), now).is_ok() {
                    allowed[usize::from(client)] += 1;
                }
            }
            // No client ever got more than its burst plus what refilled.
            for count in allowed {
                prop_assert!(count <= 5 + now / 12_000);
            }
        }

        // More clients than slots: they churn through the table, which
        // stays sorted and full, and keeps whoever was seen last.
        #[test]
        fn a_full_table_churns_in_order(attempts in proptest::collection::vec((0_u8..40, 0_u64..5_000), 1..400)) {
            let policy = RateLimitPolicy { client_slots: 8, ..RateLimitPolicy::DEFAULT };
            let mut limiter = RateLimiter::new(policy);
            let mut now = 0;
            for (client, step) in attempts {
                now += step;
                let _ = limiter.check(v4(client), now);
                limiter.check_invariants();
                prop_assert!(limiter.clients.iter().any(|(key, _)| *key == v4(client)));
            }
            prop_assert!(limiter.clients.len() <= 8);
        }
    }
}
