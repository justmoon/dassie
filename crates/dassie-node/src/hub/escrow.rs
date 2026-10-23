//! Following a hub's escrow on the XRP Ledger, as a validator does
//! (spec/designs/settlement-hubs.md#6-deposits-and-withdrawals), without
//! IO: the requests to send on its own WebSocket to its XRPL server, and
//! what it attests from the answers.
//!
//! - **Connecting:** check the network, and where the server's validated
//!   ledger stands against the newest seen ([`chain`]); subscribe to the
//!   escrow; catch up with the escrow's history from the last ledger
//!   attested, a window back ([`RESCAN_LEDGERS`]), since an attestation
//!   lost in a crash must go again and the hub refuses one sent twice; then
//!   follow the stream.
//! - **What it attests:** each validated transaction of the escrow's, as
//!   `dassie_hub::escrow` reads it: a deposit, the outcome of a pending
//!   withdrawal, the old validators' change of the signer list; the escrow
//!   checked, at the start and after a reset, until a quorum agreed; and a
//!   reset of the test network (spec/decisions/ledger-resets.md).
//! - **What it submits:** the escrow's transactions a quorum signed.

use std::collections::{BTreeMap, BTreeSet, VecDeque};

use dassie_hub::Rules;
use dassie_hub::escrow::{check_escrow, deposit_of, outcome_of, withdrawal_outcome};
use dassie_hub::ledger::Withdrawal;
use dassie_hub::operation::ValidatorOperation;
use dassie_xrpl::AccountId;
use dassie_xrpl::account::resets;
use dassie_xrpl::api::{self, AccountSettings, ApiError, Message, TxRecord};
use serde_json::Value;

use crate::store::EscrowCursor;

/// How far back from the last ledger attested a catch-up starts: what was
/// attested just before a crash may never have reached a block.
pub const RESCAN_LEDGERS: u32 = 1024;

/// A reset is attested by the new chain's ledger rounded down to this, so
/// that validators that see the same new chain within its first days
/// attest the same value, and sightings of chains far apart never add up
/// (about 2.6 days of ledgers).
pub const RESET_EPOCH_LEDGERS: u32 = 1 << 16;

/// How often the escrow is checked while no quorum has.
pub const ESCROW_CHECK_EVERY_MS: u64 = 30_000;

/// How often a server behind, or on another network, is asked again where
/// it stands.
pub const RETRY_MS: u64 = 10_000;

/// The most signed transactions waiting for a connection.
pub const MAX_QUEUED_SUBMITS: usize = 64;

/// The most transactions remembered as attested, to attest each once.
const MAX_REMEMBERED: usize = 4096;

/// The most outcomes of the escrow's own transactions held until the hub
/// gives their sequence to something.
const MAX_HELD: usize = 64;

/// The most requests waiting for an answer.
const MAX_PENDING: usize = 16;

/// What the validator's ledger says that bears on following the escrow.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EscrowView {
    /// The rules in force, for the check of the escrow.
    pub rules: Rules,
    /// Whether a quorum checked the escrow.
    pub checked: bool,
    /// The escrow's next sequence the hub gives out.
    pub next_sequence: u32,
    /// The withdrawals with an escrow sequence, not yet settled.
    pub withdrawals: Vec<(u32, Withdrawal)>,
    /// The sequence kept for the old validators' change of signer list.
    pub signer_list: Option<u32>,
}

/// Where a server's validated ledger stands against the newest seen.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Chain {
    /// At or past it.
    Current,
    /// Below it, closed before it: a server behind.
    Behind,
    /// Below it, closed after it: a new chain, which a server behind cannot
    /// report. A reset, on the testnet.
    Restarted,
    /// The same, on a network that never resets.
    WentBack,
}

/// Where ledger `index`, closed at `close_time`, stands against `newest`
/// on `network` (spec/decisions/ledger-resets.md).
#[must_use]
pub fn chain(newest: Option<(u32, u64)>, network: u32, index: u32, close_time: u64) -> Chain {
    match newest {
        Some((newest, newest_close)) if index < newest => {
            if close_time <= newest_close {
                Chain::Behind
            } else if resets(network) {
                Chain::Restarted
            } else {
                Chain::WentBack
            }
        }
        _ => Chain::Current,
    }
}

/// What the watcher asks its host to do. Taken apart at once: boxing would
/// only allocate.
#[allow(clippy::large_enum_variant)]
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Out {
    /// Send this on the WebSocket.
    Send(Vec<u8>),
    /// Have the validator attest this.
    Attest(ValidatorOperation),
    /// Persist how far it followed the escrow.
    Cursor(EscrowCursor),
    /// Something the owner should read in the log.
    Problem(&'static str),
}

/// What a request was.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Asked {
    ServerState,
    Subscribe,
    History,
    Settings,
    Objects,
    Submit,
}

