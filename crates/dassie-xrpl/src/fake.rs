//! A model of the XRP Ledger for simulations and tests: accounts with
//! balances, sequences and reserves, Payments of XRP checked as the ledger
//! checks them (signature, sequence, fee, last ledger, funds), ledgers that
//! close on command with real result codes, the metadata, streams and
//! history the node reads, and a faucet.
//!
//! It answers the subset of the rippled WebSocket API in [`crate::api`],
//! at API version 1, as rippled does. It checks what it is sent with the
//! node's own codec, so the codec is pinned by the xrpl.js vectors, not by
//! this.

use alloc::collections::{BTreeMap, BTreeSet};
use alloc::string::{String, ToString};
use alloc::vec::Vec;

use serde_json::{Value, json};

use crate::codec::{Payment, hash};
use crate::keys::{AccountId, verify};
use crate::{from_hex, hex_upper};

/// The reference fee, in drops.
pub const FEE_BASE: u64 = 10;
/// The base reserve, in drops: 1 XRP.
pub const RESERVE_BASE: u64 = 1_000_000;
/// The reserve per owned object, in drops.
pub const RESERVE_INC: u64 = 200_000;
/// What the faucet gives, in drops: 1000 XRP.
pub const FAUCET_DROPS: u64 = 1_000_000_000;
/// The first ledger.
pub const FIRST_LEDGER: u32 = 1_000;

/// A connection to the fake ledger.
pub type ConnectionId = u64;

#[derive(Clone, Copy, Debug)]
struct Account {
    balance: u64,
    sequence: u32,
    owner_count: u32,
}

/// A transaction in a validated ledger, with its metadata.
#[derive(Clone, Debug)]
struct Applied {
    ledger: u32,
    hash: [u8; 32],
    accounts: Vec<AccountId>,
    message: Value,
}

/// The fake ledger.
pub struct FakeLedger {
    accounts: BTreeMap<AccountId, Account>,
    /// The last validated ledger.
    ledger: u32,
    /// The first ledger the history holds: before it, `account_tx` and
    /// `tx` know nothing.
    history_from: u32,
    /// The first ledger `account_tx` reaches, at or after `history_from`.
    account_tx_from: u32,
    history: Vec<Applied>,
    queued: Vec<Payment>,
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
}

impl Default for FakeLedger {
    fn default() -> Self {
        Self::new()
    }
}

impl FakeLedger {
    /// A ledger with only the genesis account.
    #[must_use]
    pub fn new() -> Self {
        let genesis = AccountId([0xAB; 20]);
        let mut accounts = BTreeMap::new();
        accounts.insert(
            genesis,
            Account {
                balance: crate::MAX_DROPS,
                sequence: 1,
                owner_count: 0,
            },
        );
        Self {
            accounts,
            ledger: FIRST_LEDGER,
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
        }
    }

    /// The last validated ledger.
    #[must_use]
    pub const fn ledger(&self) -> u32 {
        self.ledger
    }

