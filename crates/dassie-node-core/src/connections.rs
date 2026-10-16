//! A bounded table of open connections, per client.
//!
//! A pool of connections shared by everyone is easy to fill: one client
//! that opens the whole pool and keeps it open locks everyone else out,
//! the owner included. So each client (an IPv4 address, or an IPv6 /56, as
//! for [`rate_limit`](crate::rate_limit)) gets at most a share of the pool,
//! and the pool still bounds the total.
//!
//! The table needs no eviction: every entry holds at least one open
//! connection, so it never has more entries than the pool has places.
//! Connections that are not counted per client (loopback, in dev
//! environments) take a place in the pool only.

use alloc::vec::Vec;

use crate::rate_limit::ClientKey;

/// How many connections may be open.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ConnectionLimits {
    /// Connections open at once, in all.
    pub max_total: usize,
    /// Connections open at once from one client.
    pub max_per_client: u32,
}

/// Why a connection was refused.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Refused {
    /// The pool is full.
    Full,
    /// The client has its share open already.
    ClientFull,
}

/// What the table has done, for the node's counters.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ConnectionStats {
    /// Connections opened.
    pub opened: u64,
    /// Connections refused because the pool was full.
    pub refused_full: u64,
    /// Connections refused because their client had its share.
    pub refused_client: u64,
}

/// The table.
#[derive(Clone, Debug)]
pub struct ConnectionTable {
    limits: ConnectionLimits,
    /// Sorted by key, so a client is found by binary search; each count is
    /// in `1..=max_per_client`.
    clients: Vec<(ClientKey, u32)>,
    /// Connections open, counted per client or not.
    open: usize,
    stats: ConnectionStats,
}

impl ConnectionTable {
    /// An empty table, with all its memory allocated now.
    #[must_use]
    pub fn new(limits: ConnectionLimits) -> Self {
        assert!(limits.max_total > 0 && limits.max_per_client > 0);
        Self {
            limits,
            clients: Vec::with_capacity(limits.max_total),
            open: 0,
            stats: ConnectionStats::default(),
        }
    }

    /// Takes a place for a connection from `client`, or says why there is
    /// none. `None` is a connection counted in the total only.
    pub fn open(&mut self, client: Option<ClientKey>) -> Result<(), Refused> {
        let result = self.place(client);
        match result {
            Ok(()) => {
                self.open += 1;
                self.stats.opened = self.stats.opened.saturating_add(1);
            }
            Err(Refused::Full) => {
                self.stats.refused_full = self.stats.refused_full.saturating_add(1);
            }
            Err(Refused::ClientFull) => {
                self.stats.refused_client = self.stats.refused_client.saturating_add(1);
            }
        }
        self.check_invariants();
        result
    }

    /// Counts the connection in its client's entry, if there is room.
    fn place(&mut self, client: Option<ClientKey>) -> Result<(), Refused> {
        if self.open >= self.limits.max_total {
            return Err(Refused::Full);
        }
        let Some(client) = client else {
            return Ok(());
        };
        match self.find(client) {
            Ok(index) => match self.clients.get_mut(index) {
                Some((_, count)) if *count < self.limits.max_per_client => {
                    *count += 1;
                    Ok(())
                }
                _ => Err(Refused::ClientFull),
            },
            Err(index) => {
                self.clients.insert(index, (client, 1));
                Ok(())
            }
        }
    }

    /// Returns the place of a connection from `client`.
    ///
    /// Panics if `client` has no connection open: only a successful
    /// [`open`](Self::open) makes one, so that is a broken invariant.
    pub fn close(&mut self, client: Option<ClientKey>) {
        assert!(self.open > 0, "a connection is open");
        if let Some(client) = client {
            let index = self.find(client).ok();
            assert!(index.is_some(), "the client has a connection open");
            let entry = index.and_then(|index| Some((index, self.clients.get_mut(index)?)));
            if let Some((index, (_, count))) = entry {
                *count -= 1;
                if *count == 0 {
                    self.clients.remove(index);
                }
            }
        }
        self.open -= 1;
        self.check_invariants();
    }

    /// Where `client` is, or where it would go.
    fn find(&self, client: ClientKey) -> Result<usize, usize> {
        self.clients.binary_search_by_key(&client, |(key, _)| *key)
    }

    /// Connections open, in all.
    #[must_use]
    pub const fn open_count(&self) -> usize {
        self.open
    }

    /// Connections open from `client`.
    #[must_use]
    pub fn client_count(&self, client: ClientKey) -> u32 {
        self.find(client)
            .ok()
            .and_then(|index| self.clients.get(index))
            .map_or(0, |(_, count)| *count)
    }

    /// The counters so far.
    #[must_use]
    pub const fn stats(&self) -> ConnectionStats {
        self.stats
    }

