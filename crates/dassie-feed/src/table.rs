//! The subscriber table (spec/areas/rate-feed.md#subscribers).
//!
//! Who gets the feed: proven addresses, each until its entry expires
//! unless a Subscribe with a valid cookie renews it. The table is bounded
//! in all and per client (an IPv4 address, or an IPv6 /56, as the node's
//! other per-client limits), so one host cannot take every place. A full
//! table refuses newcomers rather than evicting anyone: an entry is only
//! ever a proven address that asked within its lifetime, and evicting one
//! for a newcomer would let a party with many addresses push the others
//! out. It is kept in memory only; after a restart, subscribers are
//! challenged and come back.
//!
//! Every answer is a [`Decision`] the caller carries out; nothing a
//! datagram says can make the table panic or grow past its bounds.

use alloc::vec::Vec;
use core::net::{IpAddr, SocketAddr, SocketAddrV6};

use dassie_kit::ClientKey;
use dassie_kit::MINUTE_MS;

use crate::cookie::{Cookie, CookieCheck, CookieSecret};
use crate::wire::{self, Message, WireError};

/// Most subscribers a node sends its feed to.
pub const MAX_SUBSCRIBERS: usize = 1024;

/// Most subscribers from one client: an IPv4 address or an IPv6 /56.
pub const MAX_SUBSCRIBERS_PER_CLIENT: u32 = 4;

/// How long an entry lasts without a renewal.
pub const SUBSCRIPTION_TTL_MS: u64 = 5 * MINUTE_MS;

/// The table's bounds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Limits {
    /// Subscribers in all.
    pub max_subscribers: usize,
    /// Subscribers from one client.
    pub max_per_client: u32,
    /// How long an entry lasts without a renewal.
    pub ttl_ms: u64,
}

impl Limits {
    /// The node's.
    pub const DEFAULT: Self = Self {
        max_subscribers: MAX_SUBSCRIBERS,
        max_per_client: MAX_SUBSCRIBERS_PER_CLIENT,
        ttl_ms: SUBSCRIPTION_TTL_MS,
    };
}

/// What to do about a datagram.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Decision {
    /// The address is not proven: send it a Challenge with this cookie,
    /// which is no larger than the Subscribe it answers.
    Challenge(Cookie),
    /// The address is subscribed until its entry expires. `new` says it was
    /// not before, so the caller may send it the latest feed at once.
    /// `refresh` is a newer cookie to send it in a Challenge, when the one
    /// it sent is from the epoch before, so that its next renewal is taken.
    Subscribed {
        /// Whether the address was not subscribed before.
        new: bool,
        /// A newer cookie to hand out.
        refresh: Option<Cookie>,
    },
    /// The address is proven, but there is no room for it. Nothing is sent;
    /// the subscriber tries again at its next renewal.
    Refused(Refused),
    /// The datagram is not a Subscribe, or is broken. Nothing is sent.
    Ignored(Ignored),
}

/// Why a proven address was not subscribed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Refused {
    /// The table is full.
    Full,
    /// Its client has its share.
    ClientFull,
}

/// Why a datagram got no answer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Ignored {
    /// It does not decode.
    Malformed(WireError),
    /// It is a message only a node sends.
    NotSubscribe,
    /// It comes from an address a feed cannot go to: port 0, or an IP that
    /// is unspecified, multicast or broadcast.
    NotUnicast,
}

/// What the table has done, for the node's counters.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TableStats {
    /// Challenges asked for.
    pub challenged: u64,
    /// Addresses subscribed.
    pub subscribed: u64,
    /// Entries renewed.
    pub renewed: u64,
    /// Proven addresses refused because the table was full.
    pub refused_full: u64,
    /// Proven addresses refused because their client had its share.
    pub refused_client: u64,
    /// Datagrams ignored.
    pub ignored: u64,
    /// Entries that expired.
    pub expired: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Entry {
    address: SocketAddr,
    client: ClientKey,
    expires_at_ms: u64,
}

/// The table.
#[derive(Debug)]
pub struct SubscriberTable {
    limits: Limits,
    secret: CookieSecret,
    /// Sorted by address, each once.
    entries: Vec<Entry>,
    /// How many entries each client has, sorted by client; each count is in
    /// `1..=max_per_client`.
    clients: Vec<(ClientKey, u32)>,
    stats: TableStats,
}

impl SubscriberTable {
    /// An empty table, with all its memory allocated now, making cookies
    /// under `secret`.
    #[must_use]
    pub fn new(limits: Limits, secret: CookieSecret) -> Self {
        assert!(limits.max_subscribers > 0 && limits.max_per_client > 0 && limits.ttl_ms > 0);
        Self {
            limits,
            secret,
            entries: Vec::with_capacity(limits.max_subscribers),
            clients: Vec::with_capacity(limits.max_subscribers),
            stats: TableStats::default(),
        }
    }