    /// An account's balance in drops, if it exists.
    #[must_use]
    pub fn balance(&self, account: AccountId) -> Option<u64> {
        self.accounts.get(&account).map(|a| a.balance)
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

    /// Wipes every account but genesis, as a testnet reset does.
    pub fn reset(&mut self) {
        let genesis = self.accounts.get(&self.genesis).copied();
        let wiped: u64 = self
            .accounts
            .iter()
            .filter(|(id, _)| **id != self.genesis)
            .map(|(_, account)| account.balance)
            .sum();
        self.wiped = self.wiped.saturating_add(wiped);
        self.accounts.clear();
        if let Some(genesis) = genesis {
            self.accounts.insert(self.genesis, genesis);
        }
        self.queued.clear();
        self.queued_hashes.clear();
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
        self.queued.push(Payment {
            account: self.genesis,
            destination: to,
            amount: drops,
            fee: FEE_BASE,
            sequence,
            last_ledger_sequence: self.ledger.saturating_add(20),
            flags: 0,
            network_id: None,
            memos: Vec::new(),
            signing_pub_key: [0; 33],
            signature: None,
        });
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

    /// Closes a ledger: applies what was submitted, and returns what the
    /// streams carry, by connection.
    pub fn close(&mut self) -> Vec<(ConnectionId, Vec<u8>)> {
        self.ledger = self.ledger.saturating_add(1);
        let mut out = Vec::new();
        let queued = core::mem::take(&mut self.queued);
        self.queued_hashes.clear();
        for (index, payment) in queued.into_iter().enumerate() {
            let index = u32::try_from(index).unwrap_or(u32::MAX);
            if let Some(applied) = self.apply(payment, index) {
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
        let queued = self.queued.iter().filter(|p| p.account == account).count();
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
        json!({ "state": {
            "validated_ledger": {
                "seq": self.ledger,
                "base_fee": FEE_BASE,
                "reserve_base": self.reserve.0,
                "reserve_inc": self.reserve.1,
            },
            "load_factor": 256,
            "load_base": 256,
            "network_id": 1,
            "complete_ledgers": alloc::format!("{}-{}", self.history_from, self.ledger),
        }})
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
        let account = self
            .accounts
            .get(&id)
            .ok_or_else(|| json!({ "error": "actNotFound" }))?;
        Ok(json!({
            "account_data": {
                "Account": id.to_address(),
                "Balance": account.balance.to_string(),
                "Sequence": account.sequence,
                "OwnerCount": account.owner_count,
            },
            "ledger_index": self.ledger,
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
        let Ok(payment) = Payment::decode(&blob) else {
            return result("temMALFORMED");
        };
        let tx_hash = hash(&blob);
        if self.queued_hashes.contains(&tx_hash) || self.history.iter().any(|a| a.hash == tx_hash) {
            return result("tefALREADY");
        }
        let Some(signature) = payment.signature else {
            return result("temBAD_SIGNATURE");
        };
        let unsigned = Payment {
            signature: None,
            ..payment.clone()
        };
        if !verify(
            &payment.signing_pub_key,
            &unsigned.signing_payload(),
            &signature,
        ) || AccountId::from_public_key(&payment.signing_pub_key) != payment.account
        {
            return result("temBAD_SIGNATURE");
        }
        if payment.amount == 0 {
            return result("temBAD_AMOUNT");
        }
        if !self.accounts.contains_key(&payment.account) {
            return result("terNO_ACCOUNT");
        }
        if payment.fee < FEE_BASE {
            return result("telINSUF_FEE_P");
        }
        if payment.last_ledger_sequence <= self.ledger {
            return result("tefMAX_LEDGER");
        }
        let next = self.next_sequence(payment.account);
        if payment.sequence < next {
            return result("tefPAST_SEQ");
        }
        if payment.sequence > next {
            return result("terPRE_SEQ");
        }
        self.queued_hashes.insert(tx_hash);
        self.queued.push(payment);
        result("tesSUCCESS")
    }

    /// Applies a queued Payment, as the ledger closes.
    fn apply(&mut self, payment: Payment, index: u32) -> Option<Applied> {
        let sender = *self.accounts.get(&payment.account)?;
        if payment.last_ledger_sequence < self.ledger || payment.sequence != sender.sequence {
            return None;
        }
        let fee = payment.fee.min(sender.balance);
        let destination = self.accounts.get(&payment.destination).copied();
        let after_fee = sender.balance - fee;
        let code = if destination.is_none() && payment.amount < self.reserve.0 {
            "tecNO_DST_INSUF_XRP"
        } else if after_fee < payment.amount.saturating_add(self.reserve_of(&sender))
            && payment.account != self.genesis
        {
            "tecUNFUNDED_PAYMENT"
        } else {
            "tesSUCCESS"
        };
        let moved = if code == "tesSUCCESS" {
            payment.amount
        } else {
            0
        };
        self.burned = self.burned.saturating_add(fee);
        let sender_after = Account {
            balance: after_fee - moved,
            sequence: sender.sequence.saturating_add(1),
            ..sender
        };
        self.accounts.insert(payment.account, sender_after);
        let mut nodes = alloc::vec![account_node(
            "ModifiedNode",
            payment.account,
            sender.balance,
            &sender_after
        )];
        if moved > 0 {
            let before = destination.map_or(0, |d| d.balance);
            let after = Account {
                balance: before.saturating_add(moved),
                sequence: destination.map_or(self.ledger, |d| d.sequence),
                owner_count: destination.map_or(0, |d| d.owner_count),
            };
            self.accounts.insert(payment.destination, after);
            let kind = if destination.is_some() {
                "ModifiedNode"
            } else {
                "CreatedNode"
            };
            nodes.push(account_node(kind, payment.destination, before, &after));
        }
        Some(self.applied(&payment, index, code, moved, nodes))
    }

    fn applied(
        &self,
        payment: &Payment,
        index: u32,
        code: &str,
        moved: u64,
        nodes: Vec<Value>,
    ) -> Applied {
        // The hash of the transaction as submitted. Genesis's own are not
        // signed: their ledger and index make them unique.
        let tx_hash = if payment.signature.is_some() {
            hash(&payment.blob())
        } else {
            let mut seed = Vec::new();
            seed.extend_from_slice(&self.ledger.to_be_bytes());
            seed.extend_from_slice(&index.to_be_bytes());
            hash(&seed)
        };
        let memos: Vec<Value> = payment
            .memos
            .iter()
            .map(|memo| {
                json!({ "Memo": {
                    "MemoType": hex_upper(&memo.memo_type),
                    "MemoData": hex_upper(&memo.memo_data),
                }})
            })
            .collect();
        let mut transaction = json!({
            "TransactionType": "Payment",
            "Account": payment.account.to_address(),
            "Destination": payment.destination.to_address(),
            "Amount": payment.amount.to_string(),
            "Fee": payment.fee.to_string(),
            "Sequence": payment.sequence,
            "LastLedgerSequence": payment.last_ledger_sequence,
            "hash": hex_upper(&tx_hash),
        });
        if !memos.is_empty()
            && let Some(object) = transaction.as_object_mut()
        {
            object.insert("Memos".into(), memos.into());
        }
        let mut meta = json!({
            "TransactionIndex": index,
            "TransactionResult": code,
            "AffectedNodes": nodes,
        });
        if code == "tesSUCCESS"
            && let Some(object) = meta.as_object_mut()
        {
            object.insert("delivered_amount".into(), moved.to_string().into());
        }
        Applied {
            ledger: self.ledger,
            hash: tx_hash,
            accounts: alloc::vec![payment.account, payment.destination],
            message: json!({
                "type": "transaction",
                "validated": true,
                "ledger_index": self.ledger,
                "engine_result": code,
                "transaction": transaction,
                "meta": meta,
            }),
        }
    }
}

fn account_node(kind: &str, account: AccountId, before: u64, after: &Account) -> Value {
    let final_fields = json!({
        "Account": account.to_address(),
        "Balance": after.balance.to_string(),
        "Sequence": after.sequence,
        "OwnerCount": after.owner_count,
    });
    let inner = if kind == "CreatedNode" {
        json!({ "LedgerEntryType": "AccountRoot", "NewFields": final_fields })
    } else {
        json!({
            "LedgerEntryType": "AccountRoot",
            "FinalFields": final_fields,
            "PreviousFields": { "Balance": before.to_string() },
        })
    };
    let mut node = serde_json::Map::new();
    node.insert(String::from(kind), inner);
    Value::Object(node)
}
