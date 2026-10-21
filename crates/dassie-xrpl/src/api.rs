//! The rippled WebSocket API the node speaks, at API version 1: the
//! requests it sends, and what it reads from what comes back. Every parse
//! is bounded and typed; fields the node does not need are ignored, and a
//! message it cannot read is [`Message::Ignored`].
//!
//! Tokens (spec/areas/xrpl.md#tokens): the token a Payment delivered, what
//! a transaction did to trust lines, a line as the ledger holds it, with
//! its flags and limits read from either side, and an issuer's account.

use alloc::string::{String, ToString};
use alloc::vec::Vec;

use serde_json::{Map, Value, json};

use crate::amount::{Currency, TokenAmount, TokenValue};
use crate::keys::AccountId;
use crate::{MAX_DROPS, from_hex, hex_upper};

/// A trust line's flags (`RippleState`), each set by one side: the low
/// account is the one whose ID is the lower number.
pub mod line_flags {
    /// The low account's side holds an owner reserve.
    pub const LOW_RESERVE: u32 = 0x0001_0000;
    /// The high account's side holds an owner reserve.
    pub const HIGH_RESERVE: u32 = 0x0002_0000;
    /// The low account authorised the high one to hold its token.
    pub const LOW_AUTH: u32 = 0x0004_0000;
    /// The high account authorised the low one to hold its token.
    pub const HIGH_AUTH: u32 = 0x0008_0000;
    /// The low account disabled rippling through its side.
    pub const LOW_NO_RIPPLE: u32 = 0x0010_0000;
    /// The high account disabled rippling through its side.
    pub const HIGH_NO_RIPPLE: u32 = 0x0020_0000;
    /// The low account froze the line: the high account can pay its
    /// balance only to the low one.
    pub const LOW_FREEZE: u32 = 0x0040_0000;
    /// The high account froze the line: the low account can pay its
    /// balance only to the high one.
    pub const HIGH_FREEZE: u32 = 0x0080_0000;
    /// The low account deep-froze the line: the high account can neither
    /// pay nor receive, but to and from the low one.
    pub const LOW_DEEP_FREEZE: u32 = 0x0200_0000;
    /// The high account deep-froze the line: the low account can neither
    /// pay nor receive, but to and from the high one.
    pub const HIGH_DEEP_FREEZE: u32 = 0x0400_0000;
}

/// An account's flags (`AccountRoot`) that bear on its tokens.
pub mod account_flags {
    /// Holders of its tokens must be authorised.
    pub const REQUIRE_AUTH: u32 = 0x0004_0000;
    /// It gave up freezing lines for good.
    pub const NO_FREEZE: u32 = 0x0020_0000;
    /// It froze every line of its tokens: holders pay only it.
    pub const GLOBAL_FREEZE: u32 = 0x0040_0000;
    /// Its lines ripple by default.
    pub const DEFAULT_RIPPLE: u32 = 0x0080_0000;
    /// It may claw back its tokens.
    pub const ALLOW_TRUST_LINE_CLAWBACK: u32 = 0x8000_0000;
}

/// A transfer rate that charges nothing: 1.0, in billionths.
pub const TRANSFER_RATE_PAR: u32 = 1_000_000_000;

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

/// The trust line between `account` and `issuer` in `currency`, as of
/// the last validated ledger; `entryNotFound` if there is none.
#[must_use]
pub fn ripple_state(id: u64, account: AccountId, issuer: AccountId, currency: Currency) -> Vec<u8> {
    request(
        id,
        "ledger_entry",
        json!({
            "ripple_state": {
                "accounts": [account.to_address(), issuer.to_address()],
                "currency": currency.to_string(),
            },
            "ledger_index": "validated",
        }),
    )
}

