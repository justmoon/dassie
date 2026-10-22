//! The binary form of the transactions the node signs: a `Payment` of XRP
//! or of a token, with memos, and a `TrustSet` that opens or changes a
//! line to a token's issuer. Fields go in the ledger's canonical order, by
//! type code and then field code; the decoder takes only that order and
//! only these fields, which is all the fake ledger needs to check what it
//! is sent. A Payment has no paths and is never partial. It may name a
//! `SendMax`, the most it spends of another asset than it delivers, which
//! the ledger then converts through its default paths, such as an AMM
//! pool ([`sign_send_max`]).

use alloc::vec::Vec;

use sha2::{Digest, Sha512};

pub use crate::amount::{Amount, Currency, TokenAmount, TokenValue};
use crate::keys::{AccountId, Wallet};
use crate::{MAX_DROPS, hex_upper};

/// Most memos on one Payment, and most bytes in a memo's field.
pub const MAX_MEMOS: usize = 4;
/// Most bytes in a memo's type or data.
pub const MAX_MEMO_BYTES: usize = 256;

/// What signing covers, before the fields.
const SIGNING_PREFIX: [u8; 4] = *b"STX\0";
/// What a transaction's hash covers, before its blob.
const HASH_PREFIX: [u8; 4] = *b"TXN\0";
/// The `Payment` transaction type.
const PAYMENT: u16 = 0;
/// The `TrustSet` transaction type.
const TRUST_SET: u16 = 20;

/// A Payment's flag that lets it deliver less than its amount; nothing
/// here sets it, and the decoder refuses it.
pub const TF_PARTIAL_PAYMENT: u32 = 0x0002_0000;
/// A TrustSet's flag that disables rippling on the sender's side.
pub const TF_SET_NO_RIPPLE: u32 = 0x0002_0000;
/// A TrustSet's flag that enables rippling on the sender's side.
pub const TF_CLEAR_NO_RIPPLE: u32 = 0x0004_0000;
/// A TrustSet's flag that freezes the line on the sender's side: the
/// other side pays its balance only to the sender.
pub const TF_SET_FREEZE: u32 = 0x0010_0000;
/// A TrustSet's flag that lifts the sender's freeze.
pub const TF_CLEAR_FREEZE: u32 = 0x0020_0000;
/// A TrustSet's flag that deep-freezes the line on the sender's side: the
/// other side neither pays nor receives, but to and from the sender. Only
/// on a line frozen already, or with [`TF_SET_FREEZE`].
pub const TF_SET_DEEP_FREEZE: u32 = 0x0040_0000;
/// A TrustSet's flag that lifts the sender's deep freeze.
pub const TF_CLEAR_DEEP_FREEZE: u32 = 0x0080_0000;

// Type codes.
const UINT16: u8 = 1;
const UINT32: u8 = 2;
const AMOUNT: u8 = 6;
const BLOB: u8 = 7;
const ACCOUNT_ID: u8 = 8;
const OBJECT: u8 = 14;
const ARRAY: u8 = 15;

// Field codes, by type.
const TRANSACTION_TYPE: (u8, u8) = (UINT16, 2);
const NETWORK_ID: (u8, u8) = (UINT32, 1);
const FLAGS: (u8, u8) = (UINT32, 2);
const SEQUENCE: (u8, u8) = (UINT32, 4);
const LAST_LEDGER_SEQUENCE: (u8, u8) = (UINT32, 27);
const AMOUNT_FIELD: (u8, u8) = (AMOUNT, 1);
const LIMIT_AMOUNT: (u8, u8) = (AMOUNT, 3);
const FEE: (u8, u8) = (AMOUNT, 8);
const SEND_MAX: (u8, u8) = (AMOUNT, 9);
const SIGNING_PUB_KEY: (u8, u8) = (BLOB, 3);
const TXN_SIGNATURE: (u8, u8) = (BLOB, 4);
const MEMO_TYPE: (u8, u8) = (BLOB, 12);
const MEMO_DATA: (u8, u8) = (BLOB, 13);
const ACCOUNT: (u8, u8) = (ACCOUNT_ID, 1);
const DESTINATION: (u8, u8) = (ACCOUNT_ID, 3);
const OBJECT_END: (u8, u8) = (OBJECT, 1);
const MEMO: (u8, u8) = (OBJECT, 10);
const ARRAY_END: (u8, u8) = (ARRAY, 1);
const MEMOS: (u8, u8) = (ARRAY, 9);

/// A memo: a type and data, both bytes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Memo {
    /// What kind of memo it is.
    pub memo_type: Vec<u8>,
    /// What it says.
    pub memo_data: Vec<u8>,
}

/// A Payment of XRP or of a token: no paths, never partial.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Payment {
    /// Who pays.
    pub account: AccountId,
    /// Who is paid.
    pub destination: AccountId,
    /// How much it delivers, all of it or nothing: drops, or a token
    /// amount not below zero.
    pub amount: Amount,
    /// The most it spends, through the ledger's default paths: of another
    /// asset than `amount`, which the ledger converts, or of the same
    /// token, to pay its issuer's transfer fee. Never XRP for XRP. `None`
    /// spends `amount` itself, as the settlement schemes' Payments do.
    pub send_max: Option<Amount>,
    /// The fee, in drops.
    pub fee: u64,
    /// The payer's sequence number.
    pub sequence: u32,
    /// The last ledger the transaction may be in.
    pub last_ledger_sequence: u32,
    /// Flags; 0 for the node's, and never [`TF_PARTIAL_PAYMENT`].
    pub flags: u32,
    /// The network, required above 1024 and left out below.
    pub network_id: Option<u32>,
    /// Memos.
    pub memos: Vec<Memo>,
    /// The signer's public key.
    pub signing_pub_key: [u8; 33],
    /// The signature, once signed.
    pub signature: Option<[u8; 64]>,
}

