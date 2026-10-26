//! What the account reads from proven bytes
//! (spec/designs/xrpl-validation.md#8-the-verifier): the keys of the
//! ledger objects it follows, those objects in rippled's binary form, and a
//! binary transaction with its metadata as the record the schemes read.
//!
//! - **Keys** are rippled's: an account's root, a trust line by its two
//!   accounts and currency, an account's signer list, the `AMM` object of
//!   the pool of XRP and a token, and the fee settings.
//! - **Objects** are read on the walker of canonical serializations
//!   ([`crate::binary`]), each by the fields its reader needs: an account's
//!   root, a trust line, a signer list, the fee settings in either of
//!   rippled's forms, and an `AMM` object's account and assets. Each names
//!   the transaction that last changed it, where rippled threads it.
//! - **A transaction and its metadata** decode into [`TxRecord`], as the
//!   JSON the stream and `tx` give does: who sent it, what it paid, its
//!   result, what it did to accounts' XRP and to trust lines, and its
//!   first memo. What it delivered is the metadata's `DeliveredAmount`,
//!   else a successful Payment's `Amount` when it is not partial. Beside
//!   the record, what the account walks the history by: for each object it
//!   follows that the transaction changed, the one that changed it before,
//!   and the sender's sequence numbers it used up.
//!
//! Fields a reader does not need are left as they are; bytes that are not
//! canonical, or lack what a reader needs, are refused with an error, never
//! a panic. Nothing here is believed until a branch proves it.

use alloc::string::{String, ToString};
use alloc::vec::Vec;

use crate::amount::{Amount, Asset, Currency, TokenAmount, TokenValue};
use crate::api::{BalanceChange, LineChange, SignerList, TrustLine, TxRecord};
use crate::binary::{Field, FieldId, Fields, Type};
use crate::codec::{DecodeError, MAX_SIGNERS, SignerEntry, TF_PARTIAL_PAYMENT};
use crate::keys::{AccountId, sha512_half};

/// The transaction that last changed an object, and its ledger, as the
/// object or a transaction's metadata names them.
pub type Thread = ([u8; 32], u32);

// The kinds of ledger objects read (rippled's `ledger_entries.macro`).
const ACCOUNT_ROOT: u16 = 0x0061;
const SIGNER_LIST: u16 = 0x0053;
const RIPPLE_STATE: u16 = 0x0072;
const FEE_SETTINGS: u16 = 0x0073;
const AMM: u16 = 0x0079;

// The kinds of transactions told apart (`transactions.macro`).
const PAYMENT: u16 = 0;
const TICKET_CREATE: u16 = 10;

// The fields read (rippled's `sfields.macro`).
const TRANSACTION_RESULT: FieldId = FieldId::new(Type::UInt8, 3);
const LEDGER_ENTRY_TYPE: FieldId = FieldId::new(Type::UInt16, 1);
const TRANSACTION_TYPE: FieldId = FieldId::new(Type::UInt16, 2);
const SIGNER_WEIGHT: FieldId = FieldId::new(Type::UInt16, 3);
const FLAGS: FieldId = FieldId::new(Type::UInt32, 2);
const SEQUENCE: FieldId = FieldId::new(Type::UInt32, 4);
const PREVIOUS_TXN_LGR_SEQ: FieldId = FieldId::new(Type::UInt32, 5);
const TRANSFER_RATE: FieldId = FieldId::new(Type::UInt32, 11);
const OWNER_COUNT: FieldId = FieldId::new(Type::UInt32, 13);
const DESTINATION_TAG: FieldId = FieldId::new(Type::UInt32, 14);
const TRANSACTION_INDEX: FieldId = FieldId::new(Type::UInt32, 28);
const RESERVE_BASE: FieldId = FieldId::new(Type::UInt32, 31);
const RESERVE_INCREMENT: FieldId = FieldId::new(Type::UInt32, 32);
const SIGNER_QUORUM: FieldId = FieldId::new(Type::UInt32, 35);
const SIGNER_LIST_ID: FieldId = FieldId::new(Type::UInt32, 38);
const TICKET_COUNT: FieldId = FieldId::new(Type::UInt32, 40);
const BASE_FEE: FieldId = FieldId::new(Type::UInt64, 5);
const PREVIOUS_TXN_ID: FieldId = FieldId::new(Type::Hash256, 5);
const LEDGER_INDEX: FieldId = FieldId::new(Type::Hash256, 6);
const AMOUNT: FieldId = FieldId::new(Type::Amount, 1);
const BALANCE: FieldId = FieldId::new(Type::Amount, 2);
const LOW_LIMIT: FieldId = FieldId::new(Type::Amount, 6);
const HIGH_LIMIT: FieldId = FieldId::new(Type::Amount, 7);
const FEE: FieldId = FieldId::new(Type::Amount, 8);
const DELIVERED_AMOUNT: FieldId = FieldId::new(Type::Amount, 18);
const BASE_FEE_DROPS: FieldId = FieldId::new(Type::Amount, 22);
const RESERVE_BASE_DROPS: FieldId = FieldId::new(Type::Amount, 23);
const RESERVE_INCREMENT_DROPS: FieldId = FieldId::new(Type::Amount, 24);
const MEMO_TYPE: FieldId = FieldId::new(Type::Blob, 12);
const MEMO_DATA: FieldId = FieldId::new(Type::Blob, 13);
const ACCOUNT: FieldId = FieldId::new(Type::AccountId, 1);
const DESTINATION: FieldId = FieldId::new(Type::AccountId, 3);
const REGULAR_KEY: FieldId = FieldId::new(Type::AccountId, 8);
const ASSET: FieldId = FieldId::new(Type::Issue, 3);
const ASSET2: FieldId = FieldId::new(Type::Issue, 4);
const CREATED_NODE: FieldId = FieldId::new(Type::Object, 3);
const DELETED_NODE: FieldId = FieldId::new(Type::Object, 4);
const MODIFIED_NODE: FieldId = FieldId::new(Type::Object, 5);
const PREVIOUS_FIELDS: FieldId = FieldId::new(Type::Object, 6);
const FINAL_FIELDS: FieldId = FieldId::new(Type::Object, 7);
const NEW_FIELDS: FieldId = FieldId::new(Type::Object, 8);
const MEMO: FieldId = FieldId::new(Type::Object, 10);
const SIGNER_ENTRY: FieldId = FieldId::new(Type::Object, 11);
const SIGNER_ENTRIES: FieldId = FieldId::new(Type::Array, 4);
const AFFECTED_NODES: FieldId = FieldId::new(Type::Array, 8);
const MEMOS: FieldId = FieldId::new(Type::Array, 9);

