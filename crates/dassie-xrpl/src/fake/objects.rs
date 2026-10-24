//! The fake ledger's records in the ledger's one binary form
//! (spec/designs/xrpl-validation.md#11-the-fake-ledger-the-lying-server-and-the-simulation):
//! each ledger object as rippled keeps it under its key, the transactions
//! the fake makes itself, and the JSON rippled renders of binary fields.
//!
//! An [`Object`] holds fields by their ids, so it is written in canonical
//! order whatever order it was built in. The JSON the fake answers with
//! for metadata and transactions is rendered from the bytes, so what it
//! says and what it proves are one.
//!
//! The fake keeps no owner directories: an object's `OwnerNode`, `LowNode`
//! and `HighNode` are always the first page, which no proof reads.

use alloc::collections::BTreeMap;
use alloc::string::{String, ToString};
use alloc::vec;
use alloc::vec::Vec;

use serde_json::{Map, Value, json};

use super::Account;
use super::pools::Pool;
use super::tokens::{Clawback, IssuerSet, Token};
use crate::amm;
use crate::amount::Asset;
use crate::api::{SignerList, TrustLine};
use crate::binary::{self, Field, FieldId, Fields, Type};
use crate::codec::{Amount, Currency, TokenAmount, TokenValue};
use crate::keys::{AccountId, sha512_half};
use crate::{MAX_DROPS, hex_upper};

/// What last changed an object: a transaction's hash and its ledger. Zero
/// for one no transaction changed, such as what the fake made at its
/// start.
pub(super) type Thread = ([u8; 32], u32);

/// No transaction.
pub(super) const UNTHREADED: Thread = ([0; 32], 0);

// The fields the fake writes, by type and code (rippled's `sfields.macro`).
pub(super) const TRANSACTION_RESULT: FieldId = FieldId::new(Type::UInt8, 3);
pub(super) const LEDGER_ENTRY_TYPE: FieldId = FieldId::new(Type::UInt16, 1);
pub(super) const TRANSACTION_TYPE: FieldId = FieldId::new(Type::UInt16, 2);
const SIGNER_WEIGHT: FieldId = FieldId::new(Type::UInt16, 3);
const TRADING_FEE: FieldId = FieldId::new(Type::UInt16, 5);
pub(super) const FLAGS: FieldId = FieldId::new(Type::UInt32, 2);
pub(super) const SEQUENCE: FieldId = FieldId::new(Type::UInt32, 4);
pub(super) const PREVIOUS_TXN_LGR_SEQ: FieldId = FieldId::new(Type::UInt32, 5);
const TRANSFER_RATE: FieldId = FieldId::new(Type::UInt32, 11);
const OWNER_COUNT: FieldId = FieldId::new(Type::UInt32, 13);
const LAST_LEDGER_SEQUENCE: FieldId = FieldId::new(Type::UInt32, 27);
pub(super) const TRANSACTION_INDEX: FieldId = FieldId::new(Type::UInt32, 28);
const SET_FLAG: FieldId = FieldId::new(Type::UInt32, 33);
const CLEAR_FLAG: FieldId = FieldId::new(Type::UInt32, 34);
const SIGNER_QUORUM: FieldId = FieldId::new(Type::UInt32, 35);
const SIGNER_LIST_ID: FieldId = FieldId::new(Type::UInt32, 38);
const OWNER_NODE: FieldId = FieldId::new(Type::UInt64, 4);
const LOW_NODE: FieldId = FieldId::new(Type::UInt64, 7);
const HIGH_NODE: FieldId = FieldId::new(Type::UInt64, 8);
pub(super) const PREVIOUS_TXN_ID: FieldId = FieldId::new(Type::Hash256, 5);
pub(super) const LEDGER_INDEX: FieldId = FieldId::new(Type::Hash256, 6);
const AMM_ID: FieldId = FieldId::new(Type::Hash256, 14);
const AMOUNT: FieldId = FieldId::new(Type::Amount, 1);
pub(super) const BALANCE: FieldId = FieldId::new(Type::Amount, 2);
const LOW_LIMIT: FieldId = FieldId::new(Type::Amount, 6);
const HIGH_LIMIT: FieldId = FieldId::new(Type::Amount, 7);
const FEE: FieldId = FieldId::new(Type::Amount, 8);
const AMOUNT2: FieldId = FieldId::new(Type::Amount, 11);
const BASE_FEE_DROPS: FieldId = FieldId::new(Type::Amount, 22);
const RESERVE_BASE_DROPS: FieldId = FieldId::new(Type::Amount, 23);
const RESERVE_INCREMENT_DROPS: FieldId = FieldId::new(Type::Amount, 24);
const LP_TOKEN_BALANCE: FieldId = FieldId::new(Type::Amount, 31);
const SIGNING_PUB_KEY: FieldId = FieldId::new(Type::Blob, 3);
const ACCOUNT: FieldId = FieldId::new(Type::AccountId, 1);
const DESTINATION: FieldId = FieldId::new(Type::AccountId, 3);
const REGULAR_KEY: FieldId = FieldId::new(Type::AccountId, 8);
pub(super) const HASHES: FieldId = FieldId::new(Type::Vector256, 2);
const ASSET: FieldId = FieldId::new(Type::Issue, 3);
const ASSET2: FieldId = FieldId::new(Type::Issue, 4);
pub(super) const CREATED_NODE: FieldId = FieldId::new(Type::Object, 3);
pub(super) const DELETED_NODE: FieldId = FieldId::new(Type::Object, 4);
pub(super) const MODIFIED_NODE: FieldId = FieldId::new(Type::Object, 5);
pub(super) const PREVIOUS_FIELDS: FieldId = FieldId::new(Type::Object, 6);
pub(super) const FINAL_FIELDS: FieldId = FieldId::new(Type::Object, 7);
pub(super) const NEW_FIELDS: FieldId = FieldId::new(Type::Object, 8);
const MEMO: FieldId = FieldId::new(Type::Object, 10);
const SIGNER_ENTRY: FieldId = FieldId::new(Type::Object, 11);
const SIGNER: FieldId = FieldId::new(Type::Object, 16);
const SIGNERS: FieldId = FieldId::new(Type::Array, 3);
const SIGNER_ENTRIES: FieldId = FieldId::new(Type::Array, 4);
pub(super) const AFFECTED_NODES: FieldId = FieldId::new(Type::Array, 8);
const MEMOS: FieldId = FieldId::new(Type::Array, 9);

