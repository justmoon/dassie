//! The core: what the engine owns, and its steps, without IO
//! (spec/decisions/tigerstyle.md).
//!
//! The core never reads a clock, an RNG or a socket of its own. Its driver
//! gives it the time at the start of each step, an RNG when it starts, and
//! the inputs; it hands back [`Output`]s once the step's changes are on
//! disk. So the daemon's task and a simulation drive the same code.

#![deny(
    clippy::disallowed_types,
    clippy::disallowed_methods,
    clippy::disallowed_macros
)]

mod downlinks;
mod events;
mod hub_allotment;
mod hub_events;
mod hub_funding;
mod hub_moves;
mod hubs;
mod incoming;
mod invariants;
mod ledgers;
mod names;
mod own_snapshot;
mod owner_payments;
mod packets;
mod payers;
mod peers;
mod probes;
mod schemes;
mod settlement;
mod snapshots;
mod swapping;
mod swaps;
mod tokens;
mod trust;

#[cfg(test)]
mod tests;

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use dassie_connector::pricing::MAX_PRICED_LEDGERS;
use dassie_connector::{
    Config as ConnectorConfig, Connector, Holding, Inventory, PacketRecord, PriceTable, Rates,
    TraceEvent,
};
use dassie_identity::IdentityKey;
use dassie_ilp::MAX_PACKET_LENGTH;
use dassie_ledger::{Account, AccountId, AccountName, Ledger, Name, TraceId};
use dassie_node_core::config::{LiveRealm, Realm};
use dassie_stream::BUFFER_LENGTH;

pub use events::{
    BtpEvent, EngineEvent, Event, MAX_EVENTS, PaymentEvent, PeerEvent, SettlementEvent, Severity,
};
pub use hub_events::{
    HubAccountState, HubEvent, HubEventKind, HubMoveEvent, HubOperation, HubPromise,
    HubStreamEvent, MoveStage, Role,
};
pub use hub_funding::{Direction, Move};
/// The simulation's oracle prices each asset whole by the node's own
/// mapping from a hub to its owner book.
#[cfg(feature = "testing")]
pub(crate) use hub_moves::owner_book_ledger;
pub use hub_moves::{HubHold, HubStatus};

/// The owner book a settlement hub's ledger counts with, as one holding of
/// its asset, or `None` for a ledger that is no hub's: every other ledger
/// is an asset of its own (spec/areas/connector.md#inventory-pricing).
#[must_use]
pub const fn owner_book_of(ledger: dassie_ledger::LedgerId) -> Option<dassie_ledger::LedgerId> {
    hub_moves::owner_book_ledger(ledger)
}
#[cfg(feature = "testing")]
pub use hubs::HubPeer;
pub use incoming::{IncomingPolicy, NewIncoming};
pub use ledgers::{BalanceView, LedgerView, RemovalBlocker};
pub use names::NamesView;
pub use own_snapshot::{Content as SnapshotContent, OwnSnapshots, grid_bp};
pub use packets::ledger_slug;
pub use payers::{CreatedOutgoing, NewOutgoing};
pub use peers::{NetworkView, PeerView};
pub use probes::{
    MAX_PROBES, MAX_PROBES_PER_CLIENT, Measured, OWNER_PROBES, PROBE_TIMEOUT_MS, Prober,
};
pub use schemes::{AssetView, LedgerEvent, SCHEMES, offered};
pub use settlement::Lie;
pub use snapshots::{NoSnapshots, PeerSnapshotView, Quoting, SnapshotSource};
pub use swapping::{
    MAX_OWNER_SWAPS, MAX_SWAP_PEERS, OWNER_SWAP_MS, OwnerSwap, PACKETS_PER_PEER, SWAP_TIMEOUT_MS,
    SwapEnd, SwapReport, SwapStats,
};
pub use swaps::{
    MAX_SWAP_OUTCOMES, MAX_SWAPS_IN_FLIGHT, NewSwap, SWAP_DATA_LENGTH, SWAP_DATA_VERSION,
    SwapOutcome, SwapSent,
};
pub use tokens::{BtpSession, NewToken, TokenView, token_hash};

use incoming::OpenIncoming;
use ledgers::ActiveScheme;
use packets::Receiver;
use settlement::Settlements;
use tokens::{BtpLink, TokenState};

use crate::clock::SharedRng;
use crate::node::Policies;
use crate::store::{Checkpoint, LedgerBatch, PaymentId, PeerSettings, Store, StoreError};
use events::Events;

/// Why the engine did not do something.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EngineError {
    /// The engine is not running: before setup, or restarting.
    Unavailable,
    /// The request is invalid.
    Invalid(String),
    /// The node is not in a state to do it.
    Precondition(String),
    /// No such thing.
    NotFound(String),
    /// A limit is reached.
    Exhausted(String),
    /// What it would spend is more than the funds or a grant allow.
    Refused(String),
    /// A bug; the details are logged.
    Internal(String),
}

impl std::fmt::Display for EngineError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unavailable => f.write_str("the ledger is not running"),
            Self::Invalid(message)
            | Self::Precondition(message)
            | Self::NotFound(message)
            | Self::Exhausted(message)
            | Self::Refused(message)
            | Self::Internal(message) => f.write_str(message),
        }
    }
}

/// What a BTP connection's task sends its client.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Outbound {
    /// An ILP Prepare, as a BTP Message.
    Prepare {
        /// The request ID.
        request_id: u32,
        /// The packet.
        packet: Vec<u8>,
    },
    /// An ILP Fulfill or Reject, as a BTP Response.
    Response {
        /// The request ID of the Prepare it answers.
        request_id: u32,
        /// The packet.
        packet: Vec<u8>,
    },
    /// A BTP Error for the request.
    Error {
        /// The request ID.
        request_id: u32,
        /// Why.
        message: &'static str,
    },
    /// The connection is over: its token was revoked, or it did not read.
    Close,
}

/// A packet as the trace records it, with the node of each of its
/// endpoints that is a peer's: the connector knows a peer only by the
/// node's number for the peering.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TracedPacket {
    /// The record.
    pub record: PacketRecord,
    /// The peer it came from, if it came from one.
    pub source_node: Option<dassie_identity::NodeId>,
    /// The peer it went to, if it went to one.
    pub destination_node: Option<dassie_identity::NodeId>,
}

