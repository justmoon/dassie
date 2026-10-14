//! The core: what the engine task owns, and its steps.

use std::collections::BTreeMap;
use std::sync::Arc;

use dassie_connector::{Config as ConnectorConfig, Connector, PacketRecord, Rates, TraceEvent};
use dassie_identity::NodeKey;
use dassie_ilp::MAX_PACKET_LENGTH;
use dassie_ledger::{Account, AccountId, Ledger, Name, TraceId};
use dassie_stream::BUFFER_LENGTH;
use tokio::sync::{broadcast, mpsc};

use super::ledgers::ActiveScheme;
use super::packets::Receiver;
use super::tokens::{BtpLink, TokenState};
use super::{Command, Outbound};
use crate::clock::Clock;
use crate::log::target;
use crate::node::{Node, Policies};
use crate::store::{LedgerBatch, StoreError};

/// The engine's state.
pub struct Core {
    pub(super) node: Arc<Node>,
    pub(super) clock: Arc<dyn Clock>,
    pub(super) key: Arc<NodeKey>,
    pub(super) ledger: Ledger,
    pub(super) connector: Connector,
    pub(super) rates: Rates,
    pub(super) schemes: BTreeMap<Name, ActiveScheme>,
    pub(super) receiver: Option<Receiver>,
    pub(super) tokens: BTreeMap<u64, TokenState>,
    pub(super) by_hash: BTreeMap<[u8; 32], u64>,
    pub(super) links: BTreeMap<u64, BtpLink>,
    pub(super) next_link: u64,
    /// What this batch changed, beyond the ledger's journal.
    pub(super) batch: LedgerBatch,
    /// Messages for BTP clients, sent once the batch is persisted.
    pub(super) outputs: Vec<(u64, Outbound)>,
    /// Links to end once this batch's messages are handed over: their
    /// client stopped reading or went away, or their token was revoked.
    pub(super) closing: Vec<u64>,
    deferred: Vec<Box<dyn FnOnce() + Send>>,
    packet_events: Option<broadcast::Sender<TraceEvent>>,
    /// Where the connector writes the packet it sends on.
    pub(super) out: Vec<u8>,
    /// Where responses for upstream are written.
    pub(super) up: Vec<u8>,
    /// Where the receiver writes.
    pub(super) scratch: Vec<u8>,
    /// Where the receiver's connections work.
    pub(super) work: Vec<u8>,
}

impl Core {
    /// Rebuilds the core from the database: the ledger from its journal,
    /// the tokens, and the schemes, which book what changed while the node
    /// was down.
    pub(super) fn start(node: &Arc<Node>, key: &Arc<NodeKey>) -> Result<Self, String> {
        let store = node.store();
        let policies = *node.policies();
        check_bounds(&policies);
        let epoch = store.next_boot_epoch().map_err(text)?;
        let stored = store.load_ledger().map_err(text)?;
        let ledger = Ledger::restore(
            policies.ledger,
            stored.accounts,
            stored.pending,
            stored.next_transfer,
        )
        .map_err(text)?;
        let realm = node.config().realm;
        let own_address = format!("{}.das.{}", realm.ilp_allocation_scheme(), key.node_id());
        let connector = Connector::new(
            ConnectorConfig {
                epoch,
                ..policies.connector
            },
            &own_address,
        );
        let mut core = Self {
            node: Arc::clone(node),
            clock: node.clock(),
            key: Arc::clone(key),
            ledger,
            connector,
            rates: Rates::new(0),
            schemes: BTreeMap::new(),
            receiver: None,
            tokens: BTreeMap::new(),
            by_hash: BTreeMap::new(),
            links: BTreeMap::new(),
            next_link: 0,
            batch: LedgerBatch::default(),
            outputs: Vec::new(),
            closing: Vec::new(),
            deferred: Vec::new(),
            packet_events: None,
            out: vec![0; MAX_PACKET_LENGTH],
            up: vec![0; MAX_PACKET_LENGTH],
            scratch: vec![0; BUFFER_LENGTH.max(MAX_PACKET_LENGTH)],
            work: vec![0; BUFFER_LENGTH],
        };
        core.load_tokens(store).map_err(text)?;
        core.load_schemes(store)?;
        core.flush();
        Ok(core)
    }

