//! The ledger's binary format, for the fields the codec knows: written in
//! canonical order, by type code and then field code, and read back only
//! in that order, each field once, with its header in its shortest form,
//! so that a blob decodes only if it encodes to itself.

use alloc::vec::Vec;

use super::{
    DecodeError, MAX_MEMO_BYTES, MAX_MEMOS, MAX_SIGNERS, Memo, Signer, SignerEntry, Signing,
};
use crate::MAX_DROPS;
use crate::amount::{Amount, TokenAmount};
use crate::keys::{AccountId, PublicKey, Signature};

// Type codes.
const UINT16: u8 = 1;
const UINT32: u8 = 2;
const AMOUNT: u8 = 6;
const BLOB: u8 = 7;
const ACCOUNT_ID: u8 = 8;
const OBJECT: u8 = 14;
const ARRAY: u8 = 15;

// Field codes, by type.
pub const TRANSACTION_TYPE: (u8, u8) = (UINT16, 2);
const SIGNER_WEIGHT: (u8, u8) = (UINT16, 3);
pub const NETWORK_ID: (u8, u8) = (UINT32, 1);
pub const FLAGS: (u8, u8) = (UINT32, 2);
pub const SEQUENCE: (u8, u8) = (UINT32, 4);
pub const DESTINATION_TAG: (u8, u8) = (UINT32, 14);
pub const LAST_LEDGER_SEQUENCE: (u8, u8) = (UINT32, 27);
pub const SET_FLAG: (u8, u8) = (UINT32, 33);
pub const CLEAR_FLAG: (u8, u8) = (UINT32, 34);
pub const SIGNER_QUORUM: (u8, u8) = (UINT32, 35);
pub const AMOUNT_FIELD: (u8, u8) = (AMOUNT, 1);
pub const LIMIT_AMOUNT: (u8, u8) = (AMOUNT, 3);
pub const FEE: (u8, u8) = (AMOUNT, 8);
pub const SEND_MAX: (u8, u8) = (AMOUNT, 9);
pub const SIGNING_PUB_KEY: (u8, u8) = (BLOB, 3);
pub const TXN_SIGNATURE: (u8, u8) = (BLOB, 4);
const MEMO_TYPE: (u8, u8) = (BLOB, 12);
const MEMO_DATA: (u8, u8) = (BLOB, 13);
pub const ACCOUNT: (u8, u8) = (ACCOUNT_ID, 1);
pub const DESTINATION: (u8, u8) = (ACCOUNT_ID, 3);
pub const REGULAR_KEY: (u8, u8) = (ACCOUNT_ID, 8);
const OBJECT_END: (u8, u8) = (OBJECT, 1);
const MEMO: (u8, u8) = (OBJECT, 10);
const SIGNER_ENTRY: (u8, u8) = (OBJECT, 11);
const SIGNER: (u8, u8) = (OBJECT, 16);
const ARRAY_END: (u8, u8) = (ARRAY, 1);
pub const SIGNERS: (u8, u8) = (ARRAY, 3);
pub const SIGNER_ENTRIES: (u8, u8) = (ARRAY, 4);
pub const MEMOS: (u8, u8) = (ARRAY, 9);

/// Every field a transaction the codec knows may have, in canonical order:
/// the order they are written in, and the only one they are read in.
pub const TX_FIELDS: [(u8, u8); 21] = [
    TRANSACTION_TYPE,
    NETWORK_ID,
    FLAGS,
    SEQUENCE,
    DESTINATION_TAG,
    LAST_LEDGER_SEQUENCE,
    SET_FLAG,
    CLEAR_FLAG,
    SIGNER_QUORUM,
    AMOUNT_FIELD,
    LIMIT_AMOUNT,
    FEE,
    SEND_MAX,
    SIGNING_PUB_KEY,
    TXN_SIGNATURE,
    ACCOUNT,
    DESTINATION,
    REGULAR_KEY,
    SIGNERS,
    SIGNER_ENTRIES,
    MEMOS,
];

/// The bit of `field` in a set of [`TX_FIELDS`]; none for another field.
pub const fn bit(field: (u8, u8)) -> u32 {
    let mut rest: &[(u8, u8)] = &TX_FIELDS;
    let mut index = 0;
    while let [(kind, code), tail @ ..] = rest {
        if *kind == field.0 && *code == field.1 {
            return 1 << index;
        }
        rest = tail;
        index += 1;
    }
    0
}

/// The bits of `fields`.
pub const fn bits(fields: &[(u8, u8)]) -> u32 {
    let mut set = 0;
    let mut rest = fields;
    while let [first, tail @ ..] = rest {
        set |= bit(*first);
        rest = tail;
    }
    set
}