/// Where the connection is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Phase {
    Disconnected,
    /// Asked where the server stands.
    Checking,
    /// Its server is behind, on another network, or failed to answer:
    /// asked again later.
    Waiting {
        since_ms: u64,
    },
    /// Subscribed, and reading the history from `from` to `to`.
    CatchingUp {
        from: u32,
        to: u32,
        close_time: u64,
    },
    Following,
}

/// Follows one hub's escrow.
#[derive(Debug)]
pub struct Watcher {
    escrow: AccountId,
    network: u32,
    cursor: Option<EscrowCursor>,
    phase: Phase,
    next_id: u64,
    pending: BTreeMap<u64, Asked>,
    remembered: BTreeSet<[u8; 32]>,
    remembered_order: VecDeque<[u8; 32]>,
    held: VecDeque<TxRecord>,
    queued: VecDeque<Vec<u8>>,
    settings: Option<(AccountSettings, u32)>,
    escrow_checked_ms: Option<u64>,
    attested_escrow: Option<u32>,
    out: VecDeque<Out>,
}

impl Watcher {
    /// A watcher of `escrow` on `network`, from where it left off.
    #[must_use]
    pub const fn new(escrow: AccountId, network: u32, cursor: Option<EscrowCursor>) -> Self {
        Self {
            escrow,
            network,
            cursor,
            phase: Phase::Disconnected,
            next_id: 1,
            pending: BTreeMap::new(),
            remembered: BTreeSet::new(),
            remembered_order: VecDeque::new(),
            held: VecDeque::new(),
            queued: VecDeque::new(),
            settings: None,
            escrow_checked_ms: None,
            attested_escrow: None,
            out: VecDeque::new(),
        }
    }

    /// Whether it follows the escrow's stream now.
    #[must_use]
    pub const fn following(&self) -> bool {
        matches!(self.phase, Phase::Following)
    }

    /// How far it followed the escrow.
    #[must_use]
    pub const fn cursor(&self) -> Option<EscrowCursor> {
        self.cursor
    }

    /// The next thing to do.
    pub fn poll(&mut self) -> Option<Out> {
        self.out.pop_front()
    }

    /// A connection is up: ask where the server stands.
    pub fn connected(&mut self) {
        self.pending.clear();
        self.settings = None;
        self.phase = Phase::Checking;
        self.ask(Asked::ServerState, api::server_state);
    }

    /// The connection is gone.
    pub fn disconnected(&mut self) {
        self.pending.clear();
        self.settings = None;
        self.phase = Phase::Disconnected;
    }

    /// Submits a signed escrow transaction, now or once following.
    pub fn submit(&mut self, blob: Vec<u8>) {
        if self.following() {
            self.ask(Asked::Submit, |id| api::submit(id, &blob));
        } else if self.queued.len() < MAX_QUEUED_SUBMITS {
            self.queued.push_back(blob);
        } else {
            self.out.push_back(Out::Problem(
                "too many escrow transactions wait for the XRPL server: one was dropped",
            ));
        }
    }

    /// Does what is due at `now`: ask a server that was behind again, check
    /// the escrow while no quorum has, look again at outcomes held.
    pub fn tick(&mut self, now_ms: u64, view: &EscrowView) {
        match self.phase {
            Phase::Waiting { since_ms } if now_ms.saturating_sub(since_ms) >= RETRY_MS => {
                self.phase = Phase::Checking;
                self.ask(Asked::ServerState, api::server_state);
            }
            Phase::Following if !view.checked => {
                let due = self
                    .escrow_checked_ms
                    .is_none_or(|at| now_ms.saturating_sub(at) >= ESCROW_CHECK_EVERY_MS);
                if due {
                    self.escrow_checked_ms = Some(now_ms);
                    let escrow = self.escrow;
                    self.ask(Asked::Settings, |id| api::account_info(id, escrow));
                }
            }
            _ => {}
        }
        let held = std::mem::take(&mut self.held);
        for record in held {
            self.record(&record, view);
        }
    }

    /// Takes a message from the server.
    pub fn message(&mut self, now_ms: u64, bytes: &[u8], view: &EscrowView) {
        match api::parse(bytes) {
            Message::Response { id, outcome } => {
                if let Some(asked) = self.pending.remove(&id) {
                    self.answer(now_ms, asked, outcome, view);
                }
            }
            Message::Transaction(record) if self.subscribed() => self.record(&record, view),
            Message::LedgerClosed(closed) if self.following() => {
                self.closed(closed.ledger_index, closed.close_time);
            }
            Message::Transaction(_) | Message::LedgerClosed(_) | Message::Ignored => {}
        }
    }

