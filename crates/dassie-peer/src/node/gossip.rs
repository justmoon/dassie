//! Link states: our own, those we receive, flooding, heartbeats and
//! refreshing stale ones (spec/areas/network.md#link-states).

use alloc::string::String;
use alloc::vec::Vec;

use dassie_identity::NodeId;
use dassie_oer::Decode;
use rand_core::CryptoRng;

use super::{MAX_OWN_HUBS, MAX_SCHEMES, PeerNode, Problem, Purpose, offers_keep_the_rules};
use crate::config::Config;
use crate::io::Event;
use crate::table::{LinkState, Offered, Sources, backoff};
use crate::wire::{
    Content, EXTENSION_FEED, EXTENSION_FREE_CORE, EXTENSION_PERIPHERY, Entry, HubDetails,
    InvalidNodeInfo, LinkStateResponse, MAX_ENTRIES, MAX_NODE_INFO_BYTES, MAX_OFFERS, NodeInfo,
    SchemeId, SettlementOffer, SignedNodeInfo, max_node_info_bytes, sign_node_info,
};

/// The routes are recomputed at least this often, as link states go stale.
const ROUTES_PERIOD_MS: u64 = 60_000;

/// Most extensions our own link state carries: the periphery one, or a
/// feed port and the free core relationships
/// ([`PeerNode::own_extensions`]).
const MAX_OWN_EXTENSIONS: usize = 2;

/// Most bytes their values take together: a feed port and a count of free
/// core relationships; the periphery one is empty.
const MAX_OWN_EXTENSION_BYTES: usize = 4;

/// Most bytes the details of our own offers take together: a hub's on each
/// of [`MAX_OWN_HUBS`] schemes over a hub, and none on any other.
const MAX_OWN_DETAILS_BYTES: usize = MAX_OWN_HUBS * HubDetails::ENCODED_BYTES;

/// Whether our own link state, listing at most `neighbors` neighbors,
/// always fits what every node decodes: in entries, offers and bytes, with
/// every offer, extension and the longest URL and alias it may carry.
pub(crate) const fn own_link_state_fits(neighbors: usize) -> bool {
    neighbors + MAX_OWN_EXTENSIONS <= MAX_ENTRIES
        && MAX_SCHEMES <= MAX_OFFERS
        && max_node_info_bytes(
            neighbors,
            MAX_SCHEMES,
            MAX_OWN_DETAILS_BYTES,
            MAX_OWN_EXTENSIONS,
            MAX_OWN_EXTENSION_BYTES,
        ) <= MAX_NODE_INFO_BYTES
}

/// The gossip side of the node's state.
pub(crate) struct Gossip {
    /// Our highest sequence issued.
    pub own_sequence: u64,
    /// Whether it changed since persisted.
    pub sequence_dirty: bool,
    /// Whether what our link state says changed.
    pub own_changed: bool,
    last_issue_ms: Option<u64>,
    heartbeat_at_ms: u64,
    refresh_at_ms: u64,
    sweep_at_ms: u64,
    /// Where the last sweep stopped.
    sweep_after: Option<NodeId>,
    /// Whether the routes may have changed.
    pub routes_dirty: bool,
    routes_at_ms: u64,
    routes_by_ms: u64,
}

impl Gossip {
    pub(crate) const fn new(now_ms: u64, own_sequence: u64) -> Self {
        Self {
            own_sequence,
            sequence_dirty: false,
            own_changed: true,
            last_issue_ms: None,
            heartbeat_at_ms: now_ms,
            refresh_at_ms: now_ms,
            sweep_at_ms: now_ms,
            sweep_after: None,
            routes_dirty: true,
            routes_at_ms: now_ms,
            routes_by_ms: now_ms,
        }
    }

    /// Brings the next heartbeat forward to now: a new peer hears from us
    /// at once.
    pub(crate) const fn heartbeat_soon(&mut self, now_ms: u64) {
        if self.heartbeat_at_ms > now_ms {
            self.heartbeat_at_ms = now_ms;
        }
    }

    pub(crate) const fn routes_due(&self, now_ms: u64) -> bool {
        (self.routes_dirty && now_ms >= self.routes_at_ms) || now_ms >= self.routes_by_ms
    }