/// A TrustSet: opens or changes the sender's line to a token's issuer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TrustSet {
    /// Whose line it is.
    pub account: AccountId,
    /// The line: the issuer, the currency, and as value the most the
    /// sender holds of it, not below zero.
    pub limit: TokenAmount,
    /// The fee, in drops.
    pub fee: u64,
    /// The sender's sequence number.
    pub sequence: u32,
    /// The last ledger the transaction may be in.
    pub last_ledger_sequence: u32,
    /// Flags, such as [`TF_SET_NO_RIPPLE`].
    pub flags: u32,
    /// The network, required above 1024 and left out below.
    pub network_id: Option<u32>,
    /// Memos.
    pub memos: Vec<Memo>,
    /// The signer's public key.
    pub signing_pub_key: [u8; 33],
    /// The signature, once signed.
    pub signature: Option<[u8; 64]>,
}

/// A transaction the codec reads: either kind.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Transaction {
    /// A Payment.
    Payment(Payment),
    /// A TrustSet.
    TrustSet(TrustSet),
}

/// A signed transaction, ready to submit.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Signed<T = Payment> {
    /// The transaction.
    pub tx: T,
    /// Its binary form.
    pub blob: Vec<u8>,
    /// Its hash.
    pub hash: [u8; 32],
}

impl<T> Signed<T> {
    /// The hash as the ledger writes it, in uppercase hex.
    #[must_use]
    pub fn hash_hex(&self) -> alloc::string::String {
        hex_upper(&self.hash)
    }
}

/// Why bytes are not a Payment the node would send.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DecodeError {
    /// It ended early.
    Truncated,
    /// A field is missing, unknown or out of order.
    Field,
    /// A value is out of range, such as an amount.
    Value,
    /// Bytes are left over.
    Trailing,
}

/// The hash of a signed transaction's blob: SHA-512Half of `TXN\0‖blob`.
#[must_use]
pub fn hash(blob: &[u8]) -> [u8; 32] {
    let mut hasher = Sha512::new();
    hasher.update(HASH_PREFIX);
    hasher.update(blob);
    let digest = hasher.finalize();
    let mut out = [0; 32];
    out.copy_from_slice(digest.get(..32).unwrap_or(&[0; 32]));
    out
}

/// What signing, writing and reading share between the kinds.
trait Fields: Sized {
    /// The fields in canonical order; the signature only `with_signature`.
    fn encode(&self, out: &mut Vec<u8>, with_signature: bool);
    /// Who signs, and with which key.
    fn signer(&self) -> (AccountId, [u8; 33]);
    /// Signs it.
    fn set_signature(&mut self, signature: [u8; 64]);
}

/// What the sender signs: `STX\0` and the fields, signature left out.
fn signing_payload(tx: &impl Fields) -> Vec<u8> {
    let mut out = Vec::with_capacity(256);
    out.extend_from_slice(&SIGNING_PREFIX);
    tx.encode(&mut out, false);
    out
}

fn sign<T: Fields>(mut tx: T, wallet: &Wallet) -> Signed<T> {
    let (account, key) = tx.signer();
    assert_eq!(key, *wallet.public_key(), "signed by its sender");
    assert_eq!(account, wallet.account(), "signed by its sender");
    tx.set_signature(wallet.sign(&signing_payload(&tx)));
    let mut blob = Vec::with_capacity(256);
    tx.encode(&mut blob, true);
    Signed {
        hash: hash(&blob),
        tx,
        blob,
    }
}

fn blob_of(tx: &impl Fields) -> Vec<u8> {
    let mut out = Vec::with_capacity(256);
    tx.encode(&mut out, true);
    out
}

/// The fields both kinds have, borrowed to be written.
struct Common<'a> {
    kind: u16,
    network_id: Option<u32>,
    flags: u32,
    sequence: u32,
    last_ledger_sequence: u32,
    fee: u64,
    signing_pub_key: &'a [u8; 33],
    signature: Option<&'a [u8; 64]>,
    account: AccountId,
    memos: &'a [Memo],
}

