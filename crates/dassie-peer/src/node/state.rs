//! The table's upkeep: restoring it, persisting its changes, expiring
//! registrations, removing nodes nobody needs, the routes, and the
//! invariants.

use alloc::string::String;
use alloc::vec::Vec;

use dassie_identity::{NodeId, PublicKey};
use dassie_oer::Decode;

use super::PeerNode;
use crate::channel::Retry;
use crate::io::Event;
use crate::persist::{PeerBatch, Stored, StoredChannel, StoredNode};
use crate::routing;
use crate::table::{Advertise, Entry, LinkState, PeerState, Peering, Sources, Termination};
use crate::wire::{SchemeId, SignedNodeInfo};

/// How far our peering with a node has come.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PeerStatus {
    /// Not peered.
    None,
    /// We asked, and wait for the answer.
    Requesting,
    /// Peered.
    Peered,
    /// Being ended.
    Terminating,
}

/// A node as the owner sees it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NodeView {
    /// Its ID.
    pub id: NodeId,
    /// Its URL, if its link state is known.
    pub url: Option<String>,
    /// Its alias.
    pub alias: Option<String>,
    /// Its public key, once known.
    pub public_key: Option<PublicKey>,
    /// The neighbors its link state advertises, sorted.
    pub neighbors: Vec<NodeId>,
    /// The settlement schemes its link state lists.
    pub schemes: Vec<SchemeId>,
    /// Its link state's sequence.
    pub sequence: Option<u64>,
    /// Why it is in the table.
    pub sources: Sources,
    /// Our peering with it.
    pub peering: Option<Peering>,
    /// The peering's termination, if it is being ended.
    pub termination: Option<Termination>,
    /// Whether it is listed as our neighbor.
    pub advertised: bool,
    /// The advertise flag's damping.
    pub advertise: Advertise,
    /// Whether it is banned.
    pub banned: bool,
    /// Its registration with us, if it expires: when.
    pub registered_until_ms: Option<u64>,
    /// How far our peering with it has come.
    pub peer_status: PeerStatus,
    /// How many hops away it is, if reachable: 0 for ourselves.
    pub distance_hops: Option<u32>,
    /// Its first hops, if reachable.
    pub first_hops: Vec<NodeId>,
}

impl PeerNode {
    /// Fills the table from what was persisted. Link states were verified
    /// when received, so they are only decoded.
    pub(super) fn restore(&mut self, stored: Stored) {
        let now = self.now_ms;
        self.restore_channels(stored.channels, stored.bans);
        let mut peered = false;
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
                peered = true;
                self.restore_peering(&mut entry, node.id, peering, node.termination);
            } else {
                // An attempt cut short by the restart.
                entry.sources.remove(Sources::PEER);
            }
            if !self
                .bootstrap
                .iter()
                .any(|bootstrap| bootstrap.id == node.id)
            {
                entry.sources.remove(Sources::BOOTSTRAP);
            }
            if entry.sources.contains(Sources::MAJORITY) {
                self.lists.majority.insert(node.id);
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
        // Peerings from before channels existed get one now.
        let unchanneled: Vec<NodeId> = self
            .peers()
            .map(|(id, _)| *id)
            .filter(|id| !self.channels.contains_key(id))
            .collect();
        for id in unchanneled {
            self.open_channel(id);
        }
        self.gossip.routes_dirty = true;
        if peered {
            // The peers we have get a heartbeat's time to confirm
            // themselves before others are sought.
            let wait = self.config.heartbeat_min_ms + self.config.heartbeat_jitter_ms;
            self.peering.hold_until(now.saturating_add(wait));
        }
    }

    /// Restores the settlement channels, with their messages on their way,
    /// and the bans.
    fn restore_channels(&mut self, channels: Vec<StoredChannel>, bans: Vec<NodeId>) {
        let now = self.now_ms;
        for channel in channels.into_iter().take(self.config.max_channels) {
            if channel.channel.outbox.is_some() {
                self.retries.insert(
                    channel.node,
                    Retry {
                        at_ms: now,
                        in_flight: false,
                    },
                );
            }
            self.auth.pin(channel.node);
            self.channels.insert(channel.node, channel.channel);
        }
        self.bans = bans
            .into_iter()
            .filter(|id| *id != self.own)
            .take(self.config.max_bans)
            .collect();
    }

    /// Restores a peering, and its termination if it was being ended.
    fn restore_peering(
        &mut self,
        entry: &mut Entry,
        id: NodeId,
        peering: Peering,
        termination: Option<Termination>,
    ) {
        let now = self.now_ms;
        // Advertised at once if heard from recently: a restart does not
        // drop our neighbors (D12).
        let confirmed = peering
            .last_seen_ms
            .saturating_add(self.config.peer_confirm_ms)
            >= now;
        entry.advertise = Advertise {
            on: confirmed && termination.is_none(),
            on_since_ms: now,
            hold_until_ms: 0,
            flaps: 0,
        };
        entry.peer = match termination {
            Some(termination) => {
                if !termination.told(now, self.config.termination_timeout_ms) {
                    self.telling.insert(id, now);
                }
                PeerState::Terminating(peering, termination)
            }
            None => PeerState::Peered(peering),
        };
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
            let termination = entry.termination().copied();
            batch.nodes.push(StoredNode {
                id,
                link_state: entry.link.as_ref().map(|link| link.bytes.clone()),
                sources: entry.sources,
                first_seen_ms: entry.first_seen_ms,
                registration: entry.registration.clone(),
                termination,
                replay_floor_ms: if peering.is_some() {
                    self.auth.newest(&id).unwrap_or(0)
                } else {
                    0
                },
                peering,
            });
        }
        batch.removed.append(&mut self.removed);
        for node in core::mem::take(&mut self.channels_dirty) {
            if let Some(channel) = self.channels.get(&node) {
                batch.channels.push(StoredChannel {
                    node,
                    channel: channel.clone(),
                });
            }
        }
        batch.banned.append(&mut self.banned);
        batch.unbanned.append(&mut self.unbanned);
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