/// What every kind has: required, and allowed besides.
pub const COMMON_REQUIRED: u32 = bits(&[
    TRANSACTION_TYPE,
    FLAGS,
    SEQUENCE,
    FEE,
    SIGNING_PUB_KEY,
    ACCOUNT,
]);
pub const COMMON_ALLOWED: u32 = COMMON_REQUIRED
    | bits(&[
        NETWORK_ID,
        LAST_LEDGER_SEQUENCE,
        TXN_SIGNATURE,
        SIGNERS,
        MEMOS,
    ]);

/// What is written: all of it, or what one key signs, or what each signer
/// of a multi-signed transaction signs.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// Every field: the blob that is submitted and hashed.
    Blob,
    /// Without `TxnSignature` and `Signers`.
    Single,
    /// Without `TxnSignature` and `Signers`, and `SigningPubKey` empty.
    Multi,
}

/// A transaction's fields, borrowed to be written; a kind leaves out what
/// it does not have.
pub struct Body<'a> {
    pub kind: u16,
    pub network_id: Option<u32>,
    pub flags: u32,
    pub sequence: u32,
    pub destination_tag: Option<u32>,
    pub last_ledger_sequence: Option<u32>,
    pub set_flag: Option<u32>,
    pub clear_flag: Option<u32>,
    pub signer_quorum: Option<u32>,
    pub amount: Option<Amount>,
    pub limit_amount: Option<Amount>,
    pub fee: u64,
    pub send_max: Option<Amount>,
    pub signing: &'a Signing,
    pub account: AccountId,
    pub destination: Option<AccountId>,
    pub regular_key: Option<AccountId>,
    pub signer_entries: &'a [SignerEntry],
    pub memos: &'a [Memo],
}

/// What signing, writing and reading share between the kinds. Public in a
/// private module, so only the codec's kinds have it.
pub trait Fields: Sized {
    /// Its fields.
    fn body(&self) -> Body<'_>;
    /// Who signs it, to be set.
    fn signing_mut(&mut self) -> &mut Signing;
    /// Asserts what the node never writes: a bug if it would.
    fn check(&self) {}
    /// The transaction in `fields`, if they are one of this kind's.
    ///
    /// # Errors
    ///
    /// Why they are not.
    fn from_fields(fields: Decoded) -> Result<Self, DecodeError>;
}

/// Writes `body`'s fields for `mode`, in canonical order.
pub fn write(body: &Body<'_>, mode: Mode, out: &mut Vec<u8>) {
    assert!(body.fee <= MAX_DROPS, "fees within the ledger's");
    assert!(body.memos.len() <= MAX_MEMOS, "few memos");
    assert!(
        body.signer_entries.len() <= MAX_SIGNERS,
        "signer lists the ledger takes"
    );
    write_numbers(body, out);
    write_amounts(body, out);
    write_signing(body, mode, out);
    write_accounts(body, out);
    write_arrays(body, mode, out);
}

fn write_numbers(body: &Body<'_>, out: &mut Vec<u8>) {
    field(out, TRANSACTION_TYPE);
    out.extend_from_slice(&body.kind.to_be_bytes());
    for (id, value) in [
        (NETWORK_ID, body.network_id),
        (FLAGS, Some(body.flags)),
        (SEQUENCE, Some(body.sequence)),
        (DESTINATION_TAG, body.destination_tag),
        (LAST_LEDGER_SEQUENCE, body.last_ledger_sequence),
        (SET_FLAG, body.set_flag),
        (CLEAR_FLAG, body.clear_flag),
        (SIGNER_QUORUM, body.signer_quorum),
    ] {
        if let Some(value) = value {
            field(out, id);
            out.extend_from_slice(&value.to_be_bytes());
        }
    }
}

fn write_amounts(body: &Body<'_>, out: &mut Vec<u8>) {
    for (id, amount) in [
        (AMOUNT_FIELD, body.amount),
        (LIMIT_AMOUNT, body.limit_amount),
        (FEE, Some(Amount::Xrp(body.fee))),
        (SEND_MAX, body.send_max),
    ] {
        if let Some(amount) = amount {
            field(out, id);
            write_amount(out, &amount);
        }
    }
}

fn write_signing(body: &Body<'_>, mode: Mode, out: &mut Vec<u8>) {
    field(out, SIGNING_PUB_KEY);
    match (mode, body.signing) {
        (Mode::Multi, _) | (_, Signing::Multi(_)) => blob(out, &[]),
        (_, Signing::Single { public_key, .. }) => blob(out, public_key.as_bytes()),
    }
    if let (
        Mode::Blob,
        Signing::Single {
            signature: Some(signature),
            ..
        },
    ) = (mode, body.signing)
    {
        field(out, TXN_SIGNATURE);
        blob(out, signature.as_bytes());
    }
}