    pub(super) fn set_packet_events(&mut self, sender: broadcast::Sender<TraceEvent>) {
        self.packet_events = Some(sender);
    }

    /// The node's ILP address.
    #[must_use]
    pub fn own_address(&self) -> &str {
        self.connector.own_address()
    }

    pub(super) fn now_ms(&self) -> u64 {
        self.clock.now_ms()
    }

    /// Runs `reply` once the batch is persisted.
    pub(super) fn defer(&mut self, reply: impl FnOnce() + Send + 'static) {
        self.deferred.push(Box::new(reply));
    }

    pub(super) fn handle(&mut self, command: Command) {
        match command {
            Command::Btp {
                connection,
                request_id,
                packet,
                response,
            } => {
                if response {
                    self.btp_response(connection, request_id, &packet);
                } else {
                    self.btp_prepare(connection, request_id, &packet);
                }
            }
            Command::BtpClosed { connection } => self.btp_closed(connection),
            Command::Call(call) => call(self),
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
    pub(super) fn deadline(&self) -> Option<u64> {
        let receiver = self.receiver.as_ref().and_then(Receiver::deadline);
        match (self.connector.poll_timeout(), receiver) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (a, b) => a.or(b),
        }
    }

    /// Ends a batch: expires what is due, persists, then sends.
    pub(super) fn finish_step(&mut self) {
        loop {
            let now = self.now_ms();
            self.expire(now);
            self.flush();
            self.check_invariants();
            self.publish_packets();
            self.dispatch();
            for reply in std::mem::take(&mut self.deferred) {
                reply();
            }
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
    /// nothing may act on: the task stops, and its successor starts from the
    /// disk.
    pub(super) fn flush(&mut self) {
        self.take_journal();
        if self.batch.is_empty() {
            return;
        }
        self.batch.at_ms = self.now_ms();
        let batch = std::mem::take(&mut self.batch);
        if let Err(error) = self.node.store().commit_ledger(&batch) {
            tracing::error!(target: target::ENGINE, %error, "persisting the ledger failed; restarting it");
            panic!("persisting the ledger failed: {error}");
        }
    }

    fn check_invariants(&self) {
        if cfg!(debug_assertions) {
            self.ledger.check_invariants();
            self.connector.check_invariants(&self.ledger);
        }
    }

    fn publish_packets(&mut self) {
        while let Some(event) = self.connector.poll_event() {
            if let Some(sender) = &self.packet_events {
                let _ = sender.send(event);
            }
        }
    }

    /// Hands the batch's messages to the BTP connections. A client whose
    /// queue is full is not reading: it is disconnected, rather than allowed
    /// to make the node hold its messages. A connection whose task is gone
    /// is ended too.
    fn dispatch(&mut self) {
        for (link, message) in std::mem::take(&mut self.outputs) {
            let Some(sender) = self.links.get(&link).map(|link| &link.outbound) else {
                continue;
            };
            let failure = match sender.try_send(message) {
                Ok(()) => continue,
                Err(mpsc::error::TrySendError::Full(_)) => "a client does not read; disconnecting",
                Err(mpsc::error::TrySendError::Closed(_)) => "a client's connection is gone",
            };
            if !self.closing.contains(&link) {
                tracing::info!(target: target::BTP, link, "{failure}");
                self.closing.push(link);
            }
        }
    }

    pub(super) fn account_list(&self) -> Vec<(AccountId, Account)> {
        self.ledger
            .accounts()
            .map(|(id, account)| (id, *account))
            .collect()
    }

    pub(super) fn packets(&self, after: Option<TraceId>, limit: usize) -> Vec<PacketRecord> {
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
    assert!(
        connector.max_pending
            >= connector.max_pending_per_endpoint * (policies.max_btp_connections + 1),
        "every BTP client and the receiver can fill their share of the table at once"
    );
    assert!(
        connector.max_endpoints > policies.max_btp_connections,
        "every BTP connection, and the receiver, is an endpoint"
    );
    assert!(
        policies.ledger.max_journal >= 64,
        "a command's journal entries fit the journal"
    );
}

fn text(error: impl std::fmt::Display) -> String {
    error.to_string()
}

impl From<StoreError> for super::EngineError {
    fn from(error: StoreError) -> Self {
        tracing::error!(target: target::ENGINE, %error, "database");
        Self::Internal("database error".into())
    }
}