    pub(crate) const fn routes_computed(&mut self, now_ms: u64, config: &Config) {
        self.routes_dirty = false;
        self.routes_at_ms = now_ms.saturating_add(config.route_debounce_ms);
        self.routes_by_ms = now_ms.saturating_add(ROUTES_PERIOD_MS);
    }

    /// When our own link state is next due.
    fn issue_due(&self, config: &Config) -> Option<u64> {
        let Some(last) = self.last_issue_ms else {
            return Some(0);
        };
        let earliest = last.saturating_add(config.issue_debounce_ms);
        let latest = last.saturating_add(config.reissue_interval_ms);
        Some(if self.own_changed { earliest } else { latest })
    }

    /// The next time gossip has something to do.
    pub(crate) fn next_due(&self, node: &PeerNode) -> Option<u64> {
        let retransmit = node
            .table
            .values()
            .filter_map(|entry| entry.link.as_ref()?.retransmit_at_ms)
            .min();
        // Every node issues one: a node without a URL a periphery node's.
        let issue = self.issue_due(&node.config);
        let heartbeat = node.peers().next().map(|_| self.heartbeat_at_ms);
        let routes = if self.routes_dirty {
            self.routes_at_ms
        } else {
            self.routes_by_ms
        };
        [
            retransmit,
            issue,
            heartbeat,
            node.next_refresh().map(|at| at.max(self.refresh_at_ms)),
            Some(self.sweep_at_ms),
            Some(routes),
        ]
        .into_iter()
        .flatten()
        .min()
    }
}

/// How a received link state is passed on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Retransmit {
    /// Our own: at once.
    Immediately,
    /// Flooded: after a random delay, unless enough copies arrive.
    Scheduled,
    /// Not at all: answers to our own queries, registrations, peerings.
    Never,
}

/// What became of a received link state.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Processed {
    /// It is new, and now held.
    Accepted,
    /// A copy of the one held, or older.
    Old,
    /// Of a node not in the table, or over its rate.
    Dropped,
    /// It does not verify.
    Invalid(InvalidNodeInfo),
}

/// When a link state taken in by `mode` at `now` is forwarded, if it is:
/// a scheduled one after a random delay of up to `delay_ms`, so that the
/// copies of a flood suppress each other.
fn retransmit_at(
    rng: &mut dyn CryptoRng,
    delay_ms: u64,
    mode: Retransmit,
    now: u64,
) -> Option<u64> {
    match mode {
        Retransmit::Immediately => Some(now),
        Retransmit::Scheduled => {
            let delay = 1 + rng.next_u64() % delay_ms.max(1);
            Some(now.saturating_add(delay))
        }
        Retransmit::Never => None,
    }
}

/// The details of the offer on `scheme` among `offers`, if there is one.
fn details_on(offers: &[Offered], scheme: SchemeId) -> Option<&[u8]> {
    (offers.iter())
        .find(|offer| offer.scheme == scheme)
        .map(|offer| offer.details.as_slice())
}

