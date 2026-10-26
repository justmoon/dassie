//! The rippled WebSocket API the node speaks, at API version 1: the
//! requests it sends, and what it reads from what comes back. Every parse
//! is bounded and typed; fields the node does not need are ignored, and a
//! message it cannot read is [`Message::Ignored`].
//!
//! Tokens (spec/areas/xrpl.md#tokens): the token a Payment delivered, what
//! a transaction did to trust lines, a line as the ledger holds it, with
//! its flags and limits read from either side, and an issuer's account.
//!
//! AMM pools (spec/areas/xrpl.md#amm-pools): what a pool of two assets
//! holds and charges, from `amm_info`.
//!
//! Who signs for an account (spec/areas/xrpl.md#the-transactions): whether
//! its master key is disabled and destination tags are required, its
//! regular key, and its signer list with what else it owns, from
//! `account_objects`; and the destination tag a transaction names.
//!
//! Proofs (spec/designs/xrpl-validation.md#3-the-proof-api): the methods a
//! proof server adds, `state_proof`, `tx_proof` and `validations`, and rippled's
//! `manifest`, their answers read as bytes for the verifier to check:
//! nothing in them is believed here.

use alloc::string::{String, ToString};
use alloc::vec::Vec;

use serde_json::{Map, Value, json};

use crate::amm::MAX_TRADING_FEE;
use crate::amount::{Amount, Asset, Currency, TokenAmount, TokenValue};
use crate::codec::SignerEntry;
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