/// Each field's name, as rippled's JSON writes it.
const NAMES: [(FieldId, &str); 61] = [
    (TRANSACTION_RESULT, "TransactionResult"),
    (LEDGER_ENTRY_TYPE, "LedgerEntryType"),
    (TRANSACTION_TYPE, "TransactionType"),
    (SIGNER_WEIGHT, "SignerWeight"),
    (TRADING_FEE, "TradingFee"),
    (FieldId::new(Type::UInt32, 1), "NetworkID"),
    (FLAGS, "Flags"),
    (SEQUENCE, "Sequence"),
    (PREVIOUS_TXN_LGR_SEQ, "PreviousTxnLgrSeq"),
    (TRANSFER_RATE, "TransferRate"),
    (OWNER_COUNT, "OwnerCount"),
    (FieldId::new(Type::UInt32, 14), "DestinationTag"),
    (LAST_LEDGER_SEQUENCE, "LastLedgerSequence"),
    (TRANSACTION_INDEX, "TransactionIndex"),
    (SET_FLAG, "SetFlag"),
    (CLEAR_FLAG, "ClearFlag"),
    (SIGNER_QUORUM, "SignerQuorum"),
    (SIGNER_LIST_ID, "SignerListID"),
    (OWNER_NODE, "OwnerNode"),
    (LOW_NODE, "LowNode"),
    (HIGH_NODE, "HighNode"),
    (PREVIOUS_TXN_ID, "PreviousTxnID"),
    (LEDGER_INDEX, "LedgerIndex"),
    (AMM_ID, "AMMID"),
    (AMOUNT, "Amount"),
    (BALANCE, "Balance"),
    (FieldId::new(Type::Amount, 3), "LimitAmount"),
    (LOW_LIMIT, "LowLimit"),
    (HIGH_LIMIT, "HighLimit"),
    (FEE, "Fee"),
    (FieldId::new(Type::Amount, 9), "SendMax"),
    (AMOUNT2, "Amount2"),
    (FieldId::new(Type::Amount, 18), "DeliveredAmount"),
    (BASE_FEE_DROPS, "BaseFeeDrops"),
    (RESERVE_BASE_DROPS, "ReserveBaseDrops"),
    (RESERVE_INCREMENT_DROPS, "ReserveIncrementDrops"),
    (LP_TOKEN_BALANCE, "LPTokenBalance"),
    (SIGNING_PUB_KEY, "SigningPubKey"),
    (FieldId::new(Type::Blob, 4), "TxnSignature"),
    (FieldId::new(Type::Blob, 12), "MemoType"),
    (FieldId::new(Type::Blob, 13), "MemoData"),
    (FieldId::new(Type::Blob, 14), "MemoFormat"),
    (ACCOUNT, "Account"),
    (DESTINATION, "Destination"),
    (REGULAR_KEY, "RegularKey"),
    (HASHES, "Hashes"),
    (ASSET, "Asset"),
    (ASSET2, "Asset2"),
    (CREATED_NODE, "CreatedNode"),
    (DELETED_NODE, "DeletedNode"),
    (MODIFIED_NODE, "ModifiedNode"),
    (PREVIOUS_FIELDS, "PreviousFields"),
    (FINAL_FIELDS, "FinalFields"),
    (NEW_FIELDS, "NewFields"),
    (MEMO, "Memo"),
    (SIGNER_ENTRY, "SignerEntry"),
    (SIGNER, "Signer"),
    (SIGNERS, "Signers"),
    (SIGNER_ENTRIES, "SignerEntries"),
    (AFFECTED_NODES, "AffectedNodes"),
    (MEMOS, "Memos"),
];