    const fn subscribed(&self) -> bool {
        matches!(self.phase, Phase::Following | Phase::CatchingUp { .. })
    }

    fn ask(&mut self, asked: Asked, request: impl FnOnce(u64) -> Vec<u8>) {
        if self.pending.len() >= MAX_PENDING {
            return;
        }
        let id = self.next_id;
        self.next_id = self.next_id.wrapping_add(1).max(1);
        self.pending.insert(id, asked);
        self.out.push_back(Out::Send(request(id)));
    }

    fn answer(
        &mut self,
        now_ms: u64,
        asked: Asked,
        outcome: Result<Value, ApiError>,
        view: &EscrowView,
    ) {
        match (asked, outcome) {
            (Asked::ServerState, Ok(result)) => self.server_state(now_ms, &result),
            (Asked::Subscribe, Ok(_)) => self.read_history(None),
            (Asked::History, Ok(result)) => self.history(now_ms, &result, view),
            (Asked::Settings, Ok(result)) => self.escrow_settings(&result),
            (Asked::Objects, Ok(result)) => self.escrow_objects(&result, view),
            (Asked::Submit, Ok(result)) => {
                if api::submit_result(&result).is_none() {
                    self.out.push_back(Out::Problem(
                        "the XRPL server did not take an escrow transaction",
                    ));
                }
            }
            (Asked::ServerState | Asked::Subscribe | Asked::History, Err(_)) => {
                self.phase = Phase::Waiting { since_ms: now_ms };
            }
            // An escrow that is not there (yet, or since a reset) is
            // checked again later.
            (Asked::Settings | Asked::Objects | Asked::Submit, Err(_)) => {}
        }
    }

    /// Where the server stands: on the hub's network, and on the chain the
    /// watcher follows, or a new one. Then it subscribes.
    fn server_state(&mut self, now_ms: u64, result: &Value) {
        let Some(state) = api::server_state_result(result) else {
            self.phase = Phase::Waiting { since_ms: now_ms };
            return;
        };
        if state.network_id != Some(self.network) {
            self.out.push_back(Out::Problem(
                "the XRPL server is on another network than the hub's escrow",
            ));
            self.phase = Phase::Waiting { since_ms: now_ms };
            return;
        }
        if !self.take_ledger(state.validated_ledger, state.close_time) {
            self.phase = Phase::Waiting { since_ms: now_ms };
            return;
        }
        let from = self
            .cursor
            .map_or(0, |cursor| cursor.scanned_to.saturating_sub(RESCAN_LEDGERS));
        self.phase = Phase::CatchingUp {
            from,
            to: state.validated_ledger,
            close_time: state.close_time,
        };
        let escrow = self.escrow;
        self.ask(Asked::Subscribe, |id| api::subscribe(id, escrow));
    }

    /// Takes a validated ledger the server reports as the newest: whether
    /// it is on the chain the watcher follows, a new one after a reset of
    /// the test network, which it attests, or neither.
    fn take_ledger(&mut self, index: u32, close_time: u64) -> bool {
        let newest = self
            .cursor
            .map(|cursor| (cursor.newest_ledger, cursor.newest_close_time));
        match chain(newest, self.network, index, close_time) {
            Chain::Current => true,
            Chain::Behind => false,
            Chain::WentBack => {
                self.out.push_back(Out::Problem(
                    "the XRPL server's ledger went back on a network that is never reset",
                ));
                false
            }
            Chain::Restarted => {
                let ledger = index - index % RESET_EPOCH_LEDGERS;
                self.out
                    .push_back(Out::Attest(ValidatorOperation::AttestReset { ledger }));
                self.out.push_back(Out::Problem(
                    "the XRP Ledger testnet was reset: attested, and the escrow is checked again",
                ));
                let cursor = EscrowCursor {
                    newest_ledger: index,
                    newest_close_time: close_time,
                    scanned_to: 0,
                };
                self.cursor = Some(cursor);
                self.out.push_back(Out::Cursor(cursor));
                self.escrow_checked_ms = None;
                self.attested_escrow = None;
                true
            }
        }
    }

    /// Asks for the next page of the escrow's history, from `marker`.
    fn read_history(&mut self, marker: Option<&Value>) {
        let Phase::CatchingUp { from, to, .. } = self.phase else {
            return;
        };
        let escrow = self.escrow;
        self.ask(Asked::History, |id| {
            api::account_tx(id, escrow, from, to, marker)
        });
    }