/// The results a transaction in a validated ledger can have, by their
/// codes (rippled's `TER.h`): success, or a failure that claims the fee.
pub const RESULTS: [(&str, u8); 71] = [
    ("tesSUCCESS", 0),
    ("tecCLAIM", 100),
    ("tecPATH_PARTIAL", 101),
    ("tecUNFUNDED_ADD", 102),
    ("tecUNFUNDED_OFFER", 103),
    ("tecUNFUNDED_PAYMENT", 104),
    ("tecFAILED_PROCESSING", 105),
    ("tecDIR_FULL", 121),
    ("tecINSUF_RESERVE_LINE", 122),
    ("tecINSUF_RESERVE_OFFER", 123),
    ("tecNO_DST", 124),
    ("tecNO_DST_INSUF_XRP", 125),
    ("tecNO_LINE_INSUF_RESERVE", 126),
    ("tecNO_LINE_REDUNDANT", 127),
    ("tecPATH_DRY", 128),
    ("tecUNFUNDED", 129),
    ("tecNO_ALTERNATIVE_KEY", 130),
    ("tecNO_REGULAR_KEY", 131),
    ("tecOWNERS", 132),
    ("tecNO_ISSUER", 133),
    ("tecNO_AUTH", 134),
    ("tecNO_LINE", 135),
    ("tecINSUFF_FEE", 136),
    ("tecFROZEN", 137),
    ("tecNO_TARGET", 138),
    ("tecNO_PERMISSION", 139),
    ("tecNO_ENTRY", 140),
    ("tecINSUFFICIENT_RESERVE", 141),
    ("tecNEED_MASTER_KEY", 142),
    ("tecDST_TAG_NEEDED", 143),
    ("tecINTERNAL", 144),
    ("tecOVERSIZE", 145),
    ("tecCRYPTOCONDITION_ERROR", 146),
    ("tecINVARIANT_FAILED", 147),
    ("tecEXPIRED", 148),
    ("tecDUPLICATE", 149),
    ("tecKILLED", 150),
    ("tecHAS_OBLIGATIONS", 151),
    ("tecTOO_SOON", 152),
    ("tecHOOK_REJECTED", 153),
    ("tecMAX_SEQUENCE_REACHED", 154),
    ("tecNO_SUITABLE_NFTOKEN_PAGE", 155),
    ("tecNFTOKEN_BUY_SELL_MISMATCH", 156),
    ("tecNFTOKEN_OFFER_TYPE_MISMATCH", 157),
    ("tecCANT_ACCEPT_OWN_NFTOKEN_OFFER", 158),
    ("tecINSUFFICIENT_FUNDS", 159),
    ("tecOBJECT_NOT_FOUND", 160),
    ("tecINSUFFICIENT_PAYMENT", 161),
    ("tecUNFUNDED_AMM", 162),
    ("tecAMM_BALANCE", 163),
    ("tecAMM_FAILED", 164),
    ("tecAMM_INVALID_TOKENS", 165),
    ("tecAMM_EMPTY", 166),
    ("tecAMM_NOT_EMPTY", 167),
    ("tecAMM_ACCOUNT", 168),
    ("tecINCOMPLETE", 169),
    ("tecXCHAIN_BAD_TRANSFER_ISSUE", 170),
    ("tecXCHAIN_NO_CLAIM_ID", 171),
    ("tecXCHAIN_BAD_CLAIM_ID", 172),
    ("tecXCHAIN_CLAIM_NO_QUORUM", 173),
    ("tecXCHAIN_PROOF_UNKNOWN_KEY", 174),
    ("tecXCHAIN_CREATE_ACCOUNT_NONXRP_ISSUE", 175),
    ("tecXCHAIN_WRONG_CHAIN", 176),
    ("tecXCHAIN_REWARD_MISMATCH", 177),
    ("tecXCHAIN_NO_SIGNERS_LIST", 178),
    ("tecXCHAIN_SENDING_ACCOUNT_MISMATCH", 179),
    ("tecXCHAIN_INSUFF_CREATE_AMOUNT", 180),
    ("tecXCHAIN_ACCOUNT_CREATE_PAST", 181),
    ("tecXCHAIN_ACCOUNT_CREATE_TOO_MANY", 182),
    ("tecXCHAIN_PAYMENT_FAILED", 183),
    ("tecPRECISION_LOSS", 197),
];

/// The code of `result`, if it is one a validated ledger holds.
#[must_use]
pub fn result_code(result: &str) -> Option<u8> {
    (RESULTS.iter())
        .find(|(name, _)| *name == result)
        .map(|(_, code)| *code)
}