// The kinds of ledger objects the fake keeps (`ledger_entries.macro`).
pub(super) const ACCOUNT_ROOT: u16 = 0x0061;
pub(super) const RIPPLE_STATE: u16 = 0x0072;
const SIGNER_LIST: u16 = 0x0053;
pub(super) const LEDGER_HASHES: u16 = 0x0068;
const FEE_SETTINGS: u16 = 0x0073;
const AMM: u16 = 0x0079;

const ENTRY_TYPES: [(u16, &str); 6] = [
    (ACCOUNT_ROOT, "AccountRoot"),
    (RIPPLE_STATE, "RippleState"),
    (SIGNER_LIST, "SignerList"),
    (LEDGER_HASHES, "LedgerHashes"),
    (FEE_SETTINGS, "FeeSettings"),
    (AMM, "AMM"),
];

// The kinds of transactions the fake applies (`transactions.macro`).
const ACCOUNT_SET: u16 = 3;
const CLAWBACK: u16 = 30;
/// The fake's own transaction that makes or sets a pool: no ledger has
/// one, so no ledger's kind is taken.
const POOL_SET: u16 = 0xFFFF;

const TRANSACTION_TYPES: [(u16, &str); 7] = [
    (0, "Payment"),
    (ACCOUNT_SET, "AccountSet"),
    (5, "SetRegularKey"),
    (12, "SignerListSet"),
    (20, "TrustSet"),
    (CLAWBACK, "Clawback"),
    (POOL_SET, "AMMSet"),
];

/// The results a transaction in a ledger can have, by their codes
/// (`TER.h`): success, or a failure that claims the fee.
const RESULTS: [(&str, u8); 18] = [
    ("tesSUCCESS", 0),
    ("tecPATH_PARTIAL", 101),
    ("tecUNFUNDED_PAYMENT", 104),
    ("tecINSUF_RESERVE_LINE", 122),
    ("tecNO_DST", 124),
    ("tecNO_DST_INSUF_XRP", 125),
    ("tecNO_LINE_INSUF_RESERVE", 126),
    ("tecNO_LINE_REDUNDANT", 127),
    ("tecPATH_DRY", 128),
    ("tecUNFUNDED", 129),
    ("tecNO_ALTERNATIVE_KEY", 130),
    ("tecNO_LINE", 135),
    ("tecNO_PERMISSION", 139),
    ("tecINSUFFICIENT_RESERVE", 141),
    ("tecNEED_MASTER_KEY", 142),
    ("tecDST_TAG_NEEDED", 143),
    ("tecINTERNAL", 144),
    ("tecPRECISION_LOSS", 197),
];

/// The code of a transaction's result. A result no ledger holds, such as
/// a failure the fake reports where a bug left it no other, is written as
/// the ledger's internal failure.
pub(super) fn result_code(result: &str) -> u8 {
    RESULTS
        .iter()
        .find(|(name, _)| *name == result)
        .map_or(144, |(_, code)| *code)
}

/// A ledger object, a transaction or metadata: its fields by id, written
/// in canonical order. A value is as [`Field::value`] holds it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(super) struct Object {
    fields: BTreeMap<FieldId, Vec<u8>>,
}

impl Object {
    /// An object of `entry_type`.
    pub fn entry(entry_type: u16) -> Self {
        let mut object = Self::default();
        object.put(LEDGER_ENTRY_TYPE, &entry_type.to_be_bytes());
        object
    }

    /// The fields in `bytes`, if they are a serialization in canonical
    /// form.
    pub fn from_bytes(bytes: &[u8]) -> Option<Self> {
        let mut object = Self::default();
        for field in Fields::new(bytes) {
            let field = field.ok()?;
            object.put(field.id, field.value);
        }
        Some(object)
    }

    /// Sets `id` to `value`.
    pub fn put(&mut self, id: FieldId, value: &[u8]) {
        self.fields.insert(id, value.to_vec());
    }

    /// Sets `id`, a `UInt32`.
    pub fn put_u32(&mut self, id: FieldId, value: u32) {
        self.put(id, &value.to_be_bytes());
    }

    /// Sets `id`, an amount.
    pub fn put_amount(&mut self, id: FieldId, amount: &Amount) {
        match amount {
            Amount::Xrp(drops) => self.put(id, &binary::xrp(*drops)),
            Amount::Token(token) => self.put(id, &token.to_bytes()),
        }
    }

    /// The value of `id`, if it is set.
    pub fn get(&self, id: FieldId) -> Option<&[u8]> {
        self.fields.get(&id).map(Vec::as_slice)
    }

    /// Unsets `id`.
    pub fn remove(&mut self, id: FieldId) {
        self.fields.remove(&id);
    }

    /// Its fields, in canonical order.
    pub fn iter(&self) -> impl Iterator<Item = (FieldId, &[u8])> {
        self.fields
            .iter()
            .map(|(id, value)| (*id, value.as_slice()))
    }

    /// Whether it has no fields.
    pub fn is_empty(&self) -> bool {
        self.fields.is_empty()
    }

