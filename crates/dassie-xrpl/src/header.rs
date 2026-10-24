//! A ledger's header, as rippled serializes it, and its hash: the ledger's
//! own hash, which validators sign, and from which its parent, its
//! transactions and its state are proven
//! (spec/designs/xrpl-validation.md#4-what-is-proven-and-how).

use crate::MAX_DROPS;
use crate::codec::DecodeError;
use crate::keys::sha512_half;

/// A header's length, without the hash: what `ledger` with `binary` gives
/// as `ledger_data`.
pub const HEADER_BYTES: usize = 118;

/// What a ledger's hash covers, before the header.
const LEDGER_PREFIX: [u8; 4] = *b"LWR\0";

/// The flag of a ledger whose validators did not agree on a close time.
pub const NO_CONSENSUS_TIME: u8 = 0x01;

/// A ledger's header. Times are seconds since 2000-01-01 (the ledger's
/// epoch).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LedgerHeader {
    /// Its index.
    pub sequence: u32,
    /// The XRP there is, in drops.
    pub total_drops: u64,
    /// Its parent's hash.
    pub parent_hash: [u8; 32],
    /// The root of its transaction tree.
    pub transaction_hash: [u8; 32],
    /// The root of its state tree.
    pub account_hash: [u8; 32],
    /// When its parent closed.
    pub parent_close_time: u32,
    /// When it closed.
    pub close_time: u32,
    /// How finely close times were rounded, in seconds.
    pub close_time_resolution: u8,
    /// [`NO_CONSENSUS_TIME`], or none.
    pub close_flags: u8,
}

impl LedgerHeader {
    /// The header in `bytes`, if it is one: exactly [`HEADER_BYTES`], with
    /// no more XRP than there is.
    ///
    /// # Errors
    ///
    /// Another length ([`DecodeError::Truncated`] or
    /// [`DecodeError::Trailing`]), or more XRP ([`DecodeError::Value`]).
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, DecodeError> {
        let bytes: &[u8; HEADER_BYTES] = match bytes.len() {
            HEADER_BYTES => bytes.try_into().map_err(|_| DecodeError::Truncated)?,
            length if length < HEADER_BYTES => return Err(DecodeError::Truncated),
            _ => return Err(DecodeError::Trailing),
        };
        let mut reader = Reader { rest: bytes };
        let header = Self {
            sequence: u32::from_be_bytes(reader.take()?),
            total_drops: u64::from_be_bytes(reader.take()?),
            parent_hash: reader.take()?,
            transaction_hash: reader.take()?,
            account_hash: reader.take()?,
            parent_close_time: u32::from_be_bytes(reader.take()?),
            close_time: u32::from_be_bytes(reader.take()?),
            close_time_resolution: u8::from_be_bytes(reader.take()?),
            close_flags: u8::from_be_bytes(reader.take()?),
        };
        if header.total_drops > MAX_DROPS {
            return Err(DecodeError::Value);
        }
        Ok(header)
    }

    /// Its bytes, as rippled writes them.
    #[must_use]
    pub fn to_bytes(&self) -> [u8; HEADER_BYTES] {
        let mut out = [0; HEADER_BYTES];
        let parts: [&[u8]; 9] = [
            &self.sequence.to_be_bytes(),
            &self.total_drops.to_be_bytes(),
            &self.parent_hash,
            &self.transaction_hash,
            &self.account_hash,
            &self.parent_close_time.to_be_bytes(),
            &self.close_time.to_be_bytes(),
            &[self.close_time_resolution],
            &[self.close_flags],
        ];
        let mut at = 0;
        for part in parts {
            let end = at + part.len();
            if let Some(slot) = out.get_mut(at..end) {
                slot.copy_from_slice(part);
            }
            at = end;
        }
        assert_eq!(at, HEADER_BYTES, "a header's fields fill it");
        out
    }

    /// Its hash: the ledger's.
    #[must_use]
    pub fn hash(&self) -> [u8; 32] {
        let mut message = [0; LEDGER_PREFIX.len() + HEADER_BYTES];
        let (prefix, header) = message.split_at_mut(LEDGER_PREFIX.len());
        prefix.copy_from_slice(&LEDGER_PREFIX);
        header.copy_from_slice(&self.to_bytes());
        sha512_half(&message)
    }
}

struct Reader<'a> {
    rest: &'a [u8],
}

impl Reader<'_> {
    fn take<const N: usize>(&mut self) -> Result<[u8; N], DecodeError> {
        let (head, rest) = self
            .rest
            .split_first_chunk::<N>()
            .ok_or(DecodeError::Truncated)?;
        self.rest = rest;
        Ok(*head)
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::indexing_slicing)]
mod tests {
    use super::*;
    use crate::{from_hex, hex_upper};

    // Mainnet ledger 107862101 (testdata of `tests/golden_ledger.rs`).
    const DATA: &str = "066DD8550163385C1423D34230370EAB7D909099E41E84F1CF4D076CD2A580BF203208BC9C8196F20056D413E23B32994211E797940144B6660D1C35D97B3B76A01A852881A14B4939AD8A9C3A0E6A29538970354695E6223BDC2FDBE6543D2D306E9FC0E5CD35562F71E8B8326ED516326ED5170A00";

    #[test]
    fn a_mainnet_header_hashes_to_its_ledger() {
        let bytes = from_hex(DATA).unwrap();
        let header = LedgerHeader::from_bytes(&bytes).unwrap();
        assert_eq!(header.sequence, 107_862_101);
        assert_eq!(header.total_drops, 99_985_584_858_321_730);
        assert_eq!(header.close_time, 846_124_311);
        assert_eq!(header.parent_close_time, 846_124_310);
        assert_eq!(header.close_time_resolution, 10);
        assert_eq!(header.to_bytes().as_slice(), bytes.as_slice());
        assert_eq!(
            hex_upper(&header.hash()),
            "96FBE0EB96837471EAA8E77BA3D8801E8E0D227167AA98ED3A8A6A63148ED609"
        );
    }

    #[test]
    fn only_a_whole_header_with_the_xrp_there_is_reads() {
        let bytes = from_hex(DATA).unwrap();
        assert_eq!(
            LedgerHeader::from_bytes(&bytes[..117]),
            Err(DecodeError::Truncated)
        );
        assert_eq!(
            LedgerHeader::from_bytes(&[bytes.as_slice(), &[0]].concat()),
            Err(DecodeError::Trailing)
        );
        let mut more = bytes;
        more[4..12].copy_from_slice(&(MAX_DROPS + 1).to_be_bytes());
        assert_eq!(LedgerHeader::from_bytes(&more), Err(DecodeError::Value));
    }
}
