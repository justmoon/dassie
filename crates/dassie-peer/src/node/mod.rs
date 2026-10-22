//! The peer node: the protocol's state machine
//! (spec/areas/network.md#the-peer-node). It takes inbound requests, the
//! results of its own requests and the passing of time, and gives back
//! replies, requests to send, events, routes and what to persist.

mod gossip;
mod inbound;
mod lifecycle;
mod membership;
mod peering;
mod settle;
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
use crate::channel::{Channel, Retry};
use crate::config::Config;
use crate::io::{Event, Failure, Output, Request, RequestId, Response, Stats};
use crate::routing::Routes;
use crate::table::{Bucket, Entry, NetworkBuckets, Sources};
use crate::wire::{Authentication, Content, ResponseEnvelope, SchemeId, encode_envelope};

pub use lifecycle::BanError;
pub use membership::{RegisterError, RegistrationStatus};
pub use state::{NodeView, PeerStatus};

use membership::Lists;

/// Most events kept for the driver to poll.
const MAX_EVENTS: usize = 1024;

/// Most uplinks a periphery node can be pinned to: no more than the
/// peerings any valid configuration lets it start, since `peer_target` is
/// at least one and a node starts two more than its target
/// ([`Config::max_outbound`]).
pub const MAX_PINNED_UPLINKS: usize = 3;

/// What this node says about itself.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OwnInfo {
    /// Its HTTPS origin; without one, it can't publish a link state.
    pub url: Option<String>,
    /// Its alias.
    pub alias: String,
    /// Whether it has a QUIC endpoint: with a URL, one that takes the peer
    /// protocol on UDP at the URL's host and port; without one, one it
    /// dials out from, which makes it a periphery node
    /// (spec/areas/network.md#periphery-nodes).
    pub quic: bool,
    /// The UDP port of its rate feed, at its URL's host, if it publishes
    /// one (spec/areas/rate-feed.md#finding-a-feed). Only a node with a URL
    /// names it.
    pub feed_port: Option<u16>,
}

impl OwnInfo {
    /// Whether the node runs as a periphery node: it has a QUIC endpoint
    /// but no URL, so it dials its uplinks and is reached only over the
    /// connections it opened.
    #[must_use]
    pub const fn is_periphery(&self) -> bool {
        self.url.is_none() && self.quic
    }

    /// The same, without a URL that is not a valid node URL (too long, for
    /// one): a link state could not carry it, so the node goes without.
    #[must_use]
    pub fn checked(self) -> Self {
        let url = self.url.filter(|url| crate::wire::is_valid_url(url));
        let alias = if self.alias.len() <= crate::wire::MAX_ALIAS_BYTES {
            self.alias
        } else {
            String::new()
        };
        // A feed is at the URL's host: without a URL, there is none to
        // name, and port 0 is none.
        let feed_port = self.feed_port.filter(|port| *port != 0 && url.is_some());
        Self {
            url,
            alias,
            quic: self.quic,
            feed_port,
        }
    }
}

/// A configured bootstrap node (spec/areas/network.md#bootstrap-nodes).
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
            Self::Status(status) => matches!(status, 400 | 401 | 409 | 503),
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
    /// An ILP Prepare, by its request ID.
    Prepare(u32),
    /// An ILP Reject, by the request ID of the Prepare it answers, kept to
    /// be sent again.
    Response(Box<Answer>),
    /// A settlement message, by its sequence.
    Settlement(u64),
    /// Asking a peer to settle.
    SettlementRequest,
    /// Asking a peer to settle a last time, before we tell it we end the
    /// peering (spec/decisions/ending-a-peering.md).
    FinalRequest,
    /// Telling a peer we end the peering.
    Termination,
}

/// An ILP Reject for a peer: sent again until it arrives or `until_ms`, so
/// the peer need not wait for the packet's expiry.
#[derive(Clone, Debug)]
pub(crate) struct Answer {
    pub peer: NodeId,
    pub request_id: u32,
    pub packet: Vec<u8>,
    pub until_ms: u64,
}