    /// Its bytes, in canonical form.
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut out = Vec::new();
        for (id, value) in &self.fields {
            binary::write_field(&mut out, *id, value);
        }
        assert!(
            binary::check(&out).is_ok(),
            "the fake writes canonical form"
        );
        out
    }

    /// The kind of ledger object it is, if it is one.
    pub fn entry_type(&self) -> Option<u16> {
        Some(u16::from_be_bytes(
            self.get(LEDGER_ENTRY_TYPE)?.try_into().ok()?,
        ))
    }
}

/// Writes `elements`, each an object of `id`, as an array's value.
pub(super) fn array(id: FieldId, elements: impl IntoIterator<Item = Object>) -> Vec<u8> {
    let mut out = Vec::new();
    for element in elements {
        binary::write_field(&mut out, id, &element.to_bytes());
    }
    out
}

/// Whether a value is one rippled leaves out of a new object's metadata:
/// zero, empty, or no XRP.
pub(super) fn is_default(id: FieldId, value: &[u8]) -> bool {
    value.iter().all(|byte| *byte == 0) || (id.kind() == Type::Amount && value == binary::xrp(0))
}

/// Whether rippled threads an object of `entry_type`: it names the last
/// transaction that changed it.
pub(super) const fn is_threaded(entry_type: u16) -> bool {
    !matches!(entry_type, LEDGER_HASHES | FEE_SETTINGS)
}

fn key(space: u8, parts: &[&[u8]]) -> [u8; 32] {
    let mut message = vec![0, space];
    for part in parts {
        message.extend_from_slice(part);
    }
    sha512_half(&message)
}

/// An account root's key.
pub(super) fn account_key(account: AccountId) -> [u8; 32] {
    key(b'a', &[&account.0])
}

/// The key of the `AMM` object of the pool of XRP and `token`: its two
/// issues, each an issuer and a currency, XRP's first, as it sorts first.
pub(super) fn amm_key(token: Token) -> [u8; 32] {
    key(
        b'A',
        &[&[0; 20], &[0; 20], &token.issuer.0, &token.currency.0],
    )
}

/// The key of the skip list of the last 256 ledgers' hashes.
pub(super) fn skip_key() -> [u8; 32] {
    crate::skip_list::recent_key()
}

/// The key of the skip list that holds the hash of flag ledger `ledger`,
/// one in each 65,536 ledgers' worth.
pub(super) fn flag_skip_key(ledger: u32) -> [u8; 32] {
    crate::skip_list::flag_key(ledger)
}

/// The key of the fee settings.
pub(super) fn fees_key() -> [u8; 32] {
    key(b'e', &[])
}

fn put_thread(object: &mut Object, (txn, ledger): Thread) {
    object.put(PREVIOUS_TXN_ID, &txn);
    object.put_u32(PREVIOUS_TXN_LGR_SEQ, ledger);
}

/// An account's root: its XRP, sequence, owner count, flags, transfer rate
/// and regular key, and a pool account's `AMMID`.
pub(super) fn account_root(
    id: AccountId,
    account: &Account,
    amm: Option<[u8; 32]>,
    thread: Thread,
) -> Object {
    let mut object = Object::entry(ACCOUNT_ROOT);
    object.put_u32(FLAGS, account.flags);
    object.put_u32(SEQUENCE, account.sequence);
    object.put_u32(OWNER_COUNT, account.owner_count);
    if account.transfer_rate != 0 {
        object.put_u32(TRANSFER_RATE, account.transfer_rate);
    }
    if let Some(amm) = amm {
        object.put(AMM_ID, &amm);
    }
    object.put(BALANCE, &binary::xrp(account.balance));
    object.put(ACCOUNT, &id.0);
    if let Some(key) = account.regular_key {
        object.put(REGULAR_KEY, &key.0);
    }
    put_thread(&mut object, thread);
    object
}

/// The placeholder issuer of a line's balance, `ACCOUNT_ONE`: a balance
/// is between the line's two accounts, whichever way it goes.
pub(super) const ACCOUNT_ONE: AccountId =
    AccountId([0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1]);

/// A trust line: its balance from the low side, each side's limit, issued
/// by that side, and its flags.
pub(super) fn ripple_state(line: &TrustLine, thread: Thread) -> Object {
    let amount = |value: TokenValue, issuer: AccountId| TokenAmount {
        value,
        currency: line.currency,
        issuer,
    };
    let mut object = Object::entry(RIPPLE_STATE);
    object.put_u32(FLAGS, line.flags);
    object.put(LOW_NODE, &0_u64.to_be_bytes());
    object.put(HIGH_NODE, &0_u64.to_be_bytes());
    object.put(BALANCE, &amount(line.balance, ACCOUNT_ONE).to_bytes());
    object.put(LOW_LIMIT, &amount(line.low_limit, line.low).to_bytes());
    object.put(HIGH_LIMIT, &amount(line.high_limit, line.high).to_bytes());
    put_thread(&mut object, thread);
    object
}

/// A signer list's flag: it counts once toward its owner's reserve.
pub(super) const LSF_ONE_OWNER_COUNT: u32 = 0x0001_0000;