/// A change in the trace, for live subscribers.
#[allow(clippy::large_enum_variant)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PacketEvent {
    /// A packet arrived, or was resolved: its record as it is now.
    Packet(TracedPacket),
    /// Events were dropped because nobody polled: this many.
    Lagged(u64),
}

/// What a step hands its driver, once the step's changes are persisted.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Output {
    /// A message for a BTP connection. After [`Outbound::Close`], the core
    /// has forgotten the link.
    Btp {
        /// The connection.
        link: u64,
        /// The message.
        message: Outbound,
    },
    /// A packet as the trace records it, for subscribers.
    Trace(PacketEvent),
    /// What the node saw of a settlement hub, for subscribers.
    Hub(HubStreamEvent),
    /// What the core reports, for the log (spec/decisions/tigerstyle.md).
    Event(Event),
    /// A peer request to send.
    PeerRequest(dassie_peer::Request),
    /// The reply to an inbound peer request.
    PeerReply {
        /// The request it answers.
        inbound: dassie_peer::InboundId,
        /// The reply.
        reply: dassie_peer::Reply,
    },
    /// A rate probe is done: what the path delivers per unit sent, and
    /// the receiver's asset, or nothing, when it could not be measured.
    ProbeDone {
        /// The probe.
        id: u64,
        /// What it measured.
        measured: Option<Measured>,
    },
    /// A swap the owner asked for is done: what it moved, and why it
    /// ended ([`Core::start_owner_swap`]).
    SwapDone {
        /// The swap.
        id: u64,
        /// What it moved.
        report: SwapReport,
    },
    /// Open an external asset's connection to its ledger, numbered
    /// `epoch`; what comes over it is fed back with the same epoch.
    LedgerConnect {
        /// The asset's ID.
        asset: String,
        /// The connection.
        epoch: u64,
        /// Where: a WebSocket URL.
        url: String,
    },
    /// Close an asset's connection.
    LedgerClose {
        /// The asset's ID.
        asset: String,
        /// The connection.
        epoch: u64,
    },
    /// Send a message over an asset's connection.
    LedgerSend {
        /// The asset's ID.
        asset: String,
        /// The connection.
        epoch: u64,
        /// The message.
        message: Vec<u8>,
    },
    /// Send a message to one validator of a settlement hub, for the hub
    /// account `asset`; its answer comes back as [`LedgerEvent::From`]
    /// with the same endpoint and epoch (spec/areas/hubs.md#the-members-side).
    HubSend {
        /// The asset's ID.
        asset: String,
        /// Its connection, which the answer is for.
        epoch: u64,
        /// The hub, which the request names.
        hub: dassie_hub::HubId,
        /// The validator's place in the hub's rules.
        endpoint: usize,
        /// The validator's identity, whose node ID the connection is pinned
        /// to.
        node: Box<dassie_identity::NodeIdentity>,
        /// Where it is reached: `host:port`.
        address: String,
        /// The message.
        message: Vec<u8>,
    },
    /// Ask a faucet for an asset: an HTTPS POST, whose outcome comes back
    /// as [`LedgerEvent::Funded`].
    Fund {
        /// The asset's ID.
        asset: String,
        /// Its connection, which the outcome is for.
        epoch: u64,
        /// The faucet.
        url: String,
        /// What to post.
        body: Vec<u8>,
    },
    /// Check that a node answers at its URL over HTTPS, with a certificate
    /// a public authority issued: a GET of its discovery document, which
    /// must name it. How it went comes back as [`Core::https_checked`], for
    /// a downlink asking to be promoted or a node registered with us
    /// (spec/decisions/promotion-to-core.md), or, if `registration`, as
    /// [`Core::registration_checked`], for a node registering, whose
    /// registration waits on it (spec/areas/network.md#registration).
    CheckHttps {
        /// The node.
        node: dassie_identity::NodeId,
        /// Its URL.
        url: String,
        /// Whether a registration waits on it.
        registration: bool,
    },
    /// Fetch a document for an asset, such as a validator list: an HTTPS
    /// GET that follows no redirect, whose body comes back as
    /// [`LedgerEvent::Fetched`].
    Fetch {
        /// The asset's ID.
        asset: String,
        /// Its connection, which the answer is for.
        epoch: u64,
        /// What to fetch.
        url: String,
        /// The longest body taken.
        max_bytes: usize,
    },
}

/// What a step changed that live views show: they fetch again when it
/// did (spec/areas/rpc.md#live-streams).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Changes {
    /// The ledgers, balances, tokens or the journal.
    pub ledger: bool,
    /// The node table, peers, routes or registrations.
    pub network: bool,
    /// Open Payments resources.
    pub payments: bool,
}

/// What the core is made from.
pub struct CoreParts {
    /// The database it restores from and persists to.
    pub store: Arc<Store>,
    /// The node's key.
    pub key: Arc<IdentityKey>,
    /// The realm: the node's address, and the only ledgers it runs.
    pub realm: Realm,
    /// Whether it may be the live realm: by the dev switch only
    /// (spec/decisions/mainnet-schemes.md).
    pub live: LiveRealm,
    /// Its bounds.
    pub policies: Policies,
    /// Its randomness.
    pub rng: Box<dyn SharedRng>,
    /// The peer protocol's randomness.
    pub peer_rng: Box<dyn SharedRng>,
    /// What our link state says about us.
    pub own_info: dassie_peer::OwnInfo,
    /// The bootstrap nodes.
    pub bootstrap: Vec<dassie_peer::Bootstrap>,
    /// The uplinks a periphery node asks only; empty for any.
    pub uplinks: Vec<dassie_identity::NodeId>,
    /// The nodes a dev network makes core outright; empty outside one.
    pub dev_core: Vec<dassie_identity::NodeId>,
}

