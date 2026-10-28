//! The table's upkeep: restoring it, and a registrar's state, persisting
//! their changes, expiring registrations, removing nodes nobody needs, the
//! routes, and the invariants.

use alloc::collections::BTreeSet;
use alloc::string::String;
use alloc::vec::Vec;

use dassie_identity::{NodeId, PublicKey};
use dassie_oer::Decode;

use super::PeerNode;
use crate::channel::Retry;
use crate::io::Event;
use crate::names::Registrar;
use crate::persist::{PeerBatch, Stored, StoredChannel, StoredClosed, StoredNode};
use crate::table::{
    Advertise, Depeering, Entry, LinkState, PeerState, Peering, Room, Sources, Termination,
};
use crate::wire::{Endpoint, SchemeId, SignedNodeInfo};
use crate::{core_nodes, routing};

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
    /// The settlement schemes its link state offers, in its order.
    pub schemes: Vec<SchemeId>,
    /// Its link state's sequence.
    pub sequence: Option<u64>,
    /// Why it is in the table.
    pub sources: Sources,
    /// Our peering with it.
    pub peering: Option<Peering>,
    /// The peering's termination, if it is being ended.
    pub termination: Option<Termination>,
    /// Whether it is in the core, as the signed data defines it
    /// (spec/decisions/promotion-to-core.md).
    pub core: bool,
    /// How many more core peers it takes, as its link state says, if it
    /// says (spec/decisions/promotion-to-core.md).
    pub free_core: Option<u16>,
    /// Whether it is listed as our neighbor.
    pub advertised: bool,
    /// Whether our peer was heard from within the confirm window: unless
    /// it is, it is offline and not advertised.
    pub online: bool,
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
    pub(super) fn restore(&mut self, mut stored: Stored) {
        let now = self.now_ms;
        self.restore_registrar(&mut stored);
        self.restore_channels(stored.channels, stored.bans);
        let mut peered = false;
        let mut downlinks = 0;
        for node in stored.nodes {
            let channel = self.channels.get(&node.id);
            let mut entry = stored_entry(&node, channel, self.config.link_state_rate);
            // A peering has a method, or it is none.
            let peering = node.peering.filter(|peering| !peering.methods.is_empty());
            if let Some(peering) = peering {
                peered = true;
                self.restore_peering(&mut entry, node.id, peering, node.termination);
            } else {
                // An attempt cut short by the restart.
                entry.sources.remove(Sources::PEER);
                entry.sources.remove(Sources::DOWNLINK);
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
            let room = if entry.is_downlink() {
                downlinks < self.config.max_downlinks
            } else {
                self.table.len() - downlinks < self.config.max_nodes
            };
            if room && node.id != self.own {
                downlinks += usize::from(entry.is_downlink());
                self.table.insert(node.id, entry);
            }
        }
        self.ensure(self.own, Sources::SELF);
        if let Some(entry) = self.table.get_mut(&self.own) {
            entry.key = Some(self.key.public_key());
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
        // Peerings from before channels existed get one now. None of them
        // is closed: a channel closes only once its peering is over, in
        // the batch that writes the peering's end or after it.
        let unchanneled: Vec<NodeId> = self
            .peers()
            .map(|(id, _)| *id)
            .filter(|id| !self.channels.contains_key(id))
            .collect();
        for id in unchanneled {
            self.open_channel_from(id, None);
        }
        // A node that lost its URL while down ends its downlinks.
        self.end_downlinks();
        self.gossip.routes_dirty = true;
        if peered {
            // The peers we have get a heartbeat's time to confirm
            // themselves before others are sought.
            let wait = self.config.heartbeat_min_ms + self.config.heartbeat_jitter_ms;
            self.peering.hold_until(now.saturating_add(wait));
        }
    }

    /// Restores the registrar's state, if this node is a registrar: one of
    /// its own bootstrap nodes, with names on. What another node kept of
    /// being one, or this node while names are off, stays on disk, unread.
    fn restore_registrar(&mut self, stored: &mut Stored) {
        let bootstrap = self.bootstrap.iter().any(|node| node.id == self.own);
        if self.config.names_enabled && bootstrap {
            self.registrar = Some(Registrar::restore(
                self.config.names,
                stored.name_log,
                core::mem::take(&mut stored.commitments),
                core::mem::take(&mut stored.names),
                self.now_ms,
            ));
        }
    }

    /// Restores the settlement channels, with their messages on their way,
    /// and the bans.
    fn restore_channels(&mut self, channels: Vec<StoredChannel>, bans: Vec<NodeId>) {
        let now = self.now_ms;
        let most = self.config.max_channels + self.config.max_downlinks;
        for channel in channels.into_iter().take(most) {
            if channel.channel.outbox.is_some() {
                self.retries.insert(
                    channel.node,
                    Retry {
                        at_ms: now,
                        in_flight: false,
                        early: true,
                        went_early: false,
                    },
                );
            }
            self.downlink_channels += usize::from(channel.channel.endpoint.is_none());
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
        // A decision to end it is kept; its steps are taken again.
        let mut peering = peering;
        peering.depeering = peering
            .depeering
            .map(|depeering| Depeering::new(depeering.since_ms, depeering.reason));
        // Each method on a scheme of its own, within the bound; the time
        // short starts from the next check.
        let mut seen = Vec::new();
        peering.methods.retain(|method| {
            let first = !seen.contains(&method.scheme);
            seen.push(method.scheme);
            first
        });
        peering.methods.truncate(super::MAX_METHODS);
        // A method being wound down keeps its decision, and takes its steps
        // again (spec/decisions/settlement-methods.md); whether its scheme
        // can receive is looked at afresh.
        for method in &mut peering.methods {
            method.short_checked_ms = None;
            method.winding = method
                .winding
                .map(|winding| crate::table::Winding::new(winding.since_ms));
            method.unpayable_since_ms = None;
        }
        // Advertised at once if heard from recently: a restart does not
        // drop our neighbors (spec/decisions/peering-lifecycle.md).
        let confirmed = peering.heard_within(now, self.config.peer_confirm_ms);
        entry.advertise = Advertise {
            on: confirmed && termination.is_none(),
            on_since_ms: now,
            ..Advertise::default()
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
                sources: stored_sources(entry.sources),
                first_seen_ms: entry.first_seen_ms,
                registration: entry.registration.clone(),
                termination,
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
        batch.closed.extend(
            core::mem::take(&mut self.closed)
                .into_iter()
                .map(|(node, closed)| StoredClosed { node, closed }),
        );
        batch.banned.append(&mut self.banned);
        batch.unbanned.append(&mut self.unbanned);
        if let Some(registrar) = &mut self.registrar {
            registrar.take_changes(batch);
        }
        self.holders
            .take_changes(&mut batch.holders, &mut batch.holders_removed);
        self.registrant.take_changes(batch);
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

    /// Notes what a settlement check found of the room our books give
    /// `peer` on its method on `scheme`. A method short, settling nothing,
    /// for longer than [`Config::advertise_short_grace_ms`](crate::Config)
    /// of checks at which the node could ask is withdrawn, and the peer is
    /// no longer advertised once all its methods are: it is stuck, not
    /// waiting for a routine settlement
    /// (spec/decisions/peering-lifecycle.md,
    /// spec/decisions/settlement-methods.md). Of that time, only what passed
    /// while the last request to settle may have reached the peer counts
    /// toward ending the peering: one that cannot be reached is asked at
    /// every check, and each attempt fails before it reaches the peer, so it
    /// costs the peer nothing; nor does any time while no settlement from
    /// the peer could reach us ([`Room::ShortUnpayable`]).
    pub fn room(&mut self, now_ms: u64, peer: &NodeId, scheme: SchemeId, room: Room) {
        self.advance(now_ms);
        let now = self.now_ms;
        let Some(entry) = self.table.get_mut(peer) else {
            return;
        };
        let PeerState::Peered(peering) = &mut entry.peer else {
            return;
        };
        let Some(method) = peering.method_mut(scheme) else {
            return;
        };
        let before = (method.short_for_ms, method.short_reached_ms);
        match room {
            Room::Short { most_ms } | Room::ShortUnpayable { most_ms } => {
                let since = method
                    .short_checked_ms
                    .map_or(0, |at| now.saturating_sub(at).min(most_ms));
                method.short_for_ms = method.short_for_ms.saturating_add(since);
                // Only time the peer could have restored its room counts
                // toward ending the peering for it.
                let payable = matches!(room, Room::Short { .. });
                if payable && !entry.advertise.settle_unreached {
                    method.short_reached_ms = method.short_reached_ms.saturating_add(since);
                }
                method.short_checked_ms = Some(now);
            }
            Room::Enough => {
                method.short_for_ms = 0;
                method.short_reached_ms = 0;
                method.short_checked_ms = None;
            }
            // The time until the next check that can judge does not count.
            Room::Unjudged => method.short_checked_ms = None,
        }
        assert!(
            method.short_reached_ms <= method.short_for_ms,
            "more time short while reached than short"
        );
        if (method.short_for_ms, method.short_reached_ms) != before {
            self.dirty.insert(*peer);
        }
    }

    /// The nodes we are peered with, ending it or not.
    pub fn peers(&self) -> impl Iterator<Item = (&NodeId, &Peering)> {
        self.table
            .iter()
            .filter_map(|(id, entry)| entry.peering().map(|peering| (id, peering)))
    }

    /// The core nodes we are peered with, ending it or not: our peers but
    /// our downlinks, which have slots of their own.
    pub fn core_peers(&self) -> impl Iterator<Item = (&NodeId, &Peering)> {
        self.table.iter().filter_map(|(id, entry)| {
            entry
                .peering()
                .filter(|_| !entry.is_downlink())
                .map(|peering| (id, peering))
        })
    }

    /// Notes a message from a peer: it is alive. A banned node's messages
    /// confirm nothing.
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
                url: entry.link.as_ref().map(|link| {
                    link.endpoint
                        .as_ref()
                        .map(Endpoint::url)
                        .unwrap_or_default()
                }),
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
                    .map(|link| link.schemes().collect())
                    .unwrap_or_default(),
                sequence: entry.link.as_ref().map(|link| link.sequence),
                sources: entry.sources,
                peering: entry.peering().cloned(),
                termination: entry.termination().copied(),
                core: self.core.contains(id),
                free_core: entry.link.as_ref().and_then(|link| link.free_core),
                advertised: !entry.is_downlink()
                    && entry.active_peering().is_some()
                    && entry.advertise.on,
                online: entry
                    .peering()
                    .is_some_and(|p| p.heard_within(self.now_ms, self.config.peer_confirm_ms)),
                advertise: entry.advertise,
                banned: self.bans.contains(id),
                registered_until_ms: entry.registration.as_ref().map(|r| r.expires_ms),
                peer_status: match &entry.peer {
                    PeerState::None => PeerStatus::None,
                    PeerState::Requesting { .. } => PeerStatus::Requesting,
                    // Ending it privately is ending it, to its owner
                    // (spec/decisions/ending-a-peering.md).
                    PeerState::Peered(peering) if peering.depeering.is_some() => {
                        PeerStatus::Terminating
                    }
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

    /// Expires registrations and removes nodes nobody needs.
    pub(super) fn maintain(&mut self) {
        let now = self.now_ms;
        let failing = self.config.registration_failing_ms;
        // A registration ends once it lapses, or once the node's URL has
        // failed our checks for longer than becoming core takes, so that a
        // node that went away loses its place in the majority
        // (spec/decisions/promotion-to-core.md).
        let expired: Vec<NodeId> = self
            .table
            .iter()
            .filter(|(_, entry)| {
                let gone = entry
                    .url
                    .failing_since_ms
                    .is_some_and(|since| since.saturating_add(failing) <= now);
                entry
                    .registration
                    .as_ref()
                    .is_some_and(|r| r.expires_ms <= now || gone)
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
        let (grace, failing) = (
            self.config.removal_grace_ms,
            self.config.registration_failing_ms,
        );
        self.table
            .values()
            .filter_map(|entry| {
                let gone = entry
                    .url
                    .failing_since_ms
                    .map(|since| since.saturating_add(failing));
                let expiry = entry
                    .registration
                    .as_ref()
                    .map(|r| gone.map_or(r.expires_ms, |gone| gone.min(r.expires_ms)));
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
        let neighbors = |node: &NodeId| {
            let link = table.get(node)?.link.as_ref()?;
            link.is_fresh(now, stale, future)
                .then_some(link.neighbors.as_slice())
        };
        // A periphery node's uplinks never list it, and carry everything it
        // sends.
        let routes = if self.is_periphery() {
            routing::through(&self.own, &self.route_key, &self.advertised(), neighbors)
        } else {
            routing::compute(&self.own, &self.route_key, neighbors)
        };
        if routes != self.routes {
            self.routes = routes;
            self.event(Event::RoutesChanged);
        }
        let core = self.compute_core();
        if core != self.core {
            self.core = core;
            self.event(Event::CoreChanged);
        }
        if !self.core.contains(&self.own) {
            self.own_core_since_ms = None;
        } else if self.own_core_since_ms.is_none() {
            self.own_core_since_ms = Some(now);
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

    /// However often the node finds a peer short, one request to settle per
    /// peer and method is in flight at most
    /// (spec/decisions/settle-at-the-trigger.md).
    fn check_requests_to_settle(&self) {
        let mut asking = alloc::collections::BTreeSet::new();
        for request in self.requests.values() {
            if let super::Purpose::SettlementRequest(scheme) = request.purpose {
                assert!(
                    asking.insert((request.to, scheme)),
                    "one request to settle per method in flight"
                );
            }
        }
    }

    /// The core, from the majority list and fresh link states
    /// (spec/decisions/promotion-to-core.md). A node with no bootstrap
    /// nodes runs alone, as its own bootstrap node, registered with
    /// itself: a core alone.
    fn compute_core(&self) -> BTreeSet<NodeId> {
        let (now, stale, future) = (self.now_ms, self.config.stale_ms, self.config.max_future_ms);
        let table = &self.table;
        let neighbors = |node: &NodeId| {
            let link = table.get(node)?.link.as_ref()?;
            link.is_fresh(now, stale, future)
                .then_some(link.neighbors.as_slice())
        };
        if self.bootstrap.is_empty() {
            let alone = BTreeSet::from([self.own]);
            return core_nodes::compute(&alone, &alone, &self.dev_core, neighbors);
        }
        let bootstrap: BTreeSet<NodeId> = self.bootstrap.iter().map(|node| node.id).collect();
        core_nodes::compute(self.majority(), &bootstrap, &self.dev_core, neighbors)
    }

    /// Checks the node's invariants (spec/decisions/tigerstyle.md): bounds,
    /// ourselves in the table, and peers that are what the table says.
    ///
    /// # Panics
    ///
    /// If one does not hold.
    pub fn check_invariants(&self) {
        let downlinks = self.downlinks();
        assert!(
            self.table.len() - downlinks <= self.config.max_nodes
                && downlinks <= self.config.max_downlinks,
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
        self.check_requests_to_settle();
        assert!(
            self.deferred <= self.config.max_deferred,
            "deferred within bounds"
        );
        assert!(
            self.network_probes.len() <= self.config.max_probe_networks,
            "probe networks within bounds"
        );
        assert!(
            self.answers.len() <= self.config.max_requests,
            "retries within bounds"
        );
        assert!(
            self.told_not_peered.len() <= super::lifecycle::MAX_TOLD_NOT_PEERED,
            "told not peered within bounds"
        );
        let own = self.table.get(&self.own);
        assert!(
            own.is_some_and(|entry| entry.sources.contains(Sources::SELF)),
            "we are in our table"
        );
        let (peers, all) = (self.core_peers().count(), self.peers().count());
        assert!(
            peers <= self.config.max_core_peers() && all - peers <= self.config.max_downlinks,
            "peers within bounds"
        );
        // A peering that ends forgets the mark
        // (spec/decisions/unreachable-peers.md); one that is left behind
        // would refuse a later peering's first Prepares.
        assert!(
            self.unreached
                .iter()
                .all(|node| self.peering_of(node).is_some()),
            "only peers are marked unreached"
        );
        self.check_periphery();
        self.check_snapshots();
        self.check_channels();
        self.check_names();
        self.check_entries();
        assert!(
            !self.routes.contains_key(&self.own),
            "no route to ourselves"
        );
        self.check_core();
        self.check_url_checks();
        for route in self.routes.values() {
            assert!(
                !route.first_hops.is_empty() && route.first_hops.len() <= routing::MAX_FIRST_HOPS,
                "routes have first hops"
            );
            assert!(route.distance_hops >= 1, "routes lead away from us");
        }
    }

    /// Each entry's invariants: peers have channels, sources and keys, and
    /// a method or more each on a scheme of its own; only active peers are
    /// advertised.
    fn check_entries(&self) {
        assert!(
            self.dropping
                .keys()
                .all(|(node, _)| self.peering_of(node).is_some()),
            "only methods of peerings are dropped"
        );
        for (id, entry) in &self.table {
            if let Some(peering) = entry.peering() {
                let methods = &peering.methods;
                assert!(
                    (1..=super::MAX_METHODS).contains(&methods.len()),
                    "a peering has a method, within the bound"
                );
                assert!(
                    methods.iter().enumerate().all(|(index, method)| {
                        methods
                            .iter()
                            .skip(index + 1)
                            .all(|other| other.scheme != method.scheme)
                    }),
                    "a method per scheme"
                );
                for method in methods {
                    if let Some(winding) = method.winding {
                        assert!(
                            winding.asked_ms.is_none_or(|asked| asked <= self.now_ms),
                            "a method's peer is asked to settle at a time passed"
                        );
                        assert!(
                            winding.asked_ms.is_some()
                                || (winding.promised.is_none() && winding.booked == 0),
                            "nothing is collected on a method before it is asked"
                        );
                    }
                }
            }
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
                entry.peering().is_none()
                    || entry.sources.contains(Sources::PEER)
                    || entry.is_downlink(),
                "a peer has the peer source"
            );
            assert!(
                *id == self.own || entry.peering().is_none() || entry.key.is_some(),
                "a peer's key is known"
            );
        }
    }

    /// The periphery's invariants (spec/areas/network.md#periphery-nodes):
    /// a periphery node has no downlinks, and its link state is a periphery
    /// node's.
    fn check_periphery(&self) {
        if !self.is_periphery() {
            return;
        }
        // Downlinks from when it was a core node are being ended.
        for entry in self.table.values().filter(|entry| entry.is_downlink()) {
            let ending = entry.termination().is_some()
                || entry.peering().is_some_and(|p| p.depeering.is_some());
            assert!(ending, "a periphery node's downlinks are ending");
        }
        if let Some(own) = self.table.get(&self.own).and_then(|e| e.link.as_ref()) {
            assert!(
                own.periphery || own.endpoint.is_some(),
                "a link state without a URL is a periphery node's"
            );
        }
    }

    /// The core's invariants: of nodes once in the table, or the dev core,
    /// and our own place in it known.
    fn check_core(&self) {
        assert!(
            self.core.len() <= self.config.max_nodes + core_nodes::MAX_DEV_CORE,
            "the core within bounds"
        );
        assert_eq!(
            self.own_core_since_ms.is_some(),
            self.core.contains(&self.own),
            "we know since when we are core"
        );
    }

    /// The channels' and bans' invariants.
    fn check_channels(&self) {
        assert!(
            self.channels.len() <= self.config.max_channels + self.config.max_downlinks,
            "channels within bounds"
        );
        assert_eq!(
            self.downlink_channels,
            self.channels
                .values()
                .filter(|c| c.endpoint.is_none())
                .count(),
            "channels with downlinks counted"
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
        assert!(
            self.channel_active
                .keys()
                .all(|node| self.channels.contains_key(node)),
            "only open channels are active"
        );
        assert!(
            self.closed
                .keys()
                .all(|node| !self.channels.contains_key(node)),
            "a channel is open or closed"
        );
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

/// `sources` as stored: a downlink's as a peer's, which its channel, or
/// else its periphery link state, tells apart again on restore, so the
/// stored bits stay those of a node in the node list's terms.
fn stored_sources(sources: Sources) -> Sources {
    let mut stored = sources;
    if stored.contains(Sources::DOWNLINK) {
        stored.remove(Sources::DOWNLINK);
        stored.insert(Sources::PEER);
    }
    stored
}

/// A node's entry as stored, without its peering. Link states were verified
/// when received, so they are only decoded. A downlink is stored as a peer;
/// its open `channel`, reached over its connection, tells it apart, as the
/// peering's kind was when the node stopped, whatever its link state says
/// by now (spec/areas/network.md#periphery-nodes); without one, its link
/// state, a periphery node's.
fn stored_entry(
    node: &crate::persist::StoredNode,
    channel: Option<&crate::channel::Channel>,
    rate: crate::config::Rate,
) -> Entry {
    let mut entry = Entry::new(node.sources, node.first_seen_ms, rate);
    if let Some(bytes) = &node.link_state
        && let Ok(signed) = SignedNodeInfo::decode_exact(bytes)
    {
        entry.key = PublicKey::from_bytes(&signed.info.public_key).ok();
        entry.link = Some(LinkState::from_signed(&signed));
    }
    entry.registration.clone_from(&node.registration);
    let downlink = channel.map_or_else(
        || entry.link.as_ref().is_some_and(|l| l.periphery),
        |channel| channel.endpoint.is_none(),
    );
    if entry.sources.contains(Sources::PEER) && downlink {
        entry.sources.remove(Sources::PEER);
        entry.sources.insert(Sources::DOWNLINK);
    }
    entry
}
