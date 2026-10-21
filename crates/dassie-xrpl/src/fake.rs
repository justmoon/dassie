//! A model of the XRP Ledger for simulations and tests: accounts with
//! balances, sequences and reserves, Payments of XRP checked as the ledger
//! checks them (signature, sequence, fee, last ledger, funds), ledgers that
//! close on command with real result codes and close times, the metadata,
//! streams and history the node reads, resets, and a faucet.
//!
//! Tokens on trust lines ([`tokens`]): lines as `RippleState` entries with
//! their owner reserve, `TrustSet`, token Payments that ripple through the
//! issuer with the ledger's failures, and issuers that mint, freeze, set a
//! transfer fee and claw back. The fake is the only source of test RLUSD:
//! [`FakeLedger::new`] makes RLUSD's testnet issuer, at its address.
//!
//! It answers the subset of the rippled WebSocket API in [`crate::api`],
//! at API version 1, as rippled does. It checks what it is sent with the
//! node's own codec, so the codec is pinned by the xrpl.js vectors, not by
//! this.

mod meta;
pub mod tokens;

use alloc::collections::{BTreeMap, BTreeSet};
use alloc::string::ToString;
use alloc::vec::Vec;

use serde_json::{Value, json};

use crate::api::account_flags;
use crate::codec::{Amount, Currency, Payment, TokenAmount, Transaction, TrustSet, hash};
use crate::keys::{AccountId, verify};
use crate::{DROPS_PER_XRP, from_hex, hex_upper};

pub use tokens::{ExactSum, Token};

use meta::{Applied, Touched};
use tokens::{Clawback, IssuerSet, LineKey};

/// The reference fee, in drops.
pub const FEE_BASE: u64 = 10;
/// The base reserve, in drops: 1 XRP.
pub const RESERVE_BASE: u64 = 1_000_000;
/// The reserve per owned object, in drops.
pub const RESERVE_INC: u64 = 200_000;
/// What the faucet gives, in drops: 1000 XRP.
pub const FAUCET_DROPS: u64 = 1_000_000_000;
/// The first ledger: in the millions, as the testnet's are, so a lookup
/// from ledger 1 spans more than a server searches.
pub const FIRST_LEDGER: u32 = 5_000_000;
/// When the first ledger closed, in seconds since the XRP Ledger's epoch.
pub const FIRST_CLOSE_TIME: u64 = 800_000_000;
/// The seconds between one ledger's close and the next's.
pub const CLOSE_INTERVAL_S: u64 = 4;
/// The ledger a reset starts the chain again from: far below where it was,
/// as a new genesis is.
pub const RESTART_LEDGER: u32 = 2_000;
/// The most ledgers a `tx` lookup may span, as rippled's `kMaxRange`.
const MAX_TX_RANGE_LEDGERS: u64 = 1_000;
/// The ledgers an issuer's or genesis's own transaction may wait.
const OWN_TX_LEDGERS: u32 = 20;

/// RLUSD's issuer on the XRPL testnet, `rQhWct2fv4Vc4KRjRgMrxa8xPN9Zx9iLKV`:
/// the fake ledger issues its test RLUSD at the same address.
pub const RLUSD_ISSUER: AccountId = AccountId([
    0xFC, 0xF4, 0xDD, 0x8C, 0x64, 0x63, 0x6B, 0xC5, 0x03, 0xF4, 0xA5, 0x8D, 0xC6, 0xC6, 0x84, 0xD2,
    0xC7, 0xC3, 0xC2, 0x4F,
]);
/// RLUSD's currency code: "RLUSD" in 40 hex digits.
pub const RLUSD_CURRENCY: Currency = Currency([
    0x52, 0x4C, 0x55, 0x53, 0x44, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
]);
/// What an issuer is given from genesis, in drops, to pay its fees.
pub const ISSUER_DROPS: u64 = 1_000 * DROPS_PER_XRP;
/// An issuer's flags, as RLUSD's on the testnet: lines ripple through it,
/// it may claw back, it requires no authorisation and keeps the right to
/// freeze.
pub const ISSUER_FLAGS: u32 =
    account_flags::DEFAULT_RIPPLE | account_flags::ALLOW_TRUST_LINE_CLAWBACK;