/// An account's flags (`AccountRoot`) that bear on its tokens, and on who
/// may pay it and sign for it.
pub mod account_flags {
    /// Payments to it must name a destination tag.
    pub const REQUIRE_DEST_TAG: u32 = 0x0002_0000;
    /// Its master key no longer signs: only its regular key or its signer
    /// list.
    pub const DISABLE_MASTER: u32 = 0x0010_0000;
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

/// Most objects an `account_objects` page asks for.
pub const OBJECTS_PAGE: u32 = 200;

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

/// The objects `account` owns as of the last validated ledger, such as its
/// signer list and its trust lines, a page of at most [`OBJECTS_PAGE`] at
/// a time from `marker`.
#[must_use]
pub fn account_objects(id: u64, account: AccountId, marker: Option<&Value>) -> Vec<u8> {
    let mut params = json!({
        "account": account.to_address(),
        "ledger_index": "validated",
        "limit": OBJECTS_PAGE,
    });
    if let (Some(object), Some(marker)) = (params.as_object_mut(), marker) {
        object.insert("marker".into(), marker.clone());
    }
    request(id, "account_objects", params)
}

/// Submits a signed transaction.
#[must_use]
pub fn submit(id: u64, blob: &[u8]) -> Vec<u8> {
    request(id, "submit", json!({ "tx_blob": hex_upper(blob) }))
}

/// The AMM pool of `asset` and `asset2` as of the last validated ledger;
/// `actNotFound` if there is none. The answer names the two in this order.
#[must_use]
pub fn amm_info(id: u64, asset: Asset, asset2: Asset) -> Vec<u8> {
    request(
        id,
        "amm_info",
        json!({
            "asset": asset_json(asset),
            "asset2": asset_json(asset2),
            "ledger_index": "validated",
        }),
    )
}

/// An asset as a request names it: `{"currency": "XRP"}`, or a currency
/// and its issuer.
fn asset_json(asset: Asset) -> Value {
    match asset {
        Asset::Xrp => json!({ "currency": "XRP" }),
        Asset::Token { currency, issuer } => {
            json!({ "currency": currency.to_string(), "issuer": issuer.to_address() })
        }
    }
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
    /// The destination tag it names, if it names one.
    pub destination_tag: Option<u32>,
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
        destination_tag: match tx.get("DestinationTag") {
            None => None,
            tag => Some(number(tag)?),
        },
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
            // A deleted root's final balance is the zero the ledger left it
            // with: what it held before is in its previous fields.
            let (before, after) = match kind.as_str() {
                "CreatedNode" => (0, final_balance?),
                _ => {
                    let balance = final_balance?;
                    let before = drops(inner.get("PreviousFields").and_then(|p| p.get("Balance")))
                        .unwrap_or(balance);
                    let after = if kind == "DeletedNode" { 0 } else { balance };
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

/// What an account's root says about its tokens, an issuer's freeze and
/// transfer fee, and about who signs for it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AccountSettings {
    /// Its flags ([`account_flags`]).
    pub flags: u32,
    /// Its transfer rate in billionths: 0 when it has none, which charges
    /// nothing, as [`TRANSFER_RATE_PAR`] does.
    pub transfer_rate: u32,
    /// Its regular key, as the account of the key, if it has one.
    pub regular_key: Option<AccountId>,
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

    /// Whether its master key no longer signs.
    #[must_use]
    pub const fn master_disabled(&self) -> bool {
        self.flags & account_flags::DISABLE_MASTER != 0
    }

    /// Whether Payments to it must name a destination tag.
    #[must_use]
    pub const fn requires_destination_tag(&self) -> bool {
        self.flags & account_flags::REQUIRE_DEST_TAG != 0
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
        regular_key: match data.get("RegularKey") {
            None => None,
            key => Some(AccountId::parse(key?.as_str()?)?),
        },
        ledger_index: number(result.get("ledger_index"))?,
        validated: result.get("validated").and_then(Value::as_bool) == Some(true),
    })
}

/// An account's signer list (`SignerList`): the accounts that together sign
/// for it, each with its weight, and the weight they must reach.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SignerList {
    /// The weight the signers of one transaction must reach together.
    pub quorum: u32,
    /// Its entries, sorted by account, each once, at most
    /// [`MAX_SIGNERS`](crate::codec::MAX_SIGNERS).
    pub entries: Vec<SignerEntry>,
}

/// What an account owns, as of a validated ledger, from `account_objects`:
/// its signer list, how many other objects, and the trust lines among them.
#[derive(Clone, Debug, PartialEq)]
pub struct AccountObjects {
    /// Its signer list, if it has one.
    pub signer_list: Option<SignerList>,
    /// How many objects of other kinds it owns on this page, trust lines
    /// included.
    pub others: usize,
    /// The trust lines among them whose fields read as the ledger writes a
    /// line: one that does not is counted in `others` alone.
    pub lines: Vec<TrustLine>,
    /// Where the next page starts, if there is one.
    pub marker: Option<Value>,
    /// The ledger this is as of.
    pub ledger_index: u32,
    /// Whether that ledger is validated.
    pub validated: bool,
}

/// Reads `account_objects`' result: a signer list at most once, with
/// entries the ledger would hold. `None` if any of it is not so, or the
/// page is longer than asked for.
#[must_use]
pub fn account_objects_result(result: &Value) -> Option<AccountObjects> {
    let objects = result.get("account_objects")?.as_array()?;
    if objects.len() > usize::try_from(OBJECTS_PAGE).unwrap_or(usize::MAX) {
        return None;
    }
    let mut signer_list = None;
    let mut others = 0_usize;
    let mut lines = Vec::new();
    for object in objects {
        match object.get("LedgerEntryType").and_then(Value::as_str) {
            Some("SignerList") => {
                if signer_list.is_some() {
                    return None;
                }
                signer_list = Some(signer_list_fields(object)?);
            }
            kind => {
                others = others.saturating_add(1);
                if kind == Some("RippleState")
                    && let Some(line) = trust_line(object, false)
                {
                    lines.push(line);
                }
            }
        }
    }
    Some(AccountObjects {
        signer_list,
        others,
        lines,
        marker: result
            .get("marker")
            .filter(|marker| !marker.is_null())
            .cloned(),
        ledger_index: number(result.get("ledger_index"))?,
        validated: result.get("validated").and_then(Value::as_bool) == Some(true),
    })
}

/// A `SignerList`'s quorum and entries: at least one and at most the
/// ledger's limit, sorted by account and each once, as the ledger keeps
/// them.
fn signer_list_fields(object: &Value) -> Option<SignerList> {
    let quorum = number(object.get("SignerQuorum"))?;
    let list = object.get("SignerEntries")?.as_array()?;
    if list.is_empty() || list.len() > crate::codec::MAX_SIGNERS {
        return None;
    }
    let entries = list
        .iter()
        .map(|entry| {
            let entry = entry.get("SignerEntry")?;
            Some(SignerEntry {
                account: AccountId::parse(entry.get("Account")?.as_str()?)?,
                weight: u16::try_from(entry.get("SignerWeight")?.as_u64()?).ok()?,
            })
        })
        .collect::<Option<Vec<_>>>()?;
    let sorted = entries
        .iter()
        .zip(entries.iter().skip(1))
        .all(|(a, b)| a.account < b.account);
    sorted.then_some(SignerList { quorum, entries })
}

/// An AMM pool of two assets, as of a validated ledger, from `amm_info`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AmmInfo {
    /// The pool's account, which holds both assets and issues its LP
    /// token.
    pub account: AccountId,
    /// What it holds of the first asset asked for.
    pub amount: Amount,
    /// What it holds of the second.
    pub amount2: Amount,
    /// Its LP tokens outstanding.
    pub lp_token: TokenAmount,
    /// Its trading fee, in
    /// [`TRADING_FEE_DENOMINATOR`](crate::amm::TRADING_FEE_DENOMINATOR)ths,
    /// at most [`MAX_TRADING_FEE`].
    pub trading_fee: u32,
    /// Whether the first asset is frozen in the pool: never for XRP.
    pub asset_frozen: bool,
    /// Whether the second is.
    pub asset2_frozen: bool,
    /// The ledger this is as of.
    pub ledger_index: u32,
    /// Whether that ledger is validated.
    pub validated: bool,
}