impl PeerNode {
    /// Takes a link state in, in the order that makes floods cheap: copies
    /// and older states are dropped before the signature is checked.
    pub(crate) fn process_link_state(
        &mut self,
        signed: &SignedNodeInfo<'_>,
        from: Option<NodeId>,
        mode: Retransmit,
    ) -> Processed {
        let id = signed.node_id;
        // A periphery node's link state never floods, and a periphery node
        // forwards nothing (spec/areas/network.md#periphery-nodes).
        let mode =
            if mode == Retransmit::Scheduled && (signed.info.is_periphery() || !self.forwards()) {
                Retransmit::Never
            } else {
                mode
            };
        if id == self.own && mode != Retransmit::Immediately {
            // Only we issue our link state; copies of it coming back are
            // echoes.
            return Processed::Old;
        }
        let (now, rate) = (self.now_ms, self.config.link_state_rate);
        let Some(entry) = self.table.get_mut(&id) else {
            self.stats.link_states_dropped += 1;
            return Processed::Dropped;
        };
        if let Some(link) = &mut entry.link {
            if link.sequence == signed.info.sequence && link.bytes == signed.bytes {
                if mode == Retransmit::Scheduled {
                    link.copies = link.copies.saturating_add(1);
                }
                return Processed::Old;
            }
            if signed.info.sequence <= link.sequence {
                return Processed::Old;
            }
        }
        // The subject's rate is checked before the signature, which is the
        // costly part, but spent only on a state that verifies: forged
        // states cannot use up the real node's allowance.
        if signed.info.sequence > now.saturating_add(self.config.max_future_ms)
            || (id != self.own && !entry.bucket.has_token(rate, now))
        {
            self.stats.link_states_dropped += 1;
            return Processed::Dropped;
        }
        let key = match signed.verify() {
            Ok(key) => key,
            Err(invalid) => {
                self.stats.link_states_dropped += 1;
                return Processed::Invalid(invalid);
            }
        };
        if id != self.own {
            entry.bucket.take(rate, now);
        }
        let mut link = LinkState::from_signed(signed);
        link.from = from;
        link.retransmit_at_ms =
            retransmit_at(&mut *self.rng, self.config.retransmit_delay_ms, mode, now);
        let changed = entry
            .link
            .as_ref()
            .is_none_or(|old| old.neighbors != link.neighbors);
        entry.watch_link(&link, from == Some(id), now);
        // What a peer offered before, to tell an account it names anew.
        let before =
            (entry.peering().is_some()).then(|| entry.link.as_ref().map(|old| old.offers.clone()));
        entry.key = Some(key.key());
        entry.link = Some(link);
        entry.refresh_failures = 0;
        self.dirty.insert(id);
        self.stats.link_states_accepted += 1;
        if changed {
            self.gossip.routes_dirty = true;
        }
        if let Some(before) = before {
            self.tell_accounts_named(id, before.as_deref());
        }
        Processed::Accepted
    }

    /// Tells the node of each account in a hub that peer `id`'s link state
    /// names anew ([`Self::advertised_accounts`]).
    fn tell_accounts_named(&mut self, id: NodeId, before: Option<&[Offered]>) {
        for (scheme, details) in self.advertised_accounts(id, before) {
            self.event(Event::HubAccountAdvertised {
                peer: id,
                scheme,
                details,
            });
        }
    }

    /// The offers of peer `id`'s link state, as we read them, on methods of
    /// our peering over a hub, that name an account in the hub where its
    /// offers `before` named none or another: at most one per method.
    fn advertised_accounts(
        &self,
        id: NodeId,
        before: Option<&[Offered]>,
    ) -> Vec<(SchemeId, HubDetails)> {
        let Some(entry) = self.table.get(&id) else {
            return Vec::new();
        };
        let (Some(peering), Some(link)) = (entry.peering(), entry.link.as_ref()) else {
            return Vec::new();
        };
        (self.their_offers(link).into_iter())
            .filter(|(scheme, _)| peering.method(*scheme).is_some())
            .filter_map(|(scheme, hub)| Some((scheme, hub?)))
            .filter(|(_, hub)| hub.account != HubDetails::NO_ACCOUNT)
            .filter(|(scheme, _)| {
                before.is_none_or(|before| {
                    details_on(before, *scheme) != details_on(&link.offers, *scheme)
                })
            })
            .collect()
    }