/// The trust line between `account` and `issuer` in `currency`, as of
/// validated ledger `ledger`: the one `account` was read at.
#[must_use]
pub fn ripple_state_at(
    id: u64,
    account: AccountId,
    issuer: AccountId,
    currency: Currency,
    ledger: u32,
) -> Vec<u8> {
    request(
        id,
        "ledger_entry",
        json!({
            "ripple_state": {
                "accounts": [account.to_address(), issuer.to_address()],
                "currency": currency.to_string(),
            },
            "ledger_index": ledger,
        }),
    )
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
    Transaction(alloc::boxed::Box<TxRecord>),
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
    /// The token it delivered, for a Payment of a token: never with
    /// [`delivered`](Self::delivered).
    pub delivered_token: Option<TokenAmount>,
    /// Its first memo's type and data.
    pub memo: Option<(Vec<u8>, Vec<u8>)>,
    /// What it did to accounts' XRP.
    pub changes: Vec<BalanceChange>,
    /// What it did to trust lines.
    pub lines: Vec<LineChange>,
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

    /// What it did to the line between `account` and `issuer` in
    /// `currency`, if anything.
    #[must_use]
    pub fn line(
        &self,
        account: AccountId,
        issuer: AccountId,
        currency: Currency,
    ) -> Option<&LineChange> {
        self.lines
            .iter()
            .find(|change| change.line().is_between(account, issuer, currency))
    }
}

/// One side's flags on a trust line: what that side set.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct LineFlags {
    /// Its side holds an owner reserve.
    pub reserve: bool,
    /// It authorised the other side to hold its token.
    pub auth: bool,
    /// It disabled rippling through its side.
    pub no_ripple: bool,
    /// It froze the line: the other side pays only to it.
    pub freeze: bool,
    /// It deep-froze the line: the other side neither pays nor receives,
    /// but to and from it.
    pub deep_freeze: bool,
}

/// A trust line (`RippleState`) as the ledger holds it, between a low and
/// a high account: the one whose ID is the lower number is low.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TrustLine {
    /// The low account.
    pub low: AccountId,
    /// The high account, above the low one.
    pub high: AccountId,
    /// Its currency.
    pub currency: Currency,
    /// The balance from the low account's side: above zero, the low
    /// account holds the high one's token; below, the other way round.
    pub balance: TokenValue,
    /// The most the low account holds of the high one's token.
    pub low_limit: TokenValue,
    /// The most the high account holds of the low one's token.
    pub high_limit: TokenValue,
    /// Its flags ([`line_flags`]).
    pub flags: u32,
}

/// A trust line from one account's side.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LineView {
    /// The other side, such as the issuer.
    pub counterparty: AccountId,
    /// What the account holds of the counterparty's token; below zero,
    /// what it owes.
    pub balance: TokenValue,
    /// The most the account holds of the counterparty's token.
    pub limit: TokenValue,
    /// The most the counterparty holds of the account's.
    pub counterparty_limit: TokenValue,
    /// What the account set.
    pub ours: LineFlags,
    /// What the counterparty set, such as an issuer's freeze.
    pub theirs: LineFlags,
}

impl TrustLine {
    /// Whether it is the line between `a` and `b`, either way round, in
    /// `currency`.
    #[must_use]
    pub fn is_between(&self, a: AccountId, b: AccountId, currency: Currency) -> bool {
        self.currency == currency
            && ((self.low, self.high) == (a, b) || (self.low, self.high) == (b, a))
    }

    /// The low side's flags.
    #[must_use]
    pub const fn low_flags(&self) -> LineFlags {
        use line_flags::{LOW_AUTH, LOW_DEEP_FREEZE, LOW_FREEZE, LOW_NO_RIPPLE, LOW_RESERVE};
        LineFlags {
            reserve: self.flags & LOW_RESERVE != 0,
            auth: self.flags & LOW_AUTH != 0,
            no_ripple: self.flags & LOW_NO_RIPPLE != 0,
            freeze: self.flags & LOW_FREEZE != 0,
            deep_freeze: self.flags & LOW_DEEP_FREEZE != 0,
        }
    }

    /// The high side's flags.
    #[must_use]
    pub const fn high_flags(&self) -> LineFlags {
        use line_flags::{HIGH_AUTH, HIGH_DEEP_FREEZE, HIGH_FREEZE, HIGH_NO_RIPPLE, HIGH_RESERVE};
        LineFlags {
            reserve: self.flags & HIGH_RESERVE != 0,
            auth: self.flags & HIGH_AUTH != 0,
            no_ripple: self.flags & HIGH_NO_RIPPLE != 0,
            freeze: self.flags & HIGH_FREEZE != 0,
            deep_freeze: self.flags & HIGH_DEEP_FREEZE != 0,
        }
    }