impl Common<'_> {
    /// Writes the fields, with the kind's one amount, its `SendMax` and
    /// its destination, if it has them, in their places.
    fn encode(
        &self,
        out: &mut Vec<u8>,
        (amount_field, amount): ((u8, u8), &Amount),
        send_max: Option<&Amount>,
        destination: Option<AccountId>,
        with_signature: bool,
    ) {
        assert!(self.fee <= MAX_DROPS, "fees within the ledger's");
        assert!(self.memos.len() <= MAX_MEMOS, "few memos");
        field(out, TRANSACTION_TYPE);
        out.extend_from_slice(&self.kind.to_be_bytes());
        if let Some(network) = self.network_id {
            field(out, NETWORK_ID);
            out.extend_from_slice(&network.to_be_bytes());
        }
        field(out, FLAGS);
        out.extend_from_slice(&self.flags.to_be_bytes());
        field(out, SEQUENCE);
        out.extend_from_slice(&self.sequence.to_be_bytes());
        field(out, LAST_LEDGER_SEQUENCE);
        out.extend_from_slice(&self.last_ledger_sequence.to_be_bytes());
        // Amount (1) or LimitAmount (3) goes before the Fee (8).
        field(out, amount_field);
        write_amount(out, amount);
        field(out, FEE);
        out.extend_from_slice(&xrp(self.fee));
        // SendMax (9) goes after the Fee (8), before the blobs.
        if let Some(send_max) = send_max {
            field(out, SEND_MAX);
            write_amount(out, send_max);
        }
        field(out, SIGNING_PUB_KEY);
        blob(out, self.signing_pub_key);
        if let (true, Some(signature)) = (with_signature, self.signature) {
            field(out, TXN_SIGNATURE);
            blob(out, signature);
        }
        field(out, ACCOUNT);
        blob(out, &self.account.0);
        if let Some(destination) = destination {
            field(out, DESTINATION);
            blob(out, &destination.0);
        }
        if !self.memos.is_empty() {
            field(out, MEMOS);
            for memo in self.memos {
                assert!(
                    memo.memo_type.len() <= MAX_MEMO_BYTES
                        && memo.memo_data.len() <= MAX_MEMO_BYTES,
                    "short memos"
                );
                field(out, MEMO);
                field(out, MEMO_TYPE);
                blob(out, &memo.memo_type);
                field(out, MEMO_DATA);
                blob(out, &memo.memo_data);
                field(out, OBJECT_END);
            }
            field(out, ARRAY_END);
        }
    }
}

/// The fields both kinds have, read.
struct Decoded {
    network_id: Option<u32>,
    flags: u32,
    sequence: u32,
    last_ledger_sequence: u32,
    amount: Amount,
    fee: u64,
    send_max: Option<Amount>,
    signing_pub_key: [u8; 33],
    signature: Option<[u8; 64]>,
    account: AccountId,
    destination: Option<AccountId>,
    memos: Vec<Memo>,
}

/// Reads a transaction of `kind`, whose one amount is `amount_field`, with
/// a `Destination` and perhaps a `SendMax` if it is a Payment: these
/// fields, in this order, nothing else.
fn decode_fields(
    bytes: &[u8],
    kind: u16,
    amount_field: (u8, u8),
    is_payment: bool,
) -> Result<Decoded, DecodeError> {
    let mut reader = Reader { bytes };
    reader.expect(TRANSACTION_TYPE)?;
    if u16::from_be_bytes(reader.take()?) != kind {
        return Err(DecodeError::Value);
    }
    let network_id = if reader.peek(NETWORK_ID) {
        reader.expect(NETWORK_ID)?;
        Some(u32::from_be_bytes(reader.take()?))
    } else {
        None
    };
    reader.expect(FLAGS)?;
    let flags = u32::from_be_bytes(reader.take()?);
    reader.expect(SEQUENCE)?;
    let sequence = u32::from_be_bytes(reader.take()?);
    reader.expect(LAST_LEDGER_SEQUENCE)?;
    let last_ledger_sequence = u32::from_be_bytes(reader.take()?);
    reader.expect(amount_field)?;
    let amount = reader.amount()?;
    reader.expect(FEE)?;
    let fee = from_xrp(reader.take()?)?;
    let send_max = if is_payment && reader.peek(SEND_MAX) {
        reader.expect(SEND_MAX)?;
        Some(reader.amount()?)
    } else {
        None
    };
    reader.expect(SIGNING_PUB_KEY)?;
    let signing_pub_key = reader.blob()?.try_into().map_err(|_| DecodeError::Value)?;
    let signature = if reader.peek(TXN_SIGNATURE) {
        reader.expect(TXN_SIGNATURE)?;
        Some(reader.blob()?.try_into().map_err(|_| DecodeError::Value)?)
    } else {
        None
    };
    reader.expect(ACCOUNT)?;
    let account = AccountId(reader.blob()?.try_into().map_err(|_| DecodeError::Value)?);
    let destination = if is_payment {
        reader.expect(DESTINATION)?;
        Some(AccountId(
            reader.blob()?.try_into().map_err(|_| DecodeError::Value)?,
        ))
    } else {
        None
    };
    let memos = reader.memos()?;
    if !reader.bytes.is_empty() {
        return Err(DecodeError::Trailing);
    }
    Ok(Decoded {
        network_id,
        flags,
        sequence,
        last_ledger_sequence,
        amount,
        fee,
        send_max,
        signing_pub_key,
        signature,
        account,
        destination,
        memos,
    })
}

/// Whether `send_max` may go with `amount`: an amount the ledger takes,
/// and never XRP for XRP, which the ledger refuses
/// (`temBAD_SEND_XRP_MAX`).
fn send_max_fits(amount: &Amount, send_max: &Amount) -> bool {
    send_max.is_payable() && !matches!((amount, send_max), (Amount::Xrp(_), Amount::Xrp(_)))
}

