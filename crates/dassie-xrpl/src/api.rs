//! The rippled WebSocket API the node speaks, at API version 1: the
//! requests it sends, and what it reads from what comes back. Every parse
//! is bounded and typed; fields the node does not need are ignored, and a
//! message it cannot read is [`Message::Ignored`].

use alloc::string::{String, ToString};
use alloc::vec::Vec;

use serde_json::{Value, json};

use crate::keys::AccountId;
use crate::{MAX_DROPS, from_hex, hex_upper};

/// Most transactions an `account_tx` page asks for.
pub const PAGE: u32 = 200;

fn request(id: u64, command: &str, mut params: Value) -> Vec<u8> {
    if let Some(object) = params.as_object_mut() {
        object.insert("id".into(), id.into());
        object.insert("command".into(), command.into());
        object.insert("api_version".into(), 1.into());
    }
    serde_json::to_vec(&params).unwrap_or_default()
}

/// The server's validated ledger, fees and reserves.
#[must_use]
pub fn server_state(id: u64) -> Vec<u8> {
    request(id, "server_state", json!({}))
}

/// Validated transactions of `account`, and every validated ledger.
#[must_use]
pub fn subscribe(id: u64, account: AccountId) -> Vec<u8> {
    request(
        id,
        "subscribe",
        json!({ "accounts": [account.to_address()], "streams": ["ledger"] }),
    )
}

/// `account` as of the last validated ledger.
#[must_use]
pub fn account_info(id: u64, account: AccountId) -> Vec<u8> {
    request(
        id,
        "account_info",
        json!({ "account": account.to_address(), "ledger_index": "validated" }),
    )
}

/// `account`'s transactions from ledger `min` to `max`, oldest first, a
/// page at a time from `marker`.
#[must_use]
pub fn account_tx(
    id: u64,
    account: AccountId,
    min: u32,
    max: u32,
    marker: Option<&Value>,
) -> Vec<u8> {
    let mut params = json!({
        "account": account.to_address(),
        "ledger_index_min": min,
        "ledger_index_max": max,
        "forward": true,
        "limit": PAGE,
    });
    if let (Some(object), Some(marker)) = (params.as_object_mut(), marker) {
        object.insert("marker".into(), marker.clone());
    }
    request(id, "account_tx", params)
}

/// The transaction `hash`, looked for from ledger `min` to `max`, so the
/// answer says whether all of them were searched.
#[must_use]
pub fn tx(id: u64, hash: &[u8; 32], min: u32, max: u32) -> Vec<u8> {
    request(
        id,
        "tx",
        json!({ "transaction": hex_upper(hash), "min_ledger": min, "max_ledger": max }),
    )
}

/// The transaction `hash`, wherever the server's history has it. Without
/// a range the answer can say that it applied, never that it did not; a
/// server searches a range of at most 1000 ledgers.
#[must_use]
pub fn tx_anywhere(id: u64, hash: &[u8; 32]) -> Vec<u8> {
    request(id, "tx", json!({ "transaction": hex_upper(hash) }))
}

/// Submits a signed transaction.
#[must_use]
pub fn submit(id: u64, blob: &[u8]) -> Vec<u8> {
    request(id, "submit", json!({ "tx_blob": hex_upper(blob) }))
}

/// What came over the connection.
#[derive(Clone, Debug, PartialEq)]
pub enum Message {
    /// The answer to a request.
    Response {
        /// The request.
        id: u64,
        /// Its result, or the error.
        outcome: Result<Value, ApiError>,
    },
    /// A validated transaction that touches a subscribed account.
    Transaction(TxRecord),
    /// A ledger was validated.
    LedgerClosed(LedgerClosed),
    /// Anything else.
    Ignored,
}

/// An error answer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ApiError {
    /// Its code, for example `actNotFound` or `txnNotFound`.
    pub error: String,
    /// For `txnNotFound`: whether every ledger asked for was searched.
    pub searched_all: Option<bool>,
    /// The ledger the answer is as of, if it names one: rippled names it
    /// with `actNotFound`, so an old absence can be told from a new one.
    pub ledger_index: Option<u32>,
    /// Whether that ledger is validated.
    pub validated: bool,
}