/// The engine's state.
pub struct Core {
    store: Arc<Store>,
    rng: Box<dyn SharedRng>,
    /// The time of the current step.
    now_ms: u64,
    key: Arc<IdentityKey>,
    ledger: Ledger,
    connector: Connector,
    /// The exchange rates, and what the connector converts at.
    prices: Prices,
    schemes: BTreeMap<Name, ActiveScheme>,
    /// What the books last kept for each scheme.
    checkpoints: BTreeMap<Name, Checkpoint>,
    /// The stored schemes whose config or state no longer makes a scheme,
    /// left out at start: the owner may add each again or remove it.
    unusable: BTreeSet<Name>,
    /// The external assets the active schemes settle through.
    assets: BTreeMap<dassie_settlement::AssetId, schemes::ActiveAsset>,
    /// What the books last kept for each external asset: its state.
    asset_checkpoints: BTreeMap<dassie_settlement::AssetId, Vec<u8>>,
    /// The owner's config of each external asset that has one
    /// (spec/areas/settlement.md#external-assets).
    asset_configs: BTreeMap<dassie_settlement::AssetId, String>,
    /// The last connection number given to an asset.
    next_epoch: u64,
    /// Each active ledger's STREAM receiver.
    receivers: BTreeMap<dassie_ledger::LedgerId, Receiver>,
    /// The next local endpoint's number.
    next_local: u16,
    /// The open incoming payments.
    incoming: BTreeMap<PaymentId, OpenIncoming>,
    /// Those completed lately.
    finished_incoming: incoming::Finished,
    incoming_policy: IncomingPolicy,
    tokens: BTreeMap<u64, TokenState>,
    by_hash: BTreeMap<[u8; 32], u64>,
    links: BTreeMap<u64, BtpLink>,
    next_link: u64,
    /// What this batch changed, beyond the ledger's journal.
    batch: LedgerBatch,
    /// Messages for BTP clients, handed over once the batch is persisted.
    outputs: Vec<(u64, Outbound)>,
    /// Links to end once this batch's messages are handed over: their
    /// client stopped reading or went away, or their token was revoked.
    closing: Vec<u64>,
    /// Where the connector writes the packet it sends on.
    out: Vec<u8>,
    /// Where responses for upstream are written.
    up: Vec<u8>,
    /// Where the receiver writes.
    scratch: Vec<u8>,
    /// Where the receiver's connections work.
    work: Vec<u8>,
    /// The peer protocol.
    peer: dassie_peer::PeerNode,
    /// The accounts opened for downlinks, which take the downlinks' share
    /// of the peers' account room for good, and the downlinks the share
    /// keeps room for (spec/decisions/account-budgets.md,
    /// spec/decisions/lazy-downlink-accounts.md).
    downlinks: downlinks::Downlinks,
    /// Peers' connector endpoints, both ways: one per peering, with a slot
    /// per settlement method (spec/decisions/settlement-methods.md).
    peer_endpoints: BTreeMap<dassie_identity::NodeId, u64>,
    endpoint_peers: BTreeMap<u64, dassie_identity::NodeId>,
    next_peer_endpoint: u64,
    /// What the node sees of its hubs, while someone listens.
    hub_tap: hub_events::HubTap,
    /// The realm: `test` or `g` in nodes' addresses, and the realm of the
    /// only ledgers the node runs.
    realm: Realm,
    /// Whether the live realm may run.
    live: LiveRealm,
    /// The credit by which a payer over a hub sizes what a request to
    /// settle funds (spec/areas/settlement.md#credit).
    peer_credit: u64,
    /// Peer requests and replies, handed over once the batch is persisted.
    outputs_peer: Vec<Output>,
    /// Settling with peers.
    settle: Settlements,
    /// What changed since the driver last asked.
    changes: Changes,
    /// Whether a peering began or ended since the driver last asked: at
    /// start, the peerings restored.
    peerings_changed: bool,
    /// Whether the holder of a name changed in the node's view since the
    /// driver last asked: at start, the view restored.
    held_names_changed: bool,
    /// The ledger's counters when last looked at: pending transfers move
    /// balances without a journal entry.
    ledger_stats: dassie_ledger::Stats,
    /// What sends outgoing payments.
    spending: payers::Spending,
    /// The node's own swaps with its peers.
    swaps: swaps::Swaps,
    /// The swap service: with whom, how much and when the node swaps, and
    /// the swaps its owner asks for.
    swapping: swapping::Swapping,
    /// The owner's peer settings.
    peer_settings: PeerSettings,
    /// When the invariants are swept.
    sweeps: Sweeps,
    /// What the core reports, until it is handed over.
    events: Events,
    /// Moving the owner's funds between each hub the node is in and the
    /// owner book of its asset, by the scheme over the hub: at most
    /// `dassie_peer::MAX_OWN_HUBS`.
    funding: BTreeMap<Name, hub_funding::Funding>,
    /// Pushing the node's snapshots to the peers subscribed to them
    /// (spec/areas/network.md#snapshot-subscriptions).
    publishing: snapshots::Publishing,
    /// The UDP port the node answers snapshot requests on, as the node
    /// names it: the link state names it only while the node offers swaps
    /// ([`Self::own_info_offered`]).
    feed_port: Option<u16>,
}

/// The exchange rates, and the price table the connector charges transit and
/// converts at, built from them, the owner's funds on each ledger and the
/// peer settings (spec/decisions/passive-rebalancing.md).
#[derive(Debug, Default)]
struct Prices {
    rates: Rates,
    /// Built at the last commit; a table never built is due at once.
    table: PriceTable,
    /// Whether what the table is built from changed since.
    due: bool,
}

/// When the invariants are swept (spec/decisions/tigerstyle.md).
#[derive(Debug)]
struct Sweeps {
    /// The next sweep, at the earliest.
    at_ms: u64,
    /// How long a sweep holds for: [`SWEEP_EVERY_MS`].
    every_ms: u64,
    /// Sweeps so far.
    count: u64,
}

impl Sweeps {
    /// A sweep due at the first commit.
    const DUE: Self = Self {
        at_ms: 0,
        every_ms: SWEEP_EVERY_MS,
        count: 0,
    };
}

/// Most rounds a step's end takes (spec/decisions/tigerstyle.md). A round
/// repeats only for what the one before set going, which is finite:
/// settlements executed, links closed. A few suffice; past this, what is left
/// waits for the next step.
const MAX_FINISH_ROUNDS: usize = 16;

/// Most packets one connection sends at a time
/// (spec/decisions/tigerstyle.md). A connection's packets in flight are
/// fewer; only answers given at once could take it further. What is left goes
/// in the next step, which is due at once.
pub(super) const MAX_SENDS_PER_POLL: usize = 256;