    /// A page of the escrow's history: each transaction attested, then the
    /// next page, or the stream from here on.
    fn history(&mut self, now_ms: u64, result: &Value, view: &EscrowView) {
        let Some(page) = api::account_tx_result(result) else {
            self.out.push_back(Out::Problem(
                "the XRPL server's history of the escrow is unreadable",
            ));
            self.phase = Phase::Waiting { since_ms: now_ms };
            return;
        };
        for record in &page.transactions {
            self.record(record, view);
        }
        if let Some(marker) = &page.marker {
            self.read_history(Some(marker));
            return;
        }
        let Phase::CatchingUp { to, close_time, .. } = self.phase else {
            return;
        };
        self.closed(to, close_time);
        self.phase = Phase::Following;
        self.escrow_checked_ms = None;
        while let Some(blob) = self.queued.pop_front() {
            self.ask(Asked::Submit, |id| api::submit(id, &blob));
        }
        self.tick(now_ms, view);
    }

    /// Ledger `index` closed, and every transaction of the escrow's up to
    /// it was attested.
    fn closed(&mut self, index: u32, close_time: u64) {
        if !self.take_ledger(index, close_time) {
            return;
        }
        let previous = self.cursor;
        let cursor = EscrowCursor {
            newest_ledger: index.max(previous.map_or(0, |c| c.newest_ledger)),
            newest_close_time: close_time.max(previous.map_or(0, |c| c.newest_close_time)),
            scanned_to: index.max(previous.map_or(0, |c| c.scanned_to)),
        };
        if previous != Some(cursor) {
            self.cursor = Some(cursor);
            self.out.push_back(Out::Cursor(cursor));
        }
    }

    /// The escrow's settings and next sequence; then what it owns.
    fn escrow_settings(&mut self, result: &Value) {
        let settings = api::account_settings_result(result);
        let info = api::account_info_result(result);
        let (Some(settings), Some(info)) = (settings, info) else {
            return;
        };
        self.settings = Some((settings, info.sequence));
        let escrow = self.escrow;
        self.ask(Asked::Objects, |id| api::account_objects(id, escrow, None));
    }

    /// Checks the escrow as the rules say, and attests its next sequence
    /// once it passes (spec/designs/settlement-hubs.md#2-founding-a-hub).
    fn escrow_objects(&mut self, result: &Value, view: &EscrowView) {
        let (Some(objects), Some((settings, sequence))) =
            (api::account_objects_result(result), self.settings.take())
        else {
            return;
        };
        if view.checked {
            return;
        }
        match check_escrow(&view.rules, &settings, &objects) {
            Ok(()) if self.attested_escrow != Some(sequence) => {
                self.attested_escrow = Some(sequence);
                self.out
                    .push_back(Out::Attest(ValidatorOperation::AttestEscrow { sequence }));
            }
            Ok(()) => {}
            Err(_) => self.out.push_back(Out::Problem(
                "the hub's escrow is not set up as its rules say: no deposits are credited",
            )),
        }
    }

    /// What a validated transaction of the escrow's attests.
    fn record(&mut self, record: &TxRecord, view: &EscrowView) {
        if !record.validated || self.remembered.contains(&record.hash) {
            return;
        }
        if let Some(deposit) = deposit_of(record, self.escrow) {
            self.attest(record.hash, ValidatorOperation::AttestDeposit(deposit));
            return;
        }
        let Some((sequence, succeeded)) = outcome_of(record, self.escrow) else {
            return;
        };
        let withdrawal = view.withdrawals.iter().find(|(s, _)| *s == sequence);
        if let Some((_, withdrawal)) = withdrawal {
            match withdrawal_outcome(record, self.escrow, sequence, withdrawal) {
                Some(delivered) => self.attest(
                    record.hash,
                    ValidatorOperation::AttestWithdrawal {
                        escrow_sequence: sequence,
                        delivered,
                    },
                ),
                // Not the withdrawal's own Payment: it burns nothing.
                None => self.remember(record.hash),
            }
        } else if view.signer_list == Some(sequence) {
            if succeeded {
                self.attest(
                    record.hash,
                    ValidatorOperation::AttestSignerList { sequence },
                );
            } else {
                self.remember(record.hash);
            }
        } else if sequence >= view.next_sequence {
            // A sequence the hub has not given out, as far as this
            // validator executed: held until it has.
            if self.held.len() >= MAX_HELD {
                self.held.pop_front();
            }
            self.held.push_back(record.clone());
        } else {
            // Settled already, or one of the escrow's setup: nothing to say.
            self.remember(record.hash);
        }
    }

    fn attest(&mut self, hash: [u8; 32], operation: ValidatorOperation) {
        self.remember(hash);
        self.out.push_back(Out::Attest(operation));
    }

    fn remember(&mut self, hash: [u8; 32]) {
        if !self.remembered.insert(hash) {
            return;
        }
        self.remembered_order.push_back(hash);
        if self.remembered_order.len() > MAX_REMEMBERED
            && let Some(oldest) = self.remembered_order.pop_front()
        {
            self.remembered.remove(&oldest);
        }
    }
}

#[cfg(test)]
mod tests;
