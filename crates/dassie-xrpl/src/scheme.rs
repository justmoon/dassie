//! The `xrpl-testnet` settlement scheme (spec 5 §6.3, spec 7 §4.5), sans-IO.
//!
//! It follows its account on the ledger through the node's connection: it
//! says what to send ([`Effect::LedgerSend`]) and reads what comes back.
//! Every validated transaction that touches the account is booked once, in
//! ledger order, from a cursor kept with the books:
//!
//! - one of our settlements: posted when it succeeded, voided when it
//!   failed, its fee booked either way;
//! - a Payment from a peer's known address, with our memo naming the peer:
//!   a settlement from it, by the XRP it delivered;
//! - anything else that adds XRP: a deposit; that takes it: a withdrawal.
//!
//! After each, what the books hold must be the account's balance less its
//! reserve; any difference (a reserve change, a balance below the reserve)
//! is reconciled through `equity/suspense`. The reserve itself is never
//! booked, so it is never spent (ACC-19). Settlements are whole drops
//! (ACC-21), failures are checked (ACC-22), and an account created by a
//! payment is a deposit like any other (ACC-20).

use alloc::collections::{BTreeMap, BTreeSet};
use alloc::string::{String, ToString};
use alloc::vec::Vec;

use dassie_ledger::{LedgerId, MAX_TOTAL, Name};
use dassie_settlement::{
    Effect, LedgerInput, LedgerStatus, PreparedSettlement, SchemeError, SettlementScheme,
};
use rand_core::CryptoRng;
use serde_json::{Value, json};

use crate::api::{self, AccountInfo, ApiError, LedgerClosed, Message, TxRecord};
use crate::codec::{Memo, Payment};
use crate::keys::{AccountId, Wallet};
use crate::{UNITS_PER_DROP, from_hex, hex_upper};

/// The scheme's ID.
pub const SCHEME_ID: &str = "xrpl-testnet";
/// The testnet's public server.
pub const DEFAULT_SERVER: &str = "wss://s.altnet.rippletest.net:51233";
/// The testnet's faucet.
pub const DEFAULT_FAUCET: &str = "https://faucet.altnet.rippletest.net/accounts";
/// The memo that marks a settlement, as the original wrote it (spec 7 §2).
pub const MEMO_TYPE: &[u8] = b"https://dassie.land/settlement/xrp-testnet/v1";

/// Most of our transactions on their way at once.
pub const MAX_IN_FLIGHT: usize = 8;
/// Most peers whose addresses are kept.
pub const MAX_PEERS: usize = 512;
/// The most fee the scheme pays, in drops: above it, it waits.
pub const MAX_FEE_DROPS: u64 = 1_000;
/// How many ledgers a transaction may take to get in.
const LEDGER_WINDOW: u32 = 20;
/// How long a request may go unanswered before the connection is made
/// anew.
const REQUEST_TIMEOUT_MS: u64 = 20_000;
/// How often the faucet is asked, while the account does not exist.
const FUND_RETRY_MS: u64 = 60_000;
/// Most requests waiting for an answer: a submission and a lookup for
/// each transaction in flight, the lookups of a restart, and the four
/// that keep the account in sync.
const MAX_PENDING: usize = 2 * MAX_IN_FLIGHT + MAX_RECOVER_ASKING + 4;
/// Most lookups of a restart's settlements asked at once; the rest are
/// asked as answers come in, a ledger at a time.
const MAX_RECOVER_ASKING: usize = 8;
/// Most transactions held while catching up.
const MAX_BUFFERED: usize = 256;
/// Most bytes of config.
const MAX_CONFIG: usize = 1024;
/// How long the connection may say nothing, no ledger closing, before it
/// is made anew: a ledger closes every few seconds.
const LEDGER_SILENCE_MS: u64 = 30_000;
/// Most settlements of ours whose outcome is looked for after a restart.
const MAX_RECOVERING: usize = 64;
/// The most drops the books can hold for the account: [`MAX_TOTAL`] in
/// units. A server that reports more for it is no ledger to follow.
const MAX_ACCOUNT_DROPS: u64 = MAX_TOTAL / UNITS_PER_DROP;
/// Why the scheme stops when a server reports more than that.
const TOO_MUCH_XRP: &str = "the server reported more XRP for the account than the books can hold";

/// Where the scheme connects, and where it asks for test XRP.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Config {
    /// A WebSocket URL, `wss://` (or `ws://`, which the node allows only
    /// where it allows private addresses).
    pub server: String,
    /// The faucet, an HTTPS URL; `None` to never ask.
    pub faucet: Option<String>,
}

impl Config {
    /// The config in `text`: empty for the testnet's defaults, or JSON with
    /// `server` and `faucet` (a URL or `null`), each optional.
    ///
    /// # Errors
    ///
    /// [`SchemeError::InvalidConfig`] if it is not that.
    pub fn parse(text: &str) -> Result<Self, SchemeError> {
        let mut config = Self {
            server: DEFAULT_SERVER.to_string(),
            faucet: Some(DEFAULT_FAUCET.to_string()),
        };
        let text = text.trim();
        if text.is_empty() {
            return Ok(config);
        }
        if text.len() > MAX_CONFIG {
            return Err(SchemeError::InvalidConfig);
        }
        let value: Value = serde_json::from_str(text).map_err(|_| SchemeError::InvalidConfig)?;
        let object = value.as_object().ok_or(SchemeError::InvalidConfig)?;
        for (key, field) in object {
            match (key.as_str(), field) {
                ("server", Value::String(server))
                    if server.starts_with("wss://") || server.starts_with("ws://") =>
                {
                    config.server.clone_from(server);
                }
                ("faucet", Value::String(faucet)) if faucet.starts_with("https://") => {
                    config.faucet = Some(faucet.clone());
                }
                ("faucet", Value::Null) => config.faucet = None,
                _ => return Err(SchemeError::InvalidConfig),
            }
        }
        Ok(config)
    }
}

/// One of our transactions on its way, kept with the books.
#[derive(Clone, Debug, PartialEq, Eq)]
struct InFlight {
    hash: [u8; 32],
    blob: Vec<u8>,
    drops: u64,
    fee: u64,
    sequence: u32,
    last_ledger: u32,
    submitted_at: u32,
    /// Whether the node executed it: its pending transfer is on disk, so
    /// it may be submitted. One the node never executed is never sent.
    executed: bool,
    /// To submit when the connection allows.
    needs_submit: bool,
    /// Past its last ledger and looked for.
    looked_up: bool,
}