/// A signer list: its quorum and its entries, by account.
pub(super) fn signer_list(list: &SignerList, thread: Thread) -> Object {
    let mut object = Object::entry(SIGNER_LIST);
    object.put_u32(FLAGS, LSF_ONE_OWNER_COUNT);
    object.put_u32(SIGNER_QUORUM, list.quorum);
    object.put_u32(SIGNER_LIST_ID, 0);
    object.put(OWNER_NODE, &0_u64.to_be_bytes());
    let entries = list.entries.iter().map(|entry| {
        let mut element = Object::default();
        element.put(SIGNER_WEIGHT, &entry.weight.to_be_bytes());
        element.put(ACCOUNT, &entry.account.0);
        element
    });
    object.put(SIGNER_ENTRIES, &array(SIGNER_ENTRY, entries));
    put_thread(&mut object, thread);
    object
}

/// A token's issue, as an `Issue` field holds it.
fn issue(token: Token) -> [u8; 40] {
    let mut out = [0; 40];
    let (currency, issuer) = out.split_at_mut(20);
    currency.copy_from_slice(&token.currency.0);
    issuer.copy_from_slice(&token.issuer.0);
    out
}

/// The `AMM` object of the pool of XRP and `token`: its account, its
/// trading fee and its LP tokens.
pub(super) fn amm(token: Token, pool: &Pool, thread: Thread) -> Object {
    let lp_tokens = TokenAmount {
        value: pool.lp_tokens,
        currency: amm::lp_token_currency(
            Asset::Xrp,
            Asset::Token {
                currency: token.currency,
                issuer: token.issuer,
            },
        ),
        issuer: pool.account,
    };
    let mut object = Object::entry(AMM);
    object.put(
        TRADING_FEE,
        &u16::try_from(pool.trading_fee)
            .unwrap_or(u16::MAX)
            .to_be_bytes(),
    );
    object.put_u32(FLAGS, 0);
    object.put(OWNER_NODE, &0_u64.to_be_bytes());
    object.put(LP_TOKEN_BALANCE, &lp_tokens.to_bytes());
    object.put(ACCOUNT, &pool.account.0);
    object.put(ASSET, &[0; 20]);
    object.put(ASSET2, &issue(token));
    put_thread(&mut object, thread);
    object
}

/// A skip list: `hashes`, oldest first, the last of ledger `last`.
pub(super) fn ledger_hashes(last: u32, hashes: &[[u8; 32]]) -> Object {
    let mut object = Object::entry(LEDGER_HASHES);
    object.put_u32(FLAGS, 0);
    object.put_u32(LAST_LEDGER_SEQUENCE, last);
    object.put(HASHES, hashes.as_flattened());
    object
}

/// The fee settings: the base fee and the reserve, in drops, as the
/// ledger keeps them since its `XRPFees` amendment.
pub(super) fn fee_settings(base_fee: u64, (base, increment): (u64, u64)) -> Object {
    let drops = |drops: u64| binary::xrp(drops.min(MAX_DROPS));
    let mut object = Object::entry(FEE_SETTINGS);
    object.put_u32(FLAGS, 0);
    object.put(BASE_FEE_DROPS, &drops(base_fee));
    object.put(RESERVE_BASE_DROPS, &drops(base));
    object.put(RESERVE_INCREMENT_DROPS, &drops(increment));
    object
}

/// What every transaction of the fake's own has: unsigned, by `account`.
fn own(kind: u16, account: AccountId, sequence: u32, last_ledger: u32, fee: u64) -> Object {
    let mut object = Object::default();
    object.put(TRANSACTION_TYPE, &kind.to_be_bytes());
    object.put_u32(FLAGS, 0);
    object.put_u32(SEQUENCE, sequence);
    object.put_u32(LAST_LEDGER_SEQUENCE, last_ledger);
    object.put(FEE, &binary::xrp(fee));
    object.put(SIGNING_PUB_KEY, &[]);
    object.put(ACCOUNT, &account.0);
    object
}

/// An issuer's `AccountSet`.
pub(super) fn issuer_set(set: &IssuerSet, fee: u64) -> Object {
    let mut object = own(
        ACCOUNT_SET,
        set.account,
        set.sequence,
        set.last_ledger_sequence,
        fee,
    );
    for (id, value) in [
        (SET_FLAG, set.set_flag),
        (CLEAR_FLAG, set.clear_flag),
        (TRANSFER_RATE, set.transfer_rate),
    ] {
        if let Some(value) = value {
            object.put_u32(id, value);
        }
    }
    object
}

/// An issuer's `Clawback`.
pub(super) fn clawback(clawback: &Clawback, fee: u64) -> Object {
    let mut object = own(
        CLAWBACK,
        clawback.account,
        clawback.sequence,
        clawback.last_ledger_sequence,
        fee,
    );
    object.put(AMOUNT, &clawback.amount.to_bytes());
    object
}