    /// Issues our own link state when what it says changed, every
    /// hour regardless, never faster than the debounce. The
    /// sequence is persisted with it.
    pub(super) fn issue_own(&mut self) {
        // A node without a URL says so with the periphery extension.
        let periphery = !self.has_url();
        let Some(url) = self
            .own_info
            .url
            .clone()
            .or_else(|| periphery.then(String::new))
        else {
            return;
        };
        if self
            .gossip
            .issue_due(&self.config)
            .is_none_or(|due| due > self.now_ms)
        {
            return;
        }

        let now = self.now_ms;
        self.gossip.last_issue_ms = Some(now);
        let high = self.gossip.own_sequence;
        // A sequence far ahead of the clock was never accepted by anyone
        // whose clock is right: start again from now.
        let sequence = if high > now.saturating_add(self.config.max_future_ms) {
            now
        } else {
            now.max(high.saturating_add(1))
        };
        let neighbors = self.own_neighbors(periphery);
        let extensions = self.own_extensions(periphery);
        let details = self.own_details();
        let settlement = self
            .offers
            .iter()
            .zip(&details)
            .map(|(offer, details)| SettlementOffer {
                scheme: offer.scheme,
                details,
            })
            .collect();
        let entries = neighbors
            .iter()
            .map(|id| Entry::Neighbor(*id))
            .chain(
                extensions
                    .iter()
                    .map(|(kind, value)| Entry::Extension { kind: *kind, value }),
            )
            .collect();
        let info = NodeInfo {
            sequence,
            url: &url,
            alias: &self.own_info.alias,
            public_key: *self.key.public_key().as_bytes(),
            slh_dsa_public_key: *self.key.identity().slh_dsa().as_bytes(),
            settlement,
            entries,
        };
        let bytes = sign_node_info(&self.key, &info);
        // It fits by construction (`own_link_state_fits`, checked by
        // `Config::validate`): one that did not would be kept by nobody.
        let signed = SignedNodeInfo::decode_exact(&bytes);
        assert!(signed.is_ok(), "our own link state decodes");
        let Ok(signed) = signed else {
            return;
        };
        if let Some(entry) = self.table.get_mut(&self.own) {
            entry.link = None;
        }
        self.process_link_state(&signed, None, Retransmit::Immediately);
        self.gossip.own_sequence = sequence;
        self.gossip.sequence_dirty = true;
        self.gossip.own_changed = false;
        self.gossip.last_issue_ms = Some(now);
        self.push_to_bootstrap(&bytes);
    }

    /// The neighbors our link state lists. Only a node with a URL lists
    /// any: its advertised peers, confirmed and not held down
    /// (spec/decisions/peering-lifecycle.md), and the downlinks it
    /// promoted (spec/decisions/promotion-to-core.md); one that is not core
    /// yet, its uplinks, so that the edges that make it core are advertised
    /// by both ends. A node without a URL lists none: only its uplinks see
    /// its link state, which never route by it, and it may keep more
    /// uplinks than a link state could list
    /// (spec/decisions/periphery-lists-no-neighbors.md). A node lists at
    /// most `max_peers`: one pinned to more uplinks lists only as many as
    /// fit.
    fn own_neighbors(&self, periphery: bool) -> Vec<NodeId> {
        if periphery {
            return Vec::new();
        }
        let mut neighbors = self.advertised();
        neighbors.truncate(self.config.max_peers);
        neighbors
    }

    /// The details of our offers, in their order, within what our link
    /// state budgets for them ([`own_link_state_fits`]).
    fn own_details(&self) -> Vec<Vec<u8>> {
        let details: Vec<Vec<u8>> = self.offers.iter().map(super::Offer::details).collect();
        assert!(
            offers_keep_the_rules(&self.offers)
                && details.iter().map(Vec::len).sum::<usize>() <= MAX_OWN_DETAILS_BYTES,
            "our own offers within their bounds"
        );
        details
    }

    /// The extensions our link state carries, in order, each a type and
    /// its value, within what our link state budgets for them
    /// ([`own_link_state_fits`]).
    fn own_extensions(&self, periphery: bool) -> Vec<(u16, Vec<u8>)> {
        let mut extensions = Vec::new();
        if periphery {
            extensions.push((EXTENSION_PERIPHERY, Vec::new()));
        }
        // Where a core node answers requests for its snapshot, at its URL's
        // host; a periphery node has no URL to name one at.
        if let Some(port) = self.own_info.feed_port.filter(|_| !periphery) {
            extensions.push((EXTENSION_FEED, port.to_be_bytes().to_vec()));
        }
        // How many more core peers a core node takes, so that a node seeking
        // to be promoted asks uplinks that can promote it
        // (spec/decisions/promotion-to-core.md).
        if self.is_core() {
            let free = u16::try_from(self.free_core()).unwrap_or(u16::MAX);
            extensions.push((EXTENSION_FREE_CORE, free.to_be_bytes().to_vec()));
        }
        let bytes: usize = extensions.iter().map(|(_, value)| value.len()).sum();
        assert!(
            extensions.len() <= MAX_OWN_EXTENSIONS && bytes <= MAX_OWN_EXTENSION_BYTES,
            "our own extensions within their bounds"
        );
        extensions
    }