/// A request waiting for its answer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Request {
    Subscribe,
    ServerState,
    AccountInfo,
    AccountTx,
    Tx([u8; 32]),
    Submit([u8; 32]),
    /// The outcome of a settlement of ours the node still holds pending
    /// after a restart, though this state no longer has it in flight.
    Recover([u8; 32]),
}

/// Where the scheme stands with its ledger.
#[derive(Clone, Debug, PartialEq)]
enum Phase {
    /// No connection.
    Disconnected,
    /// Connected; waiting for the subscription, then the server's state
    /// and the account's, as of a ledger the stream covers.
    Starting {
        subscribed: bool,
        state: bool,
        info: Option<AccountInfo>,
    },
    /// Reading what happened since the cursor, up to `info`'s ledger.
    CatchingUp { info: AccountInfo },
    /// The account does not exist: waiting for funds.
    Unfunded,
    /// Up to date.
    Synced,
}

/// The `xrpl-testnet` scheme.
pub struct XrplScheme {
    config: Config,
    wallet: Wallet,
    /// Our node's ID, as our memos carry it.
    node: Name,
    /// What the books hold on the account, in internal units.
    booked: u64,
    // Kept with the books.
    cursor: Option<(u32, u32)>,
    sequence: u32,
    in_flight: Vec<InFlight>,
    /// Peers' addresses, with when each was learnt: past [`MAX_PEERS`], the
    /// longest known goes.
    peers: BTreeMap<Name, (AccountId, u64)>,
    next_learnt: u64,
    recovering: Vec<[u8; 32]>,
    // Learnt from the ledger.
    phase: Phase,
    next_id: u64,
    pending: BTreeMap<u64, (Request, u64)>,
    validated: u32,
    fee_base: u64,
    load: (u64, u64),
    reserve_base: u64,
    reserve_inc: u64,
    owner_count: u32,
    balance: Option<u64>,
    network_id: Option<u32>,
    buffer: Vec<TxRecord>,
    fund_at_ms: u64,
    funding: bool,
    failed: Option<&'static str>,
    now_ms: u64,
    /// When the connection last said anything.
    heard_ms: u64,
}

impl XrplScheme {
    /// The scheme of the wallet of `entropy` (the node's settlement entropy
    /// for `xrpl-testnet`), for node `node`, with what the books last
    /// checkpointed and the state they kept.
    ///
    /// # Errors
    ///
    /// If the config or the state is invalid.
    pub fn new(
        config: &str,
        entropy: &[u8; 32],
        node: Name,
        checkpoint: Option<u64>,
        state: &[u8],
    ) -> Result<Self, SchemeError> {
        let mut scheme = Self {
            config: Config::parse(config)?,
            wallet: Wallet::from_entropy(entropy),
            node,
            booked: checkpoint.unwrap_or(0),
            cursor: None,
            sequence: 0,
            in_flight: Vec::new(),
            peers: BTreeMap::new(),
            next_learnt: 0,
            recovering: Vec::new(),
            phase: Phase::Disconnected,
            next_id: 1,
            pending: BTreeMap::new(),
            validated: 0,
            fee_base: 10,
            load: (1, 1),
            reserve_base: 0,
            reserve_inc: 0,
            owner_count: 0,
            balance: None,
            network_id: None,
            buffer: Vec::new(),
            fund_at_ms: 0,
            funding: false,
            failed: None,
            now_ms: 0,
            heard_ms: 0,
        };
        if !state.is_empty() {
            scheme.restore(state).ok_or(SchemeError::InvalidData)?;
        }
        Ok(scheme)
    }

    /// The scheme's account.
    #[must_use]
    pub fn account(&self) -> AccountId {
        self.wallet.account()
    }

    /// Its config.
    #[must_use]
    pub const fn config(&self) -> &Config {
        &self.config
    }

    /// The reserve the account holds back now, in drops.
    fn reserve_drops(&self, owner_count: u32) -> u64 {
        self.reserve_base
            .saturating_add(self.reserve_inc.saturating_mul(u64::from(owner_count)))
    }

    /// What the books should hold for a balance: what is above the reserve,
    /// in internal units.
    fn bookable(&self, balance: u64, owner_count: u32) -> u64 {
        units(balance.saturating_sub(self.reserve_drops(owner_count)))
    }

    /// The fee a transaction pays now, in drops.
    fn fee(&self) -> u64 {
        let (factor, base) = self.load;
        let fee = u128::from(self.fee_base) * u128::from(factor) / u128::from(base.max(1));
        u64::try_from(fee).unwrap_or(u64::MAX).max(10)
    }

    fn send(&mut self, request: Request, effects: &mut Vec<Effect>) {
        // Without a connection, nothing is sent: what is needed is asked
        // once it is made.
        if self.phase == Phase::Disconnected {
            return;
        }
        if self.pending.len() >= MAX_PENDING {
            // The connection is not keeping up: make it anew.
            self.reconnect(effects);
            return;
        }
        let id = self.next_id;
        self.next_id = self.next_id.wrapping_add(1).max(1);
        let account = self.account();
        let message = match request {
            Request::Subscribe => api::subscribe(id, account),
            Request::ServerState => api::server_state(id),
            Request::AccountInfo => api::account_info(id, account),
            Request::AccountTx => {
                let (from, to) = match (&self.phase, self.cursor) {
                    (Phase::CatchingUp { info }, Some((cursor, _))) => (cursor, info.ledger_index),
                    _ => return,
                };
                api::account_tx(id, account, from, to, None)
            }
            Request::Tx(hash) => {
                let Some(transaction) = self.in_flight.iter().find(|t| t.hash == hash) else {
                    return;
                };
                api::tx(id, &hash, transaction.submitted_at, transaction.last_ledger)
            }
            Request::Submit(hash) => {
                let Some(transaction) = self.in_flight.iter().find(|t| t.hash == hash) else {
                    return;
                };
                api::submit(id, &transaction.blob)
            }
            Request::Recover(hash) => api::tx(id, &hash, 1, self.validated.max(1)),
        };
        self.pending.insert(id, (request, self.now_ms));
        effects.push(Effect::LedgerSend { message });
    }