/// A connection to the fake ledger.
pub type ConnectionId = u64;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Account {
    balance: u64,
    sequence: u32,
    owner_count: u32,
    /// Its flags ([`account_flags`]).
    flags: u32,
    /// Its transfer rate in billionths; 0 when it charges nothing.
    transfer_rate: u32,
}

impl Account {
    const fn new(balance: u64, sequence: u32) -> Self {
        Self {
            balance,
            sequence,
            owner_count: 0,
            flags: 0,
            transfer_rate: 0,
        }
    }
}

/// A transaction waiting for the next close.
#[derive(Clone, Debug)]
enum Tx {
    Payment(Payment),
    TrustSet(TrustSet),
    /// An issuer's own: a global freeze or a transfer rate.
    AccountSet(IssuerSet),
    /// An issuer's own.
    Clawback(Clawback),
}

impl Tx {
    fn account(&self) -> AccountId {
        match self {
            Self::Payment(tx) => tx.account,
            Self::TrustSet(tx) => tx.account,
            Self::AccountSet(tx) => tx.account,
            Self::Clawback(tx) => tx.account,
        }
    }

    fn sequence(&self) -> u32 {
        match self {
            Self::Payment(tx) => tx.sequence,
            Self::TrustSet(tx) => tx.sequence,
            Self::AccountSet(tx) => tx.sequence,
            Self::Clawback(tx) => tx.sequence,
        }
    }

    fn last_ledger_sequence(&self) -> u32 {
        match self {
            Self::Payment(tx) => tx.last_ledger_sequence,
            Self::TrustSet(tx) => tx.last_ledger_sequence,
            Self::AccountSet(tx) => tx.last_ledger_sequence,
            Self::Clawback(tx) => tx.last_ledger_sequence,
        }
    }

    fn fee(&self) -> u64 {
        match self {
            Self::Payment(tx) => tx.fee,
            Self::TrustSet(tx) => tx.fee,
            Self::AccountSet(_) | Self::Clawback(_) => FEE_BASE,
        }
    }

    /// Its blob, if it was signed and submitted.
    fn signed_blob(&self) -> Option<Vec<u8>> {
        match self {
            Self::Payment(tx) => tx.signature.map(|_| tx.blob()),
            Self::TrustSet(tx) => tx.signature.map(|_| tx.blob()),
            Self::AccountSet(_) | Self::Clawback(_) => None,
        }
    }
}

/// What applying a transaction came to.
struct Outcome {
    code: &'static str,
    /// What a successful Payment delivered.
    delivered: Option<Amount>,
}

impl Outcome {
    const fn code(code: &'static str) -> Self {
        Self {
            code,
            delivered: None,
        }
    }
}

/// The fake ledger. A clone is a server that stops where the ledger was:
/// one behind the others, once they go on.
#[derive(Clone)]
pub struct FakeLedger {
    accounts: BTreeMap<AccountId, Account>,
    /// Trust lines, by their low and high account and currency.
    lines: BTreeMap<LineKey, crate::api::TrustLine>,
    /// The accounts that issue tokens here, which the fake acts as.
    issuers: BTreeSet<AccountId>,
    /// What each issuer's holders hold of each of its currencies, by
    /// what it minted less what came back to it, was clawed back or wiped.
    supply: BTreeMap<Token, ExactSum>,
    /// The last validated ledger.
    ledger: u32,
    /// When it closed, in seconds since the XRP Ledger's epoch.
    close_time: u64,
    /// The first ledger the history holds: before it, `account_tx` and
    /// `tx` know nothing.
    history_from: u32,
    /// The first ledger `account_tx` reaches, at or after `history_from`.
    account_tx_from: u32,
    history: Vec<Applied>,
    queued: Vec<Tx>,
    queued_hashes: BTreeSet<[u8; 32]>,
    accounts_of: BTreeMap<ConnectionId, BTreeSet<AccountId>>,
    ledger_stream: BTreeSet<ConnectionId>,
    /// The genesis account, which pays the faucet's XRP.
    genesis: AccountId,
    /// What the fees burned, in drops.
    burned: u64,
    /// What resets wiped, in drops.
    wiped: u64,
    /// The reserve, which [`set_reserve`](Self::set_reserve) changes.
    reserve: (u64, u64),
    /// The network ID `server_state` reports: the testnet's, unless
    /// [`set_network_id`](Self::set_network_id) changes it.
    network_id: Option<u32>,
}

impl Default for FakeLedger {
    fn default() -> Self {
        Self::new()
    }
}