    /// The line from `account`'s side, if it is one of the two.
    #[must_use]
    pub fn view(&self, account: AccountId) -> Option<LineView> {
        if account == self.low {
            Some(LineView {
                counterparty: self.high,
                balance: self.balance,
                limit: self.low_limit,
                counterparty_limit: self.high_limit,
                ours: self.low_flags(),
                theirs: self.high_flags(),
            })
        } else if account == self.high {
            Some(LineView {
                counterparty: self.low,
                balance: self.balance.negate(),
                limit: self.high_limit,
                counterparty_limit: self.low_limit,
                ours: self.high_flags(),
                theirs: self.low_flags(),
            })
        } else {
            None
        }
    }
}

/// What a transaction did to one trust line: the line before and after,
/// `None` before it was created and after it was deleted. At least one is
/// there, and both are of the same accounts and currency.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LineChange {
    /// After if it is still there, else before.
    line: TrustLine,
    before: Option<TrustLine>,
    after: Option<TrustLine>,
}

impl LineChange {
    /// The change from `before` to `after`, if they are of one line.
    #[must_use]
    pub fn new(before: Option<TrustLine>, after: Option<TrustLine>) -> Option<Self> {
        let line = after.or(before)?;
        if before.is_some_and(|before| !before.is_between(line.low, line.high, line.currency)) {
            return None;
        }
        Some(Self {
            line,
            before,
            after,
        })
    }

    /// The line before, unless the transaction created it.
    #[must_use]
    pub const fn before(&self) -> Option<&TrustLine> {
        self.before.as_ref()
    }

    /// The line after, unless the transaction deleted it.
    #[must_use]
    pub const fn after(&self) -> Option<&TrustLine> {
        self.after.as_ref()
    }

    /// The line, after if it is still there, else before.
    #[must_use]
    pub const fn line(&self) -> &TrustLine {
        &self.line
    }

    /// `account`'s balance on the line before and after, zero where it
    /// was not; `None` unless `account` is one of its two.
    #[must_use]
    pub fn balance_of(&self, account: AccountId) -> Option<(TokenValue, TokenValue)> {
        let balance = |line: Option<&TrustLine>| match line {
            None => Some(TokenValue::ZERO),
            Some(line) => line.view(account).map(|view| view.balance),
        };
        self.line().view(account)?;
        Some((balance(self.before())?, balance(self.after())?))
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
            record.map_or(Message::Ignored, |record| {
                Message::Transaction(alloc::boxed::Box::new(record))
            })
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
        delivered_token: token_amount(meta.get("delivered_amount")),
        memo,
        changes: balance_changes(meta),
        lines: line_changes(meta),
    })
}

/// A token amount, as the ledger writes one: `currency`, `issuer` and
/// `value`, nothing else.
#[must_use]
pub fn token_amount(value: Option<&Value>) -> Option<TokenAmount> {
    let object = value?.as_object()?;
    if object.len() != 3 {
        return None;
    }
    let amount = TokenAmount {
        value: TokenValue::parse(object.get("value")?.as_str()?)?,
        currency: Currency::parse(object.get("currency")?.as_str()?)?,
        issuer: AccountId::parse(object.get("issuer")?.as_str()?)?,
    };
    amount.is_valid().then_some(amount)
}

/// A `RippleState`'s fields as a line: its two accounts from the limits'
/// issuers, low below high, and one currency throughout. A line just
/// created may leave out a zero balance and no flags.
fn trust_line(fields: &Value, created: bool) -> Option<TrustLine> {
    let low_limit = token_amount(fields.get("LowLimit"))?;
    let high_limit = token_amount(fields.get("HighLimit"))?;
    // The balance's issuer is a placeholder (ACCOUNT_ONE): only its value
    // and currency say anything.
    let balance = match fields.get("Balance") {
        None if created => TokenValue::ZERO,
        balance => {
            let balance = balance?;
            if Currency::parse(balance.get("currency")?.as_str()?)? != low_limit.currency {
                return None;
            }
            TokenValue::parse(balance.get("value")?.as_str()?)?
        }
    };
    let flags = match fields.get("Flags") {
        None if created => 0,
        flags => number(flags)?,
    };
    if low_limit.issuer >= high_limit.issuer || low_limit.currency != high_limit.currency {
        return None;
    }
    Some(TrustLine {
        low: low_limit.issuer,
        high: high_limit.issuer,
        currency: low_limit.currency,
        balance,
        low_limit: low_limit.value,
        high_limit: high_limit.value,
        flags,
    })
}