/// How often the full sweep of the invariants runs, in engine time
/// (spec/decisions/tigerstyle.md). Where debug assertions are on (tests,
/// simulations and the end-to-end binary) it runs before every commit; in
/// release at most this often. Each change asserts what it can see on its own
/// either way, and the time is the engine's, so a run stays the same for the
/// same inputs.
///
/// Measured with the tables full (October 2026, optimized): the ledger's
/// part takes 0.4 ms (4 000 accounts, 25 000 pending), the connector's
/// 1.3 ms (8 000 packets in flight). A busy commit (256 packets) takes
/// 6 ms. Every commit would cost a node at its
/// limits more than the commit; once a second costs it under 1 %, and a
/// node with small tables microseconds. In the simulations the sweep runs
/// every step and costs less than their runs vary.
const SWEEP_EVERY_MS: u64 = if cfg!(debug_assertions) { 0 } else { 1_000 };

impl Core {
    /// Rebuilds the core from the database: the ledger from its journal,
    /// the tokens, and the schemes, which book what changed while the node
    /// was down.
    ///
    /// # Errors
    ///
    /// The database is unreadable or inconsistent.
    // One line per field of the core, which it builds whole.
    #[allow(clippy::too_many_lines)]
    pub fn start(parts: CoreParts, now_ms: u64) -> Result<Self, String> {
        check_parts(&parts)?;
        let feed_port = parts.own_info.feed_port;
        let peer = new_peer(
            &parts.store,
            &parts.key,
            peer_config(&parts.policies, parts.realm),
            (
                parts.own_info,
                parts.bootstrap,
                parts.uplinks,
                parts.dev_core,
            ),
            parts.peer_rng,
            now_ms,
        )?;
        let policies = parts.policies;
        let ledger = restore_ledger(&parts.store, policies.ledger)?;
        let opened = restore_downlink_accounts(&parts.store, &ledger)?;
        let connector = new_connector(&parts.store, &parts.key, parts.realm, &policies)?;
        let (checkpoints, asset_checkpoints, asset_configs) = checkpoints(&parts.store)?;
        let mut core = Self {
            store: parts.store,
            rng: parts.rng,
            now_ms,
            key: parts.key,
            ledger,
            connector,
            prices: Prices::default(),
            schemes: BTreeMap::new(),
            checkpoints,
            unusable: BTreeSet::new(),
            assets: BTreeMap::new(),
            asset_checkpoints,
            asset_configs,
            next_epoch: 0,
            receivers: BTreeMap::new(),
            next_local: 0,
            incoming: BTreeMap::new(),
            finished_incoming: incoming::Finished::default(),
            incoming_policy: policies.incoming,
            tokens: BTreeMap::new(),
            by_hash: BTreeMap::new(),
            links: BTreeMap::new(),
            next_link: 0,
            batch: LedgerBatch::default(),
            outputs: Vec::new(),
            closing: Vec::new(),
            out: vec![0; MAX_PACKET_LENGTH],
            up: vec![0; MAX_PACKET_LENGTH],
            scratch: vec![0; BUFFER_LENGTH.max(MAX_PACKET_LENGTH)],
            work: vec![0; BUFFER_LENGTH],
            peer,
            downlinks: downlinks::Downlinks {
                opened,
                waiting: std::collections::BTreeSet::new(),
            },
            peer_endpoints: BTreeMap::new(),
            endpoint_peers: BTreeMap::new(),
            next_peer_endpoint: 0,
            hub_tap: hub_events::HubTap::default(),
            realm: parts.realm,
            live: parts.live,
            peer_credit: policies.peer_credit,
            outputs_peer: Vec::new(),
            settle: Settlements::new(now_ms, &policies),
            changes: Changes::default(),
            peerings_changed: true,
            held_names_changed: true,
            ledger_stats: dassie_ledger::Stats::default(),
            spending: payers::Spending::default(),
            swaps: swaps::Swaps::default(),
            swapping: swapping::Swapping::default(),
            peer_settings: PeerSettings::DEFAULT,
            sweeps: Sweeps::DUE,
            events: Events::new(),
            funding: BTreeMap::new(),
            publishing: snapshots::Publishing::new(),
            feed_port,
        };
        core.restore()?;
        let own_info = core.peer.own_info().clone();
        core.peer
            .set_own_info(now_ms, core.own_info_offered(own_info));
        Ok(core)
    }

    /// The rest of the state: tokens, schemes, and the peers, whose
    /// accounts the journal must have (spec/areas/node.md#persistence).
    fn restore(&mut self) -> Result<(), String> {
        let store = Arc::clone(&self.store);
        for (code, rate, as_of_ms) in store.exchange_rates().map_err(text)? {
            self.prices.rates.set_at(code, rate, as_of_ms);
        }
        self.peer_settings = store.peer_settings().map_err(text)?;
        self.connector
            .set_offer_swaps(self.peer_settings.offer_swaps);
        self.load_tokens(&store).map_err(text)?;
        self.load_schemes(&store)?;
        self.load_funding()?;
        self.load_incoming()?;
        self.load_outgoing()?;
        self.update_peer_schemes();
        self.restore_waiting();
        let peers: Vec<dassie_identity::NodeId> = self.peer.peers().map(|(id, _)| *id).collect();
        for node in peers {
            // A peer gets an endpoint with a slot for each method whose
            // scheme runs ([`Self::wanted_slots`]). A method whose scheme
            // does not, as one disabled or of the other realm, has no ledger
            // to book on, and gets no slot; a peer with no method that has
            // one gets no endpoint, which the rest of the node does
            // without. The peer protocol's first review drops such a
            // method, and ends a peering left with none
            // (spec/areas/settlement.md#schemes-and-ledgers,
            // spec/decisions/settlement-methods.md). The original threw, and
            // stopped what served that peer.
            if let Err(error) = self.add_peer_endpoint(node) {
                self.report(PeerEvent::NoEndpoint { node, error });
            }
            self.mark_peer_endpoint(&node);
        }
        self.reconcile_settlements(None);
        self.reconcile_owner_payments();
        self.update_routes();
        self.after_peer();
        self.flush();
        Ok(())
    }

    /// The node's ILP address.
    #[must_use]
    pub fn own_address(&self) -> &str {
        self.connector.own_address()
    }

    /// Starts a step at `now_ms`. Time never goes back within a core: a
    /// clock that does is held where it was.
    pub fn begin_step(&mut self, now_ms: u64) {
        self.now_ms = self.now_ms.max(now_ms);
    }