impl FakeLedger {
    /// A ledger with only the genesis account and RLUSD's issuer.
    #[must_use]
    pub fn new() -> Self {
        let genesis = AccountId([0xAB; 20]);
        let mut accounts = BTreeMap::new();
        accounts.insert(genesis, Account::new(crate::MAX_DROPS, 1));
        let mut ledger = Self {
            accounts,
            lines: BTreeMap::new(),
            issuers: BTreeSet::new(),
            supply: BTreeMap::new(),
            ledger: FIRST_LEDGER,
            close_time: FIRST_CLOSE_TIME,
            history_from: FIRST_LEDGER,
            account_tx_from: FIRST_LEDGER,
            history: Vec::new(),
            queued: Vec::new(),
            queued_hashes: BTreeSet::new(),
            accounts_of: BTreeMap::new(),
            ledger_stream: BTreeSet::new(),
            genesis,
            burned: 0,
            wiped: 0,
            reserve: (RESERVE_BASE, RESERVE_INC),
            network_id: Some(1),
        };
        ledger.add_issuer(RLUSD_ISSUER);
        ledger
    }

    /// Makes `issuer` an account the fake acts as, with [`ISSUER_FLAGS`]
    /// and [`ISSUER_DROPS`] from genesis, as of the current ledger. It
    /// stays through resets, as an issuer that came back at its address.
    pub fn add_issuer(&mut self, issuer: AccountId) {
        assert_ne!(issuer, self.genesis, "genesis issues nothing");
        if self.issuers.insert(issuer) && !self.accounts.contains_key(&issuer) {
            let genesis = self
                .accounts
                .entry(self.genesis)
                .or_insert(Account::new(0, 1));
            assert!(genesis.balance >= ISSUER_DROPS, "genesis funds issuers");
            genesis.balance -= ISSUER_DROPS;
            let mut account = Account::new(ISSUER_DROPS, self.ledger);
            account.flags = ISSUER_FLAGS;
            self.accounts.insert(issuer, account);
        }
    }

    /// The last validated ledger.
    #[must_use]
    pub const fn ledger(&self) -> u32 {
        self.ledger
    }

    /// How many transactions wait for the next close, which applies them.
    #[must_use]
    pub fn queued(&self) -> usize {
        self.queued.len()
    }

    /// An account's balance in drops, if it exists.
    #[must_use]
    pub fn balance(&self, account: AccountId) -> Option<u64> {
        self.accounts.get(&account).map(|a| a.balance)
    }

    /// The objects an account owns, such as trust lines, if it exists.
    #[must_use]
    pub fn owner_count(&self, account: AccountId) -> Option<u32> {
        self.accounts.get(&account).map(|a| a.owner_count)
    }

    /// An account's flags ([`account_flags`]), if it exists.
    #[must_use]
    pub fn account_flags(&self, account: AccountId) -> Option<u32> {
        self.accounts.get(&account).map(|a| a.flags)
    }

    /// All XRP there is, with what fees burned and resets wiped: it never
    /// changes.
    #[must_use]
    pub fn total(&self) -> u64 {
        self.accounts
            .values()
            .map(|a| a.balance)
            .sum::<u64>()
            .saturating_add(self.burned)
            .saturating_add(self.wiped)
    }

    /// What fees burned, in drops.
    #[must_use]
    pub const fn burned(&self) -> u64 {
        self.burned
    }

    /// Forgets the history before the current ledger, as a server that
    /// keeps little does.
    pub fn forget_history(&mut self) {
        self.history_from = self.ledger.saturating_add(1);
        self.account_tx_from = self.history_from;
        self.history.clear();
    }

    /// Cuts `account_tx` at the current ledger, while `tx` still finds
    /// what came before, as a server whose account index is behind does.
    pub fn shorten_account_history(&mut self) {
        self.account_tx_from = self.ledger.saturating_add(1);
    }

    /// Changes the reserve, as the network may vote to.
    pub fn set_reserve(&mut self, base: u64, increment: u64) {
        self.reserve = (base, increment);
    }

    /// Changes the network ID the server reports, as a server of another
    /// network would; `None` leaves it out.
    pub fn set_network_id(&mut self, id: Option<u32>) {
        self.network_id = id;
    }