    fn reconnect(&mut self, effects: &mut Vec<Effect>) {
        self.disconnected();
        effects.push(Effect::Reconnect);
    }

    fn disconnected(&mut self) {
        self.phase = Phase::Disconnected;
        self.pending.clear();
        self.buffer.clear();
        // A faucet's answer may come for an older connection, and be
        // dropped: it is asked again in time.
        self.funding = false;
        for transaction in &mut self.in_flight {
            transaction.needs_submit = transaction.executed;
            transaction.looked_up = false;
        }
    }

    fn connected(&mut self, effects: &mut Vec<Effect>) {
        self.disconnected();
        self.heard_ms = self.now_ms;
        self.phase = Phase::Starting {
            subscribed: false,
            state: false,
            info: None,
        };
        // The account is read once the subscription is on, so nothing
        // falls between what it says and what the stream says next.
        self.send(Request::Subscribe, effects);
        self.send(Request::ServerState, effects);
    }

    fn message(&mut self, bytes: &[u8], effects: &mut Vec<Effect>) {
        match api::parse(bytes) {
            Message::Response { id, outcome } => {
                if let Some((request, _)) = self.pending.remove(&id) {
                    self.response(request, outcome, effects);
                }
            }
            Message::Transaction(record) => {
                if self.phase == Phase::Synced {
                    self.transaction(&record, effects);
                } else if !matches!(self.phase, Phase::Disconnected) {
                    if self.buffer.len() >= MAX_BUFFERED {
                        self.reconnect(effects);
                    } else {
                        self.buffer.push(record);
                    }
                }
            }
            Message::LedgerClosed(closed) => self.ledger_closed(closed, effects),
            Message::Ignored => {}
        }
    }

    fn response(
        &mut self,
        request: Request,
        outcome: Result<Value, ApiError>,
        effects: &mut Vec<Effect>,
    ) {
        match request {
            Request::Subscribe => {
                if outcome.is_err() {
                    self.reconnect(effects);
                } else if let Phase::Starting { subscribed, .. } = &mut self.phase {
                    *subscribed = true;
                    self.send(Request::AccountInfo, effects);
                }
            }
            Request::ServerState => {
                match outcome.ok().as_ref().and_then(api::server_state_result) {
                    Some(state) => {
                        self.validated = self.validated.max(state.validated_ledger);
                        self.fee_base = state.base_fee;
                        self.load = (state.load_factor, state.load_base);
                        self.reserve_base = state.reserve_base;
                        self.reserve_inc = state.reserve_inc;
                        self.network_id = state.network_id.filter(|id| *id > 1024);
                        if let Phase::Starting { state, .. } = &mut self.phase {
                            *state = true;
                        }
                        self.start(effects);
                    }
                    None => self.reconnect(effects),
                }
            }
            Request::AccountInfo => self.account_info(outcome, effects),
            Request::AccountTx => self.account_tx(outcome, effects),
            Request::Tx(hash) => self.looked_up(hash, outcome, effects),
            Request::Submit(hash) => self.submitted(hash, outcome.ok().as_ref(), effects),
            Request::Recover(hash) => self.recovered(hash, outcome, effects),
        }
    }

    fn account_info(&mut self, outcome: Result<Value, ApiError>, effects: &mut Vec<Effect>) {
        match outcome {
            Ok(result) => {
                let Some(info) = api::account_info_result(&result) else {
                    self.reconnect(effects);
                    return;
                };
                if info.balance > MAX_ACCOUNT_DROPS {
                    self.failed = Some(TOO_MUCH_XRP);
                    return;
                }
                // A server behind what was booked already says nothing
                // new: asked again when the next ledger closes.
                if self
                    .cursor
                    .is_some_and(|(ledger, _)| ledger > info.ledger_index)
                {
                    return;
                }
                let next = self
                    .in_flight
                    .iter()
                    .map(|t| t.sequence.saturating_add(1))
                    .max()
                    .unwrap_or(0);
                self.sequence = info.sequence.max(next);
                match &mut self.phase {
                    Phase::Starting { info: slot, .. } => *slot = Some(info),
                    Phase::Unfunded => {
                        self.phase = Phase::Starting {
                            subscribed: true,
                            state: true,
                            info: Some(info),
                        };
                    }
                    _ => {}
                }
                self.start(effects);
            }
            Err(error) if error.error == "actNotFound" => self.unfunded(effects),
            Err(_) => self.reconnect(effects),
        }
    }

    /// The account does not exist: what the books hold is gone (a testnet
    /// reset), and the faucet is asked.
    fn unfunded(&mut self, effects: &mut Vec<Effect>) {
        // What was in flight can never apply now: an account made again
        // starts its sequence anew.
        for transaction in core::mem::take(&mut self.in_flight) {
            if transaction.executed {
                effects.push(Effect::Cancel {
                    settlement: hash_name(&transaction.hash),
                });
            }
        }
        if self.booked > 0 {
            effects.push(Effect::Reconcile {
                amount: self.booked,
                gain: false,
            });
            self.booked = 0;
        }
        self.cursor = None;
        self.balance = None;
        self.phase = Phase::Unfunded;
        self.fund(effects);
    }

    fn fund(&mut self, effects: &mut Vec<Effect>) {
        let Some(url) = self.config.faucet.clone() else {
            return;
        };
        if self.funding || self.fund_at_ms > self.now_ms {
            return;
        }
        self.funding = true;
        self.fund_at_ms = self.now_ms.saturating_add(FUND_RETRY_MS);
        let body = serde_json::to_vec(&json!({ "destination": self.account().to_address() }))
            .unwrap_or_default();
        effects.push(Effect::Fund { url, body });
    }

    /// Once the server's state and the account's are known: book what
    /// changed while away, from the cursor if there is one.
    fn start(&mut self, effects: &mut Vec<Effect>) {
        let Phase::Starting {
            state: true,
            info: Some(info),
            ..
        } = self.phase.clone()
        else {
            return;
        };
        match self.cursor {
            Some(cursor) if cursor < (info.ledger_index, u32::MAX) => {
                self.phase = Phase::CatchingUp { info };
                self.send(Request::AccountTx, effects);
            }
            _ => self.synced(info, effects),
        }
    }

