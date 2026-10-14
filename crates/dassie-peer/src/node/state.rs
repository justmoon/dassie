//! The table's upkeep: restoring it, persisting its changes, expiring
//! registrations, removing nodes nobody needs, the routes, and the
//! invariants.

use alloc::string::String;
use alloc::vec::Vec;

use dassie_identity::{NodeId, PublicKey};
use dassie_oer::Decode;

use super::PeerNode;
use crate::io::Event;
use crate::persist::{PeerBatch, Stored, StoredNode};
use crate::routing;
use crate::table::{Entry, LinkState, PeerState, Peering, Sources};
use crate::wire::SignedNodeInfo;

/// A node as the owner sees it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NodeView {
    /// Its ID.
    pub id: NodeId,
    /// Its URL, if its link state is known.
    pub url: Option<String>,
    /// Its alias.
    pub alias: Option<String>,
    /// Its link state's sequence.
    pub sequence: Option<u64>,
    /// Why it is in the table.
    pub sources: Sources,
    /// Our peering with it.
    pub peering: Option<Peering>,
    /// Its registration with us, if it expires: when.
    pub registered_until_ms: Option<u64>,
    /// Its first hops, if reachable.
    pub first_hops: Vec<NodeId>,
}

impl PeerNode {
    /// Fills the table from what was persisted. Link states were verified
    /// when received, so they are only decoded.
    pub(super) fn restore(&mut self, stored: Stored) {
        for node in stored.nodes {
            let mut entry = Entry::new(
                node.sources,
                node.first_seen_ms,
                self.config.link_state_rate,
            );
            if let Some(bytes) = &node.link_state
                && let Ok(signed) = SignedNodeInfo::decode_exact(bytes)
            {
                entry.key = PublicKey::from_bytes(&signed.info.public_key).ok();
                entry.link = Some(LinkState::from_signed(&signed));
            }
            entry.registration = node.registration;
            if let Some(peering) = node.peering {
                self.auth.restore_floor(node.id, node.replay_floor_ms);
                entry.peer = PeerState::Peered(peering);
            }
            if entry.sources.is_empty() {
                entry.orphaned_ms = Some(self.now_ms);
            }
            if self.table.len() < self.config.max_nodes && node.id != self.own {
                self.table.insert(node.id, entry);
            }
        }
        self.ensure(self.own, Sources::SELF);
        if let Some(entry) = self.table.get_mut(&self.own) {
            entry.key = Some(self.auth.node_key().public_key());
        }
        let bootstrap: Vec<(NodeId, PublicKey)> = self
            .bootstrap
            .iter()
            .map(|node| (node.id, node.key))
            .collect();
        for (id, key) in bootstrap {
            self.ensure(id, Sources::BOOTSTRAP);
            if let Some(entry) = self.table.get_mut(&id) {
                entry.key = Some(key);
            }
        }
        self.dirty.clear();
        self.gossip.routes_dirty = true;
    }

    /// What changed since the last call, for the same transaction as the
    /// ledger's.
    pub fn take_changes(&mut self, batch: &mut PeerBatch) {
        for id in core::mem::take(&mut self.dirty) {
            if id == self.own {
                continue;
            }
            let Some(entry) = self.table.get(&id) else {
                continue;
            };
            let peering = entry.peering().cloned();
            batch.nodes.push(StoredNode {
                id,
                link_state: entry.link.as_ref().map(|link| link.bytes.clone()),
                sources: entry.sources,
                first_seen_ms: entry.first_seen_ms,
                registration: entry.registration,
                replay_floor_ms: if peering.is_some() {
                    self.auth.newest(&id).unwrap_or(0)
                } else {
                    0
                },
                peering,
            });
        }
        batch.removed.append(&mut self.removed);
        if self.gossip.sequence_dirty {
            self.gossip.sequence_dirty = false;
            batch.own_sequence = Some(self.gossip.own_sequence);
        }
    }

    /// The peering with `node`, if any.
    #[must_use]
    pub fn peering_of(&self, node: &NodeId) -> Option<&Peering> {
        self.table.get(node).and_then(Entry::peering)
    }

    /// The nodes we are peered with.
    pub fn peers(&self) -> impl Iterator<Item = (&NodeId, &Peering)> {
        self.table
            .iter()
            .filter_map(|(id, entry)| entry.peering().map(|peering| (id, peering)))
    }

    /// Notes an authenticated message from a peer (NET-12): it is alive,
    /// and the newest timestamp accepted from it is persisted.
    pub(crate) fn saw(&mut self, node: &NodeId) {
        let now = self.now_ms;
        if let Some(entry) = self.table.get_mut(node)
            && let PeerState::Peered(peering) = &mut entry.peer
        {
            peering.last_seen_ms = now;
            self.dirty.insert(*node);
        }
    }