impl AmmInfo {
    /// What the pool holds of the asset a swap pays in, `paid`, and of
    /// the other, which it takes out: the pool's two sides for
    /// [`crate::amm::swap_in`] and [`crate::amm::swap_out`]. `None` if
    /// `paid` is neither of its assets.
    #[must_use]
    pub fn sides(&self, paid: Asset) -> Option<(Amount, Amount)> {
        if self.amount.asset() == paid {
            Some((self.amount, self.amount2))
        } else if self.amount2.asset() == paid {
            Some((self.amount2, self.amount))
        } else {
            None
        }
    }
}

/// Reads `amm_info`'s result: two amounts of two assets, not below zero,
/// an LP token of the pool's account, and a fee the ledger takes. `None`
/// if any of it is not so, or the answer names no ledger.
#[must_use]
pub fn amm_info_result(result: &Value) -> Option<AmmInfo> {
    let amm = result.get("amm")?;
    let account = AccountId::parse(amm.get("account")?.as_str()?)?;
    let amount = pool_amount(amm.get("amount"))?;
    let amount2 = pool_amount(amm.get("amount2"))?;
    let lp_token = token_amount(amm.get("lp_token"))?;
    let trading_fee = number(amm.get("trading_fee"))?;
    let frozen = |key: &str| match amm.get(key) {
        None => Some(false),
        Some(flag) => flag.as_bool(),
    };
    let info = AmmInfo {
        account,
        amount,
        amount2,
        lp_token,
        trading_fee,
        asset_frozen: frozen("asset_frozen")?,
        asset2_frozen: frozen("asset2_frozen")?,
        ledger_index: number(result.get("ledger_index"))?,
        validated: result.get("validated").and_then(Value::as_bool) == Some(true),
    };
    let fits = amount.asset() != amount2.asset()
        && lp_token.issuer == account
        && !lp_token.value.is_negative()
        && trading_fee <= MAX_TRADING_FEE;
    fits.then_some(info)
}

