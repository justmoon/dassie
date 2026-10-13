//! A bounded token-bucket table for login and setup attempts.
//!
//! The original had no rate limiting at all (spec 2 §8.4, RT-10). The login
//! secret is 256 bits, so guessing is hopeless anyway; the limit is there so
//! that a flood of attempts costs the node nothing, each one being an HMAC
//! and possibly a database write.
//!
//! Each client (an IPv4 address, or an IPv6 /64) has a bucket, and all of
//! them share a global one. An attempt takes a token from both or neither.
//! The table has a fixed number of client slots; when it is full, the client
//! seen least recently loses its slot. That can hand a forgetful attacker a
//! fresh bucket, which is why the global bucket exists.

use alloc::vec::Vec;
use core::net::IpAddr;

use crate::{MINUTE_MS, SECOND_MS};

/// Who an attempt comes from: an IPv4 address, or the /64 of an IPv6
/// address, since one host usually has a whole /64.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct ClientKey([u8; 16]);

impl ClientKey {
    /// The key of `address`.
    #[must_use]
    pub fn from_ip(address: IpAddr) -> Self {
        match address {
            IpAddr::V4(v4) => Self(v4.to_ipv6_mapped().octets()),
            IpAddr::V6(v6) => match v6.to_ipv4_mapped() {
                Some(v4) => Self(v4.to_ipv6_mapped().octets()),
                None => {
                    let mut octets = v6.octets();
                    for byte in octets.iter_mut().skip(8) {
                        *byte = 0;
                    }
                    Self(octets)
                }
            },
        }
    }
}

/// Bucket sizes and refill rates.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RateLimitPolicy {
    /// Attempts a client may make at once.
    pub client_burst: u32,
    /// One more attempt per client every this long.
    pub client_refill_ms: u64,
    /// Attempts all clients together may make at once.
    pub global_burst: u32,
    /// One more attempt overall every this long.
    pub global_refill_ms: u64,
    /// Client slots in the table.
    pub client_slots: usize,
}

impl RateLimitPolicy {
    /// 5 per client, then one every 12 s; 30 overall, then one every 2 s;
    /// 256 clients tracked.
    pub const DEFAULT: Self = Self {
        client_burst: 5,
        client_refill_ms: MINUTE_MS / 5,
        global_burst: 30,
        global_refill_ms: 2 * SECOND_MS,
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
    global: Bucket,
    clients: Vec<(ClientKey, Bucket)>,
    stats: RateLimitStats,
}

impl RateLimiter {
    /// An empty table, with all its memory allocated now.
    #[must_use]
    pub fn new(policy: RateLimitPolicy, now_ms: u64) -> Self {
        assert!(policy.client_slots > 0 && policy.client_burst > 0 && policy.global_burst > 0);
        Self {
            policy,
            global: Bucket::full(policy.global_burst, now_ms),
            clients: Vec::with_capacity(policy.client_slots),
            stats: RateLimitStats::default(),
        }
    }

    /// Takes one attempt for `client`, or says how long to wait.
    pub fn check(&mut self, client: ClientKey, now_ms: u64) -> Result<(), Limited> {
        let policy = self.policy;
        self.global
            .refill(policy.global_burst, policy.global_refill_ms, now_ms);
        let index = self.slot(client, now_ms);
        let Some((_, bucket)) = self.clients.get_mut(index) else {
            unreachable!("slot returns an index into the table");
        };
        bucket.refill(policy.client_burst, policy.client_refill_ms, now_ms);
        let result = if bucket.tokens == 0 {
            Err(Limited {
                retry_after_ms: bucket.wait_ms(policy.client_refill_ms, now_ms),
            })
        } else if self.global.tokens == 0 {
            Err(Limited {
                retry_after_ms: self.global.wait_ms(policy.global_refill_ms, now_ms),
            })
        } else {
            bucket.tokens -= 1;
            self.global.tokens -= 1;
            Ok(())
        };
        match result {
            Ok(()) => self.stats.allowed = self.stats.allowed.saturating_add(1),
            Err(_) => self.stats.limited = self.stats.limited.saturating_add(1),
        }
        self.check_invariants();
        result
    }