    fn account_tx(&mut self, outcome: Result<Value, ApiError>, effects: &mut Vec<Effect>) {
        let Phase::CatchingUp { info } = self.phase.clone() else {
            return;
        };
        let page = match outcome {
            Ok(result) => api::account_tx_result(&result),
            // The server's history does not reach back: the balance says
            // what changed, without who it was.
            Err(error) if error.error.starts_with("lgr") => {
                self.synced(info, effects);
                return;
            }
            // Busy, or anything else: from the start, on a new connection.
            Err(_) => None,
        };
        let Some(page) = page else {
            self.reconnect(effects);
            return;
        };
        for record in &page.transactions {
            self.transaction(record, effects);
        }
        match page.marker {
            Some(marker) => {
                let id = self.next_id;
                self.next_id = self.next_id.wrapping_add(1).max(1);
                let from = self.cursor.map_or(0, |(ledger, _)| ledger);
                let message =
                    api::account_tx(id, self.account(), from, info.ledger_index, Some(&marker));
                self.pending.insert(id, (Request::AccountTx, self.now_ms));
                effects.push(Effect::LedgerSend { message });
            }
            None => self.synced(info, effects),
        }
    }

    /// Up to date as of `info`: the books are made to match the balance,
    /// and what came meanwhile is booked.
    fn synced(&mut self, info: AccountInfo, effects: &mut Vec<Effect>) {
        let first = self.cursor.is_none() && self.booked == 0;
        self.balance = Some(info.balance);
        self.owner_count = info.owner_count;
        let target = self.bookable(info.balance, info.owner_count);
        if first && target > 0 {
            effects.push(Effect::Deposit { amount: target });
            self.booked = target;
        }
        self.reconcile(effects);
        self.cursor = Some(self.cursor.map_or((info.ledger_index, u32::MAX), |cursor| {
            cursor.max((info.ledger_index, u32::MAX))
        }));
        self.validated = self.validated.max(info.ledger_index);
        self.phase = Phase::Synced;
        for record in core::mem::take(&mut self.buffer) {
            self.transaction(&record, effects);
        }
        if self.phase != Phase::Synced {
            return;
        }
        self.ask_recover(effects);
        let waiting: Vec<[u8; 32]> = self
            .in_flight
            .iter()
            .filter(|t| t.needs_submit)
            .map(|t| t.hash)
            .collect();
        for hash in waiting {
            self.submit(hash, effects);
        }
        self.expire(effects);
    }

    /// Makes the books match the balance less the reserve.
    fn reconcile(&mut self, effects: &mut Vec<Effect>) {
        let Some(balance) = self.balance else {
            return;
        };
        let target = self.bookable(balance, self.owner_count);
        if target != self.booked {
            effects.push(Effect::Reconcile {
                amount: target.abs_diff(self.booked),
                gain: target > self.booked,
            });
            self.booked = target;
        }
    }

    fn ledger_closed(&mut self, closed: LedgerClosed, effects: &mut Vec<Effect>) {
        self.validated = self.validated.max(closed.ledger_index);
        self.fee_base = closed.fee_base;
        let reserve_changed =
            (closed.reserve_base, closed.reserve_inc) != (self.reserve_base, self.reserve_inc);
        self.reserve_base = closed.reserve_base;
        self.reserve_inc = closed.reserve_inc;
        let asking = self
            .pending
            .values()
            .any(|(r, _)| *r == Request::AccountInfo);
        match self.phase {
            Phase::Synced => {
                if reserve_changed {
                    self.reconcile(effects);
                }
                self.expire(effects);
                self.ask_recover(effects);
            }
            Phase::Unfunded
            | Phase::Starting {
                subscribed: true,
                info: None,
                ..
            } if !asking => {
                self.send(Request::AccountInfo, effects);
            }
            _ => {}
        }
    }

    /// Looks for our transactions past their last ledger.
    fn expire(&mut self, effects: &mut Vec<Effect>) {
        let validated = self.validated;
        // One the node never executed was never sent: past its last ledger
        // it is dropped, and its sequence read again.
        let before = self.in_flight.len();
        self.in_flight
            .retain(|t| t.executed || validated <= t.last_ledger);
        if self.in_flight.len() != before {
            self.send(Request::AccountInfo, effects);
        }
        let expired: Vec<[u8; 32]> = self
            .in_flight
            .iter_mut()
            .filter(|t| validated > t.last_ledger && !t.looked_up)
            .map(|t| {
                t.looked_up = true;
                t.hash
            })
            .collect();
        for hash in expired {
            self.send(Request::Tx(hash), effects);
        }
    }

    fn submit(&mut self, hash: [u8; 32], effects: &mut Vec<Effect>) {
        if let Some(transaction) = self.in_flight.iter_mut().find(|t| t.hash == hash) {
            transaction.needs_submit = false;
            self.send(Request::Submit(hash), effects);
        }
    }

    /// What `submit` said. Only a malformed transaction (`tem…`) is known
    /// never to apply: anything else may still, or may have already, after
    /// a restart. It is settled by the stream, or by looking for it once
    /// past its last ledger.
    fn submitted(&mut self, hash: [u8; 32], result: Option<&Value>, effects: &mut Vec<Effect>) {
        let Some(code) = result.and_then(api::submit_result) else {
            return;
        };
        if code.starts_with("tem") {
            self.give_up(hash, effects);
        } else if (code.starts_with("ter") || code.starts_with("tel"))
            && let Some(transaction) = self.in_flight.iter_mut().find(|t| t.hash == hash)
        {
            transaction.needs_submit = true;
        }
    }

    /// The answer to looking for a transaction past its last ledger.
    fn looked_up(
        &mut self,
        hash: [u8; 32],
        outcome: Result<Value, ApiError>,
        effects: &mut Vec<Effect>,
    ) {
        match outcome {
            Ok(result) => {
                match api::tx_result(&result).filter(|r| r.validated) {
                    Some(record) if self.phase == Phase::Synced => {
                        self.transaction(&record, effects);
                    }
                    // Not validated yet, or unreadable: ask again later.
                    _ => {
                        if let Some(t) = self.in_flight.iter_mut().find(|t| t.hash == hash) {
                            t.looked_up = false;
                        }
                    }
                }
            }
            Err(error) if error.error == "txnNotFound" && error.searched_all == Some(true) => {
                self.give_up(hash, effects);
            }
            // Not every ledger could be searched: ask again at the next.
            Err(_) => {
                if let Some(transaction) = self.in_flight.iter_mut().find(|t| t.hash == hash) {
                    transaction.looked_up = false;
                }
            }
        }
    }