/// The name of result `code` in a validated ledger: success, or a failure
/// that claims the fee (`tec`), named by rippled, or by its number where the
/// table above has none. `None` for any other code, which no validated
/// ledger holds.
#[must_use]
pub fn result_name(code: u8) -> Option<String> {
    if let Some((name, _)) = RESULTS.iter().find(|(_, known)| *known == code) {
        return Some((*name).to_string());
    }
    (code >= 100).then(|| alloc::format!("tec{code}"))
}

/// A key in the state tree: SHA-512Half of its space and its parts.
fn key(space: u8, parts: &[&[u8]]) -> [u8; 32] {
    let mut message = alloc::vec![0, space];
    for part in parts {
        message.extend_from_slice(part);
    }
    sha512_half(&message)
}

/// The key of `account`'s root.
#[must_use]
pub fn account_key(account: AccountId) -> [u8; 32] {
    key(b'a', &[&account.0])
}

/// The key of the trust line between `a` and `b` in `currency`, either way
/// round: the lower account first.
#[must_use]
pub fn line_key(a: AccountId, b: AccountId, currency: Currency) -> [u8; 32] {
    let (low, high) = if a < b { (a, b) } else { (b, a) };
    key(b'r', &[&low.0, &high.0, &currency.0])
}

/// The key of `account`'s signer list: its ID is always 0, as rippled
/// keeps one list an account.
#[must_use]
pub fn signer_list_key(account: AccountId) -> [u8; 32] {
    key(b'S', &[&account.0, &0_u32.to_be_bytes()])
}

/// The key of the `AMM` object of the pool of XRP and the token `currency`
/// of `issuer`: its two issues, each its issuer and its currency, XRP's
/// first, as rippled sorts them.
#[must_use]
pub fn amm_key(issuer: AccountId, currency: Currency) -> [u8; 32] {
    key(b'A', &[&[0; 20], &[0; 20], &issuer.0, &currency.0])
}

/// The key of the fee settings.
#[must_use]
pub fn fees_key() -> [u8; 32] {
    key(b'e', &[])
}

/// The transaction an object names as the last that changed it, if it
/// names one: a zero hash names none, as the objects a ledger began with.
fn thread(txn: Option<[u8; 32]>, ledger: Option<u32>) -> Option<Thread> {
    match (txn, ledger) {
        (Some(txn), Some(ledger)) if txn != [0; 32] => Some((txn, ledger)),
        _ => None,
    }
}

/// An amount of XRP.
fn drops(field: &Field<'_>) -> Result<u64, DecodeError> {
    field.amount()?.drops().ok_or(DecodeError::Value)
}

/// An amount of a token.
fn token(field: &Field<'_>) -> Result<TokenAmount, DecodeError> {
    field.amount()?.token().copied().ok_or(DecodeError::Value)
}

/// Its kind, if `object` is a ledger object of kind `kind`.
fn of_kind(kind: Option<u16>, expected: u16) -> Result<(), DecodeError> {
    if kind == Some(expected) {
        Ok(())
    } else {
        Err(DecodeError::Value)
    }
}

/// An account's root, as the account reads it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Root {
    /// The account.
    pub account: AccountId,
    /// Its XRP, in drops.
    pub balance: u64,
    /// The sequence its next transaction uses.
    pub sequence: u32,
    /// The objects it owns.
    pub owner_count: u32,
    /// Its flags ([`crate::api::account_flags`]).
    pub flags: u32,
    /// Its transfer rate in billionths; 0 when it has none.
    pub transfer_rate: u32,
    /// Its regular key, as the account of the key.
    pub regular_key: Option<AccountId>,
    /// The transaction that last changed it.
    pub thread: Option<Thread>,
}

/// The account root in `object`.
///
/// # Errors
///
/// It is no account root, or lacks what one holds.
pub fn account_root(object: &[u8]) -> Result<Root, DecodeError> {
    let (mut kind, mut account, mut balance, mut sequence) = (None, None, None, None);
    let (mut owner_count, mut flags, mut transfer_rate, mut regular_key) = (0, 0, 0, None);
    let (mut txn, mut ledger) = (None, None);
    for field in Fields::new(object) {
        let field = field?;
        match field.id {
            LEDGER_ENTRY_TYPE => kind = Some(field.uint16()?),
            FLAGS => flags = field.uint32()?,
            SEQUENCE => sequence = Some(field.uint32()?),
            PREVIOUS_TXN_LGR_SEQ => ledger = Some(field.uint32()?),
            TRANSFER_RATE => transfer_rate = field.uint32()?,
            OWNER_COUNT => owner_count = field.uint32()?,
            PREVIOUS_TXN_ID => txn = Some(field.hash256()?),
            BALANCE => balance = Some(drops(&field)?),
            ACCOUNT => account = field.account()?,
            REGULAR_KEY => regular_key = field.account()?,
            _ => {}
        }
    }
    of_kind(kind, ACCOUNT_ROOT)?;
    Ok(Root {
        account: account.ok_or(DecodeError::Field)?,
        balance: balance.ok_or(DecodeError::Field)?,
        sequence: sequence.ok_or(DecodeError::Field)?,
        owner_count,
        flags,
        transfer_rate,
        regular_key,
        thread: thread(txn, ledger),
    })
}

/// What a `RippleState`'s fields say, each where it is written.
#[derive(Clone, Copy, Debug, Default)]
struct LineFields {
    kind: Option<u16>,
    balance: Option<TokenAmount>,
    low_limit: Option<TokenAmount>,
    high_limit: Option<TokenAmount>,
    flags: Option<u32>,
    txn: Option<[u8; 32]>,
    ledger: Option<u32>,
}

