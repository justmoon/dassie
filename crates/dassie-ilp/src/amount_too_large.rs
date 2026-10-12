use dassie_oer::{Decode, DecodeError, Encode, Reader, Writer};

use crate::error_code::ErrorCode;
use crate::packet::Reject;

/// The data of an F08 Amount Too Large reject (RFC 27): how much arrived
/// and how much the rejecting hop accepts, both in its units. Senders scale
/// their packets down by the ratio (STREAM, RFC 29 §6.4).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AmountTooLarge {
    /// The amount the rejecting hop received.
    pub received_amount: u64,
    /// The largest amount it would have accepted.
    pub maximum_amount: u64,
}

impl AmountTooLarge {
    /// Encoded length: two fixed 64-bit integers.
    pub const LENGTH: usize = 16;

    /// The encoding, for use as a reject's `data`.
    #[must_use]
    pub fn to_bytes(self) -> [u8; Self::LENGTH] {
        let mut output = [0; Self::LENGTH];
        self.encode(&mut output);
        output
    }

    /// Reads the F08 data from `reject`, if it is an F08 reject with
    /// well-formed data. The original connector sent F08 rejects without data
    /// (ILP-4), so callers must cope with `None`.
    #[must_use]
    pub fn from_reject(reject: &Reject<'_>) -> Option<Self> {
        if reject.code != ErrorCode::F08_AMOUNT_TOO_LARGE {
            return None;
        }
        Self::decode_exact(reject.data).ok()
    }
}

impl Encode for AmountTooLarge {
    fn encoded_len(&self) -> usize {
        Self::LENGTH
    }

    fn encode_to(&self, writer: &mut Writer<'_>) {
        writer.write_u64(self.received_amount);
        writer.write_u64(self.maximum_amount);
    }
}

impl Decode<'_> for AmountTooLarge {
    type Error = DecodeError;

    fn decode(reader: &mut Reader<'_>) -> Result<Self, DecodeError> {
        Ok(Self {
            received_amount: reader.read_u64()?,
            maximum_amount: reader.read_u64()?,
        })
    }
}