    /// Our transaction never applied: its settlement is cancelled, and the
    /// account's sequence read again.
    fn give_up(&mut self, hash: [u8; 32], effects: &mut Vec<Effect>) {
        let Some(position) = self.in_flight.iter().position(|t| t.hash == hash) else {
            return;
        };
        let transaction = self.in_flight.remove(position);
        effects.push(Effect::Cancel {
            settlement: hash_name(&transaction.hash),
        });
        self.send(Request::AccountInfo, effects);
    }

    /// Books one validated transaction, once, in ledger order.
    fn transaction(&mut self, record: &TxRecord, effects: &mut Vec<Effect>) {
        if !record.validated {
            return;
        }
        if !self.bookable_record(record) {
            self.failed = Some(TOO_MUCH_XRP);
            return;
        }
        let at = (record.ledger_index, record.index);
        let behind = self.cursor.is_some_and(|cursor| at <= cursor);
        if behind {
            // Booked already, by its balance at least. One of ours is
            // still settled: what the balance booked through suspense
            // comes back out of it.
            if self.in_flight.iter().any(|t| t.hash == record.hash) {
                let booked = self.resolve(record, effects);
                self.booked = narrow(booked.max(0));
                self.reconcile(effects);
            }
            return;
        }
        self.cursor = Some(at);
        let account = self.account();
        if record.account == account {
            self.sequence = self.sequence.max(record.sequence.saturating_add(1));
        }
        let Some(change) = record.change_of(account) else {
            return;
        };
        self.balance = Some(change.after);
        if let Some(count) = change.owner_count {
            self.owner_count = count;
        }
        let mut booked = i128::from(self.booked);
        if self.in_flight.iter().any(|t| t.hash == record.hash) {
            booked = self.resolve(record, effects);
        } else if let Some(peer) = self.settled_by(record) {
            let amount = units(record.delivered.unwrap_or(0));
            effects.push(Effect::IncomingSettlement {
                peer,
                amount,
                id: Some(hash_name(&record.hash)),
            });
            booked += i128::from(amount);
        } else if change.after > change.before {
            let amount = units(change.after - change.before);
            effects.push(Effect::Deposit { amount });
            booked += i128::from(amount);
        } else if change.after < change.before {
            let amount = units(change.before - change.after);
            effects.push(Effect::Withdrawal { amount });
            booked -= i128::from(amount);
        }
        self.booked = narrow(booked.max(0));
        if booked < 0 {
            effects.push(Effect::Reconcile {
                amount: narrow(-booked),
                gain: true,
            });
        }
        self.reconcile(effects);
    }

    /// Settles one of our transactions the ledger validated: posted or
    /// voided, and its fee booked either way. Returns what the books then
    /// hold.
    fn resolve(&mut self, record: &TxRecord, effects: &mut Vec<Effect>) -> i128 {
        let mut booked = i128::from(self.booked);
        let Some(position) = self.in_flight.iter().position(|t| t.hash == record.hash) else {
            return booked;
        };
        let ours = self.in_flight.remove(position);
        let settlement = hash_name(&ours.hash);
        if record.succeeded() {
            effects.push(Effect::Finalize { settlement });
            booked -= i128::from(units(ours.drops));
        } else {
            effects.push(Effect::Cancel { settlement });
        }
        effects.push(Effect::Fee {
            amount: units(record.fee),
            id: settlement,
        });
        booked - i128::from(units(record.fee))
    }

    /// The outcome of a settlement of ours that a restart left pending in
    /// the books, its fee and the balance booked already: posted or
    /// voided now.
    fn recovered(
        &mut self,
        hash: [u8; 32],
        outcome: Result<Value, ApiError>,
        effects: &mut Vec<Effect>,
    ) {
        let Ok(result) = outcome else {
            return;
        };
        let Some(record) = api::tx_result(&result).filter(|r| r.validated) else {
            return;
        };
        self.recovering.retain(|h| *h != hash);
        let settlement = hash_name(&hash);
        effects.push(if record.succeeded() {
            Effect::Finalize { settlement }
        } else {
            Effect::Cancel { settlement }
        });
    }

    /// Asks for the outcome of a restart's settlements not asked for yet,
    /// up to [`MAX_RECOVER_ASKING`] at a time.
    fn ask_recover(&mut self, effects: &mut Vec<Effect>) {
        let asked = |pending: &BTreeMap<u64, (Request, u64)>, hash: &[u8; 32]| {
            pending
                .values()
                .any(|(request, _)| *request == Request::Recover(*hash))
        };
        for index in 0..self.recovering.len() {
            let asking = self
                .pending
                .values()
                .filter(|(request, _)| matches!(request, Request::Recover(_)))
                .count();
            if asking >= MAX_RECOVER_ASKING || self.phase != Phase::Synced {
                return;
            }
            if let Some(&hash) = self.recovering.get(index)
                && !asked(&self.pending, &hash)
            {
                self.send(Request::Recover(hash), effects);
            }
        }
    }

    /// Whether the amounts `record` names for the account fit the books.
    fn bookable_record(&self, record: &TxRecord) -> bool {
        let change = record.change_of(self.account());
        [
            Some(record.fee),
            record.delivered,
            change.map(|change| change.before),
            change.map(|change| change.after),
        ]
        .into_iter()
        .flatten()
        .all(|drops| drops <= MAX_ACCOUNT_DROPS)
    }

    /// The peer a transaction settles from: a successful Payment of XRP to
    /// us with our memo, naming a peer whose known address sent it.
    fn settled_by(&self, record: &TxRecord) -> Option<Name> {
        if !(record.succeeded() && record.is_payment && record.destination == Some(self.account()))
        {
            return None;
        }
        record.delivered?;
        let (memo_type, memo_data) = record.memo.as_ref()?;
        if memo_type.as_slice() != MEMO_TYPE {
            return None;
        }
        let peer = Name::new(core::str::from_utf8(memo_data).ok()?).ok()?;
        (self.peers.get(&peer).map(|(address, _)| *address) == Some(record.account)).then_some(peer)
    }