    pub(super) const fn now_ms(&self) -> u64 {
        self.now_ms
    }

    /// A BTP client's packet: a Prepare in a Message, or a response in a
    /// Response.
    pub fn btp_packet(&mut self, link: u64, request_id: u32, packet: &[u8], response: bool) {
        if response {
            self.btp_response(link, request_id, packet);
        } else {
            self.btp_prepare(link, request_id, packet);
        }
        self.take_journal();
    }

    /// Moves the ledger's journal into the batch, so that the ledger's own
    /// bounded outbox never fills however long the batch.
    pub(super) fn take_journal(&mut self) {
        while let Some(entry) = self.ledger.poll_journal() {
            self.batch.journal.push(entry);
        }
    }

    /// The next time something expires.
    #[must_use]
    pub fn deadline(&self) -> Option<u64> {
        let receiver = self.receivers.values().filter_map(Receiver::deadline).min();
        let settlement = self.peer.peers().next().map(|_| self.settle.at_ms);
        // With no peer left, a notice waiting to go keeps the settlement
        // check due.
        let announcing = (!self.settle.notices.is_empty()).then_some(self.settle.at_ms);
        [
            self.connector.poll_timeout(),
            receiver,
            self.peer.poll_timeout(),
            settlement,
            announcing,
            self.outgoing_deadline(),
            self.probe_deadline(),
            self.schemes_deadline(),
            self.removals_deadline(),
            self.funding_deadline(),
            self.snapshot_deadline(),
            self.swap_deadline(),
            self.work_left(),
        ]
        .into_iter()
        .flatten()
        .min()
    }

    /// Ends a step: expires what is due, persists, then hands over what
    /// the step produced. Links being closed are closed in turn, until
    /// nothing is left to close, in at most [`MAX_FINISH_ROUNDS`] rounds;
    /// what is left then waits for the next step, which is due at once.
    pub fn finish_step(&mut self, out: &mut Vec<Output>) {
        for _ in 0..MAX_FINISH_ROUNDS {
            if self.finish_round(out) {
                return;
            }
        }
        self.report(EngineEvent::RoundsLeft);
        self.flush();
        self.hand_over(out);
    }

    /// One round of [`finish_step`](Self::finish_step). Returns whether the
    /// step is done.
    fn finish_round(&mut self, out: &mut Vec<Output>) -> bool {
        self.peer_timers();
        self.publish_snapshot();
        self.run_swaps();
        self.scheme_timers();
        let checked_at = self.settle.at_ms;
        self.settlement_timers();
        // A settlement check judges each method's room, which withdraws it
        // or not: its slot follows.
        if self.settle.at_ms != checked_at {
            self.sync_peer_slots();
        }
        // What this step let the schemes spend, or freed of a method or a
        // channel, funds the requests we could not fund in full.
        self.answer_late();
        self.fund_hub();
        self.expire(self.now_ms);
        self.hub_waits(self.now_ms);
        self.drain_local(self.now_ms);
        self.pump_outgoing(self.now_ms);
        self.flush();
        // Settlements run once their pending transfers are on disk; what
        // they did is persisted in turn.
        if self.execute_settlements() {
            // Their announcements go out with this step's outputs.
            self.after_peer();
            return false;
        }
        self.hand_over(out);
        let closing = std::mem::take(&mut self.closing);
        for link in &closing {
            self.close_link(*link);
        }
        closing.is_empty()
    }

    /// Hands over what the step produced, now that it is on disk.
    fn hand_over(&mut self, out: &mut Vec<Output>) {
        out.extend(self.events.take().map(Output::Event));
        while let Some(event) = self.connector.poll_event() {
            out.push(Output::Trace(match event {
                TraceEvent::Packet(record) => PacketEvent::Packet(self.traced(record)),
                TraceEvent::Lagged(dropped) => PacketEvent::Lagged(dropped),
            }));
        }
        while let Some(event) = self.hub_tap.poll() {
            out.push(Output::Hub(event));
        }
        out.append(&mut self.outputs_peer);
        out.extend(
            self.outputs
                .drain(..)
                .map(|(link, message)| Output::Btp { link, message }),
        );
    }

    /// Now, if a step left work for the next: settlements to execute,
    /// links to close, or packets to send.
    fn work_left(&self) -> Option<u64> {
        let left = !self.settle.to_execute.is_empty()
            || !self.closing.is_empty()
            || self.spending.sends_left;
        left.then_some(self.now_ms)
    }

    /// Persists the batch, once the invariants are checked: a broken one
    /// stops the core before it reaches the disk, and the core's successor
    /// starts from the last state that held. A failed write leaves memory
    /// ahead of the disk, which nothing may act on: the core stops then
    /// too.
    // A failed write crashes the core, which restarts from the disk.
    #[allow(clippy::panic)]
    pub(super) fn flush(&mut self) {
        self.stage();
        self.sweep_if_due();
        if self.batch.is_empty() {
            return;
        }
        let batch = &self.batch;
        self.changes.ledger |= !(batch.journal.is_empty()
            && batch.schemes_added.is_empty()
            && batch.schemes_removed.is_empty()
            && batch.checkpoints.is_empty()
            && batch.asset_checkpoints.is_empty()
            && batch.asset_configs.is_empty()
            && batch.tokens_created.is_empty()
            && batch.tokens_revoked.is_empty()
            && batch.token_caps.is_empty()
            && batch.hub_automatic.is_empty()
            && batch.hub_target.is_empty()
            && batch.hub_funding.is_empty());
        self.changes.network |= !batch.peer.is_empty();
        self.changes.payments |= batch.changes_payments();
        self.batch.at_ms = self.now_ms;
        let batch = std::mem::take(&mut self.batch);
        // The supervisor logs the panic, and restarts the core.
        if let Err(error) = self.store.commit_ledger(&batch) {
            panic!("persisting the ledger failed: {error}");
        }
    }

