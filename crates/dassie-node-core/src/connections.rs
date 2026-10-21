//! Bounded tables of open connections, per client and per node.
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
//!
//! Connections from nodes that proved their key are counted per node
//! instead, in a [`NodeTable`]. A node cannot pass for another without its
//! key, so its share is its own: one that opens more than its share
//! replaces its own oldest connection rather than being refused, since the
//! one it opened last is the one it uses, and the oldest is the likeliest
//! to be gone without our knowing yet.

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

/// How many connections a [`NodeTable`] holds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NodeLimits {
    /// Nodes it has room for, each with its full share.
    pub max_nodes: usize,
    /// Connections open at once from one node.
    pub max_per_node: usize,
}

impl NodeLimits {
    /// Connections open at once, in all.
    #[must_use]
    pub const fn max_total(&self) -> usize {
        self.max_nodes.saturating_mul(self.max_per_node)
    }
}

/// A place a [`NodeTable`] gave: its number, and the number of the node's
/// oldest place, which it took over when the node had its share.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Taken {
    /// The place's number, never given twice.
    pub place: u64,
    /// The place it took over, whose connection is to close.
    pub replaced: Option<u64>,
}

/// A bounded table of open connections, per node `K`: each node holds at
/// most its share, and all of them together at most the table's places.
#[derive(Clone, Debug)]
pub struct NodeTable<K> {
    limits: NodeLimits,
    /// The places held, sorted by node, then by number: since numbers only
    /// grow, a node's oldest place comes first among its own.
    places: Vec<(K, u64)>,
    /// The next place's number.
    next: u64,
}

impl<K: Ord + Copy> NodeTable<K> {
    /// An empty table, with all its memory allocated now. A table for no
    /// nodes refuses every connection.
    #[must_use]
    pub fn new(limits: NodeLimits) -> Self {
        assert!(limits.max_per_node > 0);
        Self {
            limits,
            places: Vec::with_capacity(limits.max_total()),
            next: 0,
        }
    }

    /// Takes a place for a connection from `node`. One that holds its share
    /// already gives up its oldest place for it; otherwise it is refused
    /// when the table is full.
    ///
    /// # Errors
    ///
    /// [`Refused::Full`]: the table is full, and `node` holds less than its
    /// share.
    pub fn take(&mut self, node: K) -> Result<Taken, Refused> {
        let (first, held) = self.run(node);
        let place = self.next;
        let replaced = if held >= self.limits.max_per_node {
            assert!(
                held == self.limits.max_per_node,
                "a node holds at most its share"
            );
            let (_, oldest) = self.places.remove(first);
            self.places.insert(first + held - 1, (node, place));
            Some(oldest)
        } else if self.places.len() >= self.limits.max_total() {
            return Err(Refused::Full);
        } else {
            self.places.insert(first + held, (node, place));
            None
        };
        self.next += 1;
        self.check_invariants();
        Ok(Taken { place, replaced })
    }

    /// Gives back `node`'s place `place`. Returns whether it held it: not
    /// once another of its connections took it over.
    pub fn release(&mut self, node: K, place: u64) -> bool {
        let found = self.places.binary_search(&(node, place));
        if let Ok(index) = found {
            self.places.remove(index);
        }
        self.check_invariants();
        found.is_ok()
    }

    /// Where `node`'s places start, or would, and how many it holds.
    fn run(&self, node: K) -> (usize, usize) {
        let first = self.places.partition_point(|(key, _)| *key < node);
        let end = self.places.partition_point(|(key, _)| *key <= node);
        (first, end - first)
    }

    /// Connections open, in all.
    #[must_use]
    pub fn open_count(&self) -> usize {
        self.places.len()
    }

    /// Connections open from `node`.
    #[must_use]
    pub fn node_count(&self, node: K) -> usize {
        self.run(node).1
    }