impl LineFields {
    fn read(fields: Fields<'_>) -> Result<Self, DecodeError> {
        let mut out = Self::default();
        for field in fields {
            let field = field?;
            match field.id {
                LEDGER_ENTRY_TYPE => out.kind = Some(field.uint16()?),
                FLAGS => out.flags = Some(field.uint32()?),
                PREVIOUS_TXN_LGR_SEQ => out.ledger = Some(field.uint32()?),
                PREVIOUS_TXN_ID => out.txn = Some(field.hash256()?),
                BALANCE => out.balance = Some(token(&field)?),
                LOW_LIMIT => out.low_limit = Some(token(&field)?),
                HIGH_LIMIT => out.high_limit = Some(token(&field)?),
                _ => {}
            }
        }
        Ok(out)
    }

    /// `self`, with what `previous` holds in its place.
    fn with(self, previous: &Self) -> Self {
        Self {
            kind: self.kind,
            balance: previous.balance.or(self.balance),
            low_limit: previous.low_limit.or(self.low_limit),
            high_limit: previous.high_limit.or(self.high_limit),
            flags: previous.flags.or(self.flags),
            txn: self.txn,
            ledger: self.ledger,
        }
    }

    /// The line: its two accounts from the limits' issuers, low below high,
    /// and one currency throughout. A line just created may leave out a
    /// zero balance and no flags, as metadata does.
    fn line(&self, created: bool) -> Option<TrustLine> {
        let low_limit = self.low_limit?;
        let high_limit = self.high_limit?;
        let balance = match self.balance {
            None if created => TokenValue::ZERO,
            balance => {
                let balance = balance?;
                // The balance's issuer is a placeholder: only its value and
                // currency say anything.
                if balance.currency != low_limit.currency {
                    return None;
                }
                balance.value
            }
        };
        let flags = match self.flags {
            None if created => 0,
            flags => flags?,
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
}

/// The trust line in `object`, and the transaction that last changed it.
///
/// # Errors
///
/// It is no trust line, or not one the ledger holds.
pub fn ripple_state(object: &[u8]) -> Result<(TrustLine, Option<Thread>), DecodeError> {
    let fields = LineFields::read(Fields::new(object))?;
    of_kind(fields.kind, RIPPLE_STATE)?;
    let line = fields.line(false).ok_or(DecodeError::Value)?;
    Ok((line, thread(fields.txn, fields.ledger)))
}

/// The signer list in `object`, and the transaction that last changed it:
/// its quorum, and its entries, at least one and at most [`MAX_SIGNERS`],
/// sorted by account and each once, as the ledger keeps them.
///
/// # Errors
///
/// It is no signer list, or not one the ledger holds.
pub fn signer_list(object: &[u8]) -> Result<(SignerList, Option<Thread>), DecodeError> {
    let (mut kind, mut quorum, mut list_id, mut entries) = (None, None, None, None);
    let (mut txn, mut ledger) = (None, None);
    for field in Fields::new(object) {
        let field = field?;
        match field.id {
            LEDGER_ENTRY_TYPE => kind = Some(field.uint16()?),
            PREVIOUS_TXN_LGR_SEQ => ledger = Some(field.uint32()?),
            SIGNER_QUORUM => quorum = Some(field.uint32()?),
            SIGNER_LIST_ID => list_id = Some(field.uint32()?),
            PREVIOUS_TXN_ID => txn = Some(field.hash256()?),
            SIGNER_ENTRIES => entries = Some(signer_entries(&field)?),
            _ => {}
        }
    }
    of_kind(kind, SIGNER_LIST)?;
    if list_id != Some(0) {
        return Err(DecodeError::Value);
    }
    let list = SignerList {
        quorum: quorum.ok_or(DecodeError::Field)?,
        entries: entries.ok_or(DecodeError::Field)?,
    };
    Ok((list, thread(txn, ledger)))
}

/// A signer list's entries: at least one and at most [`MAX_SIGNERS`], each
/// an account and its weight, sorted by account and each once.
fn signer_entries(field: &Field<'_>) -> Result<Vec<SignerEntry>, DecodeError> {
    let mut entries: Vec<SignerEntry> = Vec::new();
    for element in field.elements()? {
        let element = element?;
        if element.id != SIGNER_ENTRY || entries.len() >= MAX_SIGNERS {
            return Err(DecodeError::Value);
        }
        let (mut account, mut weight) = (None, None);
        for field in element.fields()? {
            let field = field?;
            match field.id {
                ACCOUNT => account = field.account()?,
                SIGNER_WEIGHT => weight = Some(field.uint16()?),
                _ => {}
            }
        }
        let entry = SignerEntry {
            account: account.ok_or(DecodeError::Field)?,
            weight: weight.ok_or(DecodeError::Field)?,
        };
        if entries
            .last()
            .is_some_and(|last| last.account >= entry.account)
        {
            return Err(DecodeError::Value);
        }
        entries.push(entry);
    }
    if entries.is_empty() {
        return Err(DecodeError::Field);
    }
    Ok(entries)
}

/// The fee settings: the base fee, and the reserve, in drops.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Fees {
    /// The base fee.
    pub base_fee: u64,
    /// What every account holds back.
    pub reserve_base: u64,
    /// What each object it owns holds back more.
    pub reserve_inc: u64,
}

/// The fee settings in `object`, in either of rippled's forms: amounts of
/// XRP since its `XRPFees` amendment, or the numbers before it.
///
/// # Errors
///
/// It is no fee settings, or holds neither form whole.
pub fn fee_settings(object: &[u8]) -> Result<Fees, DecodeError> {
    let mut kind = None;
    let (mut base_fee, mut reserve_base, mut reserve_inc) = (None, None, None);
    let (mut old_fee, mut old_base, mut old_inc) = (None, None, None);
    for field in Fields::new(object) {
        let field = field?;
        match field.id {
            LEDGER_ENTRY_TYPE => kind = Some(field.uint16()?),
            BASE_FEE_DROPS => base_fee = Some(drops(&field)?),
            RESERVE_BASE_DROPS => reserve_base = Some(drops(&field)?),
            RESERVE_INCREMENT_DROPS => reserve_inc = Some(drops(&field)?),
            BASE_FEE => old_fee = Some(field.uint64()?),
            RESERVE_BASE => old_base = Some(u64::from(field.uint32()?)),
            RESERVE_INCREMENT => old_inc = Some(u64::from(field.uint32()?)),
            _ => {}
        }
    }
    of_kind(kind, FEE_SETTINGS)?;
    match (base_fee, reserve_base, reserve_inc) {
        (Some(base_fee), Some(reserve_base), Some(reserve_inc)) => Ok(Fees {
            base_fee,
            reserve_base,
            reserve_inc,
        }),
        _ => Ok(Fees {
            base_fee: old_fee.ok_or(DecodeError::Field)?,
            reserve_base: old_base.ok_or(DecodeError::Field)?,
            reserve_inc: old_inc.ok_or(DecodeError::Field)?,
        }),
    }
}

/// An `AMM` object: the pool's account, which holds both its assets, and
/// the two.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AmmObject {
    /// The pool's account.
    pub account: AccountId,
    /// Its first asset.
    pub asset: Asset,
    /// Its second.
    pub asset2: Asset,
}

