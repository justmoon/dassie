//! The peer node: the protocol's state machine (spec 3 §9–§13, spec 7
//! §4.7). It takes inbound requests, the results of its own requests and
//! the passing of time, and gives back replies, requests to send, events,
//! routes and what to persist.

mod gossip;
mod inbound;
mod membership;
mod peering;
mod state;

use alloc::boxed::Box;
use alloc::collections::{BTreeMap, BTreeSet, VecDeque};
use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec::Vec;

use dassie_identity::{NodeId, NodeKey, PublicKey};
use dassie_oer::Encode;
use rand_core::CryptoRng;

use crate::auth::{Authenticator, Pending};
use crate::config::Config;
use crate::io::{Event, Failure, Output, Request, RequestId, Response, Stats};
use crate::routing::Routes;
use crate::table::{Bucket, Entry, Sources};
use crate::wire::{Authentication, Content, ResponseEnvelope, SchemeId, encode_envelope};

pub use membership::RegistrationStatus;
pub use state::NodeView;

use membership::Lists;

/// Most events kept for the driver to poll.
const MAX_EVENTS: usize = 1024;

/// What this node says about itself.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OwnInfo {
    /// Its HTTPS origin; without one, it can't publish a link state.
    pub url: Option<String>,
    /// Its alias.
    pub alias: String,
}

/// A configured bootstrap node (spec 3 §11.1).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Bootstrap {
    /// Its ID, checked against its key.
    pub id: NodeId,
    /// Its URL.
    pub url: String,
    /// Its key.
    pub key: PublicKey,
}

/// Why a request of ours got nothing usable back.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Problem {
    /// An HTTP status other than 200.
    Status(u16),
    /// No response.
    Failed(Failure),
    /// A response that is malformed or not authentic.
    Invalid,
}

impl Problem {
    /// Whether the request surely had no effect at the other end.
    pub(crate) const fn is_definite(self) -> bool {
        match self {
            Self::Status(status) => matches!(status, 400..=499 | 503),
            Self::Failed(failure) => matches!(failure, Failure::NotDelivered),
            Self::Invalid => false,
        }
    }
}

/// What a request of ours was for.
#[derive(Clone, Debug)]
pub(crate) enum Purpose {
    /// Flooding, heartbeats, pushes: nothing to do with the answer.
    Gossip,
    /// A node-list hash poll.
    HashPoll,
    /// A node-list download.
    ListDownload,
    /// Registering with a bootstrap node.
    Registration,
    /// Refreshing these nodes' link states.
    Refresh(Vec<NodeId>),
    /// Sweeping these nodes' link states.
    Sweep(Vec<NodeId>),
    /// Asking a node for its peering info.
    PeeringInfo,
    /// Asking a node to peer.
    PeeringRequest,
    /// Checking that an inbound request's sender is reachable at its URL.
    Probe(Box<inbound::Probe>),
    /// An ILP packet.
    Packet(u32),
}

/// A request of ours in flight.
pub(crate) struct Outstanding {
    pub to: NodeId,
    pub purpose: Purpose,
    pub pending: Pending,
    pub deadline_ms: u64,
}

/// The peer protocol's state machine.
pub struct PeerNode {
    pub(crate) config: Config,
    pub(crate) auth: Authenticator,
    pub(crate) own: NodeId,
    pub(crate) own_info: OwnInfo,
    pub(crate) schemes: Vec<SchemeId>,
    pub(crate) rng: Box<dyn CryptoRng + Send>,
    pub(crate) now_ms: u64,
    pub(crate) table: BTreeMap<NodeId, Entry>,
    pub(crate) bootstrap: Vec<Bootstrap>,
    pub(crate) lists: Lists,
    pub(crate) gossip: gossip::Gossip,
    pub(crate) peering: peering::Peering,
    pub(crate) routes: Routes,
    pub(crate) requests: BTreeMap<RequestId, Outstanding>,
    pub(crate) deadlines: BTreeSet<(u64, RequestId)>,
    next_request: u64,
    pub(crate) deferred: usize,
    pub(crate) outputs: VecDeque<Output>,
    events: VecDeque<Event>,
    pub(crate) dirty: BTreeSet<NodeId>,
    pub(crate) removed: Vec<NodeId>,
    pub(crate) registrations: Bucket,
    pub(crate) stats: Stats,
}