    /// Sets when the last ledger closed, in seconds since the XRP Ledger's
    /// epoch, as for a chain that started then: one started later closes
    /// later than every ledger of one before it, as after a reset.
    pub fn set_close_time(&mut self, close_time: u64) {
        self.close_time = close_time;
    }

    /// Starts the chain again, as a testnet reset does: every account but
    /// genesis and the issuers, every trust line and all history wiped,
    /// from a ledger below the last, which closes later.
    pub fn reset(&mut self) {
        self.ledger = RESTART_LEDGER.min(self.ledger.saturating_sub(1));
        self.close_time = self.close_time.saturating_add(CLOSE_INTERVAL_S);
        self.history_from = self.ledger;
        self.account_tx_from = self.ledger;
        self.history.clear();
        let (genesis, issuers) = (self.genesis, &self.issuers);
        let kept = |id: &AccountId| *id == genesis || issuers.contains(id);
        let wiped: u64 = self
            .accounts
            .iter()
            .filter(|(id, _)| !kept(id))
            .map(|(_, account)| account.balance)
            .sum();
        self.wiped = self.wiped.saturating_add(wiped);
        self.accounts.retain(|id, _| kept(id));
        // Their lines are gone, and with them what their holders held.
        for account in self.accounts.values_mut() {
            account.owner_count = 0;
        }
        self.lines.clear();
        self.supply.clear();
        self.queued.clear();
        self.queued_hashes.clear();
    }

    /// Removes `account` and wipes its XRP, while the chain goes on: an
    /// account absent as of the ledgers its node reads next, which no
    /// transaction in its history explains.
    pub fn remove_account(&mut self, account: AccountId) {
        if let Some(removed) = self.accounts.remove(&account) {
            self.wiped = self.wiped.saturating_add(removed.balance);
        }
    }

    /// A connection went away.
    pub fn disconnect(&mut self, connection: ConnectionId) {
        self.accounts_of.remove(&connection);
        self.ledger_stream.remove(&connection);
    }

    /// Pays `drops` from genesis to `to`, in the next ledger: a deposit from
    /// outside.
    pub fn deposit(&mut self, to: AccountId, drops: u64) {
        let sequence = self.next_sequence(self.genesis);
        self.queued.push(Tx::Payment(Payment {
            account: self.genesis,
            destination: to,
            amount: Amount::Xrp(drops),
            fee: FEE_BASE,
            sequence,
            last_ledger_sequence: self.ledger.saturating_add(OWN_TX_LEDGERS),
            flags: 0,
            network_id: None,
            memos: Vec::new(),
            signing_pub_key: [0; 33],
            signature: None,
        }));
    }

    /// Sets the sequence `account` uses next, as for an account that has
    /// sent all but the last few transactions it ever can.
    pub fn set_sequence(&mut self, account: AccountId, sequence: u32) {
        if let Some(account) = self.accounts.get_mut(&account) {
            account.sequence = sequence;
        }
    }

    /// The faucet: funds `to` in the next ledger.
    pub fn fund(&mut self, to: AccountId) {
        self.deposit(to, FAUCET_DROPS);
    }

    /// Answers a request from `connection`. What it returns is for that
    /// connection.
    pub fn request(&mut self, connection: ConnectionId, bytes: &[u8]) -> Vec<u8> {
        let Ok(request) = serde_json::from_slice::<Value>(bytes) else {
            return Vec::new();
        };
        let id = request.get("id").cloned().unwrap_or(Value::Null);
        let command = request.get("command").and_then(Value::as_str).unwrap_or("");
        let outcome = match command {
            "server_state" => Ok(self.server_state()),
            "subscribe" => self.subscribe(connection, &request),
            "account_info" => self.account_info(&request),
            "account_tx" => self.account_tx(&request),
            "ledger_entry" => self.ledger_entry(&request),
            "tx" => self.tx(&request),
            "submit" => self.submit(&request),
            _ => Err(json!({ "error": "unknownCmd" })),
        };
        let response = match outcome {
            Ok(result) => {
                json!({ "type": "response", "id": id, "status": "success", "result": result })
            }
            Err(mut error) => {
                if let Some(object) = error.as_object_mut() {
                    object.insert("type".into(), "response".into());
                    object.insert("id".into(), id);
                    object.insert("status".into(), "error".into());
                }
                error
            }
        };
        serde_json::to_vec(&response).unwrap_or_default()
    }