impl Fields for Payment {
    fn encode(&self, out: &mut Vec<u8>, with_signature: bool) {
        assert!(self.amount.is_payable(), "amounts within the ledger's");
        assert!(
            self.send_max
                .is_none_or(|send_max| send_max_fits(&self.amount, &send_max)),
            "a SendMax the ledger takes"
        );
        assert_eq!(
            self.flags & TF_PARTIAL_PAYMENT,
            0,
            "never a partial payment"
        );
        Common {
            kind: PAYMENT,
            network_id: self.network_id,
            flags: self.flags,
            sequence: self.sequence,
            last_ledger_sequence: self.last_ledger_sequence,
            fee: self.fee,
            signing_pub_key: &self.signing_pub_key,
            signature: self.signature.as_ref(),
            account: self.account,
            memos: &self.memos,
        }
        .encode(
            out,
            (AMOUNT_FIELD, &self.amount),
            self.send_max.as_ref(),
            Some(self.destination),
            with_signature,
        );
    }

    fn signer(&self) -> (AccountId, [u8; 33]) {
        (self.account, self.signing_pub_key)
    }

    fn set_signature(&mut self, signature: [u8; 64]) {
        self.signature = Some(signature);
    }
}

impl Fields for TrustSet {
    fn encode(&self, out: &mut Vec<u8>, with_signature: bool) {
        assert!(
            self.limit.is_valid() && !self.limit.value.is_negative(),
            "a limit the ledger takes"
        );
        Common {
            kind: TRUST_SET,
            network_id: self.network_id,
            flags: self.flags,
            sequence: self.sequence,
            last_ledger_sequence: self.last_ledger_sequence,
            fee: self.fee,
            signing_pub_key: &self.signing_pub_key,
            signature: self.signature.as_ref(),
            account: self.account,
            memos: &self.memos,
        }
        .encode(
            out,
            (LIMIT_AMOUNT, &Amount::Token(self.limit)),
            None,
            None,
            with_signature,
        );
    }

    fn signer(&self) -> (AccountId, [u8; 33]) {
        (self.account, self.signing_pub_key)
    }

    fn set_signature(&mut self, signature: [u8; 64]) {
        self.signature = Some(signature);
    }
}

impl Payment {
    /// What the payer signs: `STX\0` and the fields, signature left out.
    #[must_use]
    pub fn signing_payload(&self) -> Vec<u8> {
        signing_payload(self)
    }

    /// Signs it with `wallet`, whose key it names.
    ///
    /// # Panics
    ///
    /// If `wallet` is not the payer's, an amount is beyond the ledger's or
    /// below zero, `SendMax` is XRP for XRP, or it is partial (bugs: the
    /// scheme builds every Payment it signs, and [`sign_send_max`] checks
    /// what it is given).
    #[must_use]
    pub fn sign(self, wallet: &Wallet) -> Signed<Self> {
        sign(self, wallet)
    }

    /// Its binary form, with the signature if it has one: what was
    /// submitted, since the form is canonical.
    #[must_use]
    pub fn blob(&self) -> Vec<u8> {
        blob_of(self)
    }

    /// The Payment in `bytes`, signed or not, if it is one the node would
    /// send: these fields, in this order, nothing else, amounts not below
    /// zero, a `SendMax` that is not XRP for XRP, and not partial.
    ///
    /// # Errors
    ///
    /// Why it is not.
    pub fn decode(bytes: &[u8]) -> Result<Self, DecodeError> {
        let decoded = decode_fields(bytes, PAYMENT, AMOUNT_FIELD, true)?;
        let negative = |amount: &Amount| amount.token().is_some_and(|t| t.value.is_negative());
        if decoded.flags & TF_PARTIAL_PAYMENT != 0
            || negative(&decoded.amount)
            || decoded
                .send_max
                .is_some_and(|send_max| !send_max_fits(&decoded.amount, &send_max))
        {
            return Err(DecodeError::Value);
        }
        Ok(Self {
            account: decoded.account,
            destination: decoded.destination.ok_or(DecodeError::Field)?,
            amount: decoded.amount,
            send_max: decoded.send_max,
            fee: decoded.fee,
            sequence: decoded.sequence,
            last_ledger_sequence: decoded.last_ledger_sequence,
            flags: decoded.flags,
            network_id: decoded.network_id,
            memos: decoded.memos,
            signing_pub_key: decoded.signing_pub_key,
            signature: decoded.signature,
        })
    }
}

impl TrustSet {
    /// What the sender signs: `STX\0` and the fields, signature left out.
    #[must_use]
    pub fn signing_payload(&self) -> Vec<u8> {
        signing_payload(self)
    }

    /// Signs it with `wallet`, whose key it names.
    ///
    /// # Panics
    ///
    /// If `wallet` is not the sender's, or the limit or the fee is beyond
    /// the ledger's or below zero (bugs: the scheme builds every TrustSet
    /// it signs).
    #[must_use]
    pub fn sign(self, wallet: &Wallet) -> Signed<Self> {
        sign(self, wallet)
    }

    /// Its binary form, with the signature if it has one.
    #[must_use]
    pub fn blob(&self) -> Vec<u8> {
        blob_of(self)
    }