    /// Bootstrap nodes get our new link state directly, so they can answer
    /// refreshes for it; others get it by flooding.
    fn push_to_bootstrap(&mut self, bytes: &[u8]) {
        // A node without a URL registers nowhere: its goes only to its
        // uplinks.
        if !self.has_url() {
            return;
        }
        let Ok(signed) = SignedNodeInfo::decode_exact(bytes) else {
            return;
        };
        let targets: Vec<NodeId> = self
            .bootstrap
            .iter()
            .map(|node| node.id)
            .filter(|id| self.peering_of(id).is_none())
            .collect();
        let content = Content::LinkStateUpdate(signed);
        let timeout = self.config.request_timeout_ms;
        for target in targets {
            self.send(target, &content, Purpose::Gossip, timeout);
        }
    }

    /// Forwards new link states to every peer but their subject and the
    /// peer they came from, unless enough copies arrived meanwhile.
    pub(super) fn flood(&mut self) {
        let now = self.now_ms;
        let due: Vec<NodeId> = self
            .table
            .iter()
            .filter(|(_, entry)| {
                entry
                    .link
                    .as_ref()
                    .is_some_and(|link| link.retransmit_at_ms.is_some_and(|at| at <= now))
            })
            .map(|(id, _)| *id)
            .collect();
        // Not to downlinks: they learn what they need from their uplinks'
        // sweeps and refreshes, as any node that is not a peer does.
        let peers: Vec<NodeId> = self.core_peers().map(|(id, _)| *id).collect();
        let timeout = self.config.request_timeout_ms;
        for id in due {
            let Some(link) = self
                .table
                .get_mut(&id)
                .and_then(|entry| entry.link.as_mut())
            else {
                continue;
            };
            link.retransmit_at_ms = None;
            if link.copies >= self.config.suppression_copies {
                continue;
            }
            let (bytes, from) = (link.bytes.clone(), link.from);
            let Ok(signed) = SignedNodeInfo::decode_exact(&bytes) else {
                continue;
            };
            let content = Content::LinkStateUpdate(signed);
            for peer in &peers {
                if *peer != id && Some(*peer) != from {
                    self.send(*peer, &content, Purpose::Gossip, timeout);
                }
            }
        }
    }

    /// Sends our link state to every peer, which is what keeps a peering
    /// confirmed.
    pub(super) fn heartbeat(&mut self) {
        if self.gossip.heartbeat_at_ms > self.now_ms {
            return;
        }
        let jitter = self.rng.next_u64() % self.config.heartbeat_jitter_ms.max(1);
        self.gossip.heartbeat_at_ms = self
            .now_ms
            .saturating_add(self.config.heartbeat_min_ms + jitter);
        // Every peer that is not being ended, advertised or not, so both
        // sides can confirm each other again.
        let peers: Vec<NodeId> = self
            .table
            .iter()
            .filter(|(_, entry)| entry.active_peering().is_some())
            .map(|(id, _)| *id)
            .collect();
        for peer in peers {
            self.heartbeat_to(peer);
        }
    }

    /// Sends our link state to `peer` now.
    pub(crate) fn heartbeat_to(&mut self, peer: NodeId) {
        let Some(bytes) = self
            .table
            .get(&self.own)
            .and_then(|entry| entry.link.as_ref())
            .map(|link| link.bytes.clone())
        else {
            return;
        };
        let Ok(signed) = SignedNodeInfo::decode_exact(&bytes) else {
            return;
        };
        let timeout = self.config.request_timeout_ms;
        let sent = self
            .send(
                peer,
                &Content::LinkStateUpdate(signed),
                Purpose::Heartbeat,
                timeout,
            )
            .is_some();
        // Only one that went out counts: our own bound on requests says
        // nothing of the peer. One the connection then fails to carry counts
        // as unanswered, as a downlink we cannot reach is in no standing.
        let (now, bucket) = (self.now_ms, self.standing_bucket_ms());
        if sent && let Some(entry) = self.table.get_mut(&peer) {
            entry.standing.sent(now, bucket);
        }
    }