impl AmmObject {
    /// Whether it is the pool of XRP and the token `currency` of `issuer`,
    /// either way round.
    #[must_use]
    pub fn pairs(&self, issuer: AccountId, currency: Currency) -> bool {
        let token = Asset::Token { currency, issuer };
        (self.asset, self.asset2) == (Asset::Xrp, token)
            || (self.asset, self.asset2) == (token, Asset::Xrp)
    }
}

/// An issue, as an `Issue` field holds it: XRP, or a currency and its
/// issuer.
fn issue(field: &Field<'_>) -> Result<Asset, DecodeError> {
    if field.id.kind() != Type::Issue {
        return Err(DecodeError::Value);
    }
    if field.value.len() == 20 {
        return Ok(Asset::Xrp);
    }
    let (currency, issuer) = field.value.split_at_checked(20).ok_or(DecodeError::Value)?;
    Ok(Asset::Token {
        currency: Currency(currency.try_into().map_err(|_| DecodeError::Value)?),
        issuer: AccountId(issuer.try_into().map_err(|_| DecodeError::Value)?),
    })
}

/// The `AMM` object in `object`.
///
/// # Errors
///
/// It is no `AMM` object, or lacks its account or assets.
pub fn amm(object: &[u8]) -> Result<AmmObject, DecodeError> {
    let (mut kind, mut account, mut asset, mut asset2) = (None, None, None, None);
    for field in Fields::new(object) {
        let field = field?;
        match field.id {
            LEDGER_ENTRY_TYPE => kind = Some(field.uint16()?),
            ACCOUNT => account = field.account()?,
            ASSET => asset = Some(issue(&field)?),
            ASSET2 => asset2 = Some(issue(&field)?),
            _ => {}
        }
    }
    of_kind(kind, AMM)?;
    Ok(AmmObject {
        account: account.ok_or(DecodeError::Field)?,
        asset: asset.ok_or(DecodeError::Field)?,
        asset2: asset2.ok_or(DecodeError::Field)?,
    })
}

/// A transaction proven with its metadata, as the account reads it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Decoded {
    /// The record the schemes read.
    pub record: TxRecord,
    /// For each object of those asked for that it changed, by key, the
    /// transaction that changed it before: none if it made the object, or
    /// none did.
    pub threads: Vec<([u8; 32], Option<Thread>)>,
    /// The sender's sequence numbers it used up: the first, and how many.
    /// None, `(0, 0)`, for one that used a ticket; a `TicketCreate` that
    /// succeeded uses its own and those of the tickets it made, and one
    /// that failed only its own, as any failure that claims the fee does.
    pub uses: (u32, u32),
}

/// What a transaction's own fields say.
#[derive(Clone, Debug, Default)]
struct TxFields {
    kind: Option<u16>,
    flags: u32,
    sequence: Option<u32>,
    destination_tag: Option<u32>,
    ticket_count: u32,
    amount: Option<Amount>,
    fee: Option<u64>,
    account: Option<AccountId>,
    destination: Option<AccountId>,
    memo: Option<MemoParts>,
}

impl TxFields {
    fn read(tx: &[u8]) -> Result<Self, DecodeError> {
        let mut out = Self::default();
        for field in Fields::new(tx) {
            let field = field?;
            match field.id {
                TRANSACTION_TYPE => out.kind = Some(field.uint16()?),
                FLAGS => out.flags = field.uint32()?,
                SEQUENCE => out.sequence = Some(field.uint32()?),
                DESTINATION_TAG => out.destination_tag = Some(field.uint32()?),
                TICKET_COUNT => out.ticket_count = field.uint32()?,
                // An amount of another kind, such as an MPT's, is none the
                // account books.
                AMOUNT => out.amount = field.amount().ok(),
                FEE => out.fee = Some(drops(&field)?),
                ACCOUNT => out.account = field.account()?,
                DESTINATION => out.destination = field.account()?,
                MEMOS => out.memo = first_memo(&field)?,
                _ => {}
            }
        }
        Ok(out)
    }
}

/// A memo's type and data.
type MemoParts = (Vec<u8>, Vec<u8>);

/// The first memo's type and data, each empty if it has none.
fn first_memo(memos: &Field<'_>) -> Result<Option<MemoParts>, DecodeError> {
    let Some(memo) = memos.elements()?.next().transpose()? else {
        return Ok(None);
    };
    if memo.id != MEMO {
        return Err(DecodeError::Field);
    }
    let (mut kind, mut data) = (Vec::new(), Vec::new());
    for field in memo.fields()? {
        let field = field?;
        match field.id {
            MEMO_TYPE => kind = field.blob()?.to_vec(),
            MEMO_DATA => data = field.blob()?.to_vec(),
            _ => {}
        }
    }
    Ok(Some((kind, data)))
}