/// The fake's own transaction that makes or sets a pool.
pub(super) fn pool_set(set: &super::pools::PoolSet, fee: u64) -> Object {
    let mut object = own(
        POOL_SET,
        set.account,
        set.sequence,
        set.last_ledger_sequence,
        fee,
    );
    object.put(
        TRADING_FEE,
        &u16::try_from(set.trading_fee)
            .unwrap_or(u16::MAX)
            .to_be_bytes(),
    );
    object.put(AMOUNT, &binary::xrp(set.drops));
    object.put(AMOUNT2, &set.token.amount(set.value).to_bytes());
    object.put(ASSET, &[0; 20]);
    object.put(ASSET2, &issue(set.token));
    object
}

/// A token amount as the ledger's JSON writes one.
pub(super) fn token_json(amount: &TokenAmount) -> Value {
    json!({
        "currency": amount.currency.to_string(),
        "issuer": amount.issuer.to_address(),
        "value": amount.value.to_string(),
    })
}

/// The JSON rippled renders of the fields in `bytes`, a serialization in
/// canonical form; `None` if it is not one. Nested objects and arrays are
/// rendered with a stack, at most as deep as the walker reads.
pub(super) fn to_json(bytes: &[u8]) -> Option<Value> {
    /// An object or array being rendered: its fields, what it holds so
    /// far, and its name in the one around it.
    struct Frame<'a> {
        fields: Fields<'a>,
        object: Map<String, Value>,
        array: Option<Vec<Value>>,
        name: &'static str,
    }
    let frame = |fields, array, name| Frame {
        fields,
        object: Map::new(),
        array,
        name,
    };
    let mut stack = vec![frame(Fields::new(bytes), None, "")];
    // Each turn reads a field, opens a container, or closes one; the
    // walker holds the depth to its bound.
    while let Some(top) = stack.last_mut() {
        match top.fields.next() {
            Some(Err(_)) => return None,
            Some(Ok(field)) => {
                let name = name(field.id)?;
                match field.id.kind() {
                    Type::Object => stack.push(frame(field.fields().ok()?, None, name)),
                    Type::Array => {
                        stack.push(frame(field.elements().ok()?, Some(Vec::new()), name));
                    }
                    _ => {
                        let value = leaf_json(&field)?;
                        match &mut top.array {
                            Some(_) => return None,
                            None => top.object.insert(name.into(), value),
                        };
                    }
                }
            }
            None => {
                let done = stack.pop()?;
                let value = match done.array {
                    Some(elements) => Value::Array(elements),
                    None => Value::Object(done.object),
                };
                let Some(parent) = stack.last_mut() else {
                    return Some(value);
                };
                match &mut parent.array {
                    Some(elements) => elements.push(json!({ done.name: value })),
                    None => {
                        parent.object.insert(done.name.into(), value);
                    }
                }
            }
        }
    }
    None
}

fn name(id: FieldId) -> Option<&'static str> {
    NAMES
        .iter()
        .find(|(field, _)| *field == id)
        .map(|(_, name)| *name)
}

/// A field that holds no others, as rippled's JSON writes it.
fn leaf_json(field: &Field<'_>) -> Option<Value> {
    let named = |table: &[(u16, &'static str)], code: u16| {
        table
            .iter()
            .find(|(known, _)| *known == code)
            .map(|(_, name)| Value::from(*name))
    };
    Some(match field.id.kind() {
        Type::UInt8 if field.id == TRANSACTION_RESULT => {
            let code = field.uint8().ok()?;
            let (name, _) = RESULTS.iter().find(|(_, known)| *known == code)?;
            Value::from(*name)
        }
        Type::UInt8 => field.uint8().ok()?.into(),
        Type::UInt16 if field.id == LEDGER_ENTRY_TYPE => named(&ENTRY_TYPES, field.uint16().ok()?)?,
        Type::UInt16 if field.id == TRANSACTION_TYPE => {
            named(&TRANSACTION_TYPES, field.uint16().ok()?)?
        }
        Type::UInt16 => field.uint16().ok()?.into(),
        Type::UInt32 => field.uint32().ok()?.into(),
        // In hex, without leading zeros, as rippled writes these.
        Type::UInt64 => alloc::format!("{:x}", field.uint64().ok()?).into(),
        Type::Amount => match field.amount().ok()? {
            Amount::Xrp(drops) => drops.to_string().into(),
            Amount::Token(token) => token_json(&token),
        },
        Type::AccountId => field
            .account()
            .ok()?
            .map_or_else(String::new, |account| account.to_address())
            .into(),
        Type::Vector256 => field
            .vector256()
            .ok()?
            .iter()
            .map(|hash| Value::from(hex_upper(hash)))
            .collect::<Vec<_>>()
            .into(),
        Type::Issue => match field.value {
            value if value.len() == 20 => json!({ "currency": "XRP" }),
            value => {
                let (currency, issuer) = value.split_at_checked(20)?;
                json!({
                    "currency": Currency(currency.try_into().ok()?).to_string(),
                    "issuer": AccountId(issuer.try_into().ok()?).to_address(),
                })
            }
        },
        _ => hex_upper(field.value).into(),
    })
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::indexing_slicing)]
mod tests {
    //! The fake's objects against rippled's, as mainnet holds them
    //! (spec/designs/xrpl-validation.md#13-what-pins-it): each built from
    //! the values a captured one holds writes its bytes, so the fake and
    //! the node cannot agree on a wrong layout or a wrong key.