    /// The slot of `client`, making one if needed.
    fn slot(&mut self, client: ClientKey, now_ms: u64) -> usize {
        if let Some(index) = self.clients.iter().position(|(key, _)| *key == client) {
            return index;
        }
        let fresh = (client, Bucket::full(self.policy.client_burst, now_ms));
        if self.clients.len() < self.policy.client_slots {
            self.clients.push(fresh);
            return self.clients.len() - 1;
        }
        let oldest = self
            .clients
            .iter()
            .enumerate()
            .min_by_key(|(_, (_, bucket))| bucket.updated_at_ms)
            .map_or(0, |(index, _)| index);
        if let Some(slot) = self.clients.get_mut(oldest) {
            *slot = fresh;
        }
        self.stats.evicted = self.stats.evicted.saturating_add(1);
        oldest
    }

    /// The counters so far.
    #[must_use]
    pub const fn stats(&self) -> RateLimitStats {
        self.stats
    }

    /// Panics if the table is inconsistent: more clients than slots, a client
    /// twice, or a bucket over its size.
    pub fn check_invariants(&self) {
        assert!(self.clients.len() <= self.policy.client_slots);
        assert!(self.global.tokens <= self.policy.global_burst);
        for (index, (key, bucket)) in self.clients.iter().enumerate() {
            assert!(bucket.tokens <= self.policy.client_burst);
            let rest = self.clients.iter().skip(index.saturating_add(1));
            assert!(rest.into_iter().all(|(other, _)| other != key));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::net::{Ipv4Addr, Ipv6Addr};
    use proptest::prelude::*;

    fn v4(last: u8) -> ClientKey {
        ClientKey::from_ip(IpAddr::V4(Ipv4Addr::new(10, 0, 0, last)))
    }

    // RT-10: the original allowed unlimited attempts.
    #[test]
    fn a_client_gets_its_burst_then_waits() {
        let mut limiter = RateLimiter::new(RateLimitPolicy::DEFAULT, 0);
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

    #[test]
    fn the_global_bucket_caps_many_clients() {
        let mut limiter = RateLimiter::new(RateLimitPolicy::DEFAULT, 0);
        for client in 0..30 {
            assert_eq!(limiter.check(v4(client), 0), Ok(()));
        }
        assert_eq!(
            limiter.check(v4(200), 0),
            Err(Limited {
                retry_after_ms: 2_000
            })
        );
    }

    #[test]
    fn ipv6_clients_are_grouped_by_slash_64() {
        let a = Ipv6Addr::new(0x2001, 0xdb8, 0, 1, 0, 0, 0, 1);
        let b = Ipv6Addr::new(0x2001, 0xdb8, 0, 1, 0xffff, 0, 0, 2);
        assert_eq!(
            ClientKey::from_ip(IpAddr::V6(a)),
            ClientKey::from_ip(IpAddr::V6(b))
        );
        let mapped = Ipv4Addr::new(10, 0, 0, 1).to_ipv6_mapped();
        assert_eq!(ClientKey::from_ip(IpAddr::V6(mapped)), v4(1));
    }

    #[test]
    fn a_full_table_evicts_the_least_recent_client() {
        let policy = RateLimitPolicy {
            client_slots: 2,
            ..RateLimitPolicy::DEFAULT
        };
        let mut limiter = RateLimiter::new(policy, 0);
        assert_eq!(limiter.check(v4(1), 0), Ok(()));
        assert_eq!(limiter.check(v4(2), 100), Ok(()));
        assert_eq!(limiter.check(v4(3), 200), Ok(()));
        assert_eq!(limiter.stats().evicted, 1);
        assert!(limiter.clients.iter().all(|(key, _)| *key != v4(1)));
    }

    proptest! {
        #[test]
        fn stays_within_bounds(attempts in proptest::collection::vec((0_u8..40, 0_u64..5_000), 1..400)) {
            let policy = RateLimitPolicy { client_slots: 8, ..RateLimitPolicy::DEFAULT };
            let mut limiter = RateLimiter::new(policy, 0);
            let mut now = 0;
            let mut allowed = 0_u64;
            for (client, step) in attempts {
                now += step;
                if limiter.check(v4(client), now).is_ok() {
                    allowed += 1;
                }
            }
            // Never more than the global burst plus what it refilled.
            prop_assert!(allowed <= 30 + now / 2_000);
            prop_assert_eq!(limiter.stats().allowed, allowed);
        }
    }
}
