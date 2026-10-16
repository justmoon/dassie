//! Short identifiers stored inline: node IDs, scheme IDs, settlement IDs.

use core::fmt;

/// The longest [`Name`], in bytes. Node IDs have at most 33 characters and
/// XRPL transaction hashes 64.
pub const MAX_NAME_LENGTH: usize = 64;

/// A non-empty identifier of at most [`MAX_NAME_LENGTH`] characters from
/// `[A-Za-z0-9_-]`: base64url, hex and the scheme IDs all fit, and none of
/// the separators of an account path does. Stored inline, so accounts and
/// references don't allocate.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Name {
    length: u8,
    bytes: [u8; MAX_NAME_LENGTH],
}

/// A string that is not a [`Name`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct InvalidName;

impl fmt::Display for InvalidName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "not a name: 1 to {MAX_NAME_LENGTH} characters from [A-Za-z0-9_-]"
        )
    }
}

const fn allowed(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-'
}

impl Name {
    /// Checks and copies `text`.
    ///
    /// # Errors
    ///
    /// If it is empty, too long, or has a character outside the alphabet.
    pub const fn new(text: &str) -> Result<Self, InvalidName> {
        let source = text.as_bytes();
        if source.is_empty() || source.len() > MAX_NAME_LENGTH {
            return Err(InvalidName);
        }
        let mut bytes = [0; MAX_NAME_LENGTH];
        let mut length: u8 = 0;
        let mut from = source;
        let mut to: &mut [u8] = &mut bytes;
        while let ([byte, rest @ ..], [slot, more @ ..]) = (from, to) {
            if !allowed(*byte) {
                return Err(InvalidName);
            }
            *slot = *byte;
            length += 1;
            from = rest;
            to = more;
        }
        Ok(Self { length, bytes })
    }

    /// `bytes` in hex, two digits a byte: uppercase as XRPL transaction
    /// hashes, or lowercase as the original's settlement IDs. The build
    /// fails for more bytes than a name holds.
    #[must_use]
    pub fn hex<const N: usize>(bytes: &[u8; N], uppercase: bool) -> Self {
        const { assert!(N > 0 && 2 * N <= MAX_NAME_LENGTH) };
        let digit = |nibble: u8| match nibble {
            0..=9 => b'0' + nibble,
            _ if uppercase => b'A' + nibble - 10,
            _ => b'a' + nibble - 10,
        };
        let mut name = Self {
            length: 0,
            bytes: [0; MAX_NAME_LENGTH],
        };
        for (pair, &byte) in name.bytes.as_chunks_mut::<2>().0.iter_mut().zip(bytes) {
            *pair = [digit(byte >> 4), digit(byte & 15)];
            name.length += 2;
        }
        name
    }

    /// The name.
    #[must_use]
    pub fn as_str(&self) -> &str {
        let bytes = self
            .bytes
            .get(..usize::from(self.length))
            .unwrap_or_default();
        // Only ASCII is ever stored.
        core::str::from_utf8(bytes).unwrap_or_default()
    }
}

/// Validates a string literal as a [`Name`] at compile time.
#[macro_export]
macro_rules! name {
    ($text:expr) => {{
        const NAME: $crate::Name = match $crate::Name::new($text) {
            Ok(name) => name,
            Err(_) => panic!("invalid name literal"),
        };
        NAME
    }};
}

impl fmt::Display for Name {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl fmt::Debug for Name {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:?}", self.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn checks_the_alphabet_and_length() {
        assert_eq!(
            Name::new("dNKbwOMCw0DTToPM4tyyVASWxM4NP3MKW")
                .unwrap()
                .as_str(),
            "dNKbwOMCw0DTToPM4tyyVASWxM4NP3MKW"
        );
        assert!(Name::new("").is_err());
        assert!(Name::new("a/b").is_err());
        assert!(Name::new("a:b").is_err());
        assert!(Name::new("a.b").is_err());
        assert!(Name::new("é").is_err());
        assert!(Name::new(&"a".repeat(64)).is_ok());
        assert!(Name::new(&"a".repeat(65)).is_err());
    }

    #[test]
    fn hex_names_spell_their_bytes() {
        assert_eq!(Name::hex(&[0x01, 0xab, 0xff], false).as_str(), "01abff");
        assert_eq!(Name::hex(&[0x01, 0xab, 0xff], true).as_str(), "01ABFF");
        assert_eq!(Name::hex(&[0x5a; 32], true).as_str(), "5A".repeat(32));
        assert_eq!(name!("stub"), Name::new("stub").unwrap());
    }
}