    /// Moves what changed in memory into the batch, and builds the prices
    /// again if what they are built from changed.
    fn stage(&mut self) {
        // The credit over a hub follows the holds the schemes' states keep,
        // in the batch that keeps them.
        self.sync_hub_credit();
        self.checkpoint_schemes();
        self.take_journal();
        self.take_cap_changes();
        self.peer.take_changes(&mut self.batch.peer);
        let stats = self.ledger.stats();
        if stats != self.ledger_stats {
            self.ledger_stats = stats;
            self.changes.ledger = true;
            self.prices.due = true;
        }
        let batch = &self.batch;
        if !(batch.journal.is_empty()
            && batch.schemes_added.is_empty()
            && batch.schemes_removed.is_empty())
        {
            self.prices.due = true;
        }
        // A guard that paused or resumed its scheme on a reading: the
        // prices leave its ledger out, or take it back.
        self.follow_pegs();
        self.price_if_due();
    }

    /// Builds the prices again, if what they are built from changed or a
    /// rate they use aged out or came of age: the rates, the owner's funds
    /// on each active ledger, or the peer settings
    /// (spec/decisions/passive-rebalancing.md).
    fn price_if_due(&mut self) {
        if !self.prices.due && self.now_ms < self.prices.table.valid_until_ms() {
            return;
        }
        self.prices.due = false;
        // What the owner's group holds on each active ledger, posted and in
        // flight: the table prices each pair at the worst state the packets
        // in flight can leave.
        let none = Inventory {
            ledger: dassie_ledger::LedgerId::StubUsd,
            owner: AccountId(0),
            holding: Holding::NONE,
        };
        let mut inventories = [none; MAX_PRICED_LEDGERS];
        let mut count = 0;
        // A ledger that converts nothing now, such as a dollar token's off
        // its peg, or its hub's, is left out, as one whose rate is stale:
        // its pairs convert nothing (spec/decisions/issued-assets.md).
        let ledgers = self
            .schemes
            .values()
            .filter(|active| self.converts_now(active))
            .map(|active| active.scheme.ledger());
        for ledger in ledgers {
            let Ok(owner) = self.account(ledger, AccountName::EquityOwner) else {
                continue;
            };
            let Some(balance) = self.ledger.group_balance(owner) else {
                continue;
            };
            if let Some(slot) = inventories.get_mut(count) {
                *slot = Inventory {
                    ledger,
                    owner,
                    holding: Holding::of(&balance),
                };
                count += 1;
            }
        }
        // Each asset counts whole: a hub's ledger with the owner book that
        // funds it, between which the node moves the owner's funds by
        // itself, so that the curve follows what the owner holds of the
        // asset, not where the moves left it. A book that funds no hub
        // priced here counts alone.
        let mut table = PriceTable::build_by_asset(
            &self.prices.rates,
            inventories.get(..count).unwrap_or_default(),
            |ledger| hub_moves::owner_book_ledger(ledger).unwrap_or(ledger),
            self.peer_settings.pricing(),
            self.now_ms,
            self.connector.config().max_rate_age_ms,
        );
        // And while the node's own move between a hub and that book is on
        // its way, what it carries counts toward the asset, so that the
        // asset reads as it would without the move.
        for (ledger, moving) in self.moves_on_their_way() {
            table.count_on_its_way(ledger, moving);
        }
        // Nor does it convert at spot into the owner's own endpoints, which
        // in the same currency needs no rate.
        for active in self
            .schemes
            .values()
            .filter(|active| !self.converts_now(active))
        {
            table.halt(active.scheme.ledger());
        }
        self.prices.table = table;
    }

    /// Checks the invariants, when a sweep is due: at every commit where
    /// debug assertions are on, at most every [`SWEEP_EVERY_MS`] in
    /// release, and always at the commit that ends a restore.
    fn sweep_if_due(&mut self) {
        let sweeps = &mut self.sweeps;
        if self.now_ms < sweeps.at_ms {
            return;
        }
        sweeps.at_ms = self.now_ms.saturating_add(sweeps.every_ms);
        sweeps.count += 1;
        self.check_invariants();
    }

    /// Reports `event`, which goes out with the step's outputs.
    pub(super) fn report(&mut self, event: impl Into<Event>) {
        self.events.push(event.into());
    }

    /// Marks payments changed, for what changes them outside the engine.
    pub fn payments_changed(&mut self) {
        self.changes.payments = true;
    }

    /// What changed since the last call, persisted.
    pub fn take_changes(&mut self) -> Changes {
        std::mem::take(&mut self.changes)
    }

    /// The ledger a payment to `destination` goes out on, and whether it
    /// goes to one of the node's own endpoints: `ledger`, the one the owner
    /// asks to pay from, if a first hop has a method available on it;
    /// otherwise the first available method of the first hop
    /// ([`first_hop`](dassie_connector::Connector::first_hop)).
    pub fn route_of(
        &self,
        destination: &str,
        ledger: Option<dassie_ledger::LedgerId>,
    ) -> Option<(dassie_ledger::LedgerId, bool)> {
        self.connector
            .first_hop(destination, ledger)
            .map(|(hop, ledger)| (ledger, !hop.is_peer()))
    }