fn write_accounts(body: &Body<'_>, out: &mut Vec<u8>) {
    for (id, account) in [
        (ACCOUNT, Some(body.account)),
        (DESTINATION, body.destination),
        (REGULAR_KEY, body.regular_key),
    ] {
        if let Some(account) = account {
            field(out, id);
            blob(out, &account.0);
        }
    }
}

fn write_arrays(body: &Body<'_>, mode: Mode, out: &mut Vec<u8>) {
    if let (Mode::Blob, Signing::Multi(signers)) = (mode, body.signing)
        && !signers.is_empty()
    {
        assert!(signers.len() <= MAX_SIGNERS, "signers the ledger takes");
        field(out, SIGNERS);
        for signer in signers {
            field(out, SIGNER);
            field(out, SIGNING_PUB_KEY);
            blob(out, signer.signing_pub_key.as_bytes());
            field(out, TXN_SIGNATURE);
            blob(out, signer.signature.as_bytes());
            field(out, ACCOUNT);
            blob(out, &signer.account.0);
            field(out, OBJECT_END);
        }
        field(out, ARRAY_END);
    }
    if !body.signer_entries.is_empty() {
        field(out, SIGNER_ENTRIES);
        for entry in body.signer_entries {
            field(out, SIGNER_ENTRY);
            field(out, SIGNER_WEIGHT);
            out.extend_from_slice(&entry.weight.to_be_bytes());
            field(out, ACCOUNT);
            blob(out, &entry.account.0);
            field(out, OBJECT_END);
        }
        field(out, ARRAY_END);
    }
    if !body.memos.is_empty() {
        field(out, MEMOS);
        for memo in body.memos {
            assert!(
                memo.memo_type.len() <= MAX_MEMO_BYTES && memo.memo_data.len() <= MAX_MEMO_BYTES,
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
pub fn xrp(drops: u64) -> [u8; 8] {
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

/// A transaction's fields as read, each once and in canonical order; which
/// it may have and must have is its kind's to check.
#[derive(Default)]
pub struct Decoded {
    /// The fields it has, as [`bit`]s.
    pub present: u32,
    pub kind: u16,
    pub network_id: Option<u32>,
    pub flags: u32,
    pub sequence: u32,
    pub destination_tag: Option<u32>,
    pub last_ledger_sequence: Option<u32>,
    pub set_flag: Option<u32>,
    pub clear_flag: Option<u32>,
    pub signer_quorum: Option<u32>,
    pub amount: Option<Amount>,
    pub limit_amount: Option<Amount>,
    pub fee: u64,
    pub send_max: Option<Amount>,
    /// `None` if it was empty, as a multi-signed transaction's is.
    pub signing_pub_key: Option<PublicKey>,
    pub signature: Option<Signature>,
    pub account: Option<AccountId>,
    pub destination: Option<AccountId>,
    pub regular_key: Option<AccountId>,
    pub signers: Vec<Signer>,
    pub signer_entries: Vec<SignerEntry>,
    pub memos: Vec<Memo>,
}

impl Decoded {
    /// Whether they are a transaction of `kind`, with every field of
    /// `required` and none but those of `allowed`.
    ///
    /// # Errors
    ///
    /// Another kind ([`DecodeError::Value`]), or a field missing or not
    /// allowed ([`DecodeError::Field`]).
    pub const fn expect(&self, kind: u16, required: u32, allowed: u32) -> Result<(), DecodeError> {
        if self.present & bit(TRANSACTION_TYPE) == 0 {
            return Err(DecodeError::Field);
        }
        if self.kind != kind {
            return Err(DecodeError::Value);
        }
        if self.present & required != required || self.present & !allowed != 0 {
            return Err(DecodeError::Field);
        }
        Ok(())
    }

    /// Who signed, or signs: one key, or signers each alone, never both.
    ///
    /// # Errors
    ///
    /// A signature with no key, or signers beside a key
    /// ([`DecodeError::Value`]).
    pub fn signing(&mut self) -> Result<Signing, DecodeError> {
        let signers = core::mem::take(&mut self.signers);
        match (self.signing_pub_key, self.signature) {
            (Some(public_key), signature) if signers.is_empty() => Ok(Signing::Single {
                public_key,
                signature,
            }),
            (None, None) => Ok(Signing::Multi(signers)),
            _ => Err(DecodeError::Value),
        }
    }

    /// The sender.
    ///
    /// # Errors
    ///
    /// It has none ([`DecodeError::Field`]).
    pub fn account(&self) -> Result<AccountId, DecodeError> {
        self.account.ok_or(DecodeError::Field)
    }
}

/// Reads the fields of a transaction: only those the codec knows, each
/// once, in canonical order, to the last byte.
///
/// # Errors
///
/// Why `bytes` are not such fields.
pub fn read(bytes: &[u8]) -> Result<Decoded, DecodeError> {
    let mut reader = Reader { bytes };
    let mut fields = Decoded::default();
    let mut next = 0;
    // Each turn reads at least a byte, and a field at most once.
    while !reader.bytes.is_empty() {
        let id = reader.header()?;
        let index = TX_FIELDS
            .iter()
            .position(|known| *known == id)
            .ok_or(DecodeError::Field)?;
        if index < next {
            return Err(DecodeError::Field);
        }
        next = index + 1;
        fields.present |= bit(id);
        reader.value(id, &mut fields)?;
    }
    Ok(fields)
}

struct Reader<'a> {
    bytes: &'a [u8],
}

impl<'a> Reader<'a> {
    /// The next field's header, if it is written in its shortest form: a
    /// code below 16 in a byte of its own is not.
    fn peek_header(&self) -> Result<((u8, u8), usize), DecodeError> {
        let first = *self.bytes.first().ok_or(DecodeError::Truncated)?;
        let byte = |index: usize| self.bytes.get(index).copied().ok_or(DecodeError::Truncated);
        let ((kind, code), length) = match (first >> 4, first & 0x0f) {
            (0, 0) => ((byte(1)?, byte(2)?), 3),
            (0, code) => ((byte(1)?, code), 2),
            (kind, 0) => ((kind, byte(1)?), 2),
            (kind, code) => return Ok(((kind, code), 1)),
        };
        let long = |nibble: u8, value: u8| nibble != 0 || value >= 16;
        if !long(first >> 4, kind) || !long(first & 0x0f, code) {
            return Err(DecodeError::Field);
        }
        Ok(((kind, code), length))
    }

    fn header(&mut self) -> Result<(u8, u8), DecodeError> {
        let (id, length) = self.peek_header()?;
        self.bytes = self.bytes.get(length..).ok_or(DecodeError::Truncated)?;
        Ok(id)
    }

    fn peek(&self, expected: (u8, u8)) -> bool {
        self.peek_header().is_ok_and(|(found, _)| found == expected)
    }

    fn expect(&mut self, expected: (u8, u8)) -> Result<(), DecodeError> {
        if self.header()? != expected {
            return Err(DecodeError::Field);
        }
        Ok(())
    }

    /// The value of field `id`, into `fields`.
    fn value(&mut self, id: (u8, u8), fields: &mut Decoded) -> Result<(), DecodeError> {
        match id {
            TRANSACTION_TYPE => fields.kind = u16::from_be_bytes(self.take()?),
            NETWORK_ID => fields.network_id = Some(self.uint32()?),
            FLAGS => fields.flags = self.uint32()?,
            SEQUENCE => fields.sequence = self.uint32()?,
            DESTINATION_TAG => fields.destination_tag = Some(self.uint32()?),
            LAST_LEDGER_SEQUENCE => fields.last_ledger_sequence = Some(self.uint32()?),
            SET_FLAG => fields.set_flag = Some(self.uint32()?),
            CLEAR_FLAG => fields.clear_flag = Some(self.uint32()?),
            SIGNER_QUORUM => fields.signer_quorum = Some(self.uint32()?),
            AMOUNT_FIELD => fields.amount = Some(self.amount()?),
            LIMIT_AMOUNT => fields.limit_amount = Some(self.amount()?),
            FEE => fields.fee = from_xrp(self.take()?)?,
            SEND_MAX => fields.send_max = Some(self.amount()?),
            SIGNING_PUB_KEY => {
                fields.signing_pub_key = match self.blob()? {
                    [] => None,
                    key => Some(public_key(key)?),
                };
            }
            TXN_SIGNATURE => fields.signature = Some(self.signature()?),
            ACCOUNT => fields.account = Some(self.account()?),
            DESTINATION => fields.destination = Some(self.account()?),
            REGULAR_KEY => fields.regular_key = Some(self.account()?),
            SIGNERS => fields.signers = self.array(MAX_SIGNERS, Self::signer)?,
            SIGNER_ENTRIES => {
                fields.signer_entries = self.array(MAX_SIGNERS, Self::signer_entry)?;
            }
            MEMOS => fields.memos = self.array(MAX_MEMOS, Self::memo)?,
            _ => return Err(DecodeError::Field),
        }
        Ok(())
    }

    /// An array of objects, each read by `one`: at least one, as the
    /// encoder writes an array only when it has something in it, and at
    /// most `max`.
    fn array<T>(
        &mut self,
        max: usize,
        mut one: impl FnMut(&mut Self) -> Result<T, DecodeError>,
    ) -> Result<Vec<T>, DecodeError> {
        let mut items = Vec::new();
        while !self.peek(ARRAY_END) {
            if items.len() >= max {
                return Err(DecodeError::Value);
            }
            items.push(one(self)?);
        }
        self.expect(ARRAY_END)?;
        if items.is_empty() {
            return Err(DecodeError::Value);
        }
        Ok(items)
    }

    fn memo(&mut self) -> Result<Memo, DecodeError> {
        self.expect(MEMO)?;
        self.expect(MEMO_TYPE)?;
        let memo_type = self.blob()?.to_vec();
        self.expect(MEMO_DATA)?;
        let memo_data = self.blob()?.to_vec();
        self.expect(OBJECT_END)?;
        if memo_type.len() > MAX_MEMO_BYTES || memo_data.len() > MAX_MEMO_BYTES {
            return Err(DecodeError::Value);
        }
        Ok(Memo {
            memo_type,
            memo_data,
        })
    }

    fn signer(&mut self) -> Result<Signer, DecodeError> {
        self.expect(SIGNER)?;
        self.expect(SIGNING_PUB_KEY)?;
        let signing_pub_key = public_key(self.blob()?)?;
        self.expect(TXN_SIGNATURE)?;
        let signature = self.signature()?;
        self.expect(ACCOUNT)?;
        let account = self.account()?;
        self.expect(OBJECT_END)?;
        Ok(Signer {
            account,
            signing_pub_key,
            signature,
        })
    }

    fn signer_entry(&mut self) -> Result<SignerEntry, DecodeError> {
        self.expect(SIGNER_ENTRY)?;
        self.expect(SIGNER_WEIGHT)?;
        let weight = u16::from_be_bytes(self.take()?);
        self.expect(ACCOUNT)?;
        let account = self.account()?;
        self.expect(OBJECT_END)?;
        Ok(SignerEntry { account, weight })
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

    fn uint32(&mut self) -> Result<u32, DecodeError> {
        Ok(u32::from_be_bytes(self.take()?))
    }

    fn account(&mut self) -> Result<AccountId, DecodeError> {
        Ok(AccountId(
            self.blob()?.try_into().map_err(|_| DecodeError::Value)?,
        ))
    }

    fn signature(&mut self) -> Result<Signature, DecodeError> {
        Signature::from_slice(self.blob()?).ok_or(DecodeError::Value)
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

/// A public key of a kind the ledger knows, in 33 bytes.
fn public_key(bytes: &[u8]) -> Result<PublicKey, DecodeError> {
    let bytes: [u8; 33] = bytes.try_into().map_err(|_| DecodeError::Value)?;
    PublicKey::from_bytes(bytes).ok_or(DecodeError::Value)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::indexing_slicing)]
mod tests {
    use super::*;

    #[test]
    fn the_known_fields_are_in_canonical_order() {
        assert!(TX_FIELDS.windows(2).all(|pair| pair[0] < pair[1]));
        assert_eq!(bit((ARRAY, 99)), 0);
    }

    #[test]
    fn headers_are_read_only_in_their_shortest_form() {
        let header = |bytes: &[u8]| Reader { bytes }.header();
        // Sequence (2, 4) in one byte, and the same in two or three.
        assert_eq!(header(&[0x24]), Ok(SEQUENCE));
        assert_eq!(header(&[0x20, 0x04]), Err(DecodeError::Field));
        assert_eq!(header(&[0x04, 0x02]), Err(DecodeError::Field));
        assert_eq!(header(&[0x00, 0x02, 0x04]), Err(DecodeError::Field));
        // LastLedgerSequence (2, 27) needs two, and SetFlag no more.
        assert_eq!(header(&[0x20, 0x1B]), Ok(LAST_LEDGER_SEQUENCE));
        assert_eq!(header(&[0x00, 0x02, 0x1B]), Err(DecodeError::Field));
        let mut out = Vec::new();
        field(&mut out, (16, 16));
        assert_eq!(header(&out), Ok((16, 16)));
        assert_eq!(header(&[0x20]), Err(DecodeError::Truncated));
    }
}