    /// The TrustSet in `bytes`, signed or not, if it is one the node would
    /// send: these fields, in this order, nothing else (no qualities), and
    /// a limit not below zero.
    ///
    /// # Errors
    ///
    /// Why it is not.
    pub fn decode(bytes: &[u8]) -> Result<Self, DecodeError> {
        let decoded = decode_fields(bytes, TRUST_SET, LIMIT_AMOUNT, false)?;
        let Amount::Token(limit) = decoded.amount else {
            return Err(DecodeError::Value);
        };
        if limit.value.is_negative() {
            return Err(DecodeError::Value);
        }
        Ok(Self {
            account: decoded.account,
            limit,
            fee: decoded.fee,
            sequence: decoded.sequence,
            last_ledger_sequence: decoded.last_ledger_sequence,
            flags: decoded.flags,
            network_id: decoded.network_id,
            memos: decoded.memos,
            signing_pub_key: decoded.signing_pub_key,
            signature: decoded.signature,
        })
    }
}

/// A Payment that delivers exactly `amount` and spends at most `send_max`,
/// through the ledger's default paths: a swap through an AMM pool, when
/// the two are XRP and a token ([`sign_send_max`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SendMaxTerms {
    /// Who is paid: another account, or the payer itself for a swap.
    pub destination: AccountId,
    /// What it delivers, all of it or nothing.
    pub amount: Amount,
    /// The most it spends.
    pub send_max: Amount,
    /// The fee, in drops.
    pub fee: u64,
    /// The payer's sequence number.
    pub sequence: u32,
    /// The last ledger the transaction may be in.
    pub last_ledger_sequence: u32,
    /// The network, required above 1024 and left out below.
    pub network_id: Option<u32>,
    /// Memos.
    pub memos: Vec<Memo>,
}

/// Why terms are no Payment the ledger would take.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TermsError {
    /// The amount is zero, below zero or beyond the ledger's.
    Amount,
    /// `SendMax` is zero, below zero or beyond the ledger's, or XRP for
    /// XRP.
    SendMax,
    /// The fee is beyond all the XRP there is.
    Fee,
    /// More memos than [`MAX_MEMOS`], or one longer than
    /// [`MAX_MEMO_BYTES`].
    Memos,
    /// It pays the payer the same asset it spends.
    Redundant,
}

/// Signs, with `wallet`, a Payment from its account on `terms`: one that
/// delivers all of `terms.amount` or fails, spending at most
/// `terms.send_max`, with no paths and no flags, so the ledger converts
/// through its default paths and never pays part of it. Who calls it
/// keeps the sequence: this signs, and sends nothing.
///
/// # Errors
///
/// Why `terms` are no Payment the ledger would take ([`TermsError`]).
pub fn sign_send_max(wallet: &Wallet, terms: SendMaxTerms) -> Result<Signed<Payment>, TermsError> {
    if terms.amount.is_zero() || !terms.amount.is_payable() {
        return Err(TermsError::Amount);
    }
    if terms.send_max.is_zero() || !send_max_fits(&terms.amount, &terms.send_max) {
        return Err(TermsError::SendMax);
    }
    if terms.fee > MAX_DROPS {
        return Err(TermsError::Fee);
    }
    let long = |memo: &Memo| {
        memo.memo_type.len() > MAX_MEMO_BYTES || memo.memo_data.len() > MAX_MEMO_BYTES
    };
    if terms.memos.len() > MAX_MEMOS || terms.memos.iter().any(long) {
        return Err(TermsError::Memos);
    }
    let account = wallet.account();
    if terms.destination == account && terms.amount.asset() == terms.send_max.asset() {
        return Err(TermsError::Redundant);
    }
    let payment = Payment {
        account,
        destination: terms.destination,
        amount: terms.amount,
        send_max: Some(terms.send_max),
        fee: terms.fee,
        sequence: terms.sequence,
        last_ledger_sequence: terms.last_ledger_sequence,
        flags: 0,
        network_id: terms.network_id,
        memos: terms.memos,
        signing_pub_key: *wallet.public_key(),
        signature: None,
    };
    Ok(payment.sign(wallet))
}

impl Transaction {
    /// The transaction in `bytes`, of either kind, if it is one the node
    /// would send.
    ///
    /// # Errors
    ///
    /// Why it is not.
    pub fn decode(bytes: &[u8]) -> Result<Self, DecodeError> {
        let mut reader = Reader { bytes };
        reader.expect(TRANSACTION_TYPE)?;
        match u16::from_be_bytes(reader.take()?) {
            PAYMENT => Payment::decode(bytes).map(Self::Payment),
            TRUST_SET => TrustSet::decode(bytes).map(Self::TrustSet),
            _ => Err(DecodeError::Value),
        }
    }

    /// Its binary form, with the signature if it has one.
    #[must_use]
    pub fn blob(&self) -> Vec<u8> {
        match self {
            Self::Payment(payment) => payment.blob(),
            Self::TrustSet(trust_set) => trust_set.blob(),
        }
    }
}

/// A field's header: type and field code, each in a nibble where it fits.
fn field(out: &mut Vec<u8>, (kind, code): (u8, u8)) {
    match (kind < 16, code < 16) {
        (true, true) => out.push(kind << 4 | code),
        (true, false) => out.extend_from_slice(&[kind << 4, code]),
        (false, true) => out.extend_from_slice(&[code, kind]),
        (false, false) => out.extend_from_slice(&[0, kind, code]),
    }
}