    /// The currencies of the active ledgers, and those their schemes read,
    /// such as XRP for a dollar token's peg guard.
    pub fn rate_currencies(&self) -> Vec<&'static str> {
        let mut codes: Vec<&'static str> = self
            .schemes
            .values()
            .flat_map(|active| {
                let own = active.scheme.ledger().asset().code;
                let read = active.scheme.rate_currencies().iter().copied();
                core::iter::once(own).chain(read)
            })
            .collect();
        codes.sort_unstable();
        codes.dedup();
        codes
    }

    /// Takes exchange rates just fetched; live views show them.
    pub fn set_rates(&mut self, rates: &[crate::store::StoredRate]) {
        for &(code, rate, as_of_ms) in rates {
            if !self.prices.rates.set_at(code, rate, as_of_ms) {
                self.report(EngineEvent::RateTableFull { code });
            }
        }
        self.changes.ledger = true;
        self.prices.due = true;
    }

    /// Marks the ledgers changed, for what live views show beside them.
    pub const fn ledgers_changed(&mut self) {
        self.changes.ledger = true;
    }

    /// The exchange rates the connector converts at.
    pub const fn rates(&self) -> &Rates {
        &self.prices.rates
    }

    /// What the connector charges transit and converts at, as of the last
    /// commit (spec/decisions/passive-rebalancing.md).
    pub const fn price_table(&self) -> &PriceTable {
        &self.prices.table
    }

    /// The settlement methods the node offers its peers, each once with its
    /// scheme's ledger: the schemes of the settlement offers its link state
    /// names, of which no owner book is one, as its snapshot quotes them
    /// (spec/areas/rate-feed.md#what-it-publishes).
    pub fn offered_methods(&self) -> Vec<(dassie_peer::wire::SchemeId, dassie_ledger::LedgerId)> {
        self.peer
            .offers()
            .iter()
            .filter_map(|offer| {
                let name = Name::new(offer.scheme.as_str()).ok()?;
                let active = self.schemes.get(&name)?;
                Some((offer.scheme, active.scheme.ledger()))
            })
            .collect()
    }

    /// One ledger account.
    pub fn account_of(&self, id: AccountId) -> Option<Account> {
        self.ledger.account(id).copied()
    }

    /// Every ledger account.
    pub fn account_list(&self) -> Vec<(AccountId, Account)> {
        self.ledger
            .accounts()
            .map(|(id, account)| (id, *account))
            .collect()
    }

    /// Up to `limit` packets after `after` from the trace.
    pub fn packets(&self, after: Option<TraceId>, limit: usize) -> Vec<PacketRecord> {
        self.connector
            .packets_after(after)
            .take(limit)
            .copied()
            .collect()
    }

    /// [`packets`](Self::packets), with their peers' nodes.
    pub fn traced_packets(&self, after: Option<TraceId>, limit: usize) -> Vec<TracedPacket> {
        self.connector
            .packets_after(after)
            .take(limit)
            .map(|record| self.traced(*record))
            .collect()
    }

    /// `record`, with the node of each of its endpoints that is a peer's,
    /// as far as the node still knows it.
    fn traced(&self, record: PacketRecord) -> TracedPacket {
        let node = |endpoint: Option<dassie_connector::EndpointId>| match endpoint {
            Some(dassie_connector::EndpointId::Peer(id)) => self.endpoint_peers.get(&id).copied(),
            _ => None,
        };
        TracedPacket {
            record,
            source_node: node(Some(record.source)),
            destination_node: node(record.destination),
        }
    }
}

// A peer's connector endpoint has a slot for each method of its peering
// (spec/areas/connector.md#settlement-method-slots).
const _: () = assert!(dassie_connector::MAX_SLOTS == dassie_peer::MAX_METHODS);

/// The bounds must fit one another (asserted at start; configuration, not
/// input).
fn check_bounds(policies: &Policies) {
    let connector = &policies.connector;
    assert!(
        policies.ledger.max_pending as usize >= connector.ledger_pending_needed() + 64,
        "the ledger needs room for every packet's transfers, and settlements"
    );
    // Core peers: a periphery node pinned to many uplinks may hold more
    // than `max_peers` (spec/areas/network.md#pinned-uplinks).
    let peers = policies.peer.max_core_peers();
    let downlinks = policies.peer.max_downlinks;
    let local = MAX_LOCAL_ENDPOINTS;
    assert!(
        peers <= dassie_connector::MAX_CHILD_ADDRESSES
            && dassie_connector::MAX_CHILD_ADDRESSES == dassie_peer::MAX_UPLINKS,
        "a periphery node's connector takes an address beneath each of its uplinks"
    );
    assert!(
        SCHEMES.len() < dassie_peer::MAX_SCHEMES,
        "every scheme the node runs fits its link state"
    );

    // A downlink sends only its own packets, and has a child's share.
    assert!(
        connector.max_pending
            >= connector.max_pending_per_endpoint
                * (policies.max_btp_connections + local + downlinks)
                + connector.max_pending_per_peer * peers,
        "every BTP client, local endpoint, peer and downlink can fill their share of the table at once"
    );
    assert!(
        connector.max_endpoints > policies.max_btp_connections + local + peers + downlinks,
        "every BTP connection, local endpoint, peer and downlink is an endpoint"
    );
    assert!(
        policies.incoming.max_open_owner > 0 && policies.incoming.max_open_clients > 0,
        "incoming payments need room"
    );
    assert!(
        policies.peer_credit <= dassie_ledger::MAX_TOTAL,
        "the credit fits the ledger"
    );
    // Less, and a hub link could stop settling after one crash: its drift,
    // up to what was in flight, would leave a payer asked, which counts this
    // credit whatever its hold, less than the minimum to fund; and with
    // much less, a request would fund nothing even without a crash
    // (spec/areas/settlement.md#credit).
    let least = dassie_settlement::policy::least_credit(
        connector.max_packet_amount,
        connector.max_in_flight(),
    );
    assert!(
        policies.peer_credit >= least,
        "the default credit survives a crash"
    );
    // A request that never arrived is asked again at the next check; one
    // whose fate is unknown, or that the peer refused, only after a request
    // interval. Two of those withdraw nothing: the third request is answered,
    // or times out, before the grace is over
    // (spec/decisions/peering-lifecycle.md).
    assert!(
        policies.peer.advertise_short_grace_ms
            > 2 * policies.settlement_request_every_ms + policies.peer.request_timeout_ms,
        "a peer short of room is asked three times before it is withdrawn"
    );
    assert!(
        connector.max_routes >= policies.peer.max_nodes,
        "every node the peer node knows can be routed to"
    );
    check_name_bounds(policies);
    assert!(
        policies.ledger.max_journal >= 64,
        "a command's journal entries fit the journal"
    );
    // Peers' accounts stay when their channels close, and leave room for
    // the node's own (spec/decisions/closing-channels.md): what is left is
    // shared between core peers and downlinks, and each share takes at
    // least every open channel's of its kind on one ledger
    // (spec/decisions/account-budgets.md).
    let max_accounts = policies.ledger.max_accounts;
    assert!(
        peers::peer_budget(max_accounts, false) >= 2 * policies.peer.max_channels
            && peers::peer_budget(max_accounts, true) >= 2 * downlinks,
        "the accounts hold the node's own and every open channel's"
    );
}

/// A registrar's bounds fit the node's (spec/areas/network.md#names).
fn check_name_bounds(policies: &Policies) {
    // `MAX_NAME_LIST` entries fit a response over QUIC
    // (`crates/dassie-node/src/quic/mod.rs`).
    assert!(
        policies.peer.names.max_names <= dassie_peer::wire::MAX_NAME_LIST,
        "a registrar's name list fits a response"
    );
    // Registrars of this configuration grant a node no more names than
    // it answers to (`crates/dassie-node/src/open_payments/mod.rs`).
    assert!(
        policies.peer.names.max_names_per_node <= crate::open_payments::MAX_HELD_NAMES,
        "the node answers to every name a registrar grants it"
    );
}