    /// Closes a ledger [`CLOSE_INTERVAL_S`] after the last: applies what
    /// was submitted, and returns what the streams carry, by connection.
    pub fn close(&mut self) -> Vec<(ConnectionId, Vec<u8>)> {
        self.close_at(self.close_time.saturating_add(CLOSE_INTERVAL_S))
    }

    /// Closes a ledger at `close_time`, in seconds since the XRP Ledger's
    /// epoch, or with the last if that is later: close times never go
    /// back along a chain.
    pub fn close_at(&mut self, close_time: u64) -> Vec<(ConnectionId, Vec<u8>)> {
        self.ledger = self.ledger.saturating_add(1);
        self.close_time = self.close_time.max(close_time);
        let mut out = Vec::new();
        let queued = core::mem::take(&mut self.queued);
        self.queued_hashes.clear();
        for (index, tx) in queued.into_iter().enumerate() {
            let index = u32::try_from(index).unwrap_or(u32::MAX);
            if let Some(applied) = self.apply(&tx, index) {
                for (connection, accounts) in &self.accounts_of {
                    if applied.accounts.iter().any(|a| accounts.contains(a)) {
                        out.push((
                            *connection,
                            serde_json::to_vec(&applied.message).unwrap_or_default(),
                        ));
                    }
                }
                self.history.push(applied);
            }
        }
        let closed = json!({
            "type": "ledgerClosed",
            "ledger_index": self.ledger,
            "ledger_time": self.close_time,
            "fee_base": FEE_BASE,
            "reserve_base": self.reserve.0,
            "reserve_inc": self.reserve.1,
            "validated_ledgers": alloc::format!("{}-{}", self.history_from, self.ledger),
        });
        let closed = serde_json::to_vec(&closed).unwrap_or_default();
        for connection in &self.ledger_stream {
            out.push((*connection, closed.clone()));
        }
        out
    }

    fn next_sequence(&self, account: AccountId) -> u32 {
        let base = self.accounts.get(&account).map_or(1, |a| a.sequence);
        let queued = self
            .queued
            .iter()
            .filter(|tx| tx.account() == account)
            .count();
        base.saturating_add(u32::try_from(queued).unwrap_or(u32::MAX))
    }

    fn reserve_of(&self, account: &Account) -> u64 {
        self.reserve.0.saturating_add(
            self.reserve
                .1
                .saturating_mul(u64::from(account.owner_count)),
        )
    }

    fn server_state(&self) -> Value {
        let mut state = json!({
            "validated_ledger": {
                "seq": self.ledger,
                "close_time": self.close_time,
                "base_fee": FEE_BASE,
                "reserve_base": self.reserve.0,
                "reserve_inc": self.reserve.1,
            },
            "load_factor": 256,
            "load_base": 256,
            "complete_ledgers": alloc::format!("{}-{}", self.history_from, self.ledger),
        });
        if let (Some(id), Some(object)) = (self.network_id, state.as_object_mut()) {
            object.insert("network_id".into(), id.into());
        }
        json!({ "state": state })
    }

    fn subscribe(&mut self, connection: ConnectionId, request: &Value) -> Result<Value, Value> {
        if let Some(accounts) = request.get("accounts").and_then(Value::as_array) {
            for account in accounts {
                let account = account
                    .as_str()
                    .and_then(AccountId::parse)
                    .ok_or_else(|| json!({ "error": "actMalformed" }))?;
                self.accounts_of
                    .entry(connection)
                    .or_default()
                    .insert(account);
            }
        }
        let streams = request.get("streams").and_then(Value::as_array);
        if streams.is_some_and(|streams| streams.iter().any(|s| s.as_str() == Some("ledger"))) {
            self.ledger_stream.insert(connection);
        }
        Ok(json!({
            "ledger_index": self.ledger,
            "fee_base": FEE_BASE,
            "reserve_base": self.reserve.0,
            "reserve_inc": self.reserve.1,
        }))
    }

    fn account_of(request: &Value) -> Result<AccountId, Value> {
        request
            .get("account")
            .and_then(Value::as_str)
            .and_then(AccountId::parse)
            .ok_or_else(|| json!({ "error": "actMalformed" }))
    }

    fn account_info(&self, request: &Value) -> Result<Value, Value> {
        let id = Self::account_of(request)?;
        // As rippled, an absence names the ledger it is as of.
        let account = self.accounts.get(&id).ok_or_else(
            || json!({ "error": "actNotFound", "ledger_index": self.ledger, "validated": true }),
        )?;
        Ok(json!({
            "account_data": meta::account_fields(id, account),
            "ledger_index": self.ledger,
            "validated": true,
        }))
    }