    /// Keeps a peer's address; past [`MAX_PEERS`], the one learnt longest
    /// ago goes.
    fn learn(&mut self, peer: Name, address: AccountId) {
        if !self.peers.contains_key(&peer)
            && self.peers.len() >= MAX_PEERS
            && let Some(oldest) = self
                .peers
                .iter()
                .min_by_key(|(_, (_, learnt))| *learnt)
                .map(|(name, _)| *name)
        {
            self.peers.remove(&oldest);
        }
        self.next_learnt = self.next_learnt.wrapping_add(1);
        self.peers.insert(peer, (address, self.next_learnt));
    }

    /// A number that changes whenever what [`state`](SettlementScheme::state)
    /// keeps does, cheaply: the node writes the state only then.
    fn fingerprint(&self) -> u64 {
        let mut mix: u64 = 0xcbf2_9ce4_8422_2325;
        let mut add = |value: u64| {
            mix ^= value;
            mix = mix.wrapping_mul(0x0100_0000_01b3);
        };
        let (ledger, index) = self.cursor.unwrap_or((u32::MAX, u32::MAX));
        add(u64::from(ledger) << 32 | u64::from(index));
        add(u64::from(self.sequence));
        add(self.next_learnt);
        for transaction in &self.in_flight {
            let mut head = [0u8; 8];
            head.copy_from_slice(transaction.hash.get(..8).unwrap_or(&[0; 8]));
            add(u64::from_be_bytes(head) ^ u64::from(transaction.executed));
        }
        add(self.in_flight.len() as u64);
        for hash in &self.recovering {
            add(u64::from(hash[0]) << 8 | u64::from(hash[1]));
        }
        add(self.recovering.len() as u64);
        mix
    }

    fn state_value(&self) -> Value {
        let in_flight: Vec<Value> = self
            .in_flight
            .iter()
            .map(|t| {
                json!({
                    "blob": hex_upper(&t.blob),
                    "drops": t.drops,
                    "fee": t.fee,
                    "sequence": t.sequence,
                    "last_ledger": t.last_ledger,
                    "submitted_at": t.submitted_at,
                    "executed": t.executed,
                })
            })
            .collect();
        let peers: serde_json::Map<String, Value> = self
            .peers
            .iter()
            .map(|(peer, (address, _))| (peer.as_str().to_string(), address.to_address().into()))
            .collect();
        let recovering: Vec<Value> = self
            .recovering
            .iter()
            .map(|hash| hex_upper(hash).into())
            .collect();
        json!({
            "cursor": self.cursor.map(|(ledger, index)| json!([ledger, index])),
            "sequence": self.sequence,
            "in_flight": in_flight,
            "peers": peers,
            "recovering": recovering,
        })
    }

    fn restore(&mut self, state: &[u8]) -> Option<()> {
        let value: Value = serde_json::from_slice(state).ok()?;
        let number = |value: &Value| u32::try_from(value.as_u64()?).ok();
        self.cursor = match value.get("cursor")? {
            Value::Null => None,
            cursor => Some((number(cursor.get(0)?)?, number(cursor.get(1)?)?)),
        };
        self.sequence = number(value.get("sequence")?)?;
        // More than the scheme keeps is a state it did not write: refused,
        // rather than orphaning a settlement by dropping it.
        let in_flight = value.get("in_flight")?.as_array()?;
        let peers = value.get("peers")?.as_object()?;
        let recovering = value
            .get("recovering")
            .and_then(Value::as_array)
            .map_or(&[][..], Vec::as_slice);
        if in_flight.len() > MAX_IN_FLIGHT
            || peers.len() > MAX_PEERS
            || recovering.len() > MAX_RECOVERING
        {
            return None;
        }
        for entry in in_flight {
            let blob = from_hex(entry.get("blob")?.as_str()?)?;
            let executed = entry
                .get("executed")
                .and_then(Value::as_bool)
                .unwrap_or(true);
            self.in_flight.push(InFlight {
                hash: crate::codec::hash(&blob),
                blob,
                drops: entry
                    .get("drops")?
                    .as_u64()
                    .filter(|&drops| drops <= MAX_ACCOUNT_DROPS)?,
                fee: entry
                    .get("fee")?
                    .as_u64()
                    .filter(|&fee| fee <= MAX_ACCOUNT_DROPS)?,
                sequence: number(entry.get("sequence")?)?,
                last_ledger: number(entry.get("last_ledger")?)?,
                submitted_at: number(entry.get("submitted_at")?)?,
                executed,
                needs_submit: executed,
                looked_up: false,
            });
        }
        for (peer, address) in peers {
            self.learn(Name::new(peer).ok()?, AccountId::parse(address.as_str()?)?);
        }
        for hash in recovering {
            self.recovering
                .push(from_hex(hash.as_str()?)?.try_into().ok()?);
        }
        // Each settlement once, in flight or looked for, and every one in
        // flight below the next sequence: what the scheme writes.
        let mut seen = BTreeSet::new();
        let distinct = (self.in_flight.iter().map(|t| t.hash))
            .chain(self.recovering.iter().copied())
            .all(|hash| seen.insert(hash));
        let ordered = self.in_flight.iter().all(|t| t.sequence < self.sequence);
        (distinct && ordered).then_some(())
    }
}

/// Drops in internal units. Only amounts checked against
/// [`MAX_ACCOUNT_DROPS`] come here.
fn units(drops: u64) -> u64 {
    assert!(drops <= MAX_ACCOUNT_DROPS, "drops past what the books hold");
    drops * UNITS_PER_DROP
}

/// What the books hold, or a difference from it. Every amount booked is
/// within [`MAX_TOTAL`], and so is what the books held before it.
fn narrow(booked: i128) -> u64 {
    let narrow = u64::try_from(booked);
    assert!(narrow.is_ok(), "booked past what a u64 holds");
    narrow.unwrap_or_default()
}

/// A transaction hash as a settlement ID: uppercase hex, 64 characters.
fn hash_name(hash: &[u8; 32]) -> Name {
    Name::hex(hash, true)
}

/// Peering info: the address, as the original's OER wrote it (IA5String,
/// 25 to 35 characters, after a length byte).
fn address_bytes(account: AccountId, out: &mut Vec<u8>) {
    let address = account.to_address();
    out.clear();
    out.push(u8::try_from(address.len()).unwrap_or(0));
    out.extend_from_slice(address.as_bytes());
}