/// What a pool holds of one asset: drops, or a token not below zero.
fn pool_amount(value: Option<&Value>) -> Option<Amount> {
    let value = value?;
    if value.is_string() {
        return drops(Some(value)).map(Amount::Xrp);
    }
    token_amount(Some(value))
        .filter(|token| !token.value.is_negative())
        .map(Amount::Token)
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

/// The longest message the account reads from its server: a state proof
/// of [`MAX_PROOF_KEYS`] keys, each with a branch of a few inner nodes and
/// an object as large as a skip list, or the validations of one ledger by a
/// thousand validators, each in hex, fit many times over. A server whose
/// answer is longer is cut off, and the account connects anew.
pub const MAX_ANSWER_BYTES: usize = 4 << 20;

/// The most distinct keys one `state_proof` names, as the proof API bounds
/// them (spec/designs/xrpl-validation.md#3-the-proof-api).
pub const MAX_PROOF_KEYS: usize = 32;

/// `state_proof` of `keys` at the ledger of `hash`, or at the server's
/// newest validated ledger.
#[must_use]
pub fn state_proof(id: u64, keys: &[[u8; 32]], at: Option<&[u8; 32]>) -> Vec<u8> {
    let keys: Vec<Value> = keys.iter().map(|key| hex_upper(key).into()).collect();
    let params = match at {
        Some(hash) => json!({ "keys": keys, "ledger_hash": hex_upper(hash) }),
        None => json!({ "keys": keys, "ledger_index": "validated" }),
    };
    request(id, "state_proof", params)
}

/// `tx_proof` of the transaction `hash`: in ledger `ledger`, or wherever
/// the server finds it. The answer starts its branch above the leaf, which
/// the node builds from what [`tx_binary`] reads.
#[must_use]
pub fn tx_proof(id: u64, hash: &[u8; 32], ledger: Option<u32>) -> Vec<u8> {
    let mut params = json!({ "tx_hash": hex_upper(hash) });
    if let (Some(object), Some(ledger)) = (params.as_object_mut(), ledger) {
        object.insert("ledger_index".into(), ledger.into());
    }
    request(id, "tx_proof", params)
}

/// The transaction `hash` and its metadata in binary, wherever the
/// server's history has them: the bytes its ledger's transaction tree
/// hashes, from which the node builds the leaf `tx_proof` proves.
#[must_use]
pub fn tx_binary(id: u64, hash: &[u8; 32]) -> Vec<u8> {
    request(
        id,
        "tx",
        json!({ "transaction": hex_upper(hash), "binary": true }),
    )
}

/// `validations` of the ledger of `hash`: the validations the server
/// holds for it from the validators it trusts, and with `untrusted` those
/// of other keys too, the trusted ones first, perhaps cut
/// (spec/designs/xrpl-validation.md#3-the-proof-api).
#[must_use]
pub fn validations(id: u64, hash: &[u8; 32], untrusted: bool) -> Vec<u8> {
    let params = if untrusted {
        json!({ "ledger_hash": hex_upper(hash), "untrusted": true })
    } else {
        json!({ "ledger_hash": hex_upper(hash) })
    };
    request(id, "validations", params)
}

/// `manifest` of the validator whose master key is `key`: the newest of
/// its manifests the server knows.
#[must_use]
pub fn manifest(id: u64, key: &crate::keys::PublicKey) -> Vec<u8> {
    request(
        id,
        "manifest",
        json!({ "public_key": key.to_node_public() }),
    )
}

/// One key of a `state_proof` answer, unchecked.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProofEntry {
    /// The key.
    pub key: [u8; 32],
    /// Its branch, deepest node first: from the key's leaf, which holds
    /// its object ([`crate::shamap::held_object`]), or, for a key the
    /// state does not hold, from an empty slot or another key's leaf.
    pub path: Vec<Vec<u8>>,
}

/// A `state_proof` answer, unchecked: a ledger's header and each key's
/// entry. The ledger's index and hash and whether it is validated, which
/// the server says too, are hints, and not read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StateProof {
    /// The header, without its hash.
    pub header: Vec<u8>,
    /// Each key's entry, as the server gave them.
    pub entries: Vec<ProofEntry>,
}

/// A `tx_proof` answer, unchecked: a transaction's ledger's header, and
/// its branch in that ledger's transaction tree from the inner node above
/// its leaf. The ledger's index and hash and whether it is validated,
/// which the server says too, are hints, and not read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TxProof {
    /// Its ledger's header, without its hash.
    pub header: Vec<u8>,
    /// Its branch, deepest node first, without the leaf.
    pub path: Vec<Vec<u8>>,
}

/// A transaction and its metadata in binary, as `tx` gives them with
/// `binary`, unchecked: the bytes the leaf a `tx_proof` proves is built
/// from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TxBlobs {
    /// The transaction, signed.
    pub tx: Vec<u8>,
    /// Its metadata.
    pub meta: Vec<u8>,
}

/// A `validations` answer, unchecked.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Validations {
    /// Each validation's bytes.
    pub validations: Vec<Vec<u8>>,
    /// Whether the server cut the answer: it held more than it sends, and
    /// sent those it trusts first. Too few of the node's validators in a
    /// cut answer may be the server's choice, not theirs.
    pub truncated: bool,
}

/// Bytes in hex, at most `max` of them.
fn hex_bytes(value: Option<&Value>, max: usize) -> Option<Vec<u8>> {
    let text = value?.as_str()?;
    if text.len() > max.saturating_mul(2) {
        return None;
    }
    from_hex(text)
}

