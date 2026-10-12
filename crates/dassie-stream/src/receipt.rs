//! STREAM receipts (RFC 39): a receiver's signed statement of how much a
//! stream has received, which a third party holding the receipt secret can
//! verify.

use subtle::ConstantTimeEq;

use crate::crypto::hmac_sha256;

/// Encoded length of a receipt.
pub const RECEIPT_LENGTH: usize = 58;
/// The receipt format version.
pub const RECEIPT_VERSION: u8 = 1;
/// Length of the part the HMAC covers.
const SIGNED_LENGTH: usize = RECEIPT_LENGTH - 32;

/// The nonce and secret a receiver signs receipts with, shared with whoever
/// verifies them (for example an Open Payments server).
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct ReceiptSecret {
    /// Identifies the payment the receipts belong to.
    pub nonce: [u8; 16],
    /// The HMAC key.
    pub secret: [u8; 32],
}

impl core::fmt::Debug for ReceiptSecret {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("ReceiptSecret").finish_non_exhaustive()
    }
}

/// A verified receipt.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Receipt {
    /// The receipt nonce.
    pub nonce: [u8; 16],
    /// The stream the money arrived on.
    pub stream_id: u8,
    /// Everything the stream has received so far.
    pub total_received: u64,
}

/// The receipt is malformed or its HMAC does not verify.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct InvalidReceipt;

/// Creates the receipt for `stream_id` having received `total_received`.
///
/// RFC 39 encodes the stream ID in one octet, so streams above 255 cannot
/// have receipts; callers skip them.
#[must_use]
pub fn create(secret: &ReceiptSecret, stream_id: u8, total_received: u64) -> [u8; RECEIPT_LENGTH] {
    let mut receipt = [0u8; RECEIPT_LENGTH];
    let (signed, mac) = receipt.split_at_mut(SIGNED_LENGTH);
    let (version, rest) = signed.split_at_mut(1);
    let (nonce, rest) = rest.split_at_mut(16);
    let (stream, total) = rest.split_at_mut(1);
    version.copy_from_slice(&[RECEIPT_VERSION]);
    nonce.copy_from_slice(&secret.nonce);
    stream.copy_from_slice(&[stream_id]);
    total.copy_from_slice(&total_received.to_be_bytes());
    mac.copy_from_slice(&hmac_sha256(&secret.secret, signed));
    receipt
}

/// Checks `receipt` against `secret` and decodes it.
pub fn verify(receipt: &[u8], secret: &[u8; 32]) -> Result<Receipt, InvalidReceipt> {
    let receipt: &[u8; RECEIPT_LENGTH] = receipt.try_into().map_err(|_| InvalidReceipt)?;
    let (signed, mac) = receipt.split_at(SIGNED_LENGTH);
    if !bool::from(hmac_sha256(secret, signed).ct_eq(mac)) {
        return Err(InvalidReceipt);
    }
    let (version, rest) = signed.split_at(1);
    if version != [RECEIPT_VERSION] {
        return Err(InvalidReceipt);
    }
    let (nonce, rest) = rest.split_at(16);
    let (stream, total) = rest.split_at(1);
    Ok(Receipt {
        nonce: nonce.try_into().map_err(|_| InvalidReceipt)?,
        stream_id: stream.first().copied().ok_or(InvalidReceipt)?,
        total_received: u64::from_be_bytes(total.try_into().map_err(|_| InvalidReceipt)?),
    })
}