/// The trust lines a transaction changed, from its metadata. A line whose
/// fields cannot be read is left out, as an account's are.
fn line_changes(meta: &Value) -> Vec<LineChange> {
    let Some(nodes) = meta.get("AffectedNodes").and_then(Value::as_array) else {
        return Vec::new();
    };
    nodes
        .iter()
        .filter_map(|node| {
            let (kind, inner) = node.as_object()?.iter().next()?;
            if inner.get("LedgerEntryType").and_then(Value::as_str) != Some("RippleState") {
                return None;
            }
            // Before: the final fields, with what the transaction changed
            // as it was.
            let before = || {
                let mut fields: Map<String, Value> = inner.get("FinalFields")?.as_object()?.clone();
                if let Some(previous) = inner.get("PreviousFields").and_then(Value::as_object) {
                    for (key, value) in previous {
                        fields.insert(key.clone(), value.clone());
                    }
                }
                trust_line(&Value::Object(fields), false)
            };
            let (before, after) = match kind.as_str() {
                "CreatedNode" => (None, Some(trust_line(inner.get("NewFields")?, true)?)),
                "ModifiedNode" => (
                    Some(before()?),
                    Some(trust_line(inner.get("FinalFields")?, false)?),
                ),
                "DeletedNode" => (Some(before()?), None),
                _ => return None,
            };
            LineChange::new(before, after)
        })
        .collect()
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

/// A trust line as of a validated ledger, from `ledger_entry`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LineEntry {
    /// The line.
    pub line: TrustLine,
    /// The ledger this is as of.
    pub ledger_index: u32,
    /// Whether that ledger is validated.
    pub validated: bool,
}

/// Reads the result of [`ripple_state`]'s request.
#[must_use]
pub fn ripple_state_result(result: &Value) -> Option<LineEntry> {
    let node = result.get("node")?;
    if node.get("LedgerEntryType").and_then(Value::as_str) != Some("RippleState") {
        return None;
    }
    Some(LineEntry {
        line: trust_line(node, false)?,
        ledger_index: number(result.get("ledger_index"))?,
        validated: result.get("validated").and_then(Value::as_bool) == Some(true),
    })
}

/// What an account's root says about its tokens: an issuer's freeze and
/// transfer fee.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AccountSettings {
    /// Its flags ([`account_flags`]).
    pub flags: u32,
    /// Its transfer rate in billionths: 0 when it has none, which charges
    /// nothing, as [`TRANSFER_RATE_PAR`] does.
    pub transfer_rate: u32,
    /// The ledger this is as of.
    pub ledger_index: u32,
    /// Whether that ledger is validated.
    pub validated: bool,
}

impl AccountSettings {
    /// Whether it froze every line of its tokens.
    #[must_use]
    pub const fn global_freeze(&self) -> bool {
        self.flags & account_flags::GLOBAL_FREEZE != 0
    }

    /// Whether it charges a fee on transfers of its tokens between others.
    #[must_use]
    pub const fn charges_transfer_fee(&self) -> bool {
        self.transfer_rate != 0 && self.transfer_rate != TRANSFER_RATE_PAR
    }
}