fn parse_address(bytes: &[u8]) -> Result<AccountId, SchemeError> {
    let (&length, rest) = bytes.split_first().ok_or(SchemeError::InvalidData)?;
    if usize::from(length) != rest.len() {
        return Err(SchemeError::InvalidData);
    }
    let text = core::str::from_utf8(rest).map_err(|_| SchemeError::InvalidData)?;
    AccountId::parse(text).ok_or(SchemeError::InvalidData)
}

/// What [`SettlementScheme::check_invariants`] asserts, in parts.
impl XrplScheme {
    fn check_in_flight(&self) {
        let mut seen = BTreeSet::new();
        let distinct = (self.in_flight.iter().map(|t| t.hash))
            .chain(self.recovering.iter().copied())
            .all(|hash| seen.insert(hash));
        assert!(distinct, "each settlement once, in flight or looked up");
        for transaction in &self.in_flight {
            assert!(
                transaction.sequence < self.sequence,
                "in flight below the next sequence"
            );
            assert!(
                !transaction.needs_submit || transaction.executed,
                "only what the node executed is submitted"
            );
            assert!(
                !transaction.looked_up || transaction.executed,
                "only what the node executed is looked up"
            );
            assert!(transaction.drops <= MAX_ACCOUNT_DROPS && transaction.fee <= MAX_ACCOUNT_DROPS);
        }
    }

    fn check_phase(&self) {
        match self.phase {
            Phase::Disconnected => {
                assert!(self.pending.is_empty(), "nothing asked while disconnected");
                assert!(self.buffer.is_empty());
                assert!(!self.funding);
            }
            Phase::CatchingUp { .. } => assert!(self.cursor.is_some()),
            Phase::Synced => {
                assert!(
                    self.buffer.is_empty(),
                    "what was held is booked once synced"
                );
                assert!(self.cursor.is_some());
                // The books hold what is above the reserve, once the scheme
                // knows the balance and has not stopped.
                if let Some(balance) = self.balance
                    && self.failed.is_none()
                {
                    assert_eq!(
                        self.booked,
                        self.bookable(balance, self.owner_count),
                        "the books match the balance"
                    );
                }
            }
            Phase::Starting { .. } | Phase::Unfunded => {}
        }
    }
}

impl SettlementScheme for XrplScheme {
    fn id(&self) -> Name {
        dassie_ledger::name!(SCHEME_ID)
    }

    fn ledger(&self) -> LedgerId {
        LedgerId::XrplTestnetXrp
    }

