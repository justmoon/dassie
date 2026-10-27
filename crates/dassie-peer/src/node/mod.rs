//! The peer node: the protocol's state machine
//! (spec/areas/network.md#the-peer-node). It takes inbound requests, the
//! results of its own requests and the passing of time, and gives back
//! replies, requests to send, events, routes and what to persist.

mod gossip;
mod inbound;
mod lifecycle;
mod membership;
mod names;
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

use crate::channel::{Channel, Closed, Retry};
use crate::config::Config;
use crate::io::{Event, Failure, Output, Request, RequestId, Response, Stats};
use crate::names::{Commitment, Holders, Name, Registrant, Registrar};
use crate::routing::{RouteKey, Routes};
use crate::table::{Bucket, Entry, NetworkBuckets, Sources};
use crate::wire::{
    Content, HubList, HubListing, HubMembership, MAX_CLAIM_BYTES, MAX_COMMITMENT_BYTES, SchemeId,
    encode_envelope,
};

pub use lifecycle::BanError;
pub use membership::{RegisterError, RegistrationStatus};
pub use state::{NodeView, PeerStatus};

use membership::Lists;

pub(crate) use gossip::own_link_state_fits;

/// Most events kept for the driver to poll.
const MAX_EVENTS: usize = 1024;

/// Most uplinks a periphery node can be pinned to, as a market maker's
/// node that trades through many core nodes is
/// (spec/areas/network.md#pinned-uplinks). Each takes a peering slot of
/// the node's own, an address beneath the uplink in its connector and a
/// settlement channel, so the node's bounds hold this many core peers
/// ([`Config::max_core_peers`]).
pub const MAX_UPLINKS: usize = 128;

/// Most settlement schemes a node peers on, each listed in its link
/// state: with them, its link state always fits what every node decodes
/// ([`Config::validate`]).
pub const MAX_SCHEMES: usize = 16;

/// Most settlement hubs a node is a member of, each listed in its link
/// state: one per asset, for the few assets hubs settle
/// (spec/areas/peer-protocol.md#link-states). Within the protocol's
/// [`MAX_HUBS`](crate::wire::MAX_HUBS), which every node decodes.
pub const MAX_OWN_HUBS: usize = 4;

const _: () = assert!(MAX_OWN_HUBS <= crate::wire::MAX_HUBS);

/// What this node says about itself.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OwnInfo {
    /// Its HTTPS origin, whose host and port its QUIC endpoint takes the
    /// peer protocol at; without one, it is a periphery node, which dials
    /// out to its uplinks (spec/areas/network.md#periphery-nodes).
    pub url: Option<String>,
    /// Its alias.
    pub alias: String,
    /// The UDP port of its rate feed, at its URL's host, if it publishes
    /// one (spec/areas/rate-feed.md#finding-a-feed). Only a node with a URL
    /// names it.
    pub feed_port: Option<u16>,
    /// The settlement hub it is a member of, if any, which its link state
    /// names (spec/designs/settlement-hubs.md#11-choosing-a-hub): the first
    /// of its hubs ([`PeerNode::set_own_hubs`]), which the hub extension
    /// names for nodes that read no hub list. The first of its hubs set
    /// here, with another account or `takes_peers`, changes it in the list;
    /// any other hub set here replaces them, and is named alone, without a
    /// list.
    pub hub: Option<HubMembership>,
}

