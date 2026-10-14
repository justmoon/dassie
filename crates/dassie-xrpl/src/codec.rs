//! The binary form of an XRP `Payment` with memos: the only transaction
//! the node signs. Fields go in the ledger's canonical order, by type code
//! and then field code; the decoder takes only that order and only these
//! fields, which is all the fake ledger needs to check what it is sent.

use alloc::vec::Vec;

use sha2::{Digest, Sha512};

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
const FEE: (u8, u8) = (AMOUNT, 8);
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

/// A Payment of XRP.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Payment {
    /// Who pays.
    pub account: AccountId,
    /// Who is paid.
    pub destination: AccountId,
    /// How much, in drops.
    pub amount: u64,
    /// The fee, in drops.
    pub fee: u64,
    /// The payer's sequence number.
    pub sequence: u32,
    /// The last ledger the transaction may be in.
    pub last_ledger_sequence: u32,
    /// Flags; 0 for the node's.
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

/// A signed Payment, ready to submit.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Signed {
    /// The transaction.
    pub payment: Payment,
    /// Its binary form.
    pub blob: Vec<u8>,
    /// Its hash.
    pub hash: [u8; 32],
}

impl Signed {
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

impl Payment {
    /// What the payer signs: `STX\0` and the fields, signature left out.
    #[must_use]
    pub fn signing_payload(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(256);
        out.extend_from_slice(&SIGNING_PREFIX);
        self.encode(&mut out, false);
        out
    }

    /// Signs it with `wallet`, whose key it names.
    ///
    /// # Panics
    ///
    /// If `wallet` is not the payer's, or an amount is beyond the ledger's
    /// (bugs: the scheme builds every Payment it signs).
    #[must_use]
    pub fn sign(mut self, wallet: &Wallet) -> Signed {
        assert_eq!(
            self.signing_pub_key,
            *wallet.public_key(),
            "signed by its payer"
        );
        assert_eq!(self.account, wallet.account(), "signed by its payer");
        self.signature = Some(wallet.sign(&self.signing_payload()));
        let mut blob = Vec::with_capacity(256);
        self.encode(&mut blob, true);
        Signed {
            hash: hash(&blob),
            payment: self,
            blob,
        }
    }

    /// Its binary form, with the signature if it has one: what was
    /// submitted, since the form is canonical.
    #[must_use]
    pub fn blob(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(256);
        self.encode(&mut out, true);
        out
    }

    /// The fields in canonical order; the signature only `with_signature`.
    fn encode(&self, out: &mut Vec<u8>, with_signature: bool) {
        assert!(
            self.amount <= MAX_DROPS && self.fee <= MAX_DROPS,
            "amounts within the ledger's"
        );
        assert!(self.memos.len() <= MAX_MEMOS, "few memos");
        field(out, TRANSACTION_TYPE);
        out.extend_from_slice(&PAYMENT.to_be_bytes());
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
        field(out, AMOUNT_FIELD);
        out.extend_from_slice(&xrp(self.amount));
        field(out, FEE);
        out.extend_from_slice(&xrp(self.fee));
        field(out, SIGNING_PUB_KEY);
        blob(out, &self.signing_pub_key);
        if let (true, Some(signature)) = (with_signature, &self.signature) {
            field(out, TXN_SIGNATURE);
            blob(out, signature);
        }
        field(out, ACCOUNT);
        blob(out, &self.account.0);
        field(out, DESTINATION);
        blob(out, &self.destination.0);
        if !self.memos.is_empty() {
            field(out, MEMOS);
            for memo in &self.memos {
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

    /// The Payment in `bytes`, signed or not, if it is one the node would
    /// send: these fields, in this order, nothing else.
    ///
    /// # Errors
    ///
    /// Why it is not.
    pub fn decode(bytes: &[u8]) -> Result<Self, DecodeError> {
        let mut reader = Reader { bytes };
        reader.expect(TRANSACTION_TYPE)?;
        if u16::from_be_bytes(reader.take()?) != PAYMENT {
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
        reader.expect(AMOUNT_FIELD)?;
        let amount = from_xrp(reader.take()?)?;
        reader.expect(FEE)?;
        let fee = from_xrp(reader.take()?)?;
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
        reader.expect(DESTINATION)?;
        let destination = AccountId(reader.blob()?.try_into().map_err(|_| DecodeError::Value)?);
        let memos = reader.memos()?;
        if !reader.bytes.is_empty() {
            return Err(DecodeError::Trailing);
        }
        Ok(Self {
            account,
            destination,
            amount,
            fee,
            sequence,
            last_ledger_sequence,
            flags,
            network_id,
            memos,
            signing_pub_key,
            signature,
        })
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
            amount: 1_000,
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
        assert_eq!(Payment::decode(&signed.blob), Ok(signed.payment.clone()));
        assert_eq!(hash(&signed.blob), signed.hash);
        let unsigned = Payment {
            signature: None,
            ..signed.payment
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
}