/// What a transaction's metadata says.
#[derive(Clone, Debug, Default)]
struct Meta {
    index: Option<u32>,
    result: Option<u8>,
    delivered: Option<Amount>,
    changes: Vec<BalanceChange>,
    lines: Vec<LineChange>,
    threads: Vec<([u8; 32], Option<Thread>)>,
}

/// An affected node's parts: its kind, key and thread, and its fields.
#[derive(Clone, Debug, Default)]
struct Node<'a> {
    kind: Option<u16>,
    key: Option<[u8; 32]>,
    txn: Option<[u8; 32]>,
    ledger: Option<u32>,
    previous: Option<Fields<'a>>,
    finals: Option<Fields<'a>>,
    news: Option<Fields<'a>>,
}

impl<'a> Node<'a> {
    fn read(node: &Field<'a>) -> Result<Self, DecodeError> {
        let mut out = Self::default();
        for field in node.fields()? {
            let field = field?;
            match field.id {
                LEDGER_ENTRY_TYPE => out.kind = Some(field.uint16()?),
                PREVIOUS_TXN_LGR_SEQ => out.ledger = Some(field.uint32()?),
                PREVIOUS_TXN_ID => out.txn = Some(field.hash256()?),
                LEDGER_INDEX => out.key = Some(field.hash256()?),
                PREVIOUS_FIELDS => out.previous = Some(field.fields()?),
                FINAL_FIELDS => out.finals = Some(field.fields()?),
                NEW_FIELDS => out.news = Some(field.fields()?),
                _ => {}
            }
        }
        Ok(out)
    }
}

/// The thread a deleted object's final fields name.
fn finals_thread(
    fields: Option<Fields<'_>>,
) -> Result<(Option<[u8; 32]>, Option<u32>), DecodeError> {
    let (mut txn, mut ledger) = (None, None);
    for field in fields.into_iter().flatten() {
        let field = field?;
        match field.id {
            PREVIOUS_TXN_LGR_SEQ => ledger = Some(field.uint32()?),
            PREVIOUS_TXN_ID => txn = Some(field.hash256()?),
            _ => {}
        }
    }
    Ok((txn, ledger))
}

/// What one root's fields say of its XRP.
#[derive(Clone, Copy, Debug, Default)]
struct RootFields {
    account: Option<AccountId>,
    balance: Option<u64>,
    owner_count: Option<u32>,
}

impl RootFields {
    fn read(fields: Option<Fields<'_>>) -> Result<Self, DecodeError> {
        let mut out = Self::default();
        for field in fields.into_iter().flatten() {
            let field = field?;
            match field.id {
                ACCOUNT => out.account = field.account()?,
                BALANCE => out.balance = Some(drops(&field)?),
                OWNER_COUNT => out.owner_count = Some(field.uint32()?),
                _ => {}
            }
        }
        Ok(out)
    }
}

impl Meta {
    fn read(meta: &[u8], keys: &[[u8; 32]]) -> Result<Self, DecodeError> {
        let mut out = Self::default();
        for field in Fields::new(meta) {
            let field = field?;
            match field.id {
                TRANSACTION_RESULT => out.result = Some(field.uint8()?),
                TRANSACTION_INDEX => out.index = Some(field.uint32()?),
                DELIVERED_AMOUNT => out.delivered = field.amount().ok(),
                AFFECTED_NODES => {
                    for node in field.elements()? {
                        out.node(&node?, keys)?;
                    }
                }
                _ => {}
            }
        }
        Ok(out)
    }

    /// One affected node: what it did to an account's XRP or a line, and
    /// the thread of an object of `keys`.
    fn node(&mut self, node: &Field<'_>, keys: &[[u8; 32]]) -> Result<(), DecodeError> {
        let parts = Node::read(node)?;
        let created = node.id == CREATED_NODE;
        let deleted = node.id == DELETED_NODE;
        if !created && !deleted && node.id != MODIFIED_NODE {
            return Err(DecodeError::Field);
        }
        if let Some(key) = parts.key.filter(|key| keys.contains(key)) {
            let previous = if deleted {
                let (txn, ledger) = finals_thread(parts.finals.clone())?;
                thread(txn, ledger)
            } else {
                thread(parts.txn, parts.ledger)
            };
            self.threads.push((key, previous.filter(|_| !created)));
        }
        match parts.kind {
            Some(ACCOUNT_ROOT) => self.root(&parts, created, deleted)?,
            Some(RIPPLE_STATE) => self.line(&parts, created, deleted)?,
            _ => {}
        }
        Ok(())
    }

    /// What a root's node did to its XRP, as the JSON's `AffectedNodes`
    /// say it: a root without its balance, such as one the transaction
    /// only threaded, is left out. A deleted root's final balance is the
    /// zero the ledger left it with: what it held before is in its
    /// previous fields, as a modified root's is.
    fn root(&mut self, parts: &Node<'_>, created: bool, deleted: bool) -> Result<(), DecodeError> {
        let fields = if created { &parts.news } else { &parts.finals };
        let fields = RootFields::read(fields.clone())?;
        let (Some(account), Some(balance)) = (fields.account, fields.balance) else {
            return Ok(());
        };
        let (before, after) = if created {
            (0, balance)
        } else {
            let previous = RootFields::read(parts.previous.clone())?;
            let after = if deleted { 0 } else { balance };
            (previous.balance.unwrap_or(balance), after)
        };
        self.changes.push(BalanceChange {
            account,
            before,
            after,
            owner_count: fields.owner_count,
        });
        Ok(())
    }