/// Reads `state_proof`'s result: at most [`MAX_PROOF_KEYS`] entries, each
/// with a branch of at most [`MAX_BRANCH_NODES`](crate::shamap::MAX_BRANCH_NODES)
/// nodes.
#[must_use]
pub fn state_proof_result(result: &Value) -> Option<StateProof> {
    use crate::shamap::{MAX_BRANCH_NODES, MAX_NODE_BYTES};

    let header = hex_bytes(result.get("ledger_header"), crate::header::HEADER_BYTES)?;
    let entries = result.get("entries")?.as_array()?;
    if entries.len() > MAX_PROOF_KEYS {
        return None;
    }
    let mut out = Vec::with_capacity(entries.len());
    for entry in entries {
        let key = hex_bytes(entry.get("key"), 32)?.try_into().ok()?;
        let nodes = entry.get("path")?.as_array()?;
        if nodes.len() > MAX_BRANCH_NODES {
            return None;
        }
        let path: Option<Vec<Vec<u8>>> = (nodes.iter())
            .map(|node| hex_bytes(Some(node), MAX_NODE_BYTES))
            .collect();
        out.push(ProofEntry { key, path: path? });
    }
    Some(StateProof {
        header,
        entries: out,
    })
}

/// Reads `tx_proof`'s result: a header, and a branch above the leaf of
/// at most [`MAX_DEPTH`](crate::shamap::MAX_DEPTH) inner nodes. The leaf
/// is not sent: the node builds it ([`tx_binary_result`]).
#[must_use]
pub fn tx_proof_result(result: &Value) -> Option<TxProof> {
    use crate::shamap::{MAX_DEPTH, MAX_NODE_BYTES};

    let nodes = result.get("path")?.as_array()?;
    if nodes.len() > MAX_DEPTH {
        return None;
    }
    let path: Option<Vec<Vec<u8>>> = (nodes.iter())
        .map(|node| hex_bytes(Some(node), MAX_NODE_BYTES))
        .collect();
    Some(TxProof {
        header: hex_bytes(result.get("ledger_header"), crate::header::HEADER_BYTES)?,
        path: path?,
    })
}

/// Reads the result of [`tx_binary`]: at API version 1, the transaction in
/// `tx` and its metadata in `meta`, each in hex and at most
/// [`MAX_VL_BYTES`](crate::binary::MAX_VL_BYTES), as a length prefix
/// writes them in the leaf. `None` for one with its fields as JSON.
#[must_use]
pub fn tx_binary_result(result: &Value) -> Option<TxBlobs> {
    use crate::binary::MAX_VL_BYTES;

    Some(TxBlobs {
        tx: hex_bytes(result.get("tx"), MAX_VL_BYTES)?,
        meta: hex_bytes(result.get("meta"), MAX_VL_BYTES)?,
    })
}

/// Whether the result of [`tx_binary`] is a transaction the server has not
/// saved with its ledger yet: without its metadata, or not validated. An
/// honest rippled answers so for a moment after it marks a ledger
/// validated and before it saves it, while `tx_proof` at that ledger
/// already proves; so it is no lie, only not yet.
#[must_use]
pub fn tx_not_yet(result: &Value) -> bool {
    result.get("meta").is_none() || result.get("validated").and_then(Value::as_bool) != Some(true)
}

/// Reads `validations`' result: each validation's bytes, at most
/// [`MAX_VALIDATIONS`](crate::unl::MAX_VALIDATIONS) of them, and whether
/// the server cut it.
#[must_use]
pub fn validations_result(result: &Value) -> Option<Validations> {
    use crate::validation::MAX_VALIDATION_BYTES;

    let validations = result.get("validations")?.as_array()?;
    if validations.len() > crate::unl::MAX_VALIDATIONS {
        return None;
    }
    let truncated = match result.get("truncated") {
        None => false,
        Some(truncated) => truncated.as_bool()?,
    };
    let validations = (validations.iter())
        .map(|validation| hex_bytes(Some(validation), MAX_VALIDATION_BYTES))
        .collect::<Option<_>>()?;
    Some(Validations {
        validations,
        truncated,
    })
}