    use alloc::string::String;

    use super::*;
    use crate::from_hex;
    use crate::header::LedgerHeader;

    fn data(name: &str) -> Value {
        let path = alloc::format!("{}/tests/data/{name}", env!("CARGO_MANIFEST_DIR"));
        serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
    }

    fn hex(value: &Value, key: &str) -> Vec<u8> {
        from_hex(value[key].as_str().unwrap()).unwrap()
    }

    /// A captured object by its label.
    fn object(file: &Value, list: &str, label: &str) -> Value {
        let found = file[list]
            .as_array()
            .unwrap()
            .iter()
            .find(|o| o["label"] == label);
        found.unwrap().clone()
    }

    /// The fields of `bytes`, by id.
    fn fields(bytes: &[u8]) -> BTreeMap<FieldId, Vec<u8>> {
        Fields::new(bytes)
            .map(|field| {
                let field = field.unwrap();
                (field.id, field.value.to_vec())
            })
            .collect()
    }

    fn u32_of(fields: &BTreeMap<FieldId, Vec<u8>>, id: FieldId) -> u32 {
        u32::from_be_bytes(fields[&id].as_slice().try_into().unwrap())
    }

    fn thread_of(fields: &BTreeMap<FieldId, Vec<u8>>) -> Thread {
        let txn = fields[&PREVIOUS_TXN_ID].as_slice().try_into().unwrap();
        (txn, u32_of(fields, PREVIOUS_TXN_LGR_SEQ))
    }

    fn amount_of(fields: &BTreeMap<FieldId, Vec<u8>>, id: FieldId) -> TokenAmount {
        TokenAmount::from_bytes(fields[&id].as_slice().try_into().unwrap()).unwrap()
    }

    /// Mainnet's XRP/RLUSD pool.
    const MAINNET_RLUSD: Token = Token {
        issuer: AccountId([
            0xE5, 0xE9, 0x61, 0xC6, 0xA0, 0x25, 0xC9, 0x40, 0x4A, 0xA7, 0xB6, 0x62, 0xDD, 0x1D,
            0xF9, 0x75, 0xBE, 0x75, 0xD1, 0x3E,
        ]),
        currency: crate::rlusd::CURRENCY,
    };

    #[test]
    fn keys_are_rippled_s() {
        let objects = data("ledger-objects.json");
        let index = |label: &str| {
            String::from(
                object(&objects, "objects", label)["index"]
                    .as_str()
                    .unwrap(),
            )
        };
        assert_eq!(hex_upper(&skip_key()), index("LedgerHashes (recent)"));
        let flags = object(&objects, "objects", "LedgerHashes (flag ledgers)");
        let flag_ledger = u32::try_from(flags["flag_ledger"].as_u64().unwrap()).unwrap();
        assert_eq!(
            hex_upper(&flag_skip_key(flag_ledger)),
            index("LedgerHashes (flag ledgers)")
        );
        assert_eq!(hex_upper(&fees_key()), index("FeeSettings"));
        // The pool's AMM object: XRP's issue first, each issue its issuer
        // and then its currency.
        assert_eq!(
            hex_upper(&amm_key(MAINNET_RLUSD)),
            "160C6649399D6AF625ED94A66812944BDA1D8993445A503F6B5730DECC7D3767"
        );
        let pool = AccountId::parse("rhWTXC2m2gGGA9WozUaoMm6kLAVPb1tcS3").unwrap();
        assert_eq!(
            hex_upper(&account_key(pool)),
            "95F777A378AF9428695717149DDE4EC563BE9EB0C9186CCFDED9542240B9B915"
        );
    }

    #[test]
    fn skip_lists_and_fee_settings_are_rippled_s() {
        let objects = data("ledger-objects.json");
        let header = LedgerHeader::from_bytes(&hex(&objects, "ledger_data")).unwrap();
        // The recent ledgers' list as of a ledger ends with its parent.
        let recent = hex(
            &object(&objects, "objects", "LedgerHashes (recent)"),
            "node_binary",
        );
        let read = fields(&recent);
        let hashes: Vec<[u8; 32]> = read[&HASHES].as_chunks::<32>().0.to_vec();
        assert_eq!(hashes.len(), 256);
        assert_eq!(hashes.last(), Some(&header.parent_hash));
        let last = u32_of(&read, LAST_LEDGER_SEQUENCE);
        assert_eq!(last, header.sequence - 1);
        // Mainnet's list was made before rippled stopped writing its first
        // ledger; a chain started now has none.
        let mut built = ledger_hashes(last, &hashes);
        built.put_u32(FieldId::new(Type::UInt32, 26), 2);
        assert_eq!(built.to_bytes(), recent);
        let flags = object(&objects, "objects", "LedgerHashes (flag ledgers)");
        let bytes = hex(&flags, "node_binary");
        let read = fields(&bytes);
        let last = u32_of(&read, LAST_LEDGER_SEQUENCE);
        assert_eq!(u64::from(last), flags["flag_ledger"].as_u64().unwrap());
        let hashes: Vec<[u8; 32]> = read[&HASHES].as_chunks::<32>().0.to_vec();
        assert_eq!(ledger_hashes(last, &hashes).to_bytes(), bytes);
        // Mainnet's fees: 10 drops and a reserve of 1 XRP and 0.2 XRP, as
        // the fake's, threaded since rippled names the fee vote's
        // transaction.
        let fees = hex(&object(&objects, "objects", "FeeSettings"), "node_binary");
        let mut built = fee_settings(10, (1_000_000, 200_000));
        put_thread(&mut built, thread_of(&fields(&fees)));
        assert_eq!(built.to_bytes(), fees);
    }