/// Reads `account_info`'s result for an account's settings.
#[must_use]
pub fn account_settings_result(result: &Value) -> Option<AccountSettings> {
    let data = result.get("account_data")?;
    Some(AccountSettings {
        flags: number(data.get("Flags"))?,
        transfer_rate: match data.get("TransferRate") {
            None => 0,
            rate => number(rate)?,
        },
        ledger_index: number(result.get("ledger_index"))?,
        validated: result.get("validated").and_then(Value::as_bool) == Some(true),
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

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::indexing_slicing)]
mod tests {
    use super::*;

    const RLUSD: &str = "524C555344000000000000000000000000000000";
    const ISSUER: &str = "rQhWct2fv4Vc4KRjRgMrxa8xPN9Zx9iLKV";

    #[test]
    fn a_line_is_asked_for_by_its_accounts_and_currency() {
        let holder = AccountId([1; 20]);
        let issuer = AccountId::parse(ISSUER).unwrap();
        let request: Value = serde_json::from_slice(&ripple_state(
            7,
            holder,
            issuer,
            Currency::parse(RLUSD).unwrap(),
        ))
        .unwrap();
        assert_eq!(request["command"], "ledger_entry");
        assert_eq!(request["ledger_index"], "validated");
        assert_eq!(request["ripple_state"]["currency"], RLUSD);
        assert_eq!(
            request["ripple_state"]["accounts"],
            json!([holder.to_address(), ISSUER])
        );
    }

    #[test]
    fn an_issuers_freeze_and_transfer_fee_are_read() {
        let settings = |flags: u32, rate: Option<u32>| {
            let mut data = json!({ "Account": ISSUER, "Flags": flags, "Balance": "100" });
            if let Some(rate) = rate {
                data["TransferRate"] = rate.into();
            }
            account_settings_result(
                &json!({ "account_data": data, "ledger_index": 12, "validated": true }),
            )
            .unwrap()
        };
        // RLUSD's issuer today: DefaultRipple and clawback, no fee.
        let rlusd = settings(
            account_flags::DEFAULT_RIPPLE | account_flags::ALLOW_TRUST_LINE_CLAWBACK,
            None,
        );
        assert!(!rlusd.global_freeze() && !rlusd.charges_transfer_fee());
        assert_eq!((rlusd.ledger_index, rlusd.validated), (12, true));
        assert!(settings(account_flags::GLOBAL_FREEZE, None).global_freeze());
        assert!(!settings(0, Some(TRANSFER_RATE_PAR)).charges_transfer_fee());
        assert!(!settings(0, Some(0)).charges_transfer_fee());
        assert!(settings(0, Some(1_002_000_000)).charges_transfer_fee());
        // Flags are what the freeze is read from: no flags, no answer.
        assert!(
            account_settings_result(&json!({ "account_data": {}, "ledger_index": 1 })).is_none()
        );
    }

    #[test]
    fn a_clawback_lowers_the_holders_line() {
        let issuer = AccountId::parse(ISSUER).unwrap();
        let holder = AccountId([0xFF; 20]);
        assert!(issuer < holder, "the issuer is low");
        let line = |balance: &str| {
            json!({
                "Balance": { "currency": RLUSD, "issuer": "rrrrrrrrrrrrrrrrrrrrBZbvji", "value": balance },
                "Flags": line_flags::HIGH_RESERVE | line_flags::HIGH_NO_RIPPLE | line_flags::LOW_FREEZE,
                "LowLimit": { "currency": RLUSD, "issuer": ISSUER, "value": "0" },
                "HighLimit": { "currency": RLUSD, "issuer": holder.to_address(), "value": "9999999999999999e80" },
            })
        };
        let tx = json!({
            "TransactionType": "Clawback",
            "Account": ISSUER,
            "Amount": { "currency": RLUSD, "issuer": holder.to_address(), "value": "2.5" },
            "Fee": "12",
            "Sequence": 3,
            "hash": "CD".repeat(32),
            "ledger_index": 40,
            "validated": true,
            "meta": {
                "TransactionIndex": 2,
                "TransactionResult": "tesSUCCESS",
                "AffectedNodes": [{ "ModifiedNode": {
                    "LedgerEntryType": "RippleState",
                    "FinalFields": line("-7.5"),
                    "PreviousFields": { "Balance": { "currency": RLUSD, "issuer": "rrrrrrrrrrrrrrrrrrrrBZbvji", "value": "-10" } },
                }}],
            },
        });
        let record = tx_result(&tx).unwrap();
        let currency = Currency::parse(RLUSD).unwrap();
        let change = record.line(holder, issuer, currency).unwrap();
        let value = |text| TokenValue::parse(text).unwrap();
        assert_eq!(change.balance_of(holder), Some((value("10"), value("7.5"))));
        assert_eq!(
            change.balance_of(issuer),
            Some((value("-10"), value("-7.5")))
        );
        let view = change.after().unwrap().view(holder).unwrap();
        assert_eq!(view.limit, TokenValue::MAX);
        assert!(view.ours.no_ripple && view.ours.reserve && !view.ours.freeze);
        assert!(view.theirs.freeze && !view.theirs.deep_freeze);
        assert_eq!(record.delivered_token, None);
    }
}