    /// What to do about `datagram`, which came from `from` at `now_ms`.
    pub fn handle(&mut self, from: SocketAddr, datagram: &[u8], now_ms: u64) -> Decision {
        let ignored = match wire::decode(datagram) {
            Ok(Message::Subscribe(cookie)) => return self.subscribe(from, cookie.as_ref(), now_ms),
            Ok(Message::Challenge(_) | Message::Feed(_)) => Ignored::NotSubscribe,
            Err(error) => Ignored::Malformed(error),
        };
        self.stats.ignored = self.stats.ignored.saturating_add(1);
        Decision::Ignored(ignored)
    }

    /// What to do about a Subscribe from `from`, with `cookie`, at `now_ms`.
    pub fn subscribe(
        &mut self,
        from: SocketAddr,
        cookie: Option<&Cookie>,
        now_ms: u64,
    ) -> Decision {
        if !is_unicast(from) {
            self.stats.ignored = self.stats.ignored.saturating_add(1);
            return Decision::Ignored(Ignored::NotUnicast);
        }
        let from = normalize(from);
        let check = cookie.map_or(CookieCheck::Invalid, |cookie| {
            self.secret.check(from, cookie, now_ms)
        });
        let refresh = match check {
            CookieCheck::Invalid => {
                self.stats.challenged = self.stats.challenged.saturating_add(1);
                return Decision::Challenge(self.secret.issue(from, now_ms));
            }
            CookieCheck::Current => None,
            CookieCheck::Previous => Some(self.secret.issue(from, now_ms)),
        };
        let expires_at_ms = now_ms.saturating_add(self.limits.ttl_ms);
        if let Ok(index) = self.find(from)
            && let Some(entry) = self.entries.get_mut(index)
        {
            // An entry that expired but was not dropped yet got no feeds
            // since, so it comes back as new, and gets the latest at once.
            // Only its expiry changes, which no invariant covers, so a
            // renewal costs a lookup and a MAC, however full the table.
            let new = entry.expires_at_ms <= now_ms;
            entry.expires_at_ms = entry.expires_at_ms.max(expires_at_ms);
            if new {
                self.stats.subscribed = self.stats.subscribed.saturating_add(1);
            } else {
                self.stats.renewed = self.stats.renewed.saturating_add(1);
            }
            return Decision::Subscribed { new, refresh };
        }
        let decision = match self.admit(from, expires_at_ms, now_ms) {
            Ok(()) => {
                self.stats.subscribed = self.stats.subscribed.saturating_add(1);
                Decision::Subscribed { new: true, refresh }
            }
            Err(Refused::Full) => {
                self.stats.refused_full = self.stats.refused_full.saturating_add(1);
                Decision::Refused(Refused::Full)
            }
            Err(Refused::ClientFull) => {
                self.stats.refused_client = self.stats.refused_client.saturating_add(1);
                Decision::Refused(Refused::ClientFull)
            }
        };
        self.check_invariants();
        decision
    }

    /// Adds a proven address that has no entry, if there is room, after
    /// dropping the entries that expired when there is not.
    fn admit(
        &mut self,
        address: SocketAddr,
        expires_at_ms: u64,
        now_ms: u64,
    ) -> Result<(), Refused> {
        let client = ClientKey::from_ip(address.ip());
        if self.room(client).is_err() {
            self.expire(now_ms);
        }
        self.room(client)?;
        let Err(index) = self.find(address) else {
            return Ok(());
        };
        self.entries.insert(
            index,
            Entry {
                address,
                client,
                expires_at_ms,
            },
        );
        match self.find_client(client) {
            Ok(index) => {
                if let Some((_, count)) = self.clients.get_mut(index) {
                    *count += 1;
                }
            }
            Err(index) => self.clients.insert(index, (client, 1)),
        }
        Ok(())
    }

    /// Whether there is room for another entry of `client`.
    fn room(&self, client: ClientKey) -> Result<(), Refused> {
        if self.entries.len() >= self.limits.max_subscribers {
            return Err(Refused::Full);
        }
        if self.client_count(client) >= self.limits.max_per_client {
            return Err(Refused::ClientFull);
        }
        Ok(())
    }