/// How long after a failed try a response is sent again.
const ANSWER_RETRY_MS: u64 = 1_000;

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
    /// Responses waiting to be sent again.
    pub(crate) answers: Vec<(u64, Answer)>,
    /// Probes in flight: the node and the host probed.
    pub(crate) probing: Vec<(NodeId, String)>,
    pub(crate) probes: Bucket,
    /// Each network's share of the probes, spent by those that fail.
    pub(crate) network_probes: NetworkBuckets,
    pub(crate) outputs: VecDeque<Output>,
    events: VecDeque<Event>,
    pub(crate) dirty: BTreeSet<NodeId>,
    pub(crate) removed: Vec<NodeId>,
    pub(crate) registrations: Bucket,
    pub(crate) stats: Stats,
    /// When this run of the node started.
    pub(crate) started_ms: u64,
    /// Settlement channels, by node.
    pub(crate) channels: BTreeMap<NodeId, Channel>,
    pub(crate) channels_dirty: BTreeSet<NodeId>,
    /// How many of the channels are with downlinks: those without a URL.
    pub(crate) downlink_channels: usize,
    /// When each channel's outbox goes next.
    pub(crate) retries: BTreeMap<NodeId, Retry>,
    /// Banned nodes, and the changes not yet persisted.
    pub(crate) bans: BTreeSet<NodeId>,
    pub(crate) banned: Vec<NodeId>,
    pub(crate) unbanned: Vec<NodeId>,
    /// Peers we are telling that we end the peering, and when to try
    /// next (`u64::MAX` while a message is in flight).
    pub(crate) telling: BTreeMap<NodeId, u64>,
    /// Peers our messages surely do not reach: an ILP packet of ours to
    /// each surely did not arrive, and nothing we sent it since was
    /// answered (spec/decisions/unreachable-peers.md). Only peers are kept,
    /// so it holds at most one per peering.
    pub(crate) unreached: BTreeSet<NodeId>,
    /// The only nodes a periphery node asks to be its uplinks, sorted;
    /// empty for any (spec/areas/network.md#pinned-uplinks).
    pub(crate) pinned: Vec<NodeId>,
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
            own_info: own_info.checked(),
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
            answers: Vec::new(),
            probing: Vec::new(),
            probes: Bucket::full(config.probe_rate, now_ms),
            network_probes: NetworkBuckets::new(config.max_probe_networks),
            outputs: VecDeque::new(),
            events: VecDeque::new(),
            dirty: BTreeSet::new(),
            removed: Vec::new(),
            registrations: Bucket::full(config.registration_rate, now_ms),
            stats: Stats::default(),
            started_ms: now_ms,
            channels: BTreeMap::new(),
            channels_dirty: BTreeSet::new(),
            downlink_channels: 0,
            retries: BTreeMap::new(),
            bans: BTreeSet::new(),
            banned: Vec::new(),
            unbanned: Vec::new(),
            telling: BTreeMap::new(),
            unreached: BTreeSet::new(),
            pinned: Vec::new(),
            config,
        };
        node.restore(stored);
        // Bootstrap nodes, and nodes registered with us, keep their replay
        // state (spec/areas/peer-protocol.md#replay-protection).
        let registered = node
            .table
            .iter()
            .filter(|(_, entry)| entry.registration.is_some())
            .map(|(id, _)| *id);
        let pinned: Vec<NodeId> = node
            .bootstrap
            .iter()
            .map(|b| b.id)
            .chain(registered)
            .collect();
        for id in pinned {
            node.auth.pin(id);
        }
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
        let info = info.checked();
        if info != self.own_info {
            self.own_info = info;
            self.gossip.own_changed = true;
            self.end_downlinks();
        }
    }

    /// Changes the settlement schemes the node peers on, in order of
    /// preference: those it advertises, asks for and accepts peerings on.
    /// Peerings already made on a scheme left out stay.
    pub fn set_schemes(&mut self, now_ms: u64, schemes: Vec<SchemeId>) {
        self.advance(now_ms);
        if schemes != self.schemes {
            self.schemes = schemes;
            self.gossip.own_changed = true;
        }
    }

    /// The settlement schemes the node peers on, in order of preference.
    #[must_use]
    pub fn schemes(&self) -> &[SchemeId] {
        &self.schemes
    }

    /// Pins the uplinks: while it runs as a periphery node, the node asks
    /// only these nodes to peer, and looks for as many uplinks as it
    /// names. Empty for any core node, as usual. A core node does not use
    /// the list (spec/areas/network.md#pinned-uplinks).
    ///
    /// # Panics
    ///
    /// With more than [`MAX_PINNED_UPLINKS`], or more than the node may
    /// start peerings (configuration, not input).
    pub fn pin_uplinks(&mut self, mut uplinks: Vec<NodeId>) {
        assert!(
            uplinks.len() <= MAX_PINNED_UPLINKS && uplinks.len() <= self.config.max_outbound(),
            "too many pinned uplinks"
        );
        uplinks.sort_unstable();
        uplinks.dedup();
        self.pinned = uplinks;
    }

    /// The uplinks the node is pinned to, if it runs as a periphery node
    /// and has any.
    #[must_use]
    pub fn pinned_uplinks(&self) -> Option<&[NodeId]> {
        (self.is_periphery() && !self.pinned.is_empty()).then_some(self.pinned.as_slice())
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
        // Only a link state says a node takes QUIC; a bootstrap node known
        // only from the configuration is asked over HTTPS until its state
        // arrives. A periphery node is reached only over the connection it
        // opened, which its empty URL tells the transport.
        let quic = self
            .table
            .get(&to)
            .and_then(|entry| entry.link.as_ref())
            .is_some_and(|link| link.quic || link.periphery);
        self.outputs.push_back(Output::Request(Request {
            id,
            to,
            url,
            quic,
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
        self.note_reach(outstanding.to, &outstanding.purpose, &checked);
        if checked.is_err() {
            self.stats.requests_failed += 1;
        } else {
            self.settlement_reachable(&outstanding.to);
            if self.peering_of(&outstanding.to).is_some() {
                self.saw(&outstanding.to);
            }
        }
        self.dispatch_response(hooks, outstanding.to, outstanding.purpose, checked);
    }

    /// Whether our messages reach `node`, from what became of one of our
    /// requests to it (spec/decisions/unreachable-peers.md). An authenticated
    /// answer says they do. Only an ILP packet that surely did not arrive
    /// says they do not: the protocol's other requests may be refused here,
    /// by the bounds on requests in flight, which says nothing of the peer,
    /// while packets take no place in those bounds. A request that may have
    /// arrived says nothing either way.
    fn note_reach(&mut self, node: NodeId, purpose: &Purpose, result: &Result<Vec<u8>, Problem>) {
        let reachable = match (result, purpose) {
            (Ok(_), _) => true,
            (
                Err(Problem::Failed(Failure::NotDelivered)),
                Purpose::Prepare(_) | Purpose::Response(_),
            ) => false,
            (Err(_), _) => return,
        };
        if self.peering_of(&node).is_none() {
            return;
        }
        let changed = if reachable {
            self.unreached.remove(&node)
        } else {
            self.unreached.insert(node)
        };
        assert!(
            self.unreached.len() <= self.config.max_peers + self.config.max_downlinks,
            "more peers unreached than peerings"
        );
        if changed {
            self.event(Event::Reachable {
                peer: node,
                reachable,
            });
        }
    }

    /// Whether our messages reach `peer`, as far as we know: false once an
    /// ILP packet of ours to it surely did not arrive, until it answers a
    /// request of ours (spec/decisions/unreachable-peers.md).
    #[must_use]
    pub fn reachable(&self, peer: &NodeId) -> bool {
        !self.unreached.contains(peer)
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
            Purpose::SettlementRequest => {
                // Until another request comes back, the time the peer
                // stays short counts toward ending the peering only if this
                // one may have reached it
                // (spec/decisions/peering-lifecycle.md).
                let unreached = matches!(result, Err(Problem::Failed(Failure::NotDelivered)));
                if let Some(entry) = self.table.get_mut(&to) {
                    entry.advertise.settle_unreached = unreached;
                }
                match result {
                    Ok(body) => {
                        // An answer it cannot read says nothing of the amount.
                        let settling = crate::wire::decode_settling(&body).ok().flatten();
                        self.event(Event::SettlementRequestAnswered { peer: to, settling });
                    }
                    // Refused for not being peered: asked before the peer took
                    // in our acceptance. It may be asked again at once.
                    Err(Problem::Status(409)) => {
                        self.event(Event::SettlementRequestRefused { peer: to });
                    }
                    // It never reached the peer, so it says nothing of the peer
                    // and costs it nothing: asked again at once, so that a
                    // failed delivery does not use up the grace before a peer
                    // short of room is withdrawn
                    // (spec/areas/network.md#advertised-peers). A refusal or an
                    // overloaded peer (400, 401, 503) is asked again only after
                    // the interval.
                    Err(Problem::Failed(Failure::NotDelivered)) => {
                        self.event(Event::SettlementRequestNotDelivered { peer: to });
                    }
                    Err(_) => {}
                }
            }
            Purpose::HashPoll => self.on_hash(to, result),
            Purpose::ListDownload => self.on_list(to, result),
            Purpose::Registration => self.on_registered(to, result),
            Purpose::Refresh(ids) => self.on_refresh(to, &ids, result),
            Purpose::Sweep(ids) => self.on_sweep(&ids, result),
            Purpose::PeeringInfo => self.on_peering_info(hooks, to, result),
            Purpose::PeeringRequest => self.on_peering_response(hooks, to, result),
            Purpose::Probe(probe) => self.on_probe(hooks, to, probe, result),
            Purpose::Prepare(request_id) => {
                if let Err(problem) = result {
                    self.event(Event::PacketFailed {
                        peer: to,
                        request_id,
                        definite: problem.is_definite(),
                    });
                }
            }
            Purpose::Settlement(sequence) => self.on_settlement(to, sequence, &result),
            Purpose::FinalRequest => self.on_final_request(to, &result),
            Purpose::Termination => self.on_termination(to, &result),
            Purpose::Response(answer) => {
                // A refusal after reading it will not change; anything
                // else is tried again, and a copy that arrives twice is
                // dropped by the peer's connector. A 401 is tried again
                // too: a node that just started refuses messages dated
                // before its start, and the copy sent again is dated anew.
                // Each refusal is charged at the peer's rate limit, which
                // costs a short spell of 429s at worst, not a Fulfill lost
                // though booked (spec/areas/peer-protocol.md#message-set).
                let refused = matches!(result, Err(Problem::Status(400 | 409)));
                if result.is_err() && !refused {
                    self.retry_answer(*answer);
                } else if refused {
                    self.stats.answers_dropped += 1;
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
        self.retry_answers();
        self.retry_settlements();
        // Before our link state is issued, which says whom we advertise.
        self.review_peers(hooks);
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
            self.peering.next_due(self.next_peer_check()),
            self.next_review(),
            self.next_maintenance(),
            self.answers.iter().map(|(at, _)| *at).min(),
            self.next_settlement_retry(),
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
        self.send(*peer, &content, Purpose::Prepare(request_id), timeout)
            .map(|_| ())
            .ok_or(SendError::Busy)
    }

    /// Sends an ILP Fulfill or Reject to a peer, answering its Prepare
    /// `request_id`. It is sent again until it arrives or the longest a
    /// Prepare is held has passed; a copy that arrives twice is dropped by
    /// the peer. A Reject that never arrives has the effect of the packet's
    /// expiry; a Fulfill counts in our books whether or not the peer took
    /// it (spec/decisions/own-fulfills.md).
    pub fn send_response(&mut self, now_ms: u64, peer: &NodeId, request_id: u32, packet: &[u8]) {
        self.advance(now_ms);
        let answer = Answer {
            peer: *peer,
            request_id,
            packet: packet.to_vec(),
            until_ms: self.now_ms.saturating_add(self.config.request_timeout_ms),
        };
        self.try_answer(answer);
    }

    fn try_answer(&mut self, answer: Answer) {
        if self.peering_of(&answer.peer).is_none() || answer.until_ms <= self.now_ms {
            self.stats.answers_dropped += 1;
            return;
        }
        let content = Content::InterledgerPacket {
            request_id: answer.request_id,
            packet: &answer.packet,
        };
        let timeout = answer
            .until_ms
            .saturating_sub(self.now_ms)
            .min(self.config.request_timeout_ms);
        let peer = answer.peer;
        let purpose = Purpose::Response(Box::new(answer.clone()));
        if self.send(peer, &content, purpose, timeout).is_none() {
            self.retry_answer(answer);
        }
    }

    /// Queues a response to be tried again soon, if there is room.
    fn retry_answer(&mut self, answer: Answer) {
        if self.answers.len() >= self.config.max_requests || answer.until_ms <= self.now_ms {
            self.stats.answers_dropped += 1;
            return;
        }
        let at = self.now_ms.saturating_add(ANSWER_RETRY_MS);
        self.answers.push((at, answer));
    }

    /// Sends the responses whose retry is due.
    fn retry_answers(&mut self) {
        let now = self.now_ms;
        let (due, waiting): (Vec<_>, Vec<_>) = core::mem::take(&mut self.answers)
            .into_iter()
            .partition(|(at, _)| *at <= now);
        self.answers = waiting;
        for (_, answer) in due {
            self.try_answer(answer);
        }
    }

    /// Ensures `node` is in the table with `source`, making room by
    /// removing a node with a weaker reason if needed. Returns whether it
    /// is in the table. Downlinks have a bound of their own: they neither
    /// make room nor are made room for (spec/areas/network.md#downlinks).
    pub(crate) fn ensure(&mut self, node: NodeId, source: Sources) -> bool {
        let downlink = source == Sources::DOWNLINK;
        let downlinks_full = downlink && self.downlinks() >= self.config.max_downlinks;
        if let Some(entry) = self.table.get_mut(&node) {
            if !entry.sources.contains(source) {
                if downlinks_full {
                    return false;
                }
                entry.sources.insert(source);
                entry.orphaned_ms = None;
                self.dirty.insert(node);
            }
            return true;
        }
        let room = if downlink {
            !downlinks_full
        } else {
            self.table.len() - self.downlinks() < self.config.max_nodes || self.evict_for(source)
        };
        if !room {
            return false;
        }
        let rate = self.config.link_state_rate;
        self.removed.retain(|removed| *removed != node);
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
            .filter(|(_, entry)| !entry.is_downlink() && entry.sources.rank() < source.rank())
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

    /// Takes away the reason a peer is in the table, a downlink's too.
    pub(crate) fn drop_peer_source(&mut self, node: &NodeId) {
        self.drop_source(node, Sources::PEER);
        self.drop_downlink(node);
    }

    /// Takes away `node`'s place among the downlinks: every path that ends
    /// one, refused, ended or dropped, comes here. A downlink took no core
    /// place, so the entry stays only if it gets one now: within the
    /// bound, or, kept for another reason, in place of a weaker entry.
    /// Otherwise it goes at once, its replay state kept while a channel
    /// needs it (spec/areas/network.md#downlinks).
    pub(crate) fn drop_downlink(&mut self, node: &NodeId) {
        if !self.is_downlink(node) {
            return;
        }
        assert!(
            self.peering_of(node).is_none(),
            "a downlink leaves once its peering is over"
        );
        self.drop_source(node, Sources::DOWNLINK);
        let sources = self.table.get(node).map(|entry| entry.sources);
        let core = self.table.len() - self.downlinks();
        let fits = core <= self.config.max_nodes
            || sources.is_some_and(|sources| !sources.is_empty() && self.evict_for(sources));
        if !fits {
            self.remove(node);
        }
    }

    /// The reason a peering with `node` keeps it in the table.
    pub(crate) fn peer_source(&self, node: &NodeId) -> Sources {
        if self.is_downlink(node) {
            Sources::DOWNLINK
        } else {
            Sources::PEER
        }
    }

    /// Whether `node` is our downlink, or asking to be.
    #[must_use]
    pub fn is_downlink(&self, node: &NodeId) -> bool {
        self.table.get(node).is_some_and(Entry::is_downlink)
    }

    /// Whether this node runs as a periphery node: it peers only with core
    /// nodes, as their downlink, and never forwards
    /// (spec/areas/network.md#periphery-nodes).
    #[must_use]
    pub const fn is_periphery(&self) -> bool {
        self.own_info.is_periphery()
    }

    /// Whether this node takes downlinks: a core node that takes QUIC, on
    /// which they reach it, with room for any.
    pub(crate) const fn takes_downlinks(&self) -> bool {
        self.own_info.url.is_some() && self.own_info.quic && self.config.max_downlinks > 0
    }

    /// The downlinks in the table.
    pub(crate) fn downlinks(&self) -> usize {
        self.table
            .values()
            .filter(|entry| entry.is_downlink())
            .count()
    }

    pub(crate) fn remove(&mut self, node: &NodeId) {
        // Out of the majority set too, so the node is added again if it
        // still has a majority.
        self.lists.majority.remove(node);
        if self.table.remove(node).is_some() {
            // A registered node evicted for a stronger reason keeps no
            // replay state for good.
            if !self.keeps_replay_state(node) {
                self.auth.unpin(self.now_ms, node);
            }
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