/// Most local endpoints at once: a receiver, a payer and a swap endpoint
/// for each ledger of a realm.
pub const MAX_LOCAL_ENDPOINTS: usize = 3 * MAX_PRICED_LEDGERS;

/// The peer protocol's state machine, from what the store kept of it,
/// pinned to `uplinks` while it runs as a periphery node, with the dev
/// network's `dev_core`.
fn new_peer(
    store: &Store,
    key: &Arc<IdentityKey>,
    config: dassie_peer::Config,
    (own_info, bootstrap, uplinks, dev_core): (
        dassie_peer::OwnInfo,
        Vec<dassie_peer::Bootstrap>,
        Vec<dassie_identity::NodeId>,
        Vec<dassie_identity::NodeId>,
    ),
    peer_rng: Box<dyn SharedRng>,
    now_ms: u64,
) -> Result<dassie_peer::PeerNode, String> {
    let mut peer = dassie_peer::PeerNode::new(
        config,
        Arc::clone(key),
        own_info,
        bootstrap,
        store.load_peer().map_err(text)?,
        Box::new(peer_rng),
        now_ms,
    );
    peer.pin_uplinks(uplinks);
    peer.set_dev_core(dev_core);
    Ok(peer)
}

/// The peer protocol's configuration in `realm`: the policies', with names
/// off in the live realm.
fn peer_config(policies: &Policies, realm: Realm) -> dassie_peer::Config {
    dassie_peer::Config {
        names_enabled: names_enabled(realm),
        ..policies.peer
    }
}

/// Whether a node of `realm` takes part in names: in the test realm only,
/// whose names may be reset at any time, until registrations pay fees
/// (spec/areas/network.md#names). Free names on the live realm could be
/// hoarded for good.
#[must_use]
pub const fn names_enabled(realm: Realm) -> bool {
    match realm {
        Realm::Test => true,
        Realm::Live => false,
    }
}

/// Checks the policies' bounds and the realm before the core reads
/// anything.
fn check_parts(parts: &CoreParts) -> Result<(), String> {
    check_bounds(&parts.policies);
    check_realm(parts.realm, parts.live)
}

/// Refuses a `live` realm the database names as where it is set: the realm
/// gate would otherwise open mainnet to it
/// (spec/decisions/mainnet-schemes.md).
fn check_realm(realm: Realm, live: LiveRealm) -> Result<(), String> {
    realm
        .check_settable(live)
        .map(|_| ())
        .map_err(|error| format!("the realm {}: {error}", realm.as_str()))
}

/// The realm of the ledgers a node of `realm` runs: its own
/// (spec/decisions/mainnet-schemes.md).
#[must_use]
pub const fn ledger_realm(realm: Realm) -> dassie_ledger::Realm {
    match realm {
        Realm::Test => dassie_ledger::Realm::Test,
        Realm::Live => dassie_ledger::Realm::Live,
    }
}

/// The connector, at the node's address, with this boot's epoch.
fn new_connector(
    store: &Store,
    key: &IdentityKey,
    realm: Realm,
    policies: &Policies,
) -> Result<Connector, String> {
    let epoch = store.next_boot_epoch().map_err(text)?;
    let own_address = format!("{}.das.{}", realm.ilp_allocation_scheme(), key.node_id());
    let config = ConnectorConfig {
        epoch,
        ..policies.connector
    };
    Ok(Connector::new(config, &own_address))
}

/// What the books last kept for each scheme, and for each external asset.
/// And the owner's config of each external asset that has one.
type Checkpoints = (
    BTreeMap<Name, Checkpoint>,
    BTreeMap<dassie_settlement::AssetId, Vec<u8>>,
    BTreeMap<dassie_settlement::AssetId, String>,
);

/// The owner's config of each external asset that has one, as
/// `SetAssetConfig` wrote it.
fn asset_configs(store: &Store) -> Result<BTreeMap<dassie_settlement::AssetId, String>, String> {
    store
        .asset_configs()
        .map_err(text)?
        .into_iter()
        .map(|(id, config)| {
            let id = dassie_settlement::AssetId::parse(&id)
                .ok_or_else(|| format!("an external asset this node does not know: {id}"))?;
            Ok((id, config))
        })
        .collect()
}

/// What the books last kept for each scheme and each external asset, and
/// the owner's config of each asset.
fn checkpoints(store: &Store) -> Result<Checkpoints, String> {
    let schemes = store.checkpoints().map_err(text)?.into_iter().collect();
    let assets = store
        .asset_checkpoints()
        .map_err(text)?
        .into_iter()
        .map(|(id, state)| {
            let id = dassie_settlement::AssetId::parse(&id)
                .ok_or_else(|| format!("an external asset this node does not know: {id}"))?;
            Ok((id, state))
        })
        .collect::<Result<_, String>>()?;
    Ok((schemes, assets, asset_configs(store)?))
}

/// The ledger, from its journal.
fn restore_ledger(store: &Store, config: dassie_ledger::Config) -> Result<Ledger, String> {
    let stored = store.load_ledger().map_err(text)?;
    Ledger::restore(
        config,
        stored.accounts,
        stored.pending,
        stored.next_transfer,
    )
    .map_err(text)
}

/// The peers' accounts opened for downlinks, each of which the journal
/// must have opened (spec/decisions/account-budgets.md).
fn restore_downlink_accounts(
    store: &Store,
    ledger: &Ledger,
) -> Result<std::collections::BTreeSet<(dassie_ledger::LedgerId, Name)>, String> {
    let pairs = store.downlink_accounts().map_err(text)?;
    if let Some((on, node)) = pairs
        .iter()
        .find(|(on, node)| !peers::has_accounts(ledger, *on, *node))
    {
        return Err(format!(
            "the downlink {node} has no accounts on {on}, though they were opened"
        ));
    }
    Ok(pairs.into_iter().collect())
}

fn text(error: impl std::fmt::Display) -> String {
    error.to_string()
}

/// Logged where it is answered, as every internal error is.
impl From<StoreError> for EngineError {
    fn from(error: StoreError) -> Self {
        Self::Internal(format!("database: {error}"))
    }
}