/// A blob with its length prefix (1 or 2 bytes, for up to 12 480).
fn blob(out: &mut Vec<u8>, bytes: &[u8]) {
    let length = bytes.len();
    if length <= 192 {
        out.push(u8::try_from(length).unwrap_or(0));
    } else {
        assert!(length <= 12_480, "blobs the node writes are short");
        let rest = length - 193;
        out.push(u8::try_from(193 + (rest >> 8)).unwrap_or(0));
        out.push(u8::try_from(rest & 0xff).unwrap_or(0));
    }
    out.extend_from_slice(bytes);
}

/// An amount: eight bytes of XRP, or 48 of a token.
fn write_amount(out: &mut Vec<u8>, amount: &Amount) {
    match amount {
        Amount::Xrp(drops) => out.extend_from_slice(&xrp(*drops)),
        Amount::Token(token) => out.extend_from_slice(&token.to_bytes()),
    }
}

/// An XRP amount: the "not an IOU" and "positive" bits, and the drops.
fn xrp(drops: u64) -> [u8; 8] {
    (0x4000_0000_0000_0000 | drops).to_be_bytes()
}

fn from_xrp(bytes: [u8; 8]) -> Result<u64, DecodeError> {
    let value = u64::from_be_bytes(bytes);
    // Not an IOU, and positive.
    if value & 0xC000_0000_0000_0000 != 0x4000_0000_0000_0000 {
        return Err(DecodeError::Value);
    }
    let drops = value & 0x3FFF_FFFF_FFFF_FFFF;
    if drops > MAX_DROPS {
        return Err(DecodeError::Value);
    }
    Ok(drops)
}

struct Reader<'a> {
    bytes: &'a [u8],
}

impl<'a> Reader<'a> {
    fn header(&self) -> Option<((u8, u8), usize)> {
        let first = *self.bytes.first()?;
        let byte = |index: usize| self.bytes.get(index).copied();
        let (kind, code) = (first >> 4, first & 0x0f);
        Some(match (kind, code) {
            (0, 0) => ((byte(1)?, byte(2)?), 3),
            (0, code) => ((byte(1)?, code), 2),
            (kind, 0) => ((kind, byte(1)?), 2),
            (kind, code) => ((kind, code), 1),
        })
    }

    fn peek(&self, expected: (u8, u8)) -> bool {
        self.header().is_some_and(|(found, _)| found == expected)
    }

    fn expect(&mut self, expected: (u8, u8)) -> Result<(), DecodeError> {
        let (found, length) = self.header().ok_or(DecodeError::Truncated)?;
        if found != expected {
            return Err(DecodeError::Field);
        }
        self.bytes = self.bytes.get(length..).ok_or(DecodeError::Truncated)?;
        Ok(())
    }

    /// An amount: a token if its first bit says so, else XRP.
    fn amount(&mut self) -> Result<Amount, DecodeError> {
        let first = *self.bytes.first().ok_or(DecodeError::Truncated)?;
        if first & 0x80 == 0 {
            return Ok(Amount::Xrp(from_xrp(self.take()?)?));
        }
        TokenAmount::from_bytes(&self.take()?)
            .map(Amount::Token)
            .ok_or(DecodeError::Value)
    }

    /// The memos, if any.
    fn memos(&mut self) -> Result<Vec<Memo>, DecodeError> {
        let mut memos = Vec::new();
        if !self.peek(MEMOS) {
            return Ok(memos);
        }
        self.expect(MEMOS)?;
        while !self.peek(ARRAY_END) {
            if memos.len() >= MAX_MEMOS {
                return Err(DecodeError::Value);
            }
            self.expect(MEMO)?;
            self.expect(MEMO_TYPE)?;
            let memo_type = self.blob()?.to_vec();
            self.expect(MEMO_DATA)?;
            let memo_data = self.blob()?.to_vec();
            self.expect(OBJECT_END)?;
            if memo_type.len() > MAX_MEMO_BYTES || memo_data.len() > MAX_MEMO_BYTES {
                return Err(DecodeError::Value);
            }
            memos.push(Memo {
                memo_type,
                memo_data,
            });
        }
        self.expect(ARRAY_END)?;
        Ok(memos)
    }

    fn take<const N: usize>(&mut self) -> Result<[u8; N], DecodeError> {
        let (head, rest) = self
            .bytes
            .split_at_checked(N)
            .ok_or(DecodeError::Truncated)?;
        self.bytes = rest;
        head.try_into().map_err(|_| DecodeError::Truncated)
    }

    fn blob(&mut self) -> Result<&'a [u8], DecodeError> {
        let [first] = self.take::<1>()?;
        let length = match first {
            0..=192 => usize::from(first),
            193..=240 => {
                let [second] = self.take::<1>()?;
                193 + (usize::from(first - 193) << 8) + usize::from(second)
            }
            _ => return Err(DecodeError::Value),
        };
        let (head, rest) = self
            .bytes
            .split_at_checked(length)
            .ok_or(DecodeError::Truncated)?;
        self.bytes = rest;
        Ok(head)
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::indexing_slicing)]
mod tests {
    use super::*;