    /// Drops the entries that expired at `now_ms` or before, and returns
    /// how many.
    pub fn expire(&mut self, now_ms: u64) -> usize {
        let before = self.entries.len();
        let clients = &mut self.clients;
        self.entries.retain(|entry| {
            if entry.expires_at_ms > now_ms {
                return true;
            }
            let found = clients.binary_search_by_key(&entry.client, |(key, _)| *key);
            assert!(found.is_ok(), "every entry's client is counted");
            if let Ok(index) = found
                && let Some((_, count)) = clients.get_mut(index)
            {
                *count -= 1;
                if *count == 0 {
                    clients.remove(index);
                }
            }
            false
        });
        let expired = before - self.entries.len();
        if expired > 0 {
            self.stats.expired = self.stats.expired.saturating_add(expired as u64);
            self.check_invariants();
        }
        expired
    }

    /// When the next entry expires: the deadline for [`expire`](Self::expire).
    #[must_use]
    pub fn next_expiry_ms(&self) -> Option<u64> {
        self.entries.iter().map(|entry| entry.expires_at_ms).min()
    }

    /// The addresses to send a feed to at `now_ms`, in order.
    pub fn recipients(&self, now_ms: u64) -> impl Iterator<Item = SocketAddr> + '_ {
        self.entries
            .iter()
            .filter(move |entry| entry.expires_at_ms > now_ms)
            .map(|entry| entry.address)
    }

    /// Whether `address` is subscribed at `now_ms`.
    #[must_use]
    pub fn contains(&self, address: SocketAddr, now_ms: u64) -> bool {
        self.find(normalize(address))
            .ok()
            .and_then(|index| self.entries.get(index))
            .is_some_and(|entry| entry.expires_at_ms > now_ms)
    }

    /// Entries in the table, expired ones not yet dropped included.
    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether the table has no entries.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Entries of `client`.
    #[must_use]
    pub fn client_count(&self, client: ClientKey) -> u32 {
        self.find_client(client)
            .ok()
            .and_then(|index| self.clients.get(index))
            .map_or(0, |(_, count)| *count)
    }

    /// The counters so far.
    #[must_use]
    pub const fn stats(&self) -> TableStats {
        self.stats
    }

    fn find(&self, address: SocketAddr) -> Result<usize, usize> {
        self.entries
            .binary_search_by_key(&address, |entry| entry.address)
    }

    fn find_client(&self, client: ClientKey) -> Result<usize, usize> {
        self.clients.binary_search_by_key(&client, |(key, _)| *key)
    }

    /// Panics if the table is inconsistent: more entries than it holds, an
    /// address twice or out of order, an entry whose client is not its
    /// address's or is not counted, a count out of its range, or counts
    /// that do not add up to the entries. Runs after every change to the
    /// entries or the counts.
    pub fn check_invariants(&self) {
        assert!(self.entries.len() <= self.limits.max_subscribers);
        let sorted = self.entries.windows(2).all(|pair| match pair {
            [a, b] => a.address < b.address,
            _ => true,
        });
        assert!(sorted, "entries are sorted, each address once");
        assert!(self.entries.iter().all(|entry| {
            entry.address == normalize(entry.address)
                && entry.client == ClientKey::from_ip(entry.address.ip())
                && self.find_client(entry.client).is_ok()
        }));
        let sorted = self.clients.windows(2).all(|pair| match pair {
            [(a, _), (b, _)] => a < b,
            _ => true,
        });
        assert!(sorted, "clients are sorted, each once");
        let max = self.limits.max_per_client;
        assert!(
            self.clients
                .iter()
                .all(|(_, count)| (1..=max).contains(count))
        );
        let counted = self.clients.iter().try_fold(0_usize, |sum, (_, count)| {
            usize::try_from(*count)
                .ok()
                .and_then(|count| sum.checked_add(count))
        });
        assert!(
            counted == Some(self.entries.len()),
            "counts add up to the entries"
        );
    }
}

/// The address a feed goes to: as received, without an IPv6 flow label,
/// which the sender picks and which would otherwise make one address
/// several entries. The scope stays: the receiving host sets it, and a
/// link-local address cannot be sent to without it.
fn normalize(address: SocketAddr) -> SocketAddr {
    match address {
        SocketAddr::V4(_) => address,
        SocketAddr::V6(v6) => {
            SocketAddr::V6(SocketAddrV6::new(*v6.ip(), v6.port(), 0, v6.scope_id()))
        }
    }
}

/// Whether a feed can go to `address`: a port, and an IP that is one host's.
/// A source that is not one is forged or broken, and a feed sent there
/// would go to no one, or to many.
fn is_unicast(address: SocketAddr) -> bool {
    let ip = address.ip().to_canonical();
    let one_host = match ip {
        IpAddr::V4(v4) => !v4.is_broadcast(),
        IpAddr::V6(_) => true,
    };
    address.port() != 0 && !ip.is_unspecified() && !ip.is_multicast() && one_host
}
