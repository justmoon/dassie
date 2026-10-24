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

mod events;
mod growth;
mod hub_funding;
mod hub_moves;
mod hubs;
mod incoming;
mod invariants;
mod ledgers;
mod owner_payments;
mod packets;
mod payers;
mod peers;
mod probes;
mod schemes;
mod settlement;
mod tokens;

#[cfg(test)]
mod tests;

use std::collections::BTreeMap;
use std::sync::Arc;

use dassie_connector::pricing::MAX_PRICED_LEDGERS;
use dassie_connector::{
    Config as ConnectorConfig, Connector, Holding, Inventory, PacketRecord, PriceTable, Rates,
    TraceEvent,
};
use dassie_identity::NodeKey;
use dassie_ilp::MAX_PACKET_LENGTH;
use dassie_ledger::{Account, AccountId, AccountName, Ledger, Name, TraceId};
use dassie_node_core::config::{LiveRealm, Realm};
use dassie_stream::BUFFER_LENGTH;

pub use events::{
    BtpEvent, EngineEvent, Event, MAX_EVENTS, PaymentEvent, PeerEvent, SettlementEvent, Severity,
};
pub use hub_funding::{Direction, Move};
pub use hub_moves::{HubHold, HubStatus};
#[cfg(feature = "testing")]
pub use hubs::HubPeer;
pub use incoming::{IncomingPolicy, NewIncoming};
pub use ledgers::{LedgerView, RemovalBlocker};
pub use packets::ledger_slug;
pub use payers::{CreatedOutgoing, NewOutgoing};
pub use peers::{NetworkView, PeerView};
pub use probes::{
    MAX_PROBES, MAX_PROBES_PER_CLIENT, Measured, OWNER_PROBES, PROBE_TIMEOUT_MS, Prober,
};
pub use schemes::{AssetView, LedgerEvent, SCHEMES, offered};
pub use settlement::Lie;
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
    Trace(TraceEvent),
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
        /// The validator's node key, which the connection is pinned to.
        node: Box<dassie_identity::PublicKey>,
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
    /// The price table: what the connector converts transit at, which the
    /// rate feed publishes (spec/areas/rate-feed.md#the-node-side).
    pub prices: bool,
}

/// What the core is made from.
pub struct CoreParts {
    /// The database it restores from and persists to.
    pub store: Arc<Store>,
    /// The node's key.
    pub key: Arc<NodeKey>,
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
}