    fn supported_versions(&self) -> &'static [u16] {
        &[1]
    }

    fn balance(&self) -> u64 {
        self.booked
    }

    fn status(&self) -> LedgerStatus {
        if let Some(why) = self.failed {
            return LedgerStatus::Failed(why);
        }
        match self.phase {
            Phase::Disconnected => LedgerStatus::Connecting,
            Phase::Starting { .. } | Phase::CatchingUp { .. } => LedgerStatus::Syncing,
            Phase::Unfunded => LedgerStatus::Unfunded,
            Phase::Synced => LedgerStatus::Synced,
        }
    }

    fn connection(&self) -> Option<&str> {
        Some(&self.config.server)
    }

    fn unit(&self) -> u64 {
        UNITS_PER_DROP
    }

    fn deposit_address(&self) -> Option<String> {
        Some(self.account().to_address())
    }

    fn reserve(&self) -> u64 {
        self.reserve_drops(self.owner_count)
            .saturating_mul(UNITS_PER_DROP)
    }

    fn state(&self) -> Vec<u8> {
        serde_json::to_vec(&self.state_value()).unwrap_or_default()
    }

    fn state_version(&self) -> u64 {
        self.fingerprint()
    }

    fn on_ledger(&mut self, now_ms: u64, input: LedgerInput<'_>, effects: &mut Vec<Effect>) {
        self.now_ms = self.now_ms.max(now_ms);
        if self.failed.is_some() {
            return;
        }
        match input {
            LedgerInput::Connected => self.connected(effects),
            LedgerInput::Disconnected => self.disconnected(),
            LedgerInput::Message(bytes) => {
                self.heard_ms = self.now_ms;
                self.message(bytes, effects);
            }
            LedgerInput::Funded(_) => {
                self.funding = false;
                if self.phase == Phase::Unfunded {
                    self.send(Request::AccountInfo, effects);
                }
            }
        }
    }

    fn deadline(&self) -> Option<u64> {
        if self.failed.is_some() {
            return None;
        }
        let request = self
            .pending
            .values()
            .map(|(_, sent)| sent.saturating_add(REQUEST_TIMEOUT_MS))
            .min();
        let fund = (self.phase == Phase::Unfunded && !self.funding && self.config.faucet.is_some())
            .then_some(self.fund_at_ms);
        let silence = (self.phase != Phase::Disconnected)
            .then_some(self.heard_ms.saturating_add(LEDGER_SILENCE_MS));
        [request, fund, silence].into_iter().flatten().min()
    }

    fn on_timer(&mut self, now_ms: u64, effects: &mut Vec<Effect>) {
        self.now_ms = self.now_ms.max(now_ms);
        if self.failed.is_some() {
            return;
        }
        let late = self
            .pending
            .values()
            .any(|(_, sent)| sent.saturating_add(REQUEST_TIMEOUT_MS) <= now_ms);
        // A connection that went quiet may be half open: made anew.
        let silent = self.phase != Phase::Disconnected
            && self.heard_ms.saturating_add(LEDGER_SILENCE_MS) <= now_ms;
        if late || silent {
            self.reconnect(effects);
            return;
        }
        if self.phase == Phase::Unfunded {
            self.fund(effects);
        }
    }

    fn peering_info(&self, out: &mut Vec<u8>) {
        address_bytes(self.account(), out);
    }

    fn create_peering_request(
        &mut self,
        peer: Name,
        peering_info: &[u8],
        out: &mut Vec<u8>,
    ) -> Result<(), SchemeError> {
        // The responder's address, learnt now; the original ignored it.
        let address = parse_address(peering_info)?;
        self.learn(peer, address);
        address_bytes(self.account(), out);
        Ok(())
    }

    fn accept_peering_request(
        &mut self,
        peer: Name,
        data: &[u8],
        response: &mut Vec<u8>,
    ) -> Result<Vec<u8>, SchemeError> {
        let address = parse_address(data)?;
        self.learn(peer, address);
        response.clear();
        Ok(data.to_vec())
    }

    fn finalize_peering_request(
        &mut self,
        peer: Name,
        peering_info: &[u8],
        _data: &[u8],
    ) -> Result<Vec<u8>, SchemeError> {
        let address = parse_address(peering_info)?;
        self.learn(peer, address);
        Ok(peering_info.to_vec())
    }

    fn prepare_settlement(
        &mut self,
        _rng: &mut dyn CryptoRng,
        peer: Name,
        amount: u64,
        peer_state: &[u8],
    ) -> Result<PreparedSettlement, SchemeError> {
        let destination = parse_address(peer_state)?;
        if self.peers.get(&peer).map(|(address, _)| *address) != Some(destination) {
            self.learn(peer, destination);
        }
        let drops = amount / UNITS_PER_DROP;
        let fee = self.fee();
        let in_flight: u64 = self
            .in_flight
            .iter()
            .map(|t| t.drops.saturating_add(t.fee))
            .sum();
        let spare = self
            .balance
            .map(|balance| balance.saturating_sub(self.reserve_drops(self.owner_count)))
            .unwrap_or(0)
            .saturating_sub(in_flight);
        let ready = self.phase == Phase::Synced
            && self.failed.is_none()
            && self.sequence < u32::MAX
            && self.in_flight.len() < MAX_IN_FLIGHT
            && self.sequence > 0
            && drops > 0
            && fee <= MAX_FEE_DROPS
            && drops.saturating_add(fee) <= spare;
        if !ready {
            return Err(SchemeError::Unavailable);
        }
        let payment = Payment {
            account: self.account(),
            destination,
            amount: drops,
            fee,
            sequence: self.sequence,
            last_ledger_sequence: self.validated.saturating_add(LEDGER_WINDOW),
            flags: 0,
            network_id: self.network_id,
            memos: alloc::vec![Memo {
                memo_type: MEMO_TYPE.to_vec(),
                memo_data: self.node.as_str().as_bytes().to_vec(),
            }],
            signing_pub_key: *self.wallet.public_key(),
            signature: None,
        };
        let signed = payment.sign(&self.wallet);
        self.sequence = self.sequence.saturating_add(1);
        self.in_flight.push(InFlight {
            hash: signed.hash,
            blob: signed.blob,
            drops,
            fee,
            sequence: signed.payment.sequence,
            last_ledger: signed.payment.last_ledger_sequence,
            submitted_at: self.validated,
            executed: false,
            needs_submit: false,
            looked_up: false,
        });
        Ok(PreparedSettlement {
            id: hash_name(&signed.hash),
            amount: drops * UNITS_PER_DROP,
            // The original's settlement proof: the hash, 32 bytes.
            message: signed.hash.to_vec(),
            peer_state: None,
        })
    }

    fn settlement_message(&self, settlement: Name) -> Vec<u8> {
        // The ID is the hash, which is the message.
        from_hex(settlement.as_str()).unwrap_or_default()
    }

    fn execute_settlement(&mut self, settlement: Name, effects: &mut Vec<Effect>) {
        let found = self
            .in_flight
            .iter()
            .find(|t| hash_name(&t.hash) == settlement)
            .map(|t| t.hash);
        if let Some(hash) = found
            && let Some(t) = self.in_flight.iter_mut().find(|t| t.hash == hash)
        {
            t.executed = true;
            if self.phase == Phase::Synced {
                self.submit(hash, effects);
            } else {
                t.needs_submit = true;
            }
            return;
        }
        // Signed and then lost: it was never submitted.
        effects.push(Effect::Cancel { settlement });
    }

    fn discard_settlement(&mut self, settlement: Name) {
        let Some(position) = self
            .in_flight
            .iter()
            .position(|t| hash_name(&t.hash) == settlement && !t.executed)
        else {
            return;
        };
        let transaction = self.in_flight.remove(position);
        // Its sequence is free again, if it was the last one given.
        if transaction.sequence.saturating_add(1) == self.sequence {
            self.sequence = transaction.sequence;
        }
    }

    fn reconcile(&mut self, pending: &[(Name, u64)], _effects: &mut Vec<Effect>) {
        // Every settlement the node holds pending is ours. One in flight is
        // submitted again; one settled before the node could post it (its
        // posting waited for an earlier announcement) is looked for by its
        // hash, and posted or voided by what the ledger says.
        for (id, _) in pending {
            if let Some(t) = self
                .in_flight
                .iter_mut()
                .find(|t| hash_name(&t.hash) == *id)
            {
                t.executed = true;
                t.needs_submit = true;
            } else if let Some(hash) = from_hex(id.as_str()).and_then(|h| h.try_into().ok())
                && !self.recovering.contains(&hash)
                && self.recovering.len() < MAX_RECOVERING
            {
                self.recovering.push(hash);
            }
        }
    }

    fn handle_settlement(
        &mut self,
        _peer: Name,
        _amount: u64,
        data: &[u8],
        _peer_state: &[u8],
        _effects: &mut Vec<Effect>,
    ) -> Result<(), SchemeError> {
        // The claim books nothing: the ledger says what arrived, once
        // (spec 5 §6.3.6). It must at least be a transaction hash.
        if data.len() == 32 {
            Ok(())
        } else {
            Err(SchemeError::InvalidData)
        }
    }

    fn handle_message(&mut self, _peer: Name, _message: &[u8], _effects: &mut Vec<Effect>) {}

    fn deposit(&mut self, _amount: u64, _effects: &mut Vec<Effect>) -> Result<(), SchemeError> {
        Err(SchemeError::Unsupported)
    }

    fn check_invariants(&self) {
        assert!(self.in_flight.len() <= MAX_IN_FLIGHT);
        assert!(self.peers.len() <= MAX_PEERS);
        assert!(self.recovering.len() <= MAX_RECOVERING);
        assert!(self.pending.len() <= MAX_PENDING);
        assert!(self.buffer.len() <= MAX_BUFFERED);
        assert!(self.next_id >= 1, "request IDs start at 1");
        self.check_in_flight();
        self.check_phase();
    }
}
