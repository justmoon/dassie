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
    pub fn new(text: &str) -> Result<Self, InvalidName> {
        let source = text.as_bytes();
        if source.is_empty()
            || source.len() > MAX_NAME_LENGTH
            || !source.iter().all(|&b| allowed(b))
        {
            return Err(InvalidName);
        }
        let mut bytes = [0; MAX_NAME_LENGTH];
        let target = bytes.get_mut(..source.len()).ok_or(InvalidName)?;
        target.copy_from_slice(source);
        Ok(Self {
            length: u8::try_from(source.len()).map_err(|_| InvalidName)?,
            bytes,
        })
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
}