impl OwnInfo {
    /// Whether the node runs as a periphery node: it has no URL, so it
    /// dials its uplinks and is reached only over the connections it
    /// opened.
    #[must_use]
    pub const fn is_periphery(&self) -> bool {
        self.url.is_none()
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
            feed_port,
            hub: self.hub,
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
    /// A response that is malformed.
    Invalid,
}

impl Problem {
    /// Whether the request surely had no effect at the other end.
    pub(crate) const fn is_definite(self) -> bool {
        match self {
            Self::Status(status) => matches!(status, 400 | 409 | 503),
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
    /// A name-list hash poll.
    NameHashPoll,
    /// A name-list download.
    NameListDownload,
    /// Our commitment to a name, at a registrar.
    NameCommit(Commitment),
    /// Our unsealing of the commitment, at a registrar.
    NameUnseal(Commitment),
    /// Our renewal of a name, at a registrar.
    NameRenew(Name),
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
    pub deadline_ms: u64,
}

/// The peer protocol's state machine.
pub struct PeerNode {
    pub(crate) config: Config,
    /// Our node key, which signs our link state.
    pub(crate) key: Arc<NodeKey>,
    pub(crate) own: NodeId,
    pub(crate) own_info: OwnInfo,
    /// The hubs our link state lists, each with its scheme, at most
    /// [`MAX_OWN_HUBS`]; empty for none, or for a hub named only in
    /// `own_info`, without its scheme.
    pub(crate) own_hubs: Vec<HubListing>,
    pub(crate) schemes: Vec<SchemeId>,
    pub(crate) rng: Box<dyn CryptoRng + Send>,
    pub(crate) now_ms: u64,
    pub(crate) table: BTreeMap<NodeId, Entry>,
    pub(crate) bootstrap: Vec<Bootstrap>,
    pub(crate) lists: Lists,
    pub(crate) gossip: gossip::Gossip,
    pub(crate) peering: peering::Peering,
    pub(crate) routes: Routes,
    /// What ranks first hops for the cut (spec/areas/network.md#routing).
    pub(crate) route_key: RouteKey,
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
    /// Every event ever queued, wrapping: how a step tells how many it
    /// queued, whatever the queue dropped.
    events_queued: u64,
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
    /// When each channel last carried a settlement, either way, or its
    /// peering ended, in this run: one idle for long enough closes
    /// (spec/decisions/closing-channels.md). Not persisted, so a restart
    /// starts every wait again.
    pub(crate) channel_active: BTreeMap<NodeId, u64>,
    /// Closed channels not written yet, and those read back for a
    /// settlement that came over them closed: both go with the next batch.
    pub(crate) closed: BTreeMap<NodeId, Closed>,
    /// When the channels idle long enough are next looked at to close.
    pub(crate) channel_check_at_ms: u64,
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
    /// Nodes told lately that they hold a peering we do not, and when
    /// (spec/areas/network.md#one-sided-peerings).
    pub(crate) told_not_peered: BTreeMap<NodeId, u64>,
    /// The only nodes a periphery node asks to be its uplinks, sorted;
    /// empty for any (spec/areas/network.md#pinned-uplinks).
    pub(crate) pinned: Vec<NodeId>,
    /// The registrar's state, if this node is one of its own bootstrap
    /// nodes (spec/areas/network.md#names).
    pub(crate) registrar: Option<Registrar>,
    /// Who holds each name, from the registrars' lists
    /// (spec/areas/network.md#holders).
    pub(crate) holders: Holders,
    /// The registrars whose name lists we are polling or downloading.
    pub(crate) name_polling: BTreeSet<NodeId>,
    /// Our registrations of names, and the renewals of those we hold
    /// (spec/areas/network.md#the-registrant).
    pub(crate) registrant: Registrant,
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
        mut stored: crate::persist::Stored,
        rng: Box<dyn CryptoRng + Send>,
        now_ms: u64,
    ) -> Self {
        config.validate();
        let own = key.node_id();
        let mut node = Self {
            key: Arc::clone(&key),
            own,
            own_info: own_info.checked(),
            own_hubs: Vec::new(),
            schemes: Vec::new(),
            rng,
            now_ms,
            table: BTreeMap::new(),
            lists: Lists::new(&bootstrap),
            holders: names::holders(&config, &bootstrap, core::mem::take(&mut stored.holders)),
            registrant: names::registrant(
                &config,
                own,
                &bootstrap,
                core::mem::take(&mut stored.registrations),
            ),
            bootstrap,
            gossip: gossip::Gossip::new(now_ms, stored.own_sequence),
            peering: peering::Peering::new(now_ms),
            routes: Routes::new(),
            route_key: RouteKey::of(&key),
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
            events_queued: 0,
            dirty: BTreeSet::new(),
            removed: Vec::new(),
            registrations: Bucket::full(config.registration_rate, now_ms),
            stats: Stats::default(),
            started_ms: now_ms,
            channels: BTreeMap::new(),
            channels_dirty: BTreeSet::new(),
            downlink_channels: 0,
            retries: BTreeMap::new(),
            channel_active: BTreeMap::new(),
            closed: BTreeMap::new(),
            channel_check_at_ms: now_ms,
            bans: BTreeSet::new(),
            banned: Vec::new(),
            unbanned: Vec::new(),
            telling: BTreeMap::new(),
            unreached: BTreeSet::new(),
            told_not_peered: BTreeMap::new(),
            pinned: Vec::new(),
            registrar: None,
            name_polling: BTreeSet::new(),
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

    /// What ranks first hops where a route keeps only some
    /// (spec/areas/network.md#routing).
    #[must_use]
    pub const fn route_key(&self) -> &RouteKey {
        &self.route_key
    }

    /// The configuration.
    #[must_use]
    pub const fn config(&self) -> &Config {
        &self.config
    }

    /// What we say about ourselves.
    #[must_use]
    pub const fn own_info(&self) -> &OwnInfo {
        &self.own_info
    }

    /// Changes what we say about ourselves: a new link state follows.
    pub fn set_own_info(&mut self, now_ms: u64, info: OwnInfo) {
        self.advance(now_ms);
        let info = info.checked();
        if info != self.own_info {
            // The first of our hubs, with its membership changed, keeps
            // the list; any other hub is named alone.
            if let (Some(hub), Some(first)) = (info.hub, self.own_hubs.first_mut())
                && hub.hub_id == first.membership.hub_id
            {
                first.membership = hub;
            } else {
                self.own_hubs.clear();
            }
            self.own_info = info;
            self.gossip.own_changed = true;
            self.end_downlinks();
        }
    }

    /// The settlement hubs our link state lists, each with its scheme.
    #[must_use]
    pub fn own_hubs(&self) -> &[HubListing] {
        &self.own_hubs
    }

    /// Changes the settlement hubs we are a member of, each with the scheme
    /// we settle on over it, in order: our link state lists them all, and
    /// names the first in the hub extension too, for nodes that read no
    /// list, as [`OwnInfo::hub`]. A new link state follows.
    ///
    /// # Panics
    ///
    /// With more than [`MAX_OWN_HUBS`], or a scheme or hub twice (the
    /// node's own, not input).
    pub fn set_own_hubs(&mut self, now_ms: u64, hubs: Vec<HubListing>) {
        assert!(hubs.len() <= MAX_OWN_HUBS, "too many hubs");
        assert!(
            hubs.is_empty() || HubList(hubs.clone()).is_valid(),
            "no scheme or hub twice"
        );
        self.advance(now_ms);
        let hub = hubs.first().map(|listing| listing.membership);
        if hubs != self.own_hubs || hub != self.own_info.hub {
            self.own_hubs = hubs;
            self.own_info.hub = hub;
            self.gossip.own_changed = true;
        }
    }

    /// Changes the settlement schemes the node peers on, in order of
    /// preference: those it advertises, asks for and accepts peerings on.
    /// Peerings already made on a scheme left out stay.
    ///
    /// # Panics
    ///
    /// With more than [`MAX_SCHEMES`] (the node's own, not input).
    pub fn set_schemes(&mut self, now_ms: u64, schemes: Vec<SchemeId>) {
        assert!(schemes.len() <= MAX_SCHEMES, "too many schemes");
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
    /// With more than [`MAX_UPLINKS`] (configuration, not input).
    pub fn pin_uplinks(&mut self, mut uplinks: Vec<NodeId>) {
        assert!(uplinks.len() <= MAX_UPLINKS, "too many pinned uplinks");
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
        self.events_queued = self.events_queued.wrapping_add(1);
    }

    /// The count of events queued so far, to compare with
    /// [`events_since`](Self::events_since).
    pub(crate) const fn events_queued(&self) -> u64 {
        self.events_queued
    }

    /// How many events were queued since `before`, a count of
    /// [`events_queued`](Self::events_queued).
    pub(crate) const fn events_since(&self, before: u64) -> u64 {
        self.events_queued.wrapping_sub(before)
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

    /// Sends `content` to `to`. Returns `None` when it cannot: no contact,
    /// ourselves, or too much in flight.
    pub(crate) fn send(
        &mut self,
        to: NodeId,
        content: &Content<'_>,
        purpose: Purpose,
        timeout_ms: u64,
    ) -> Option<RequestId> {
        let (_, url) = self.contact(&to)?;
        self.send_via(to, url, content, purpose, timeout_ms)
    }

    /// Sends `content` to `to` at `url`. The transport pins the connection
    /// to `to`'s key, so only `to` answers, and proves ours to it
    /// (spec/areas/peer-protocol.md#authentication).
    pub(crate) fn send_via(
        &mut self,
        to: NodeId,
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
        let body = encode_envelope(&content.to_vec());
        let id = RequestId(self.next_request);
        self.next_request += 1;
        let deadline_ms = self.now_ms.saturating_add(timeout_ms);
        self.requests.insert(
            id,
            Outstanding {
                to,
                purpose,
                deadline_ms,
            },
        );
        self.deadlines.insert((deadline_ms, id));
        // A periphery node is reached only over the connection it opened,
        // which its empty URL tells the transport.
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
        // The connection proved who answered: the node it went to.
        let checked = match result {
            Ok(response) if response.status == 200 => Ok(response.body),
            Ok(response) => Err(Problem::Status(response.status)),
            Err(failure) => Err(Problem::Failed(failure)),
        };
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
    /// requests to it (spec/decisions/unreachable-peers.md). An answer says
    /// they do. Only an ILP packet that surely did not arrive
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
            self.unreached.len() <= self.config.max_core_peers() + self.config.max_downlinks,
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
                    // An answer it cannot read says nothing of the amount.
                    Ok(body) => {
                        if let Ok(settling) = crate::wire::decode_settling(&body) {
                            self.event(Event::SettlementRequestAnswered { peer: to, settling });
                        }
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
                    // overloaded peer (400, 503) is asked again only after the
                    // interval.
                    Err(Problem::Failed(Failure::NotDelivered)) => {
                        self.event(Event::SettlementRequestNotDelivered { peer: to });
                    }
                    Err(_) => {}
                }
            }
            Purpose::HashPoll => self.on_hash(to, result),
            Purpose::ListDownload => self.on_list(to, result),
            Purpose::NameHashPoll => self.on_name_hash(to, result),
            Purpose::NameListDownload => self.on_name_list(to, result),
            Purpose::NameCommit(commitment) => self.on_name_commit(to, &commitment, result),
            Purpose::NameUnseal(commitment) => self.on_name_unseal(to, &commitment, result),
            Purpose::NameRenew(name) => self.on_name_renew(to, &name, result),
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
            Purpose::Response(answer) => self.on_answer(*answer, result),
        }
    }

    /// What became of a Fulfill or Reject we sent.
    fn on_answer(&mut self, answer: Answer, result: Result<Vec<u8>, Problem>) {
        match result {
            Ok(body) => self.on_fulfill_reply(&answer, &body),
            Err(problem) => {
                // A refusal after reading it will not change; anything
                // else is tried again, and a copy that arrives twice is
                // dropped by the peer's connector
                // (spec/areas/peer-protocol.md#message-set).
                if matches!(problem, Problem::Status(400 | 409)) {
                    self.stats.answers_dropped += 1;
                } else {
                    self.retry_answer(answer);
                }
            }
        }
    }

    /// The reply to a Fulfill we sent: on a peering over a hub, the payer's
    /// claim, which goes to the scheme
    /// (spec/designs/settlement-hubs.md#4-peering-over-a-hub). An empty
    /// reply, as every peering not over a hub gives, says nothing more; a
    /// claim too long is ignored.
    fn on_fulfill_reply(&mut self, answer: &Answer, body: &[u8]) {
        if answer.packet.first() != Some(&inbound::FULFILL_TYPE)
            || body.is_empty()
            || body.len() > MAX_CLAIM_BYTES
        {
            return;
        }
        self.event(Event::FulfillReply {
            peer: answer.peer,
            request_id: answer.request_id,
            claim: body.to_vec(),
        });
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
        self.tend_names();
        self.send_names();
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
            self.registrar.as_ref().and_then(Registrar::next_due),
            self.registrant.next_due(&self.holders, self.now_ms),
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
        let content = Content::InterledgerPacket { request_id, packet };
        self.send_prepare(now_ms, peer, request_id, &content, timeout_ms)
    }

    /// Sends an ILP Prepare to a peer over a settlement hub, with the
    /// payer's commitment, in one message
    /// (spec/designs/settlement-hubs.md#4-peering-over-a-hub). Its Fulfill
    /// or Reject comes back as any packet's does.
    ///
    /// # Errors
    ///
    /// The node is not a peer, or too much is in flight.
    ///
    /// # Panics
    ///
    /// If `packet` is not a Prepare, or `commitment` is empty or longer
    /// than [`MAX_COMMITMENT_BYTES`]: both are the node's own.
    pub fn send_hub_packet(
        &mut self,
        now_ms: u64,
        peer: &NodeId,
        request_id: u32,
        packet: &[u8],
        commitment: &[u8],
        timeout_ms: u64,
    ) -> Result<(), SendError> {
        assert_eq!(
            packet.first(),
            Some(&inbound::PREPARE_TYPE),
            "only a Prepare has a commitment"
        );
        assert!(
            !commitment.is_empty() && commitment.len() <= MAX_COMMITMENT_BYTES,
            "a commitment of a size the wire takes"
        );
        let content = Content::HubPacket {
            request_id,
            packet,
            commitment,
        };
        self.send_prepare(now_ms, peer, request_id, &content, timeout_ms)
    }

    /// Sends a Prepare, in `content`, to a peer.
    fn send_prepare(
        &mut self,
        now_ms: u64,
        peer: &NodeId,
        request_id: u32,
        content: &Content<'_>,
        timeout_ms: u64,
    ) -> Result<(), SendError> {
        self.advance(now_ms);
        if self.peering_of(peer).is_none() {
            return Err(SendError::NotPeered);
        }
        let timeout = timeout_ms.min(self.config.request_timeout_ms);
        self.send(*peer, content, Purpose::Prepare(request_id), timeout)
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
    /// Otherwise it goes at once (spec/areas/network.md#downlinks).
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

    /// Whether this node takes downlinks: a core node with room for any.
    pub(crate) const fn takes_downlinks(&self) -> bool {
        self.own_info.url.is_some() && self.config.max_downlinks > 0
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