    /// Refreshes one stale or unknown node per interval, asking it or, at
    /// random or when it has no URL, another node; up to 16 at once from
    /// another node.
    pub(super) fn refresh(&mut self) {
        if self.gossip.refresh_at_ms > self.now_ms {
            self.sweep();
            return;
        }
        self.gossip.refresh_at_ms = self.now_ms.saturating_add(self.config.refresh_interval_ms);
        self.sweep();
        let stale = self.stale_nodes();
        if stale.is_empty() {
            return;
        }
        let pick = usize::try_from(self.rng.next_u64() % (stale.len() as u64)).unwrap_or(0);
        let Some(&target) = stale.get(pick) else {
            return;
        };
        let has_url = self.table.get(&target).is_some_and(|e| e.link.is_some());
        let indirect = !has_url || self.rng.next_u64() % 100 < self.config.indirect_refresh_percent;
        // With nobody else to ask, a periphery node asks a bootstrap node at
        // the URL the configuration gives it: no node asks it to peer, so it
        // learns its first uplink's link state no other way. A core node
        // learns it from its sweeps, which ask a bootstrap node for its own
        // link state too while the node has no peers.
        let oracle = if indirect {
            self.pick_oracle(&target)
        } else {
            None
        };
        let direct = !indirect
            || (self.is_periphery() && oracle.is_none() && self.contact(&target).is_some());
        let (oracle, ids) = if !direct {
            let Some(oracle) = oracle else {
                return;
            };
            let others = stale
                .iter()
                .copied()
                .filter(|id| *id != oracle && *id != target);
            let ids: Vec<NodeId> = core::iter::once(target)
                .chain(others)
                .take(crate::wire::MAX_LINK_STATE_REQUEST_IDS)
                .collect();
            (oracle, ids)
        } else {
            (target, alloc::vec![target])
        };
        let timeout = self.config.request_timeout_ms;
        let content = Content::LinkStateRequest(ids.clone());
        if self
            .send(oracle, &content, Purpose::Refresh(ids.clone()), timeout)
            .is_none()
        {
            self.refresh_failed(&ids);
        }
    }

    /// With nothing stale, asks a peer (or a bootstrap node) for the link
    /// states of the next nodes in the table, round and round. A newer
    /// one replaces what is held; so every link state lost to the network
    /// is caught up within one sweep.
    fn sweep(&mut self) {
        if self.gossip.sweep_at_ms > self.now_ms {
            return;
        }
        self.gossip.sweep_at_ms = self.now_ms.saturating_add(self.config.sweep_interval_ms);
        let after = self.gossip.sweep_after;
        let own = self.own;
        // Downlinks' link states come from them alone, and are served to
        // nobody: no sweep asks for them.
        let swept = |id: &&NodeId| **id != own && !self.is_downlink(id);
        let candidates = self
            .table
            .keys()
            .filter(|id| swept(id) && after.is_none_or(|after| **id > after))
            .chain(self.table.keys().filter(swept));
        let mut ids: Vec<NodeId> = Vec::new();
        for id in candidates {
            if ids.contains(id) || ids.len() >= crate::wire::MAX_LINK_STATE_REQUEST_IDS {
                break;
            }
            ids.push(*id);
        }
        let Some(&last) = ids.last() else {
            return;
        };
        self.gossip.sweep_after = Some(last);
        let mut peers: Vec<NodeId> = self.core_peers().map(|(id, _)| *id).collect();
        let peered = !peers.is_empty();
        if !peered {
            peers = self
                .bootstrap
                .iter()
                .map(|node| node.id)
                .filter(|id| *id != own)
                .collect();
        }
        let count = peers.len() as u64;
        if count == 0 {
            return;
        }
        let pick = usize::try_from(self.rng.next_u64() % count).unwrap_or(0);
        let Some(&oracle) = peers.get(pick) else {
            return;
        };
        // A peer's own state comes with its heartbeats. While we have no
        // peers, the bootstrap node asked sends us none, and with no other
        // node to ask, nothing else would bring it: it is asked for its own
        // too, in the sweep whose window holds its ID, once a pass.
        if peered {
            ids.retain(|id| *id != oracle);
        }
        if ids.is_empty() {
            return;
        }
        let content = Content::LinkStateRequest(ids.clone());
        let timeout = self.config.request_timeout_ms;
        self.send(oracle, &content, Purpose::Sweep(ids), timeout);
    }