    /// `ledger_entry` for a trust line: `ripple_state` by its two accounts
    /// and currency, as of the last validated ledger.
    fn ledger_entry(&self, request: &Value) -> Result<Value, Value> {
        let malformed = || json!({ "error": "malformedRequest" });
        let state = request.get("ripple_state").ok_or_else(malformed)?;
        let accounts = state
            .get("accounts")
            .and_then(Value::as_array)
            .filter(|accounts| accounts.len() == 2)
            .ok_or_else(malformed)?;
        let account = |index: usize| {
            accounts
                .get(index)
                .and_then(Value::as_str)
                .and_then(AccountId::parse)
                .ok_or_else(malformed)
        };
        let (a, b) = (account(0)?, account(1)?);
        let currency = state
            .get("currency")
            .and_then(Value::as_str)
            .and_then(Currency::parse)
            .ok_or_else(malformed)?;
        if a == b {
            return Err(malformed());
        }
        let key = tokens::line_key(a, b, currency);
        let line = self.lines.get(&key).ok_or_else(
            || json!({ "error": "entryNotFound", "ledger_index": self.ledger, "validated": true }),
        )?;
        let mut node = meta::line_fields(line);
        if let Some(object) = node.as_object_mut() {
            object.insert("LedgerEntryType".into(), "RippleState".into());
        }
        Ok(json!({
            "index": hex_upper(&tokens::line_index(&key)),
            "ledger_index": self.ledger,
            "node": node,
            "validated": true,
        }))
    }

    fn account_tx(&self, request: &Value) -> Result<Value, Value> {
        let id = Self::account_of(request)?;
        let number = |key: &str| {
            request
                .get(key)
                .and_then(Value::as_u64)
                .and_then(|n| u32::try_from(n).ok())
        };
        // As rippled at API version 1: a range beyond the history is cut
        // to it, and the answer says so.
        let min = number("ledger_index_min")
            .unwrap_or(self.account_tx_from)
            .max(self.account_tx_from);
        let max = number("ledger_index_max")
            .unwrap_or(self.ledger)
            .min(self.ledger);
        let limit = usize::try_from(request.get("limit").and_then(Value::as_u64).unwrap_or(200))
            .unwrap_or(200)
            .clamp(1, 400);
        let skip = request
            .get("marker")
            .and_then(Value::as_u64)
            .and_then(|m| usize::try_from(m).ok())
            .unwrap_or(0);
        let matching: Vec<&Applied> = self
            .history
            .iter()
            .filter(|a| a.ledger >= min && a.ledger <= max && a.accounts.contains(&id))
            .collect();
        let page: Vec<Value> = matching
            .iter()
            .skip(skip)
            .take(limit)
            .map(|a| {
                let mut tx = a.message.get("transaction").cloned().unwrap_or(Value::Null);
                if let Some(object) = tx.as_object_mut() {
                    object.insert("ledger_index".into(), a.ledger.into());
                }
                json!({ "tx": tx, "meta": a.message.get("meta"), "validated": true })
            })
            .collect();
        let next = skip.saturating_add(page.len());
        let mut result = json!({
            "account": id.to_address(),
            "ledger_index_min": min,
            "ledger_index_max": max,
            "transactions": page,
        });
        if next < matching.len()
            && let Some(object) = result.as_object_mut()
        {
            object.insert("marker".into(), next.into());
        }
        Ok(result)
    }