    /// The nodes we are peered with and not ending it.
    pub fn active_peers(&self) -> impl Iterator<Item = (&NodeId, &Peering)> {
        self.table
            .iter()
            .filter_map(|(id, entry)| entry.active_peering().map(|peering| (id, peering)))
    }

    /// Keeps a scheme's new state for a peer (ACC-15).
    pub fn set_scheme_state(&mut self, peer: &NodeId, state: Vec<u8>) {
        if let Some(entry) = self.table.get_mut(peer)
            && let PeerState::Peered(peering) | PeerState::Terminating(peering, _) = &mut entry.peer
            && peering.state != state
        {
            peering.state = state;
            self.dirty.insert(*peer);
        }
    }

    /// The nodes we are peered with, ending it or not.
    pub fn peers(&self) -> impl Iterator<Item = (&NodeId, &Peering)> {
        self.table
            .iter()
            .filter_map(|(id, entry)| entry.peering().map(|peering| (id, peering)))
    }

    /// Notes an authenticated message from a peer (NET-12): it is alive,
    /// and the newest timestamp accepted from it is persisted. A banned
    /// node's messages confirm nothing.
    pub(crate) fn saw(&mut self, node: &NodeId) {
        let now = self.now_ms;
        if self.bans.contains(node) {
            return;
        }
        if let Some(entry) = self.table.get_mut(node)
            && let PeerState::Peered(peering) | PeerState::Terminating(peering, _) = &mut entry.peer
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
                public_key: entry.key,
                neighbors: entry
                    .link
                    .as_ref()
                    .map(|link| link.neighbors.clone())
                    .unwrap_or_default(),
                schemes: entry
                    .link
                    .as_ref()
                    .map(|link| link.schemes.clone())
                    .unwrap_or_default(),
                sequence: entry.link.as_ref().map(|link| link.sequence),
                sources: entry.sources,
                peering: entry.peering().cloned(),
                termination: entry.termination().copied(),
                advertised: entry.active_peering().is_some() && entry.advertise.on,
                advertise: entry.advertise,
                banned: self.bans.contains(id),
                registered_until_ms: entry.registration.as_ref().map(|r| r.expires_ms),
                peer_status: match entry.peer {
                    PeerState::None => PeerStatus::None,
                    PeerState::Requesting { .. } => PeerStatus::Requesting,
                    PeerState::Peered(_) => PeerStatus::Peered,
                    PeerState::Terminating(..) => PeerStatus::Terminating,
                },
                distance_hops: if *id == self.own {
                    Some(0)
                } else {
                    self.routes.get(id).map(|route| route.distance_hops)
                },
                first_hops: self
                    .routes
                    .get(id)
                    .map(|route| route.first_hops.clone())
                    .unwrap_or_default(),
            })
            .collect()
    }

    /// Expires registrations and removes nodes nobody needs (NET-18).
    pub(super) fn maintain(&mut self) {
        let now = self.now_ms;
        let expired: Vec<NodeId> = self
            .table
            .iter()
            .filter(|(_, entry)| {
                entry
                    .registration
                    .as_ref()
                    .is_some_and(|r| r.expires_ms <= now)
            })
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
                let expiry = entry.registration.as_ref().map(|r| r.expires_ms);
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
        assert!(
            self.answers.len() <= self.config.max_requests,
            "retries within bounds"
        );
        let own = self.table.get(&self.own);
        assert!(
            own.is_some_and(|entry| entry.sources.contains(Sources::SELF)),
            "we are in our table"
        );
        let peers = self.peers().count();
        assert!(peers <= self.config.max_peers, "peers within bounds");
        self.check_channels();
        for (id, entry) in &self.table {
            assert!(
                entry.peering().is_none() || entry.key.is_none() || self.channels.contains_key(id),
                "a peer we can reach has a channel"
            );
            assert!(
                !entry.advertise.on || entry.peering().is_some(),
                "only peers are advertised"
            );
            assert!(
                entry.termination().is_none() || !entry.advertise.on,
                "a peering being ended is not advertised"
            );
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
        for route in self.routes.values() {
            assert!(
                !route.first_hops.is_empty() && route.first_hops.len() <= routing::MAX_FIRST_HOPS,
                "routes have first hops"
            );
            assert!(route.distance_hops >= 1, "routes lead away from us");
        }
    }

    /// The channels' and bans' invariants.
    fn check_channels(&self) {
        assert!(
            self.channels.len() <= self.config.max_channels,
            "channels within bounds"
        );
        assert!(
            self.bans.len() <= self.config.max_bans,
            "bans within bounds"
        );
        assert!(!self.bans.contains(&self.own), "we are not banned");
        for node in self.retries.keys() {
            assert!(
                self.channels.get(node).is_some_and(|c| c.outbox.is_some()),
                "a retry is for a message on its way"
            );
        }
        for (node, channel) in &self.channels {
            assert!(
                channel.outbox.is_none() || self.retries.contains_key(node),
                "a message on its way goes again"
            );
            assert!(
                channel
                    .outbox
                    .as_ref()
                    .is_none_or(|outbox| outbox.sequence == channel.settled_out),
                "the message on its way is the last one"
            );
        }
    }
}