/// Reads `manifest`'s result: the manifest's bytes, if the server knows
/// one.
#[must_use]
pub fn manifest_result(result: &Value) -> Option<Vec<u8>> {
    let text = result.get("manifest")?.as_str()?;
    if text.len() > crate::manifest::MAX_MANIFEST_BYTES.div_ceil(3) * 4 {
        return None;
    }
    crate::base64::decode(text)
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

    /// An `amm_info` result as rippled's documentation shows one, as of a
    /// validated ledger.
    fn amm_result() -> Value {
        json!({
            "amm": {
                "account": "rp9E3FN3gNmvePGhYnf414T2TkUuoxu8vM",
                "amount": "248606",
                "amount2": {
                    "currency": "TST",
                    "issuer": "rP9jPyP5kyvFRb6ZiRghAGw5u8SGAmU4bd",
                    "value": "104.3451326160501",
                },
                "asset2_frozen": false,
                "lp_token": {
                    "currency": "039C99CD9AB0B70B32ECDA51EAAE471625608EA2",
                    "issuer": "rp9E3FN3gNmvePGhYnf414T2TkUuoxu8vM",
                    "value": "5089.991526721425",
                },
                "trading_fee": 500,
                "vote_slots": [],
            },
            "ledger_index": 316_745,
            "validated": true,
        })
    }

    #[test]
    fn a_pool_is_read_exactly() {
        let tst = Asset::Token {
            currency: Currency::parse("TST").unwrap(),
            issuer: AccountId::parse("rP9jPyP5kyvFRb6ZiRghAGw5u8SGAmU4bd").unwrap(),
        };
        let request: Value = serde_json::from_slice(&amm_info(9, Asset::Xrp, tst)).unwrap();
        assert_eq!(request["command"], "amm_info");
        assert_eq!(request["asset"], json!({ "currency": "XRP" }));
        assert_eq!(request["asset2"]["currency"], "TST");
        let info = amm_info_result(&amm_result()).unwrap();
        assert_eq!(info.amount, Amount::Xrp(248_606));
        assert_eq!(
            info.amount2.token().unwrap().value,
            TokenValue::parse("104.3451326160501").unwrap()
        );
        assert_eq!((info.trading_fee, info.ledger_index), (500, 316_745));
        assert!(info.validated && !info.asset_frozen && !info.asset2_frozen);
        // The LP token's currency is the ledger's, from the two assets.
        assert_eq!(
            info.lp_token.currency,
            crate::amm::lp_token_currency(Asset::Xrp, tst)
        );
        assert_eq!(info.sides(tst), Some((info.amount2, info.amount)));
        assert_eq!(info.sides(Asset::Xrp), Some((info.amount, info.amount2)));
        let other = Asset::Token {
            currency: Currency::parse("USD").unwrap(),
            issuer: info.account,
        };
        assert_eq!(info.sides(other), None);
    }

    #[test]
    fn a_pool_that_is_not_one_is_refused() {
        let refused = |change: &dyn Fn(&mut Value)| {
            let mut result = amm_result();
            change(&mut result);
            amm_info_result(&result)
        };
        assert!(refused(&|_| {}).is_some());
        // A fee above the ledger's, or written otherwise.
        assert!(refused(&|r| r["amm"]["trading_fee"] = 1_001.into()).is_none());
        assert!(refused(&|r| r["amm"]["trading_fee"] = "500".into()).is_none());
        // Two of one asset.
        assert!(refused(&|r| r["amm"]["amount2"] = "5".into()).is_none());
        // Below zero, or an amount the ledger cannot express.
        assert!(refused(&|r| r["amm"]["amount2"]["value"] = "-1".into()).is_none());
        assert!(
            refused(&|r| r["amm"]["amount2"]["value"] = "1.23456789012345678".into()).is_none()
        );
        assert!(refused(&|r| r["amm"]["amount"] = "-5".into()).is_none());
        assert!(refused(&|r| r["amm"]["amount"] = 5.into()).is_none());
        // An LP token another account issues.
        assert!(
            refused(
                &|r| r["amm"]["lp_token"]["issuer"] = "rP9jPyP5kyvFRb6ZiRghAGw5u8SGAmU4bd".into()
            )
            .is_none()
        );
        // A freeze that is no flag, and an answer as of no ledger.
        assert!(refused(&|r| r["amm"]["asset2_frozen"] = "yes".into()).is_none());
        assert!(refused(&|r| r["ledger_index"] = Value::Null).is_none());
        assert!(
            refused(&|r| r["amm"]["asset2_frozen"] = true.into()).is_some_and(|i| i.asset2_frozen)
        );
    }

    /// An `account_objects` result as rippled's documentation shows a
    /// signer list in one, with a trust line beside it.
    fn objects_result() -> Value {
        let entry =
            |account: &str| json!({ "SignerEntry": { "Account": account, "SignerWeight": 1 } });
        json!({
            "account": "rsUiUMpnrgxQp24dJYZDhmV4bE3aBtQyt8",
            "account_objects": [
                {
                    "Flags": 65_536,
                    "LedgerEntryType": "SignerList",
                    "OwnerNode": "0",
                    "SignerEntries": [
                        entry("rGUM1gzaff5U3gW7BVabm6ibgn2y8ThJ4i"),
                        entry("rLS65dJaZawYYbkZSLBaue52S7qMo2KZsJ"),
                    ],
                    "SignerListID": 0,
                    "SignerQuorum": 2,
                    "index": "A9C28A28B85CD533217F5C0A0C7767666B093FA58A0F2D80026FCC4CD932DDC7",
                },
                { "LedgerEntryType": "RippleState" },
            ],
            "ledger_index": 900,
            "validated": true,
        })
    }

    #[test]
    fn a_signer_list_is_read_only_as_the_ledger_keeps_one() {
        let objects = account_objects_result(&objects_result()).unwrap();
        let list = objects.signer_list.unwrap();
        assert_eq!((list.quorum, list.entries.len(), objects.others), (2, 2, 1));
        assert!(list.entries[0].account < list.entries[1].account);
        assert_eq!((objects.ledger_index, objects.validated), (900, true));
        let refused = |change: &dyn Fn(&mut Value)| {
            let mut result = objects_result();
            change(&mut result);
            account_objects_result(&result)
        };
        // Two lists, entries out of order, a weight beyond 16 bits, none.
        assert!(
            refused(&|r| {
                let list = r["account_objects"][0].clone();
                r["account_objects"][1] = list;
            })
            .is_none()
        );
        assert!(
            refused(&|r| {
                let entries = &mut r["account_objects"][0]["SignerEntries"];
                let first = entries[0].clone();
                entries[0] = entries[1].clone();
                entries[1] = first;
            })
            .is_none()
        );
        assert!(
            refused(&|r| {
                r["account_objects"][0]["SignerEntries"][0]["SignerEntry"]["SignerWeight"] =
                    65_536.into();
            })
            .is_none()
        );
        assert!(refused(&|r| r["account_objects"][0]["SignerEntries"] = json!([])).is_none());
        // An account's regular key is read with its flags.
        let settings = account_settings_result(&json!({
            "account_data": {
                "Flags": account_flags::DISABLE_MASTER | account_flags::REQUIRE_DEST_TAG,
                "RegularKey": "rGUM1gzaff5U3gW7BVabm6ibgn2y8ThJ4i",
            },
            "ledger_index": 900,
            "validated": true,
        }))
        .unwrap();
        assert!(settings.master_disabled() && settings.requires_destination_tag());
        assert_eq!(
            settings.regular_key,
            AccountId::parse("rGUM1gzaff5U3gW7BVabm6ibgn2y8ThJ4i")
        );
    }

    // spec/designs/xrpl-validation.md#3-the-proof-api: the proof methods'
    // answers are read as bytes, bounded before they are, and nothing in
    // them is believed here.
    #[test]
    fn proof_answers_are_read_bounded() {
        let entry = |key: u8| json!({ "key": hex_upper(&[key; 32]), "path": ["AB", "CD"] });
        let header = hex_upper(&[0; crate::header::HEADER_BYTES]);
        let answer = |entries: Vec<Value>| {
            state_proof_result(&json!({ "ledger_header": header, "entries": entries }))
        };
        let read = answer(alloc::vec![
            entry(1),
            json!({ "key": hex_upper(&[2; 32]), "path": [] })
        ])
        .unwrap();
        assert_eq!(read.entries[0].path, [alloc::vec![0xAB], alloc::vec![0xCD]]);
        assert!(read.entries[1].path.is_empty());
        // The proof API's final shapes carry no object beside the branch:
        // one sent anyway is not read.
        let beside = json!({ "key": hex_upper(&[3; 32]), "node_binary": "EF", "path": ["AB"] });
        assert_eq!(
            answer(alloc::vec![beside]).unwrap().entries[0].path,
            [[0xAB]]
        );
        let most = (0..MAX_PROOF_KEYS).map(|key| entry(u8::try_from(key).unwrap()));
        assert!(answer(most.clone().collect()).is_some());
        assert!(answer(most.chain([entry(99)]).collect()).is_none());
        let deep = json!({
            "key": hex_upper(&[1; 32]),
            "path": alloc::vec!["CD"; crate::shamap::MAX_BRANCH_NODES + 1],
        });
        assert!(answer(alloc::vec![deep]).is_none());
        let long = hex_upper(&[0; crate::header::HEADER_BYTES + 1]);
        assert!(state_proof_result(&json!({ "ledger_header": long, "entries": [] })).is_none());
        let validations =
            |count: usize| validations_result(&json!({ "validations": alloc::vec!["AB"; count] }));
        let two = validations(2).unwrap();
        assert_eq!((two.validations.len(), two.truncated), (2, false));
        assert!(validations(crate::unl::MAX_VALIDATIONS + 1).is_none());
        assert!(validations_result(&json!({ "validations": ["XY"] })).is_none());
        let cut = json!({ "validations": ["AB"], "truncated": true });
        assert!(validations_result(&cut).unwrap().truncated);
        assert!(validations_result(&json!({ "validations": [], "truncated": 1 })).is_none());
        assert_eq!(
            manifest_result(&json!({ "manifest": "AAEC" })),
            Some(alloc::vec![0, 1, 2])
        );
        assert_eq!(manifest_result(&json!({ "requested": "n9…" })), None);
        let request: Value = serde_json::from_slice(&state_proof(3, &[[1; 32]], None)).unwrap();
        assert_eq!(request["command"], "state_proof");
        assert_eq!(request["ledger_index"], "validated");
        let at: Value =
            serde_json::from_slice(&state_proof(4, &[[1; 32]], Some(&[2; 32]))).unwrap();
        assert_eq!(at["ledger_hash"], hex_upper(&[2; 32]));
        assert!(at.get("ledger_index").is_none());
    }

    // spec/designs/xrpl-validation.md#3-the-proof-api: a transaction's
    // proof is its header and its branch above the leaf, and the bytes the
    // leaf is built from come from `tx` in binary, each read bounded.
    #[test]
    fn a_transaction_s_proof_and_its_bytes_are_read_bounded() {
        let header = hex_upper(&[0; crate::header::HEADER_BYTES]);
        // A transaction's proof starts above its leaf: no more inner nodes
        // than a key has nibbles, and no bytes of the transaction.
        let proof = |nodes: usize| {
            tx_proof_result(&json!({
                "ledger_header": header,
                "path": alloc::vec!["CD"; nodes],
                "tx_blob": "AB",
            }))
        };
        assert_eq!(proof(2).unwrap().path.len(), 2);
        assert!(proof(crate::shamap::MAX_DEPTH).is_some());
        assert!(proof(crate::shamap::MAX_DEPTH + 1).is_none());
        // The bytes come from `tx` with binary, at API version 1, bounded.
        let blobs = tx_binary_result(&json!({ "tx": "1200", "meta": "201C" })).unwrap();
        assert_eq!(
            (blobs.tx, blobs.meta),
            (alloc::vec![0x12, 0], alloc::vec![0x20, 0x1C])
        );
        assert!(tx_binary_result(&json!({ "tx": { "TransactionType": "Payment" } })).is_none());
        let longest = "AB".repeat(crate::binary::MAX_VL_BYTES);
        assert!(tx_binary_result(&json!({ "tx": longest, "meta": "AB" })).is_some());
        let longer = "AB".repeat(crate::binary::MAX_VL_BYTES + 1);
        assert!(tx_binary_result(&json!({ "tx": "AB", "meta": longer })).is_none());
        // Not saved with its ledger yet: no metadata, or not validated.
        let saved = json!({ "tx": "1200", "meta": "201C", "validated": true });
        assert!(!tx_not_yet(&saved));
        assert!(tx_not_yet(&json!({ "tx": "1200", "validated": false })));
        assert!(tx_not_yet(
            &json!({ "tx": "1200", "meta": "201C", "validated": false })
        ));
        assert!(tx_not_yet(&json!({ "tx": "1200", "meta": "201C" })));
        let request: Value = serde_json::from_slice(&tx_binary(5, &[1; 32])).unwrap();
        assert_eq!(
            (&request["command"], &request["binary"]),
            (&json!("tx"), &json!(true))
        );
        assert_eq!(request["api_version"], 1);
    }
}