/// A validated ledger's numbers, in drops.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LedgerClosed {
    /// Its index.
    pub ledger_index: u32,
    /// When it closed, in seconds since the XRP Ledger's epoch.
    pub close_time: u64,
    /// The reference fee.
    pub fee_base: u64,
    /// What every account holds back.
    pub reserve_base: u64,
    /// What each object it owns holds back more.
    pub reserve_inc: u64,
}

/// What a transaction did to one account's XRP, from its metadata.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BalanceChange {
    /// The account.
    pub account: AccountId,
    /// Its balance before, in drops; 0 if the transaction created it.
    pub before: u64,
    /// Its balance after; 0 if it was deleted.
    pub after: u64,
    /// The objects it owns after, if known.
    pub owner_count: Option<u32>,
}

/// A transaction, as the node reads it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TxRecord {
    /// Its hash.
    pub hash: [u8; 32],
    /// The ledger it is in.
    pub ledger_index: u32,
    /// Its place in that ledger.
    pub index: u32,
    /// Whether the ledger is validated.
    pub validated: bool,
    /// Who sent it.
    pub account: AccountId,
    /// Whether it is a Payment.
    pub is_payment: bool,
    /// Who it pays, for a Payment.
    pub destination: Option<AccountId>,
    /// The sender's sequence it used.
    pub sequence: u32,
    /// The fee, in drops.
    pub fee: u64,
    /// Its result code, for example `tesSUCCESS`.
    pub result: String,
    /// The XRP it delivered, in drops, for a Payment of XRP.
    pub delivered: Option<u64>,
    /// Its first memo's type and data.
    pub memo: Option<(Vec<u8>, Vec<u8>)>,
    /// What it did to accounts' XRP.
    pub changes: Vec<BalanceChange>,
}

impl TxRecord {
    /// Whether it applied as intended.
    #[must_use]
    pub fn succeeded(&self) -> bool {
        self.result == "tesSUCCESS"
    }

    /// What it did to `account`'s XRP, if anything.
    #[must_use]
    pub fn change_of(&self, account: AccountId) -> Option<BalanceChange> {
        self.changes
            .iter()
            .copied()
            .find(|change| change.account == account)
    }
}

/// Reads a message from the connection.
#[must_use]
pub fn parse(bytes: &[u8]) -> Message {
    let Ok(value) = serde_json::from_slice::<Value>(bytes) else {
        return Message::Ignored;
    };
    match value.get("type").and_then(Value::as_str) {
        Some("response") => response(&value),
        Some("transaction") => {
            let record = tx_record(
                value.get("transaction").or_else(|| value.get("tx_json")),
                value.get("meta"),
                value.get("ledger_index"),
                value.get("hash"),
                value.get("validated"),
            );
            record.map_or(Message::Ignored, Message::Transaction)
        }
        Some("ledgerClosed") => {
            ledger_closed(&value).map_or(Message::Ignored, Message::LedgerClosed)
        }
        _ => Message::Ignored,
    }
}

fn response(value: &Value) -> Message {
    let Some(id) = value.get("id").and_then(Value::as_u64) else {
        return Message::Ignored;
    };
    let outcome = if value.get("status").and_then(Value::as_str) == Some("success") {
        Ok(value.get("result").cloned().unwrap_or(Value::Null))
    } else {
        let error = value
            .get("error")
            .and_then(Value::as_str)
            .unwrap_or("unknown")
            .chars()
            .take(64)
            .collect();
        // A WebSocket error carries its result's fields next to it.
        let field = |key: &str| {
            value
                .get(key)
                .or_else(|| value.get("result").and_then(|r| r.get(key)))
        };
        Err(ApiError {
            error,
            searched_all: field("searched_all").and_then(Value::as_bool),
            ledger_index: number(field("ledger_index")),
            validated: field("validated").and_then(Value::as_bool) == Some(true),
        })
    };
    Message::Response { id, outcome }
}