    fn tx(&self, request: &Value) -> Result<Value, Value> {
        let hash: [u8; 32] = request
            .get("transaction")
            .and_then(Value::as_str)
            .and_then(from_hex)
            .and_then(|bytes| bytes.try_into().ok())
            .ok_or_else(|| json!({ "error": "notImpl" }))?;
        // As rippled, a range is checked before anything is looked up: not
        // backwards, and at most `MAX_TX_RANGE_LEDGERS`.
        if let (Some(min), Some(max)) = (
            request.get("min_ledger").and_then(Value::as_u64),
            request.get("max_ledger").and_then(Value::as_u64),
        ) {
            if max < min {
                return Err(json!({ "error": "invalidLgrRange" }));
            }
            if max - min > MAX_TX_RANGE_LEDGERS {
                return Err(json!({ "error": "excessiveLgrRange" }));
            }
        }
        if let Some(applied) = self.history.iter().find(|a| a.hash == hash) {
            let mut result = applied
                .message
                .get("transaction")
                .cloned()
                .unwrap_or(Value::Null);
            if let Some(object) = result.as_object_mut() {
                object.insert(
                    "meta".into(),
                    applied.message.get("meta").cloned().unwrap_or(Value::Null),
                );
                object.insert("ledger_index".into(), applied.ledger.into());
                object.insert("validated".into(), true.into());
            }
            return Ok(result);
        }
        let number = |key: &str| request.get(key).and_then(Value::as_u64);
        let searched_all = match (number("min_ledger"), number("max_ledger")) {
            (Some(min), Some(max)) => {
                min >= u64::from(self.history_from) && max <= u64::from(self.ledger)
            }
            _ => false,
        };
        Err(json!({ "error": "txnNotFound", "searched_all": searched_all }))
    }

    fn submit(&mut self, request: &Value) -> Result<Value, Value> {
        let blob = request
            .get("tx_blob")
            .and_then(Value::as_str)
            .and_then(from_hex)
            .ok_or_else(|| json!({ "error": "invalidTransaction" }))?;
        let result = |code: &str| Ok(json!({ "engine_result": code, "tx_blob": hex_upper(&blob) }));
        // What the codec cannot read, the node would never send: a partial
        // Payment, paths, `SendMax`, or any other transaction.
        let Ok(tx) = Transaction::decode(&blob) else {
            return result("temMALFORMED");
        };
        let tx_hash = hash(&blob);
        if self.queued_hashes.contains(&tx_hash) || self.history.iter().any(|a| a.hash == tx_hash) {
            return result("tefALREADY");
        }
        if !signed_by_sender(&tx) {
            return result("temBAD_SIGNATURE");
        }
        let tx = match tx {
            Transaction::Payment(payment) => Tx::Payment(payment),
            Transaction::TrustSet(trust_set) => Tx::TrustSet(trust_set),
        };
        if let Err(code) = preflight(&tx) {
            return result(code);
        }
        if !self.accounts.contains_key(&tx.account()) {
            return result("terNO_ACCOUNT");
        }
        if tx.fee() < FEE_BASE {
            return result("telINSUF_FEE_P");
        }
        if tx.last_ledger_sequence() <= self.ledger {
            return result("tefMAX_LEDGER");
        }
        let next = self.next_sequence(tx.account());
        if tx.sequence() < next {
            return result("tefPAST_SEQ");
        }
        if tx.sequence() > next {
            return result("terPRE_SEQ");
        }
        self.queued_hashes.insert(tx_hash);
        self.queued.push(tx);
        result("tesSUCCESS")
    }

    /// Applies a queued transaction, as the ledger closes: its fee is
    /// burned and its sequence used whatever its result, and nothing else
    /// changes unless it succeeds.
    fn apply(&mut self, tx: &Tx, index: u32) -> Option<Applied> {
        let sender = *self.accounts.get(&tx.account())?;
        if tx.last_ledger_sequence() < self.ledger || tx.sequence() != sender.sequence {
            return None;
        }
        let fee = tx.fee().min(sender.balance);
        self.burned = self.burned.saturating_add(fee);
        let mut touched = Touched::default();
        let after_fee = Account {
            balance: sender.balance - fee,
            sequence: sender.sequence.saturating_add(1),
            ..sender
        };
        self.put_account(&mut touched, tx.account(), after_fee);
        let outcome = match tx {
            Tx::Payment(payment) => match payment.amount {
                Amount::Xrp(drops) => self.pay_xrp(&mut touched, payment, drops),
                Amount::Token(amount) => {
                    self.pay_token(&mut touched, payment.account, payment.destination, amount)
                }
            },
            Tx::TrustSet(trust_set) => {
                Outcome::code(self.trust_set(&mut touched, trust_set, sender.balance))
            }
            Tx::AccountSet(set) => Outcome::code(self.issuer_set(&mut touched, set)),
            Tx::Clawback(clawback) => Outcome::code(self.claw(&mut touched, clawback)),
        };
        Some(self.applied(tx, index, &outcome, &touched))
    }