    #[test]
    fn a_pool_s_root_and_line_are_rippled_s() {
        let captured = data("ledger-binary.json");
        let root = hex(
            &object(&captured, "blobs", "AccountRoot (RLUSD/XRP AMM account)"),
            "hex",
        );
        let read = fields(&root);
        let id = AccountId(read[&ACCOUNT].as_slice().try_into().unwrap());
        let balance = u64::from_be_bytes(read[&BALANCE].as_slice().try_into().unwrap());
        let account = Account {
            balance: balance & !0x4000_0000_0000_0000,
            sequence: u32_of(&read, SEQUENCE),
            owner_count: u32_of(&read, OWNER_COUNT),
            flags: u32_of(&read, FLAGS),
            transfer_rate: 0,
            regular_key: None,
        };
        assert_eq!(account.flags, super::super::POOL_ACCOUNT_FLAGS);
        let built = account_root(id, &account, Some(amm_key(MAINNET_RLUSD)), thread_of(&read));
        assert_eq!(built.to_bytes(), root);

        let line = hex(
            &object(
                &captured,
                "blobs",
                "RippleState (AMM account / RLUSD issuer)",
            ),
            "hex",
        );
        let read = fields(&line);
        let (low, high) = (amount_of(&read, LOW_LIMIT), amount_of(&read, HIGH_LIMIT));
        let state = TrustLine {
            low: low.issuer,
            high: high.issuer,
            currency: low.currency,
            balance: amount_of(&read, BALANCE).value,
            low_limit: low.value,
            high_limit: high.value,
            flags: u32_of(&read, FLAGS),
        };
        assert_eq!(amount_of(&read, BALANCE).issuer, ACCOUNT_ONE);
        let mut built = ripple_state(&state, thread_of(&read));
        // The fake keeps no directories: its pages are the first.
        built.put(HIGH_NODE, &read[&HIGH_NODE]);
        built.put(LOW_NODE, &read[&LOW_NODE]);
        assert_eq!(built.to_bytes(), line);
    }

    #[test]
    fn a_pool_s_amm_object_is_rippled_s_but_its_slots() {
        let captured = data("ledger-binary.json");
        let amm_object = hex(&object(&captured, "blobs", "AMM object (XRP/RLUSD)"), "hex");
        let read = fields(&amm_object);
        let lp_tokens = amount_of(&read, LP_TOKEN_BALANCE);
        let pool = Pool {
            account: AccountId(read[&ACCOUNT].as_slice().try_into().unwrap()),
            trading_fee: u32::from(u16::from_be_bytes(
                read[&TRADING_FEE].as_slice().try_into().unwrap(),
            )),
            lp_tokens: lp_tokens.value,
        };
        let built = amm(MAINNET_RLUSD, &pool, thread_of(&read));
        // The LP token's currency and issuer are derived as rippled
        // derives them.
        assert_eq!(
            fields(&built.to_bytes())[&LP_TOKEN_BALANCE],
            read[&LP_TOKEN_BALANCE]
        );
        // The fake holds no auction slot or votes; with mainnet's, the
        // bytes are mainnet's.
        let mut built = built;
        for slots in [
            FieldId::new(Type::Object, 26),
            FieldId::new(Type::Array, 12),
        ] {
            built.put(slots, &read[&slots]);
        }
        assert_eq!(built.to_bytes(), amm_object);
    }

    #[test]
    fn json_is_rendered_as_rippled_writes_it() {
        // Mainnet ledger 107862214's transaction 1: its metadata changed
        // an account root, with the transaction before.
        let captured = data("ledger-binary.json");
        let meta = hex(
            &object(&captured, "blobs", "ledger 107862214, transaction 1, meta"),
            "hex",
        );
        let json = to_json(&meta).unwrap();
        assert_eq!(json["TransactionResult"], "tesSUCCESS");
        let node = &json["AffectedNodes"][0]["ModifiedNode"];
        assert_eq!(node["LedgerEntryType"], "AccountRoot");
        assert!(
            node["PreviousTxnID"]
                .as_str()
                .is_some_and(|id| id.len() == 64)
        );
        assert!(node["FinalFields"]["Balance"].as_str().is_some());
        assert!(
            node["FinalFields"]["Account"]
                .as_str()
                .is_some_and(|a| a.starts_with('r'))
        );
    }
}