fn ledger_closed(value: &Value) -> Option<LedgerClosed> {
    Some(LedgerClosed {
        ledger_index: number(value.get("ledger_index"))?,
        close_time: value.get("ledger_time")?.as_u64()?,
        fee_base: value.get("fee_base")?.as_u64()?,
        reserve_base: value.get("reserve_base")?.as_u64()?,
        reserve_inc: value.get("reserve_inc")?.as_u64()?,
    })
}

/// A ledger index or sequence: a JSON number within `u32`.
fn number(value: Option<&Value>) -> Option<u32> {
    u32::try_from(value?.as_u64()?).ok()
}

/// Drops, as the ledger writes XRP: a decimal string.
fn drops(value: Option<&Value>) -> Option<u64> {
    let text = value?.as_str()?;
    if text.is_empty() || text.len() > 18 || !text.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    text.parse().ok().filter(|drops| *drops <= MAX_DROPS)
}

fn tx_record(
    tx: Option<&Value>,
    meta: Option<&Value>,
    ledger_index: Option<&Value>,
    hash: Option<&Value>,
    validated: Option<&Value>,
) -> Option<TxRecord> {
    let tx = tx?;
    let meta = meta?;
    let hash = hash
        .or_else(|| tx.get("hash"))
        .and_then(Value::as_str)
        .and_then(from_hex)?
        .try_into()
        .ok()?;
    let ledger_index = number(ledger_index.or_else(|| tx.get("ledger_index")))?;
    let is_payment = tx.get("TransactionType").and_then(Value::as_str) == Some("Payment");
    let account_of = |key: &str| {
        tx.get(key)
            .and_then(Value::as_str)
            .and_then(AccountId::parse)
    };
    let memo = tx
        .get("Memos")
        .and_then(|memos| memos.get(0))
        .and_then(|memo| memo.get("Memo"))
        .map(|memo| {
            let field = |key: &str| {
                memo.get(key)
                    .and_then(Value::as_str)
                    .and_then(from_hex)
                    .unwrap_or_default()
            };
            (field("MemoType"), field("MemoData"))
        });
    Some(TxRecord {
        hash,
        ledger_index,
        index: number(meta.get("TransactionIndex"))?,
        validated: validated.and_then(Value::as_bool).unwrap_or(false),
        account: account_of("Account")?,
        is_payment,
        destination: account_of("Destination"),
        sequence: number(tx.get("Sequence"))?,
        fee: drops(tx.get("Fee"))?,
        result: meta
            .get("TransactionResult")
            .and_then(Value::as_str)?
            .chars()
            .take(32)
            .collect(),
        delivered: drops(meta.get("delivered_amount")),
        memo,
        changes: balance_changes(meta),
    })
}

/// The `AccountRoot` balances a transaction changed, from its metadata.
fn balance_changes(meta: &Value) -> Vec<BalanceChange> {
    let Some(nodes) = meta.get("AffectedNodes").and_then(Value::as_array) else {
        return Vec::new();
    };
    nodes
        .iter()
        .filter_map(|node| {
            let (kind, inner) = node.as_object()?.iter().next()?;
            if inner.get("LedgerEntryType").and_then(Value::as_str) != Some("AccountRoot") {
                return None;
            }
            let fields = match kind.as_str() {
                "CreatedNode" => inner.get("NewFields")?,
                "ModifiedNode" | "DeletedNode" => inner.get("FinalFields")?,
                _ => return None,
            };
            let account = AccountId::parse(fields.get("Account")?.as_str()?)?;
            let final_balance = drops(fields.get("Balance"));
            let (before, after) = match kind.as_str() {
                "CreatedNode" => (0, final_balance?),
                "DeletedNode" => (final_balance?, 0),
                _ => {
                    let after = final_balance?;
                    let before = drops(inner.get("PreviousFields").and_then(|p| p.get("Balance")))
                        .unwrap_or(after);
                    (before, after)
                }
            };
            Some(BalanceChange {
                account,
                before,
                after,
                owner_count: number(fields.get("OwnerCount")),
            })
        })
        .collect()
}