    /// When a node may next be refreshed: once one is stale and its own
    /// refresh is due. Nothing stale, nothing to wake for.
    pub(super) fn next_refresh(&self) -> Option<u64> {
        let (now, stale, future) = (self.now_ms, self.config.stale_ms, self.config.max_future_ms);
        self.table
            .iter()
            .filter(|(id, entry)| **id != self.own && !entry.is_downlink())
            .map(|(_, entry)| {
                let stale_at = entry.link.as_ref().map_or(0, |link| {
                    if link.is_fresh(now, stale, future) {
                        link.sequence.saturating_add(stale).saturating_add(1)
                    } else {
                        0
                    }
                });
                stale_at.max(entry.refresh_at_ms)
            })
            .min()
    }

    /// Nodes whose link state is missing or stale, and not in backoff.
    fn stale_nodes(&self) -> Vec<NodeId> {
        let (now, stale, future) = (self.now_ms, self.config.stale_ms, self.config.max_future_ms);
        self.table
            .iter()
            .filter(|(id, entry)| {
                **id != self.own
                    && !entry.is_downlink()
                    && entry.refresh_at_ms <= now
                    && entry
                        .link
                        .as_ref()
                        .is_none_or(|l| !l.is_fresh(now, stale, future))
            })
            .map(|(id, _)| *id)
            .collect()
    }

    /// A node to ask about `target`: one with a fresh link state, or a
    /// bootstrap node.
    fn pick_oracle(&mut self, target: &NodeId) -> Option<NodeId> {
        let (now, stale, future) = (self.now_ms, self.config.stale_ms, self.config.max_future_ms);
        let mut oracles: Vec<NodeId> = self
            .table
            .iter()
            .filter(|(id, entry)| {
                *id != target
                    && **id != self.own
                    && !entry.is_downlink()
                    && (entry.sources.contains(Sources::BOOTSTRAP)
                        || entry
                            .link
                            .as_ref()
                            .is_some_and(|l| l.is_fresh(now, stale, future)))
            })
            .map(|(id, _)| *id)
            .collect();
        oracles.sort_unstable();
        let count = oracles.len() as u64;
        if count == 0 {
            return None;
        }
        let index = usize::try_from(self.rng.next_u64() % count).unwrap_or(0);
        oracles.get(index).copied()
    }

    /// The answer to a refresh: link states, each of a node asked about.
    pub(super) fn on_refresh(
        &mut self,
        _oracle: NodeId,
        ids: &[NodeId],
        result: Result<Vec<u8>, Problem>,
    ) {
        let Ok(body) = result else {
            self.refresh_failed(ids);
            return;
        };
        let Ok(response) = LinkStateResponse::decode_exact(&body) else {
            self.refresh_failed(ids);
            return;
        };
        let mut missing: Vec<NodeId> = Vec::new();
        for asked in ids {
            // Only states of the nodes asked about count.
            let found = response
                .0
                .iter()
                .flatten()
                .find(|signed| signed.node_id == *asked);
            // Anything but a newer state, whether old, dropped, invalid or
            // missing, waits before asking again.
            let accepted = found.is_some_and(|signed| {
                self.process_link_state(signed, None, Retransmit::Never) == Processed::Accepted
            });
            if !accepted {
                missing.push(*asked);
            }
        }
        self.refresh_failed(&missing);
    }

    fn refresh_failed(&mut self, ids: &[NodeId]) {
        let (now, min, max) = (
            self.now_ms,
            self.config.min_backoff_ms,
            self.config.max_backoff_ms,
        );
        for id in ids {
            if let Some(entry) = self.table.get_mut(id) {
                entry.refresh_failures = entry.refresh_failures.saturating_add(1);
                entry.refresh_at_ms = now.saturating_add(backoff(min, max, entry.refresh_failures));
            }
        }
    }

    /// The answer to a sweep: whatever is newer replaces what is held.
    pub(super) fn on_sweep(&mut self, ids: &[NodeId], result: Result<Vec<u8>, Problem>) {
        let Ok(body) = result else {
            return;
        };
        let Ok(response) = LinkStateResponse::decode_exact(&body) else {
            return;
        };
        for signed in response.0.iter().flatten() {
            if ids.contains(&signed.node_id) {
                self.process_link_state(signed, None, Retransmit::Never);
            }
        }
    }

    /// A link state from its own subject, a peer: a heartbeat.
    pub(crate) fn heard_from(&mut self, node: &NodeId) {
        if self
            .table
            .get(node)
            .is_some_and(|entry| entry.peering().is_some())
        {
            self.saw(node);
        }
    }
}