    /// Every node, as the owner sees it.
    #[must_use]
    pub fn nodes(&self) -> Vec<NodeView> {
        self.table
            .iter()
            .map(|(id, entry)| NodeView {
                id: *id,
                url: entry.link.as_ref().map(|link| link.url.clone()),
                alias: entry.link.as_ref().map(|link| link.alias.clone()),
                sequence: entry.link.as_ref().map(|link| link.sequence),
                sources: entry.sources,
                peering: entry.peering().cloned(),
                registered_until_ms: entry.registration.map(|r| r.expires_ms),
                first_hops: self.routes.get(id).cloned().unwrap_or_default(),
            })
            .collect()
    }

    /// Expires registrations and removes nodes nobody needs (NET-18).
    pub(super) fn maintain(&mut self) {
        let now = self.now_ms;
        let expired: Vec<NodeId> = self
            .table
            .iter()
            .filter(|(_, entry)| entry.registration.is_some_and(|r| r.expires_ms <= now))
            .map(|(id, _)| *id)
            .collect();
        for id in expired {
            if let Some(entry) = self.table.get_mut(&id) {
                entry.registration = None;
            }
            self.drop_source(&id, Sources::REGISTERED);
            self.lists.served_changed = true;
        }
        let grace = self.config.removal_grace_ms;
        let gone: Vec<NodeId> = self
            .table
            .iter()
            .filter(|(_, entry)| {
                entry.sources.is_empty()
                    && entry
                        .orphaned_ms
                        .is_some_and(|since| since.saturating_add(grace) <= now)
            })
            .map(|(id, _)| *id)
            .collect();
        for id in gone {
            self.remove(&id);
        }
    }

    /// When [`maintain`](Self::maintain) next has something to do.
    pub(super) fn next_maintenance(&self) -> Option<u64> {
        let grace = self.config.removal_grace_ms;
        self.table
            .values()
            .filter_map(|entry| {
                let expiry = entry.registration.map(|r| r.expires_ms);
                let removal = entry
                    .orphaned_ms
                    .filter(|_| entry.sources.is_empty())
                    .map(|since| since.saturating_add(grace));
                match (expiry, removal) {
                    (Some(a), Some(b)) => Some(a.min(b)),
                    (a, b) => a.or(b),
                }
            })
            .min()
    }

    /// Recomputes the routes when the graph may have changed, at most once
    /// per debounce period, and at least once per refresh of staleness.
    pub(super) fn update_routes(&mut self) {
        if !self.gossip.routes_due(self.now_ms) {
            return;
        }
        self.gossip.routes_computed(self.now_ms, &self.config);
        let (now, stale, future) = (self.now_ms, self.config.stale_ms, self.config.max_future_ms);
        let table = &self.table;
        let routes = routing::compute(&self.own, |node| {
            let link = table.get(node)?.link.as_ref()?;
            link.is_fresh(now, stale, future)
                .then_some(link.neighbors.as_slice())
        });
        if routes != self.routes {
            self.routes = routes;
            self.event(Event::RoutesChanged);
        }
        for (id, entry) in &mut self.table {
            let wanted = entry.sources.contains(Sources::MAJORITY)
                && *id != self.own
                && entry
                    .link
                    .as_ref()
                    .is_some_and(|l| l.is_fresh(now, stale, future));
            if wanted && !self.routes.contains_key(id) {
                entry.unreachable_ms.get_or_insert(now);
            } else {
                entry.unreachable_ms = None;
            }
        }
    }

    /// Checks the node's invariants (D16): bounds, ourselves in the table,
    /// and peers that are what the table says.
    ///
    /// # Panics
    ///
    /// If one does not hold.
    pub fn check_invariants(&self) {
        self.auth.check_invariants();
        assert!(
            self.table.len() <= self.config.max_nodes,
            "the table within bounds"
        );
        assert!(
            self.requests.len() <= self.config.max_requests,
            "requests within bounds"
        );
        assert_eq!(
            self.requests.len(),
            self.deadlines.len(),
            "a deadline per request"
        );
        assert!(
            self.deferred <= self.config.max_deferred,
            "deferred within bounds"
        );
        let own = self.table.get(&self.own);
        assert!(
            own.is_some_and(|entry| entry.sources.contains(Sources::SELF)),
            "we are in our table"
        );
        let peers = self.peers().count();
        assert!(peers <= self.config.max_peers, "peers within bounds");
        for (id, entry) in &self.table {
            assert!(
                entry.peering().is_none() || entry.sources.contains(Sources::PEER),
                "a peer has the peer source"
            );
            assert!(
                *id == self.own || entry.peering().is_none() || entry.key.is_some(),
                "a peer's key is known"
            );
            assert!(
                !self.routes.contains_key(&self.own),
                "no route to ourselves"
            );
        }
        for hops in self.routes.values() {
            assert!(
                !hops.is_empty() && hops.len() <= routing::MAX_FIRST_HOPS,
                "routes have first hops"
            );
        }
    }
}