/// The server's state: what the node needs to sign and to know its
/// reserve.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ServerState {
    /// The last validated ledger.
    pub validated_ledger: u32,
    /// When it closed, in seconds since the XRP Ledger's epoch.
    pub close_time: u64,
    /// The reference fee, in drops.
    pub base_fee: u64,
    /// The base reserve, in drops.
    pub reserve_base: u64,
    /// The reserve per owned object, in drops.
    pub reserve_inc: u64,
    /// The load factor, against [`load_base`](Self::load_base).
    pub load_factor: u64,
    /// The unloaded factor.
    pub load_base: u64,
    /// The network ID, which the scheme checks is the testnet's.
    pub network_id: Option<u32>,
}

/// Reads `server_state`'s result.
#[must_use]
pub fn server_state_result(result: &Value) -> Option<ServerState> {
    let state = result.get("state")?;
    let ledger = state.get("validated_ledger")?;
    Some(ServerState {
        validated_ledger: number(ledger.get("seq"))?,
        close_time: ledger.get("close_time")?.as_u64()?,
        base_fee: ledger.get("base_fee")?.as_u64()?,
        reserve_base: ledger.get("reserve_base")?.as_u64()?,
        reserve_inc: ledger.get("reserve_inc")?.as_u64()?,
        load_factor: state
            .get("load_factor")
            .and_then(Value::as_u64)
            .unwrap_or(1),
        load_base: state
            .get("load_base")
            .and_then(Value::as_u64)
            .unwrap_or(1)
            .max(1),
        network_id: number(state.get("network_id")),
    })
}

/// An account as of a validated ledger.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AccountInfo {
    /// Its balance, in drops.
    pub balance: u64,
    /// The sequence its next transaction uses.
    pub sequence: u32,
    /// The objects it owns.
    pub owner_count: u32,
    /// The ledger this is as of.
    pub ledger_index: u32,
}

/// Reads `account_info`'s result.
#[must_use]
pub fn account_info_result(result: &Value) -> Option<AccountInfo> {
    let data = result.get("account_data")?;
    Some(AccountInfo {
        balance: drops(data.get("Balance"))?,
        sequence: number(data.get("Sequence"))?,
        owner_count: number(data.get("OwnerCount")).unwrap_or(0),
        ledger_index: number(result.get("ledger_index"))?,
    })
}

/// A page of `account_tx`.
#[derive(Clone, Debug, PartialEq)]
pub struct AccountTx {
    /// Its validated transactions, oldest first.
    pub transactions: Vec<TxRecord>,
    /// Where the next page starts, if there is one.
    pub marker: Option<Value>,
    /// The last ledger it covers.
    pub ledger_index_max: u32,
}

/// Reads `account_tx`'s result. `None` if any transaction is unreadable,
/// since skipping one would book it wrong.
#[must_use]
pub fn account_tx_result(result: &Value) -> Option<AccountTx> {
    let list = result.get("transactions")?.as_array()?;
    if list.len() > usize::try_from(PAGE).unwrap_or(usize::MAX) {
        return None;
    }
    let transactions = list
        .iter()
        .map(|entry| {
            tx_record(
                entry.get("tx").or_else(|| entry.get("tx_json")),
                entry.get("meta"),
                entry.get("ledger_index"),
                entry.get("hash"),
                entry.get("validated"),
            )
        })
        .collect::<Option<Vec<_>>>()?;
    Some(AccountTx {
        transactions,
        marker: result
            .get("marker")
            .filter(|marker| !marker.is_null())
            .cloned(),
        ledger_index_max: number(result.get("ledger_index_max"))?,
    })
}

/// Reads `tx`'s result.
#[must_use]
pub fn tx_result(result: &Value) -> Option<TxRecord> {
    tx_record(
        Some(result),
        result.get("meta"),
        result.get("ledger_index"),
        result.get("hash"),
        result.get("validated"),
    )
}

/// What `submit` said at once.
#[must_use]
pub fn submit_result(result: &Value) -> Option<String> {
    Some(result.get("engine_result")?.as_str()?.to_string())
}