    /// What a line's node did to it. A line whose fields cannot be read
    /// is left out, as the JSON's are.
    fn line(&mut self, parts: &Node<'_>, created: bool, deleted: bool) -> Result<(), DecodeError> {
        let read = |fields: &Option<Fields<'_>>| match fields {
            Some(fields) => LineFields::read(fields.clone()),
            None => Ok(LineFields::default()),
        };
        let finals = read(&parts.finals)?;
        let previous = read(&parts.previous)?;
        let before = || finals.with(&previous).line(false);
        let change = if created {
            read(&parts.news)?
                .line(true)
                .and_then(|line| LineChange::new(None, Some(line)))
        } else if deleted {
            before().and_then(|before| LineChange::new(Some(before), None))
        } else {
            match (before(), finals.line(false)) {
                (Some(before), Some(after)) => LineChange::new(Some(before), Some(after)),
                _ => None,
            }
        };
        self.lines.extend(change);
        Ok(())
    }
}

/// `tx`, a transaction in binary, with `meta`, its metadata, as a record
/// of ledger `ledger_index`, its hash computed from its bytes. Threads are
/// kept for the objects of `keys` alone.
///
/// # Errors
///
/// Either is not canonical, or lacks what a transaction in a ledger holds.
pub fn transaction(
    tx: &[u8],
    meta: &[u8],
    ledger_index: u32,
    keys: &[[u8; 32]],
) -> Result<Decoded, DecodeError> {
    let fields = TxFields::read(tx)?;
    let meta = Meta::read(meta, keys)?;
    let kind = fields.kind.ok_or(DecodeError::Field)?;
    let sequence = fields.sequence.ok_or(DecodeError::Field)?;
    let result = result_name(meta.result.ok_or(DecodeError::Field)?).ok_or(DecodeError::Value)?;
    let is_payment = kind == PAYMENT;
    // What it delivered: the metadata's, else the whole amount of a
    // Payment that succeeded and was not partial.
    let delivered = meta.delivered.or_else(|| {
        let whole = is_payment && result == "tesSUCCESS" && fields.flags & TF_PARTIAL_PAYMENT == 0;
        fields.amount.filter(|_| whole)
    });
    // A `TicketCreate` makes its tickets only if it succeeds: one that
    // failed used up its own number alone, and the numbers after it are
    // still free for a transaction of ours.
    let uses = match (sequence, kind) {
        (0, _) => (0, 0),
        (first, TICKET_CREATE) if result == "tesSUCCESS" => {
            (first, fields.ticket_count.saturating_add(1))
        }
        (first, _) => (first, 1),
    };
    let record = TxRecord {
        hash: crate::codec::hash(tx),
        ledger_index,
        index: meta.index.ok_or(DecodeError::Field)?,
        validated: true,
        account: fields.account.ok_or(DecodeError::Field)?,
        is_payment,
        destination: fields.destination,
        destination_tag: fields.destination_tag,
        sequence,
        fee: fields.fee.ok_or(DecodeError::Field)?,
        result,
        delivered: delivered.and_then(|amount| amount.drops()),
        delivered_token: delivered.and_then(|amount| amount.token().copied()),
        memo: fields.memo,
        changes: meta.changes,
        lines: meta.lines,
    };
    Ok(Decoded {
        record,
        threads: meta.threads,
        uses,
    })
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::indexing_slicing)]
mod tests {
    //! The readers against what mainnet holds
    //! (spec/designs/xrpl-validation.md#13-what-pins-it).

    use super::*;
    use crate::from_hex;
    use serde_json::Value;

    fn data(name: &str) -> Value {
        let text = match name {
            "objects" => include_str!("../tests/data/ledger-objects.json"),
            "set" => include_str!("../tests/data/transaction-set.json"),
            _ => include_str!("../tests/data/rlusd-and-pools.json"),
        };
        serde_json::from_str(text).unwrap()
    }

    fn hex(value: &Value) -> Vec<u8> {
        from_hex(value.as_str().unwrap()).unwrap()
    }