    fn payment(wallet: &Wallet) -> Payment {
        Payment {
            account: wallet.account(),
            destination: AccountId([9; 20]),
            amount: Amount::Xrp(1_000),
            send_max: None,
            fee: 12,
            sequence: 5,
            last_ledger_sequence: 25,
            flags: 0,
            network_id: None,
            memos: alloc::vec![Memo {
                memo_type: alloc::vec![1; 200],
                memo_data: alloc::vec![2, 3],
            }],
            signing_pub_key: *wallet.public_key(),
            signature: None,
        }
    }

    #[test]
    fn signed_payments_decode_to_themselves() {
        let wallet = Wallet::from_seed([1; 16]);
        let signed = payment(&wallet).sign(&wallet);
        assert_eq!(Payment::decode(&signed.blob), Ok(signed.tx.clone()));
        assert_eq!(hash(&signed.blob), signed.hash);
        let unsigned = Payment {
            signature: None,
            ..signed.tx
        };
        assert_eq!(
            Payment::decode(unsigned.signing_payload().get(4..).unwrap()),
            Ok(unsigned)
        );
    }

    #[test]
    fn other_transactions_are_refused() {
        let wallet = Wallet::from_seed([1; 16]);
        let signed = payment(&wallet).sign(&wallet);
        let mut other = signed.blob.clone();
        // A TransactionType other than Payment.
        other[2] = 1;
        assert_eq!(Payment::decode(&other), Err(DecodeError::Value));
        let mut longer = signed.blob.clone();
        longer.push(0);
        assert!(Payment::decode(&longer).is_err());
        // A cut blob is no Payment, or one without the memos: optional
        // fields come last.
        for cut in 0..signed.blob.len() {
            match Payment::decode(&signed.blob[..cut]) {
                Err(_) => {}
                Ok(shorter) => assert!(shorter.memos.is_empty(), "{cut}"),
            }
        }
    }

    fn rlusd(value: &str) -> TokenAmount {
        TokenAmount {
            value: TokenValue::parse(value).unwrap(),
            currency: Currency::parse("524C555344000000000000000000000000000000").unwrap(),
            issuer: AccountId([7; 20]),
        }
    }

    #[test]
    fn token_payments_and_trust_sets_decode_to_themselves() {
        let wallet = Wallet::from_seed([1; 16]);
        let token = Payment {
            amount: Amount::Token(rlusd("1234567890.123456")),
            ..payment(&wallet)
        };
        let signed = token.sign(&wallet);
        assert_eq!(Payment::decode(&signed.blob), Ok(signed.tx.clone()));
        assert_eq!(
            Transaction::decode(&signed.blob),
            Ok(Transaction::Payment(signed.tx.clone()))
        );
        let trust_set = TrustSet {
            account: wallet.account(),
            limit: TokenAmount {
                value: TokenValue::MAX,
                ..rlusd("0")
            },
            fee: 12,
            sequence: 6,
            last_ledger_sequence: 26,
            flags: TF_SET_NO_RIPPLE,
            network_id: Some(21_338),
            memos: alloc::vec::Vec::new(),
            signing_pub_key: *wallet.public_key(),
            signature: None,
        };
        let signed = trust_set.sign(&wallet);
        assert_eq!(TrustSet::decode(&signed.blob), Ok(signed.tx.clone()));
        assert_eq!(
            Transaction::decode(&signed.blob),
            Ok(Transaction::TrustSet(signed.tx.clone()))
        );
        assert_eq!(
            Transaction::decode(&signed.blob).unwrap().blob(),
            signed.blob
        );
        // Each kind refuses the other.
        assert_eq!(Payment::decode(&signed.blob), Err(DecodeError::Value));
    }

    #[test]
    fn partial_payments_and_negative_amounts_are_refused() {
        let wallet = Wallet::from_seed([1; 16]);
        let token = Payment {
            amount: Amount::Token(rlusd("1")),
            ..payment(&wallet)
        };
        let blob = token.blob();
        // Flags are bytes 4 to 7 of the blob: TransactionType, then Flags.
        let mut partial = blob.clone();
        partial[4..8].copy_from_slice(&TF_PARTIAL_PAYMENT.to_be_bytes());
        assert_eq!(Payment::decode(&partial), Err(DecodeError::Value));
        // The sign bit is the second of the amount's first byte, after
        // Sequence and LastLedgerSequence.
        let amount_at = blob.iter().position(|b| *b == 0x61).unwrap() + 1;
        let mut negative = blob.clone();
        negative[amount_at] &= !0x40;
        assert_eq!(Payment::decode(&negative), Err(DecodeError::Value));
        // A token of the zero issuer is none.
        let mut zero_issuer = blob;
        zero_issuer[amount_at + 28..amount_at + 48].fill(0);
        assert_eq!(Payment::decode(&zero_issuer), Err(DecodeError::Value));
    }

    #[test]
    #[should_panic(expected = "never a partial payment")]
    fn the_node_never_signs_a_partial_payment() {
        let wallet = Wallet::from_seed([1; 16]);
        let partial = Payment {
            flags: TF_PARTIAL_PAYMENT,
            ..payment(&wallet)
        };
        let _ = partial.sign(&wallet);
    }

    fn terms(amount: Amount, send_max: Amount) -> SendMaxTerms {
        SendMaxTerms {
            destination: AccountId([9; 20]),
            amount,
            send_max,
            fee: 12,
            sequence: 5,
            last_ledger_sequence: 25,
            network_id: None,
            memos: Vec::new(),
        }
    }