    /// A Payment of XRP, after its fee.
    fn pay_xrp(&mut self, touched: &mut Touched, payment: &Payment, amount: u64) -> Outcome {
        let Some(sender) = self.accounts.get(&payment.account).copied() else {
            return Outcome::code("tefINTERNAL");
        };
        let destination = self.accounts.get(&payment.destination).copied();
        let code = if destination.is_none() && amount < self.reserve.0 {
            "tecNO_DST_INSUF_XRP"
        } else if sender.balance < amount.saturating_add(self.reserve_of(&sender))
            && payment.account != self.genesis
        {
            "tecUNFUNDED_PAYMENT"
        } else {
            "tesSUCCESS"
        };
        if code != "tesSUCCESS" {
            return Outcome::code(code);
        }
        let sender_after = Account {
            balance: sender.balance - amount,
            ..sender
        };
        self.put_account(touched, payment.account, sender_after);
        let after = destination.map_or_else(
            || Account::new(amount, self.ledger),
            |d| Account {
                balance: d.balance.saturating_add(amount),
                ..d
            },
        );
        self.put_account(touched, payment.destination, after);
        Outcome {
            code,
            delivered: Some(Amount::Xrp(amount)),
        }
    }

    /// Writes an account, keeping what it was before the transaction.
    fn put_account(&mut self, touched: &mut Touched, id: AccountId, account: Account) {
        touched.account(id, self.accounts.get(&id).copied());
        self.accounts.insert(id, account);
    }

    /// The fake's own transaction, unsigned: its hash is made from its
    /// ledger and index.
    fn queue_own(&mut self, tx: Tx) {
        self.queued.push(tx);
    }

    /// The sequence and last ledger of `issuer`'s next own transaction, if
    /// it is one of the fake's issuers.
    fn own_slot(&self, issuer: AccountId) -> Result<(u32, u32), &'static str> {
        if !self.issuers.contains(&issuer) || !self.accounts.contains_key(&issuer) {
            return Err("terNO_ACCOUNT");
        }
        Ok((
            self.next_sequence(issuer),
            self.ledger.saturating_add(OWN_TX_LEDGERS),
        ))
    }
}

/// Whether a submitted transaction is signed, by the key of its sender.
fn signed_by_sender(tx: &Transaction) -> bool {
    let (account, key, signature, payload) = match tx {
        Transaction::Payment(payment) => (
            payment.account,
            payment.signing_pub_key,
            payment.signature,
            Payment {
                signature: None,
                ..payment.clone()
            }
            .signing_payload(),
        ),
        Transaction::TrustSet(trust_set) => (
            trust_set.account,
            trust_set.signing_pub_key,
            trust_set.signature,
            TrustSet {
                signature: None,
                ..trust_set.clone()
            }
            .signing_payload(),
        ),
    };
    signature.is_some_and(|signature| verify(&key, &payload, &signature))
        && AccountId::from_public_key(&key) == account
}

/// Universal transaction flags: `tfFullyCanonicalSig`.
const TF_FULLY_CANONICAL_SIG: u32 = 0x8000_0000;
/// The Payment flags the fake knows: `tfNoRippleDirect` and
/// `tfLimitQuality`, which change nothing without paths. The codec refuses
/// `tfPartialPayment`.
const PAYMENT_FLAGS: u32 = 0x0001_0000 | 0x0004_0000 | TF_FULLY_CANONICAL_SIG;

/// What the ledger checks before anything else: the transaction alone.
fn preflight(tx: &Tx) -> Result<(), &'static str> {
    match tx {
        Tx::Payment(payment) => {
            if payment.flags & !PAYMENT_FLAGS != 0 {
                return Err("temINVALID_FLAG");
            }
            let zero = match &payment.amount {
                Amount::Xrp(drops) => *drops == 0,
                Amount::Token(token) => token.value.is_zero(),
            };
            if zero {
                return Err("temBAD_AMOUNT");
            }
            if payment.account == payment.destination {
                return Err("temREDUNDANT");
            }
            Ok(())
        }
        Tx::TrustSet(trust_set) => tokens::preflight_trust_set(trust_set),
        Tx::AccountSet(_) | Tx::Clawback(_) => Ok(()),
    }
}

/// A token amount as the ledger writes one.
fn token_json(amount: &TokenAmount) -> Value {
    json!({
        "currency": amount.currency.to_string(),
        "issuer": amount.issuer.to_address(),
        "value": amount.value.to_string(),
    })
}