impl PeerNode {
    /// Starts from what was persisted.
    ///
    /// # Panics
    ///
    /// If the configuration does not fit together.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        config: Config,
        key: Arc<NodeKey>,
        own_info: OwnInfo,
        bootstrap: Vec<Bootstrap>,
        stored: crate::persist::Stored,
        rng: Box<dyn CryptoRng + Send>,
        now_ms: u64,
    ) -> Self {
        config.validate();
        let own = key.node_id();
        let mut node = Self {
            auth: Authenticator::new(Arc::clone(&key), config.auth, now_ms),
            own,
            own_info,
            schemes: Vec::new(),
            rng,
            now_ms,
            table: BTreeMap::new(),
            lists: Lists::new(&bootstrap),
            bootstrap,
            gossip: gossip::Gossip::new(now_ms, stored.own_sequence),
            peering: peering::Peering::new(now_ms),
            routes: Routes::new(),
            requests: BTreeMap::new(),
            deadlines: BTreeSet::new(),
            next_request: 0,
            deferred: 0,
            outputs: VecDeque::new(),
            events: VecDeque::new(),
            dirty: BTreeSet::new(),
            removed: Vec::new(),
            registrations: Bucket::full(config.registration_rate, now_ms),
            stats: Stats::default(),
            config,
        };
        node.restore(stored);
        node
    }

    /// Our node ID.
    #[must_use]
    pub const fn own(&self) -> &NodeId {
        &self.own
    }

    /// What has been counted.
    #[must_use]
    pub const fn stats(&self) -> &Stats {
        &self.stats
    }

    /// The routes: each reachable node's first hops.
    #[must_use]
    pub const fn routes(&self) -> &Routes {
        &self.routes
    }

    /// The configuration.
    #[must_use]
    pub const fn config(&self) -> &Config {
        &self.config
    }

    /// Changes what we say about ourselves: a new link state follows
    /// (NET-6).
    pub fn set_own_info(&mut self, now_ms: u64, info: OwnInfo) {
        self.advance(now_ms);
        if info != self.own_info {
            self.own_info = info;
            self.gossip.own_changed = true;
        }
    }

    /// Changes the active settlement schemes, in order of preference.
    pub fn set_schemes(&mut self, now_ms: u64, schemes: Vec<SchemeId>) {
        self.advance(now_ms);
        if schemes != self.schemes {
            self.schemes = schemes;
            self.gossip.own_changed = true;
        }
    }

    /// Moves time on; it never goes back.
    pub(crate) fn advance(&mut self, now_ms: u64) {
        self.now_ms = self.now_ms.max(now_ms);
    }

    /// The next output for the driver.
    pub fn poll_output(&mut self) -> Option<Output> {
        self.outputs.pop_front()
    }

    /// The next event.
    pub fn poll_event(&mut self) -> Option<Event> {
        self.events.pop_front()
    }

    pub(crate) fn event(&mut self, event: Event) {
        if self.events.len() >= MAX_EVENTS {
            self.events.pop_front();
        }
        self.events.push_back(event);
    }

    /// The key and URL to reach `node`: from its link state, or the
    /// bootstrap list.
    pub(crate) fn contact(&self, node: &NodeId) -> Option<(PublicKey, String)> {
        if let Some(entry) = self.table.get(node)
            && let (Some(key), Some(link)) = (entry.key, entry.link.as_ref())
        {
            return Some((key, link.url.clone()));
        }
        self.bootstrap
            .iter()
            .find(|bootstrap| bootstrap.id == *node)
            .map(|bootstrap| (bootstrap.key, bootstrap.url.clone()))
    }

    /// Sends `content` to `to`, authenticated. Returns `None` when it
    /// cannot: no contact, ourselves, or too much in flight.
    pub(crate) fn send(
        &mut self,
        to: NodeId,
        content: &Content<'_>,
        purpose: Purpose,
        timeout_ms: u64,
    ) -> Option<RequestId> {
        let (key, url) = self.contact(&to)?;
        self.send_via(to, key, url, content, purpose, timeout_ms)
    }

    /// Sends `content` to `to` at `url`, authenticated for `key`.
    pub(crate) fn send_via(
        &mut self,
        to: NodeId,
        key: PublicKey,
        url: String,
        content: &Content<'_>,
        purpose: Purpose,
        timeout_ms: u64,
    ) -> Option<RequestId> {
        if to == self.own {
            return None;
        }
        if self.requests.len() >= self.config.max_requests {
            self.stats.requests_dropped += 1;
            return None;
        }
        let bytes = content.to_vec();
        let (hmac, pending) = self
            .auth
            .authenticate(self.now_ms, self.rng.as_mut(), &to, &key, &bytes)
            .ok()?;
        let body = encode_envelope(&self.own, &Authentication::Hmac(hmac), &bytes);
        let id = RequestId(self.next_request);
        self.next_request += 1;
        let deadline_ms = self.now_ms.saturating_add(timeout_ms);
        self.requests.insert(
            id,
            Outstanding {
                to,
                purpose,
                pending,
                deadline_ms,
            },
        );
        self.deadlines.insert((deadline_ms, id));
        self.outputs.push_back(Output::Request(Request {
            id,
            to,
            url,
            kind: content.kind(),
            body,
            timeout_ms,
        }));
        self.stats.requests_sent += 1;
        Some(id)
    }

    /// The result of one of our requests.
    pub fn handle_response(
        &mut self,
        now_ms: u64,
        hooks: &mut dyn crate::hooks::PeeringHooks,
        id: RequestId,
        result: Result<Response, Failure>,
    ) {
        self.advance(now_ms);
        let Some(outstanding) = self.requests.remove(&id) else {
            return;
        };
        self.deadlines.remove(&(outstanding.deadline_ms, id));
        let body = match result {
            Ok(response) if response.status == 200 => Ok(response.body),
            Ok(response) => Err(Problem::Status(response.status)),
            Err(failure) => Err(Problem::Failed(failure)),
        };
        let checked = body.and_then(|body| {
            let envelope = ResponseEnvelope::decode(&body).map_err(|_| Problem::Invalid)?;
            outstanding
                .pending
                .check(&envelope)
                .map_err(|_| Problem::Invalid)?;
            Ok(envelope.body.to_vec())
        });
        if checked.is_err() {
            self.stats.requests_failed += 1;
        } else if self.peering_of(&outstanding.to).is_some() {
            self.saw(&outstanding.to);
        }
        self.dispatch_response(hooks, outstanding.to, outstanding.purpose, checked);
    }

    fn dispatch_response(
        &mut self,
        hooks: &mut dyn crate::hooks::PeeringHooks,
        to: NodeId,
        purpose: Purpose,
        result: Result<Vec<u8>, Problem>,
    ) {
        match purpose {
            Purpose::Gossip => {}
            Purpose::HashPoll => self.on_hash(to, result),
            Purpose::ListDownload => self.on_list(to, result),
            Purpose::Registration => self.on_registered(to, result),
            Purpose::Refresh(ids) => self.on_refresh(to, &ids, result),
            Purpose::Sweep(ids) => self.on_sweep(&ids, result),
            Purpose::PeeringInfo => self.on_peering_info(hooks, to, result),
            Purpose::PeeringRequest => self.on_peering_response(hooks, to, result),
            Purpose::Probe(probe) => self.on_probe(hooks, to, probe, result),
            Purpose::Packet(request_id) => {
                if let Err(problem) = result {
                    self.event(Event::PacketFailed {
                        peer: to,
                        request_id,
                        definite: problem.is_definite(),
                    });
                }
            }
        }
    }

    /// Fails requests whose time is up, as if the driver had said so.
    fn expire_requests(&mut self, hooks: &mut dyn crate::hooks::PeeringHooks) {
        while let Some(&(deadline, id)) = self.deadlines.first() {
            if deadline > self.now_ms {
                break;
            }
            self.handle_response(self.now_ms, hooks, id, Err(Failure::Unknown));
            self.deadlines.remove(&(deadline, id));
        }
    }

    /// Does what is due.
    pub fn handle_timeout(&mut self, now_ms: u64, hooks: &mut dyn crate::hooks::PeeringHooks) {
        self.advance(now_ms);
        self.expire_requests(hooks);
        self.issue_own();
        self.flood();
        self.heartbeat();
        self.poll_lists();
        self.register_with_bootstrap();
        self.refresh();
        self.maintain();
        self.seek_peers(hooks);
        self.update_routes();
    }

    /// The next time something is due.
    #[must_use]
    pub fn poll_timeout(&self) -> Option<u64> {
        let candidates = [
            self.deadlines.first().map(|(deadline, _)| *deadline),
            self.gossip.next_due(self),
            self.lists.next_due(),
            self.registration_due(),
            self.peering.next_due(),
            self.next_maintenance(),
        ];
        candidates.into_iter().flatten().min()
    }

    /// Sends an ILP packet to a peer.
    ///
    /// # Errors
    ///
    /// The node is not a peer, or too much is in flight.
    pub fn send_packet(
        &mut self,
        now_ms: u64,
        peer: &NodeId,
        request_id: u32,
        packet: &[u8],
        timeout_ms: u64,
    ) -> Result<(), SendError> {
        self.advance(now_ms);
        if self.peering_of(peer).is_none() {
            return Err(SendError::NotPeered);
        }
        let content = Content::InterledgerPacket { request_id, packet };
        let timeout = timeout_ms.min(self.config.request_timeout_ms);
        self.send(*peer, &content, Purpose::Packet(request_id), timeout)
            .map(|_| ())
            .ok_or(SendError::Busy)
    }

    /// Ensures `node` is in the table with `source`, making room by
    /// removing a node with a weaker reason if needed. Returns whether it
    /// is in the table.
    pub(crate) fn ensure(&mut self, node: NodeId, source: Sources) -> bool {
        if let Some(entry) = self.table.get_mut(&node) {
            if !entry.sources.contains(source) {
                entry.sources.insert(source);
                entry.orphaned_ms = None;
                self.dirty.insert(node);
            }
            return true;
        }
        if self.table.len() >= self.config.max_nodes && !self.evict_for(source) {
            return false;
        }
        let rate = self.config.link_state_rate;
        self.table
            .insert(node, Entry::new(source, self.now_ms, rate));
        self.dirty.insert(node);
        self.event(Event::NodeAdded(node));
        true
    }

    /// Removes the node with the weakest reason to stay, if weaker than
    /// `source`; of those, the one that came last.
    fn evict_for(&mut self, source: Sources) -> bool {
        let victim = self
            .table
            .iter()
            .filter(|(_, entry)| entry.sources.rank() < source.rank())
            .min_by_key(|(_, entry)| (entry.sources.rank(), u64::MAX - entry.first_seen_ms))
            .map(|(id, _)| *id);
        victim.is_some_and(|id| {
            self.remove(&id);
            true
        })
    }

    /// Takes `source` away from `node`; with none left, it is removed
    /// after the grace period.
    pub(crate) fn drop_source(&mut self, node: &NodeId, source: Sources) {
        if let Some(entry) = self.table.get_mut(node)
            && entry.sources.contains(source)
        {
            entry.sources.remove(source);
            if entry.sources.is_empty() {
                entry.orphaned_ms = Some(self.now_ms);
            }
            self.dirty.insert(*node);
        }
    }

    pub(crate) fn remove(&mut self, node: &NodeId) {
        if self.table.remove(node).is_some() {
            self.dirty.remove(node);
            self.removed.push(*node);
            self.gossip.routes_dirty = true;
            self.event(Event::NodeRemoved(*node));
        }
    }
}

/// Why a packet could not be sent.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SendError {
    /// The node is not a peer.
    NotPeered,
    /// Too many requests are in flight.
    Busy,
}