    #[test]
    fn send_max_payments_decode_to_themselves_either_way() {
        let wallet = Wallet::from_seed([1; 16]);
        for (amount, send_max) in [
            (Amount::Token(rlusd("25")), Amount::Xrp(30_000_000)),
            (Amount::Xrp(1_000_000), Amount::Token(rlusd("0.75"))),
            (Amount::Token(rlusd("1")), Amount::Token(rlusd("1.002"))),
        ] {
            let signed = sign_send_max(&wallet, terms(amount, send_max)).unwrap();
            assert_eq!(signed.tx.send_max, Some(send_max));
            assert_eq!(Payment::decode(&signed.blob), Ok(signed.tx.clone()));
            assert_eq!(
                Transaction::decode(&signed.blob).unwrap().blob(),
                signed.blob
            );
            assert_eq!(hash(&signed.blob), signed.hash);
        }
    }

    #[test]
    fn send_max_is_refused_where_the_ledger_refuses_it() {
        let wallet = Wallet::from_seed([1; 16]);
        let xrp = |drops| Amount::Xrp(drops);
        let refused = [
            (terms(xrp(1), xrp(2)), TermsError::SendMax),
            (terms(xrp(0), Amount::Token(rlusd("1"))), TermsError::Amount),
            (
                terms(xrp(1), Amount::Token(rlusd("0"))),
                TermsError::SendMax,
            ),
            (
                terms(xrp(MAX_DROPS + 1), Amount::Token(rlusd("1"))),
                TermsError::Amount,
            ),
            (
                terms(Amount::Token(rlusd("1")), xrp(MAX_DROPS + 1)),
                TermsError::SendMax,
            ),
            (
                terms(Amount::Token(rlusd("-1")), xrp(1)),
                TermsError::Amount,
            ),
            (
                SendMaxTerms {
                    fee: MAX_DROPS + 1,
                    ..terms(xrp(1), Amount::Token(rlusd("1")))
                },
                TermsError::Fee,
            ),
            (
                SendMaxTerms {
                    memos: alloc::vec![
                        Memo {
                            memo_type: Vec::new(),
                            memo_data: Vec::new(),
                        };
                        MAX_MEMOS + 1
                    ],
                    ..terms(xrp(1), Amount::Token(rlusd("1")))
                },
                TermsError::Memos,
            ),
            (
                SendMaxTerms {
                    destination: wallet.account(),
                    ..terms(Amount::Token(rlusd("1")), Amount::Token(rlusd("2")))
                },
                TermsError::Redundant,
            ),
        ];
        for (terms, error) in refused {
            assert_eq!(
                sign_send_max(&wallet, terms.clone()),
                Err(error),
                "{terms:?}"
            );
        }
        // A swap to oneself is no redundant Payment.
        let swap = SendMaxTerms {
            destination: wallet.account(),
            ..terms(xrp(1), Amount::Token(rlusd("1")))
        };
        assert!(sign_send_max(&wallet, swap).is_ok());
    }

    #[test]
    fn the_decoder_refuses_what_the_encoder_never_writes() {
        let wallet = Wallet::from_seed([1; 16]);
        let token = Amount::Token(rlusd("1"));
        let blob = sign_send_max(&wallet, terms(token, Amount::Xrp(5)))
            .unwrap()
            .blob;
        // XRP for XRP: the Amount's eight bytes after its header, 0x61.
        let at = blob.iter().position(|b| *b == 0x61).unwrap() + 1;
        let mut xrp_for_xrp = blob[..at].to_vec();
        xrp_for_xrp.extend_from_slice(&xrp(7));
        xrp_for_xrp.extend_from_slice(&blob[at + 48..]);
        assert_eq!(Payment::decode(&xrp_for_xrp), Err(DecodeError::Value));
        // A negative SendMax.
        let mut negative = sign_send_max(&wallet, terms(Amount::Xrp(5), token))
            .unwrap()
            .blob;
        let at = negative.iter().position(|b| *b == 0x69).unwrap() + 1;
        negative[at] &= !0x40;
        assert_eq!(Payment::decode(&negative), Err(DecodeError::Value));
        // A TrustSet has no SendMax.
        let trust_set = TrustSet {
            account: wallet.account(),
            limit: rlusd("1"),
            fee: 12,
            sequence: 6,
            last_ledger_sequence: 26,
            flags: 0,
            network_id: None,
            memos: Vec::new(),
            signing_pub_key: *wallet.public_key(),
            signature: None,
        }
        .blob();
        let at = trust_set.iter().position(|b| *b == 0x68).unwrap() + 9;
        let mut with_send_max = trust_set[..at].to_vec();
        with_send_max.push(0x69);
        with_send_max.extend_from_slice(&xrp(5));
        with_send_max.extend_from_slice(&trust_set[at..]);
        assert_eq!(TrustSet::decode(&with_send_max), Err(DecodeError::Field));
    }

    #[test]
    #[should_panic(expected = "a SendMax the ledger takes")]
    fn the_node_never_signs_xrp_for_xrp() {
        let wallet = Wallet::from_seed([1; 16]);
        let payment = Payment {
            send_max: Some(Amount::Xrp(2_000)),
            ..payment(&wallet)
        };
        let _ = payment.sign(&wallet);
    }
}