    fn labelled<'a>(list: &'a Value, label: &str) -> &'a Value {
        (list.as_array().unwrap().iter())
            .find(|object| object["label"] == label)
            .unwrap()
    }

    // Mainnet's fee settings, in drops since its `XRPFees` amendment, under
    // the key rippled gives them.
    #[test]
    fn mainnet_s_fee_settings_read_as_rippled_wrote_them() {
        let objects = data("objects");
        let fees = labelled(&objects["objects"], "FeeSettings");
        assert_eq!(
            crate::hex_upper(&fees_key()),
            fees["index"].as_str().unwrap()
        );
        let read = fee_settings(&hex(&fees["node_binary"])).unwrap();
        assert_eq!(
            read,
            Fees {
                base_fee: 10,
                reserve_base: 1_000_000,
                reserve_inc: 200_000
            }
        );
        let recent = labelled(&objects["objects"], "LedgerHashes (recent)");
        assert_eq!(
            fee_settings(&hex(&recent["node_binary"])),
            Err(DecodeError::Value)
        );
    }

    // The XRP/RLUSD pool as mainnet holds it: its AMM object names its
    // account and its assets under the key of its pair; the account's root
    // holds the pool's XRP and its line to the issuer the pool's RLUSD, as
    // `amm_info` gave them, each threaded to the last transaction that
    // changed it.
    #[test]
    fn mainnet_s_pool_reads_as_amm_info_gave_it() {
        let pools = data("pools");
        let info = crate::api::amm_info_result(&pools["amm_info"]).unwrap();
        let rlusd = crate::rlusd::TOKEN;
        let object = amm(&hex(&pools["amm_object"]["node_binary"])).unwrap();
        assert_eq!(object.account, info.account);
        assert!(object.pairs(rlusd.issuer, rlusd.currency));
        assert_eq!(
            crate::hex_upper(&amm_key(rlusd.issuer, rlusd.currency)),
            pools["amm_object"]["index"].as_str().unwrap()
        );
        let root = account_root(&hex(&pools["amm_account_root"]["node_binary"])).unwrap();
        assert_eq!(root.account, info.account);
        assert_eq!(Amount::Xrp(root.balance), info.amount);
        assert!(root.thread.is_some());
        let (line, thread) = ripple_state(&hex(&pools["amm_rlusd_line"]["node_binary"])).unwrap();
        assert!(line.is_between(info.account, rlusd.issuer, rlusd.currency));
        assert_eq!(
            Some(line.view(info.account).unwrap().balance),
            info.amount2.token().map(|token| token.value)
        );
        assert!(thread.is_some());
        assert_eq!(
            crate::hex_upper(&line_key(rlusd.issuer, info.account, rlusd.currency)),
            pools["amm_rlusd_line"]["index"].as_str().unwrap()
        );
        assert_eq!(
            account_root(&hex(&pools["amm_object"]["node_binary"])).map(|_| ()),
            Err(DecodeError::Value)
        );
    }

    // Every transaction of a whole mainnet ledger decodes, with the result,
    // index and fee its metadata and fields hold; and the one that last
    // changed the pool's root and line names, for each, the transaction
    // before, and leaves them as the ledger holds them.
    #[test]
    fn a_mainnet_ledger_s_transactions_decode() {
        let set = data("set");
        let ledger = u32::try_from(set["ledger_index"].as_u64().unwrap()).unwrap();
        let mut indexes = Vec::new();
        for entry in set["transactions"].as_array().unwrap() {
            let decoded = transaction(&hex(&entry["tx_blob"]), &hex(&entry["meta"]), ledger, &[]);
            let record = decoded.unwrap().record;
            assert_eq!(record.ledger_index, ledger);
            assert!(record.result.starts_with("tes") || record.result.starts_with("tec"));
            indexes.push(record.index);
        }
        indexes.sort_unstable();
        let count = u32::try_from(indexes.len()).unwrap();
        assert_eq!(indexes, (0..count).collect::<Vec<_>>(), "each index once");
        let pools = data("pools");
        let root = account_root(&hex(&pools["amm_account_root"]["node_binary"])).unwrap();
        let (line, line_thread) =
            ripple_state(&hex(&pools["amm_rlusd_line"]["node_binary"])).unwrap();
        let branch = &set["other_branches"][0];
        let (txn, at) = root.thread.unwrap();
        assert_eq!(crate::hex_upper(&txn), branch["key"].as_str().unwrap());
        assert_eq!(Some((txn, at)), line_thread);
        let keys = [
            account_key(root.account),
            line_key(line.low, line.high, line.currency),
        ];
        let decoded =
            transaction(&hex(&branch["tx_blob"]), &hex(&branch["meta"]), at, &keys).unwrap();
        assert_eq!(decoded.record.hash, txn);
        assert_eq!(decoded.threads.len(), 2, "{:?}", decoded.threads);
        assert!(decoded.threads.iter().all(|(_, before)| before.is_some()));
        let change = decoded.record.change_of(root.account).unwrap();
        assert_eq!(change.after, root.balance);
        let changed = decoded
            .record
            .line(line.low, line.high, line.currency)
            .unwrap();
        assert_eq!(changed.after(), Some(&line));
    }

    // A `TicketCreate` uses up the numbers of the tickets it makes only if
    // it succeeds: one that failed with a fee claimed (as for want of the
    // reserve) used up its own number alone, so a transaction of ours with
    // the next is not taken for one it made impossible.
    #[test]
    fn a_failed_ticket_create_uses_up_its_own_number_alone() {
        use crate::binary::{write_field, xrp};

        let account = crate::keys::Wallet::from_entropy(&[1; 32]).account();
        let mut tx = Vec::new();
        write_field(&mut tx, TRANSACTION_TYPE, &TICKET_CREATE.to_be_bytes());
        write_field(&mut tx, FLAGS, &0u32.to_be_bytes());
        write_field(&mut tx, SEQUENCE, &7u32.to_be_bytes());
        write_field(&mut tx, TICKET_COUNT, &3u32.to_be_bytes());
        write_field(&mut tx, FEE, &xrp(12));
        write_field(&mut tx, ACCOUNT, &account.0);
        let meta = |result: &str| {
            let mut meta = Vec::new();
            write_field(&mut meta, TRANSACTION_INDEX, &0u32.to_be_bytes());
            write_field(&mut meta, AFFECTED_NODES, &[]);
            write_field(
                &mut meta,
                TRANSACTION_RESULT,
                &[result_code(result).unwrap()],
            );
            meta
        };
        let uses = |result| transaction(&tx, &meta(result), 20, &[]).unwrap().uses;
        assert_eq!(uses("tesSUCCESS"), (7, 4));
        assert_eq!(uses("tecINSUFFICIENT_RESERVE"), (7, 1));
        assert_eq!(uses("tecDIR_FULL"), (7, 1));
    }

    #[test]
    fn results_are_named_as_rippled_names_them() {
        assert_eq!(result_name(0).as_deref(), Some("tesSUCCESS"));
        assert_eq!(result_name(125).as_deref(), Some("tecNO_DST_INSUF_XRP"));
        assert_eq!(result_name(250).as_deref(), Some("tec250"));
        assert_eq!(result_name(1), None);
        assert_eq!(result_code("tecPATH_DRY"), Some(128));
        for (name, code) in RESULTS {
            assert_eq!(result_name(code).as_deref(), Some(name));
        }
    }
}
