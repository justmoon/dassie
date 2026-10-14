//! The core: what the engine owns, and its steps, without IO (D16).
//!
//! The core never reads a clock, an RNG or a socket of its own. Its driver
//! gives it the time at the start of each step, an RNG when it starts, and
//! the inputs; it hands back [`Output`]s once the step's changes are on
//! disk. So the daemon's task and a simulation drive the same code.

#![deny(clippy::disallowed_types)]

mod incoming;
mod ledgers;
mod packets;
mod peers;
mod settlement;
mod tokens;

#[cfg(test)]
mod tests;

use std::collections::BTreeMap;
use std::sync::Arc;

use dassie_connector::{Config as ConnectorConfig, Connector, PacketRecord, Rates, TraceEvent};
use dassie_identity::NodeKey;
use dassie_ilp::MAX_PACKET_LENGTH;
use dassie_ledger::{Account, AccountId, Ledger, Name, TraceId};
use dassie_node_core::config::Realm;
use dassie_stream::BUFFER_LENGTH;

pub use incoming::{IncomingPolicy, NewIncoming};
pub use ledgers::{LedgerView, RemovalBlocker};
pub use packets::ledger_slug;
pub use peers::{NetworkView, PeerView};
pub use tokens::{BtpSession, NewToken, TokenView, token_hash};

use incoming::OpenIncoming;
use ledgers::ActiveScheme;
use packets::Receiver;
use settlement::Settlements;
use tokens::{BtpLink, TokenState};

use crate::clock::SharedRng;
use crate::log::target;
use crate::node::Policies;
use crate::store::{LedgerBatch, PaymentId, Store, StoreError};

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
    /// A peer request to send.
    PeerRequest(dassie_peer::Request),
    /// The reply to an inbound peer request.
    PeerReply {
        /// The request it answers.
        inbound: dassie_peer::InboundId,
        /// The reply.
        reply: dassie_peer::Reply,
    },
}

/// What a step changed that live views show: they fetch again when it
/// did (spec 7 §4.6).
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
    pub key: Arc<NodeKey>,
    /// The realm, for the node's address.
    pub realm: Realm,
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
    rates: Rates,
    schemes: BTreeMap<Name, ActiveScheme>,
    /// Each scheme's last booked on-ledger balance.
    checkpoints: BTreeMap<Name, u64>,
    /// Each active ledger's STREAM receiver.
    receivers: BTreeMap<dassie_ledger::LedgerId, Receiver>,
    /// The next local endpoint's number; never reused.
    next_local: u16,
    /// The open incoming payments.
    incoming: BTreeMap<PaymentId, OpenIncoming>,
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
    /// `test` or `g`, for nodes' addresses.
    allocation_scheme: &'static str,
    /// The credit extended to a new peer.
    peer_credit: u64,
    /// Peer requests and replies, handed over once the batch is persisted.
    outputs_peer: Vec<Output>,
    /// Settling with peers.
    settle: Settlements,
    /// What changed since the driver last asked.
    changes: Changes,
    /// The ledger's counters when last looked at: pending transfers move
    /// balances without a journal entry.
    ledger_stats: dassie_ledger::Stats,
}

impl Core {
    /// Rebuilds the core from the database: the ledger from its journal,
    /// the tokens, and the schemes, which book what changed while the node
    /// was down.
    ///
    /// # Errors
    ///
    /// The database is unreadable or inconsistent.
    pub fn start(parts: CoreParts, now_ms: u64) -> Result<Self, String> {
        let CoreParts {
            store,
            key,
            realm,
            policies,
            rng,
            peer_rng,
            own_info,
            bootstrap,
        } = parts;
        check_bounds(&policies);
        let peer = dassie_peer::PeerNode::new(
            policies.peer,
            Arc::clone(&key),
            own_info,
            bootstrap,
            store.load_peer().map_err(text)?,
            Box::new(peer_rng),
            now_ms,
        );
        let epoch = store.next_boot_epoch().map_err(text)?;
        let ledger = restore_ledger(&store, policies.ledger)?;
        let own_address = format!("{}.das.{}", realm.ilp_allocation_scheme(), key.node_id());
        let connector = Connector::new(
            ConnectorConfig {
                epoch,
                ..policies.connector
            },
            &own_address,
        );
        let checkpoints = store.checkpoints().map_err(text)?.into_iter().collect();
        let mut core = Self {
            store,
            rng,
            now_ms,
            key,
            ledger,
            connector,
            rates: Rates::new(0),
            schemes: BTreeMap::new(),
            checkpoints,
            receivers: BTreeMap::new(),
            next_local: 0,
            incoming: BTreeMap::new(),
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
            allocation_scheme: realm.ilp_allocation_scheme(),
            peer_credit: policies.peer_credit,
            outputs_peer: Vec::new(),
            settle: Settlements::new(now_ms, &policies),
            changes: Changes::default(),
            ledger_stats: dassie_ledger::Stats::default(),
        };
        core.restore()?;
        Ok(core)
    }