/// The engine's state.
pub struct Core {
    store: Arc<Store>,
    rng: Box<dyn SharedRng>,
    /// The time of the current step.
    now_ms: u64,
    key: Arc<NodeKey>,
    ledger: Ledger,
    connector: Connector,
    /// The exchange rates, and what the connector converts at.
    prices: Prices,
    schemes: BTreeMap<Name, ActiveScheme>,
    /// What the books last kept for each scheme.
    checkpoints: BTreeMap<Name, Checkpoint>,
    /// The external assets the active schemes settle through.
    assets: BTreeMap<dassie_settlement::AssetId, schemes::ActiveAsset>,
    /// What the books last kept for each external asset: its state.
    asset_checkpoints: BTreeMap<dassie_settlement::AssetId, Vec<u8>>,
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
    /// Peers' connector endpoints, both ways.
    peer_endpoints: BTreeMap<dassie_identity::NodeId, u64>,
    endpoint_peers: BTreeMap<u64, dassie_identity::NodeId>,
    next_peer_endpoint: u64,
    /// The realm: `test` or `g` in nodes' addresses, and the realm of the
    /// only ledgers the node runs.
    realm: Realm,
    /// Whether the live realm may run.
    live: LiveRealm,
    /// The credit a junior extends to its senior, and a senior to its
    /// junior, at first (spec/decisions/earned-trust.md).
    peer_credit: u64,
    senior_credit: u64,
    /// Peer requests and replies, handed over once the batch is persisted.
    outputs_peer: Vec<Output>,
    /// Settling with peers.
    settle: Settlements,
    /// What changed since the driver last asked.
    changes: Changes,
    /// Whether a peering began or ended since the driver last asked: at
    /// start, the peerings restored.
    peerings_changed: bool,
    /// The ledger's counters when last looked at: pending transfers move
    /// balances without a journal entry.
    ledger_stats: dassie_ledger::Stats,
    /// What sends outgoing payments.
    spending: payers::Spending,
    /// The owner's peer settings.
    peer_settings: PeerSettings,
    /// When the invariants are swept.
    sweeps: Sweeps,
    /// What the core reports, until it is handed over.
    events: Events,
    /// Moving the owner's funds between the XRPL account and the hub.
    funding: hub_funding::Funding,
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
    pub fn start(parts: CoreParts, now_ms: u64) -> Result<Self, String> {
        check_bounds(&parts.policies);
        check_realm(parts.realm, parts.live)?;
        let peer = new_peer(
            &parts.store,
            &parts.key,
            &parts.policies,
            (parts.own_info, parts.bootstrap, parts.uplinks),
            parts.peer_rng,
            now_ms,
        )?;
        let CoreParts {
            store,
            key,
            policies,
            rng,
            ..
        } = parts;
        let ledger = restore_ledger(&store, policies.ledger)?;
        let connector = new_connector(&store, &key, parts.realm, &policies)?;
        let (checkpoints, asset_checkpoints) = checkpoints(&store)?;
        let mut core = Self {
            store,
            rng,
            now_ms,
            key,
            ledger,
            connector,
            prices: Prices::default(),
            schemes: BTreeMap::new(),
            checkpoints,
            assets: BTreeMap::new(),
            asset_checkpoints,
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
            peer_endpoints: BTreeMap::new(),
            endpoint_peers: BTreeMap::new(),
            next_peer_endpoint: 0,
            realm: parts.realm,
            live: parts.live,
            peer_credit: policies.peer_credit,
            senior_credit: policies.senior_credit,
            outputs_peer: Vec::new(),
            settle: Settlements::new(now_ms, &policies),
            changes: Changes::default(),
            peerings_changed: true,
            ledger_stats: dassie_ledger::Stats::default(),
            spending: payers::Spending::default(),
            peer_settings: PeerSettings::DEFAULT,
            sweeps: Sweeps::DUE,
            events: Events::new(),
            funding: hub_funding::Funding::new(now_ms, policies.settlement_interval_ms),
        };
        core.restore()?;
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
        self.load_funding()?;
        self.load_tokens(&store).map_err(text)?;
        self.load_schemes(&store)?;
        self.load_incoming()?;
        self.load_outgoing()?;
        self.update_peer_schemes();
        let peers: Vec<dassie_identity::NodeId> = self.peer.peers().map(|(id, _)| *id).collect();
        let active: std::collections::BTreeSet<dassie_identity::NodeId> =
            self.peer.active_peers().map(|(id, _)| *id).collect();
        for node in peers {
            // A peer whose scheme does not run, as one disabled or of the
            // other realm, has no ledger to book on: it gets no endpoint,
            // which the rest of the node does without, and the peer
            // protocol's first review ends the peering
            // (spec/areas/settlement.md#schemes-and-ledgers). The original
            // threw, and stopped what served that peer (ACC-25).
            if let Err(error) = self.add_peer_endpoint(node) {
                self.report(PeerEvent::NoEndpoint { node, error });
            }
            // One being ended stays drained; one we end privately still
            // refuses the peer's Prepares
            // (spec/decisions/ending-a-peering.md).
            let depeering = self
                .peer
                .peering_of(&node)
                .is_some_and(|peering| peering.depeering.is_some());
            if let Some(&endpoint) = self.peer_endpoints.get(&node) {
                let endpoint = dassie_connector::EndpointId::Peer(endpoint);
                if !active.contains(&node) {
                    self.connector.set_draining(endpoint, true);
                } else if depeering {
                    self.connector.set_refusing(endpoint, true);
                }
            }
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
        // With no peer left, a settlement waiting to be announced keeps the
        // settlement check due.
        let announcing = (!self.settle.announce.is_empty()).then_some(self.settle.at_ms);
        [
            self.connector.poll_timeout(),
            receiver,
            self.peer.poll_timeout(),
            settlement,
            announcing,
            self.outgoing_deadline(),
            self.probe_deadline(),
            self.schemes_deadline(),
            self.funding_deadline(),
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
        self.scheme_timers();
        self.settlement_timers();
        self.fund_hub();
        self.expire(self.now_ms);
        self.hub_waits(self.now_ms);
        self.drain_local(self.now_ms);
        self.pump_outgoing(self.now_ms);
        self.flush();
        // Settlements run once their pending transfers are on disk; what
        // they did is persisted in turn (ACC-16).
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
            out.push(Output::Trace(event));
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
            && batch.tokens_created.is_empty()
            && batch.tokens_revoked.is_empty()
            && batch.token_caps.is_empty()
            && batch.hub_automatic.is_none()
            && batch.hub_funding.is_none());
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
        // A ledger whose scheme converts nothing now, such as a dollar
        // token off its peg, is left out, as one whose rate is stale: its
        // pairs convert nothing (spec/decisions/issued-assets.md).
        let ledgers = self
            .schemes
            .values()
            .filter(|active| active.converts_now())
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
        let mut table = PriceTable::build(
            &self.prices.rates,
            inventories.get(..count).unwrap_or_default(),
            self.peer_settings.pricing(),
            self.now_ms,
            self.connector.config().max_rate_age_ms,
        );
        // Nor does it convert at spot into the owner's own endpoints, which
        // in the same currency needs no rate.
        for active in self
            .schemes
            .values()
            .filter(|active| !active.converts_now())
        {
            table.halt(active.scheme.ledger());
        }
        // What a feed says: the charges, the ledgers and the prices. The
        // holdings move with every packet, and only move the prices.
        let old = &self.prices.table;
        let ledger = |entry: &dassie_connector::pricing::LedgerPrice| entry.ledger;
        self.changes.prices |= old.pricing() != table.pricing()
            || old.valid_until_ms() != table.valid_until_ms()
            || !old.ledgers().map(ledger).eq(table.ledgers().map(ledger))
            || !old.prices().eq(table.prices());
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
    /// goes to one of the node's own endpoints.
    pub fn route_of(&self, destination: &str) -> Option<(dassie_ledger::LedgerId, bool)> {
        self.connector
            .first_hop(destination)
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
}

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
        policies.peer_credit <= dassie_ledger::MAX_TOTAL
            && policies.senior_credit <= dassie_ledger::MAX_TOTAL,
        "the credit fits the ledger"
    );
    // Less credit, and a link could stop settling after one crash: its
    // drift, up to what was in flight, would leave the debtor less than
    // the minimum to settle when asked
    // (spec/decisions/thresholds-in-packets.md).
    let least = dassie_settlement::policy::least_credit(
        connector.max_packet_amount,
        connector.max_in_flight(),
    );
    assert!(
        policies.peer_credit >= least
            && (policies.senior_credit == 0 || policies.senior_credit >= least),
        "the credit survives a crash"
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
    assert!(
        policies.ledger.max_journal >= 64,
        "a command's journal entries fit the journal"
    );
    // The XRPL scheme keeps the address of every node the node holds a
    // channel with, and forgets it when the channel closes, so it never
    // drops one for room.
    assert!(
        dassie_xrpl::scheme::MAX_PEERS >= policies.peer.max_channels + downlinks,
        "the XRPL scheme keeps an address for every settlement channel"
    );
    // Peers' accounts stay when their channels close, and leave room for
    // the node's own (spec/decisions/closing-channels.md): what is left
    // takes at least every open channel's on one ledger.
    assert!(
        usize::try_from(policies.ledger.max_accounts).unwrap_or(usize::MAX)
            >= ledgers::OWN_ACCOUNTS + 2 * (policies.peer.max_channels + downlinks),
        "the accounts hold the node's own and every open channel's"
    );
}

/// Most local endpoints at once: a receiver and a payer for each ledger
/// there is.
pub const MAX_LOCAL_ENDPOINTS: usize = 8;

/// The peer protocol's state machine, from what the store kept of it,
/// pinned to `uplinks` while it runs as a periphery node.
fn new_peer(
    store: &Store,
    key: &Arc<NodeKey>,
    policies: &Policies,
    (own_info, bootstrap, uplinks): (
        dassie_peer::OwnInfo,
        Vec<dassie_peer::Bootstrap>,
        Vec<dassie_identity::NodeId>,
    ),
    peer_rng: Box<dyn SharedRng>,
    now_ms: u64,
) -> Result<dassie_peer::PeerNode, String> {
    let mut peer = dassie_peer::PeerNode::new(
        policies.peer,
        Arc::clone(key),
        own_info,
        bootstrap,
        store.load_peer().map_err(text)?,
        Box::new(peer_rng),
        now_ms,
    );
    peer.pin_uplinks(uplinks);
    Ok(peer)
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
    key: &NodeKey,
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
type Checkpoints = (
    BTreeMap<Name, Checkpoint>,
    BTreeMap<dassie_settlement::AssetId, Vec<u8>>,
);

/// What the books last kept for each scheme and each external asset.
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
    Ok((schemes, assets))
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

fn text(error: impl std::fmt::Display) -> String {
    error.to_string()
}

/// Logged where it is answered, as every internal error is.
impl From<StoreError> for EngineError {
    fn from(error: StoreError) -> Self {
        Self::Internal(format!("database: {error}"))
    }
}
