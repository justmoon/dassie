//! Settlement scheme IDs.

use core::fmt;

use dassie_oer::{Alphabet, DecodeError, DecodeErrorKind, Reader, Size, Writer, size};

/// Most characters in a scheme ID. v0 allowed 128; v1 allows 64, which is
/// what the node stores, and is plenty for `xrpl-testnet`.
pub const MAX_SCHEME_ID: usize = 64;

const SIZE: Size = Size::range(2, MAX_SCHEME_ID);

/// The characters of a scheme ID.
const ALPHABET: Alphabet = Alphabet::EMPTY
    .with_range(b'a', b'z')
    .with_range(b'0', b'9')
    .with_chars(b"-");

/// A settlement scheme's ID, such as `stub`: 2 to 64 characters of
/// `[a-z0-9-]`, held inline. A ledger's ID, such as `stub+usd`, joins its
/// scheme's ID and asset with `+`, which a scheme's ID never holds: the
/// original's docs used `+` in names this alphabet refused.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SchemeId {
    bytes: [u8; MAX_SCHEME_ID],
    length: u8,
}

impl SchemeId {
    /// Parses a scheme ID.
    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        let length = u8::try_from(text.len()).ok()?;
        if !SIZE.contains(text.len()) || !text.bytes().all(|byte| ALPHABET.contains(byte)) {
            return None;
        }
        let mut bytes = [0u8; MAX_SCHEME_ID];
        bytes
            .get_mut(..text.len())?
            .copy_from_slice(text.as_bytes());
        Some(Self { bytes, length })
    }

    /// The ID.
    #[must_use]
    pub fn as_str(&self) -> &str {
        let bytes = self
            .bytes
            .get(..usize::from(self.length))
            .unwrap_or_default();
        // Only ASCII is ever stored.
        core::str::from_utf8(bytes).unwrap_or_default()
    }

    pub(crate) fn read(reader: &mut Reader<'_>) -> Result<Self, DecodeError> {
        let offset = reader.offset();
        let text = reader.read_ascii(SIZE, ALPHABET)?;
        Self::parse(text).ok_or(DecodeError::new(offset, DecodeErrorKind::SizeOutOfRange))
    }

    pub(crate) fn write(&self, writer: &mut Writer<'_>) {
        writer.write_ascii(self.as_str(), SIZE, ALPHABET);
    }

    pub(crate) fn encoded_len(&self) -> usize {
        size::prefixed(usize::from(self.length))
    }
}

impl fmt::Display for SchemeId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl fmt::Debug for SchemeId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "SchemeId({})", self.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // A ledger's ID, with its `+`, is not a scheme's.
    #[test]
    fn scheme_ids_follow_the_alphabet() {
        assert_eq!(SchemeId::parse("stub").unwrap().as_str(), "stub");
        assert!(SchemeId::parse("xrpl-testnet").is_some());
        assert!(SchemeId::parse("s").is_none());
        assert!(SchemeId::parse("Stub").is_none());
        assert!(SchemeId::parse("stub+usd").is_none());
        assert!(SchemeId::parse(&"a".repeat(65)).is_none());
    }
}