    /// The rest of the state: tokens, schemes, and the peers, whose
    /// accounts the journal must have (spec 7 §4.4).
    fn restore(&mut self) -> Result<(), String> {
        let store = Arc::clone(&self.store);
        self.load_tokens(&store).map_err(text)?;
        self.load_schemes(&store)?;
        self.load_incoming()?;
        self.update_peer_schemes();
        let peers: Vec<dassie_identity::NodeId> = self.peer.peers().map(|(id, _)| *id).collect();
        let active: std::collections::BTreeSet<dassie_identity::NodeId> =
            self.peer.active_peers().map(|(id, _)| *id).collect();
        for node in peers {
            // A peer whose scheme is gone has no ledger to book on: it gets
            // no endpoint, which the rest of the node does without.
            if let Err(error) = self.add_peer_endpoint(node) {
                tracing::error!(target: target::PEER, %node, %error, "a peer without an endpoint");
            }
            // One being ended stays drained.
            if !active.contains(&node)
                && let Some(&endpoint) = self.peer_endpoints.get(&node)
            {
                self.connector
                    .set_draining(dassie_connector::EndpointId::Peer(endpoint), true);
            }
        }
        self.reconcile_settlements(None);
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
        let finals = self
            .peer
            .channels()
            .any(|(_, channel)| channel.final_due.is_some())
            .then_some(self.settle.at_ms);
        [
            self.connector.poll_timeout(),
            receiver,
            self.peer.poll_timeout(),
            settlement,
            finals,
        ]
        .into_iter()
        .flatten()
        .min()
    }

    /// Ends a step: expires what is due, persists, then hands over what
    /// the step produced. Links being closed are closed in turn, until
    /// nothing is left to close.
    pub fn finish_step(&mut self, out: &mut Vec<Output>) {
        loop {
            self.peer_timers();
            self.settlement_timers();
            self.expire(self.now_ms);
            self.flush();
            // Settlements run once their pending transfers are on disk;
            // what they did is persisted in turn (ACC-16).
            if self.execute_settlements() {
                // Their announcements go out with this step's outputs.
                self.after_peer();
                continue;
            }
            self.check_invariants();
            while let Some(event) = self.connector.poll_event() {
                out.push(Output::Trace(event));
            }
            out.append(&mut self.outputs_peer);
            out.extend(
                self.outputs
                    .drain(..)
                    .map(|(link, message)| Output::Btp { link, message }),
            );
            let closing = std::mem::take(&mut self.closing);
            if closing.is_empty() {
                return;
            }
            for link in closing {
                self.close_link(link);
            }
        }
    }

    /// Persists the batch. A failure leaves memory ahead of the disk, which
    /// nothing may act on: the core stops, and its successor starts from
    /// the disk.
    pub(super) fn flush(&mut self) {
        self.take_journal();
        self.take_cap_changes();
        self.peer.take_changes(&mut self.batch.peer);
        let stats = self.ledger.stats();
        if stats != self.ledger_stats {
            self.ledger_stats = stats;
            self.changes.ledger = true;
        }
        if self.batch.is_empty() {
            return;
        }
        let batch = &self.batch;
        self.changes.ledger |= !(batch.journal.is_empty()
            && batch.schemes_added.is_empty()
            && batch.schemes_removed.is_empty()
            && batch.checkpoints.is_empty()
            && batch.tokens_created.is_empty()
            && batch.tokens_revoked.is_empty()
            && batch.token_caps.is_empty());
        self.changes.network |= !batch.peer.is_empty();
        self.changes.payments |= batch.changes_payments();
        self.batch.at_ms = self.now_ms;
        let batch = std::mem::take(&mut self.batch);
        if let Err(error) = self.store.commit_ledger(&batch) {
            tracing::error!(target: target::ENGINE, %error, "persisting the ledger failed; restarting it");
            panic!("persisting the ledger failed: {error}");
        }
    }

    /// What changed since the last call, persisted.
    pub fn take_changes(&mut self) -> Changes {
        std::mem::take(&mut self.changes)
    }

    fn check_invariants(&self) {
        if cfg!(debug_assertions) {
            self.ledger.check_invariants();
            self.connector.check_invariants(&self.ledger);
            self.peer.check_invariants();
            self.check_incoming();
        }
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
    let peers = policies.peer.max_peers;
    let local = MAX_LOCAL_ENDPOINTS;
    assert!(
        connector.max_pending
            >= connector.max_pending_per_endpoint * (policies.max_btp_connections + local)
                + connector.max_pending_per_peer * peers,
        "every BTP client, local endpoint and peer can fill their share of the table at once"
    );
    assert!(
        connector.max_endpoints > policies.max_btp_connections + local + peers,
        "every BTP connection, local endpoint and peer is an endpoint"
    );
    assert!(
        policies.incoming.max_open > 0 && policies.incoming.max_open_per_client > 0,
        "incoming payments need room"
    );
    assert!(
        policies.peer_credit <= dassie_ledger::MAX_TOTAL,
        "the credit fits the ledger"
    );
    assert!(
        policies.ledger.max_journal >= 64,
        "a command's journal entries fit the journal"
    );
}

/// Most local endpoints at once: a receiver and a payer for each ledger
/// there is.
pub const MAX_LOCAL_ENDPOINTS: usize = 8;

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

impl From<StoreError> for EngineError {
    fn from(error: StoreError) -> Self {
        tracing::error!(target: target::ENGINE, %error, "database");
        Self::Internal("database error".into())
    }
}