    /// Panics if the table is inconsistent: places out of order or given
    /// twice, a number not given yet, a node past its share, or more
    /// places than the table has. One pass, so it runs after every change.
    pub fn check_invariants(&self) {
        assert!(self.places.len() <= self.limits.max_total());
        let sorted = self.places.windows(2).all(|pair| match pair {
            [a, b] => a < b,
            _ => true,
        });
        assert!(sorted, "places are sorted, each once");
        assert!(self.places.iter().all(|(_, place)| *place < self.next));
        let mut run = 0_usize;
        let mut last: Option<K> = None;
        for (node, _) in &self.places {
            run = if last == Some(*node) { run + 1 } else { 1 };
            last = Some(*node);
            assert!(
                run <= self.limits.max_per_node,
                "a node holds at most its share"
            );
        }
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

    const NODES: NodeLimits = NodeLimits {
        max_nodes: 3,
        max_per_node: 2,
    };

    #[test]
    fn a_node_past_its_share_replaces_its_oldest() {
        let mut table = NodeTable::new(NODES);
        let first = table.take(1_u8).unwrap();
        let second = table.take(1).unwrap();
        assert_eq!((first.replaced, second.replaced), (None, None));
        let third = table.take(1).unwrap();
        assert_eq!(third.replaced, Some(first.place));
        assert_eq!(table.node_count(1), 2);
        // The place taken over is gone: giving it back finds nothing.
        assert!(!table.release(1, first.place));
        assert!(table.release(1, second.place));
        assert_eq!(table.take(1).unwrap().replaced, None);
    }

    // A full table refuses a node below its share, and still lets one at
    // its share replace its own: no node's place goes to another.
    #[test]
    fn a_full_table_refuses_newcomers_only() {
        let mut table = NodeTable::new(NODES);
        for node in 1..=3_u8 {
            table.take(node).unwrap();
            table.take(node).unwrap();
        }
        assert_eq!(table.open_count(), NODES.max_total());
        assert_eq!(table.take(4), Err(Refused::Full));
        let taken = table.take(2).unwrap();
        assert!(taken.replaced.is_some());
        assert_eq!(table.node_count(1), 2);
        assert_eq!(table.node_count(3), 2);
    }

    #[test]
    fn a_table_for_no_nodes_refuses_everyone() {
        let mut table = NodeTable::new(NodeLimits {
            max_nodes: 0,
            max_per_node: 2,
        });
        assert_eq!(table.take(1_u8), Err(Refused::Full));
        assert_eq!(table.open_count(), 0);
    }

    proptest! {
        // Random takes and releases over 8 nodes agree with a plain model,
        // each node's places from oldest to newest, and keep the table
        // consistent.
        #[test]
        fn nodes_match_a_model(steps in proptest::collection::vec((0_u8..8, any::<bool>(), any::<u8>()), 1..400)) {
            let limits = NodeLimits { max_nodes: 4, max_per_node: 2 };
            let mut table = NodeTable::new(limits);
            let mut model: BTreeMap<u8, Vec<u64>> = BTreeMap::new();
            for (node, taking, pick) in steps {
                let held = model.get(&node).cloned().unwrap_or_default();
                if taking {
                    let open: usize = model.values().map(Vec::len).sum();
                    let result = table.take(node);
                    if held.len() < limits.max_per_node && open >= limits.max_total() {
                        prop_assert_eq!(result, Err(Refused::Full));
                    } else {
                        let taken = result.unwrap();
                        let places = model.entry(node).or_default();
                        let oldest = (places.len() == limits.max_per_node).then(|| places.remove(0));
                        prop_assert_eq!(taken.replaced, oldest);
                        places.push(taken.place);
                    }
                } else if !held.is_empty() {
                    let place = held[usize::from(pick) % held.len()];
                    prop_assert!(table.release(node, place));
                    prop_assert!(!table.release(node, place));
                    model.entry(node).or_default().retain(|kept| *kept != place);
                }
                table.check_invariants();
                let open: usize = model.values().map(Vec::len).sum();
                prop_assert_eq!(table.open_count(), open);
                prop_assert_eq!(table.node_count(node), model.get(&node).map_or(0, Vec::len));
            }
        }

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