    /// Panics if the table is inconsistent: a client twice or out of order,
    /// a count outside its range, or more connections than the pool holds.
    /// One pass, so it runs after every change.
    pub fn check_invariants(&self) {
        assert!(self.open <= self.limits.max_total);
        assert!(self.clients.len() <= self.limits.max_total);
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
        assert!(counted.is_some_and(|counted| counted <= self.open));
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::indexing_slicing)]

    use super::*;
    use core::net::{IpAddr, Ipv4Addr};
    use proptest::prelude::*;
    use std::collections::BTreeMap;

    fn v4(last: u8) -> ClientKey {
        ClientKey::from_ip(IpAddr::V4(Ipv4Addr::new(10, 0, 0, last)))
    }

    const LIMITS: ConnectionLimits = ConnectionLimits {
        max_total: 8,
        max_per_client: 2,
    };

    #[test]
    fn a_client_gets_its_share_then_is_refused() {
        let mut table = ConnectionTable::new(LIMITS);
        assert_eq!(table.open(Some(v4(1))), Ok(()));
        assert_eq!(table.open(Some(v4(1))), Ok(()));
        assert_eq!(table.open(Some(v4(1))), Err(Refused::ClientFull));
        assert_eq!(table.open(Some(v4(2))), Ok(()));
        assert_eq!(table.client_count(v4(1)), 2);
        assert_eq!(table.open_count(), 3);
        assert_eq!(
            table.stats(),
            ConnectionStats {
                opened: 3,
                refused_full: 0,
                refused_client: 1
            }
        );
    }

    // However the pool is shared out, it never holds more than its places.
    #[test]
    fn the_total_bounds_everyone() {
        let mut table = ConnectionTable::new(LIMITS);
        for client in 1..=3 {
            assert_eq!(table.open(Some(v4(client))), Ok(()));
            assert_eq!(table.open(Some(v4(client))), Ok(()));
        }
        assert_eq!(table.open(None), Ok(()));
        assert_eq!(table.open(None), Ok(()));
        assert_eq!(table.open(Some(v4(4))), Err(Refused::Full));
        assert_eq!(table.open(None), Err(Refused::Full));
        assert_eq!(table.stats().refused_full, 2);
    }

    #[test]
    fn closing_frees_the_place() {
        let mut table = ConnectionTable::new(LIMITS);
        table.open(Some(v4(1))).unwrap();
        table.open(Some(v4(1))).unwrap();
        table.close(Some(v4(1)));
        assert_eq!(table.client_count(v4(1)), 1);
        assert_eq!(table.open(Some(v4(1))), Ok(()));
        table.close(Some(v4(1)));
        table.close(Some(v4(1)));
        assert_eq!(table.client_count(v4(1)), 0);
        assert!(table.clients.is_empty());
        assert_eq!(table.open_count(), 0);
    }

    #[test]
    fn uncounted_clients_use_only_the_total() {
        let mut table = ConnectionTable::new(LIMITS);
        for _ in 0..LIMITS.max_total {
            assert_eq!(table.open(None), Ok(()));
        }
        assert!(table.clients.is_empty());
        assert_eq!(table.open(None), Err(Refused::Full));
        table.close(None);
        assert_eq!(table.open(Some(v4(1))), Ok(()));
    }

    #[test]
    #[should_panic(expected = "the client has a connection open")]
    fn closing_what_was_never_opened_is_a_bug() {
        let mut table = ConnectionTable::new(LIMITS);
        table.open(Some(v4(1))).unwrap();
        table.close(Some(v4(2)));
    }

    proptest! {
        // Random opens and closes over 40 clients, and uncounted ones,
        // agree with a plain model and keep the table consistent.
        #[test]
        fn matches_a_model(steps in proptest::collection::vec((0_u8..41, any::<bool>()), 1..400)) {
            let limits = ConnectionLimits { max_total: 16, max_per_client: 3 };
            let mut table = ConnectionTable::new(limits);
            let mut model: BTreeMap<Option<u8>, u32> = BTreeMap::new();
            for (client, opening) in steps {
                let id = (client < 40).then_some(client);
                let key = id.map(v4);
                let open: u32 = model.values().sum();
                let held = model.get(&id).copied().unwrap_or(0);
                if opening {
                    let expected = if usize::try_from(open).unwrap() >= limits.max_total {
                        Err(Refused::Full)
                    } else if id.is_some() && held >= limits.max_per_client {
                        Err(Refused::ClientFull)
                    } else {
                        Ok(())
                    };
                    prop_assert_eq!(table.open(key), expected);
                    if expected.is_ok() {
                        *model.entry(id).or_default() += 1;
                    }
                } else if held > 0 {
                    table.close(key);
                    model.insert(id, held - 1);
                }
                table.check_invariants();
                let open: u32 = model.values().sum();
                prop_assert_eq!(table.open_count(), usize::try_from(open).unwrap());
                if let Some(id) = id {
                    prop_assert_eq!(table.client_count(v4(id)), model.get(&Some(id)).copied().unwrap_or(0));
                }
            }
        }
    }
}
