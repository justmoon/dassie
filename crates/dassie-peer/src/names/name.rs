//! What a name is (spec/designs/names.md#7-names): a DNS label, so that
//! front doors can serve it as `<name>.<domain>`.

use core::cmp::Ordering;
use core::fmt;

use dassie_oer::{Alphabet, DecodeError, DecodeErrorKind, Reader, Size, Writer, size};

/// Fewest characters in a name.
pub const MIN_NAME_BYTES: usize = 3;

/// Most characters in a name: fewer than a node label's 39
/// (spec/areas/identity.md#node-label), so that no name is ever read as a
/// label, nor a label as a name.
pub const MAX_NAME_BYTES: usize = 32;

const _: () = assert!(MAX_NAME_BYTES < dassie_identity::NODE_LABEL_LENGTH);

/// A name's size on the wire.
const SIZE: Size = Size::range(MIN_NAME_BYTES, MAX_NAME_BYTES);

/// The characters of a name: a DNS label's, in lowercase only, so that a
/// name has one spelling, and so one commitment and one holder.
const ALPHABET: Alphabet = Alphabet::EMPTY
    .with_range(b'a', b'z')
    .with_range(b'0', b'9')
    .with_chars(b"-");

/// Names no owner may register: those a front door's domain, or the
/// project, may need for itself, and those that would pass for the
/// project's or a service's own. Sorted, so that a name is looked up by
/// halves. A change to it changes only what registrars take: lists and
/// messages read any name of the right syntax.
pub const RESERVED: &[&str] = &[
    "abuse",
    "account",
    "admin",
    "administrator",
    "api",
    "app",
    "auth",
    "autoconfig",
    "autodiscover",
    "blog",
    "bootstrap",
    "dassie",
    "dassiepay",
    "dns",
    "docs",
    "download",
    "example",
    "front-door",
    "ftp",
    "get",
    "gnap",
    "help",
    "hostmaster",
    "ilp",
    "imap",
    "info",
    "interledger",
    "invalid",
    "local",
    "localhost",
    "login",
    "mail",
    "mta-sts",
    "node",
    "official",
    "open-payments",
    "openpayments",
    "owner",
    "pay",
    "payments",
    "pop",
    "postmaster",
    "registrar",
    "relay",
    "root",
    "security",
    "smtp",
    "status",
    "support",
    "system",
    "test",
    "update",
    "wallet",
    "webmaster",
    "wpad",
    "www",
];

/// Why text is not a name.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NameError {
    /// Shorter than [`MIN_NAME_BYTES`] or longer than [`MAX_NAME_BYTES`].
    Length,
    /// A character other than `a`-`z`, `0`-`9` and `-`.
    Character,
    /// It starts or ends with `-`.
    Hyphen,
    /// Its third and fourth characters are `--`, which DNS keeps for
    /// encodings such as `xn--`, internationalized names (RFC 5890,
    /// section 2.3.1). None are taken at first.
    Encoded,
    /// It is [reserved](RESERVED).
    Reserved,
}

impl fmt::Display for NameError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Length => "a name has 3 to 32 characters",
            Self::Character => "a name has only lowercase letters, digits and hyphens",
            Self::Hyphen => "a name neither starts nor ends with a hyphen",
            Self::Encoded => "a name has no hyphens in its third and fourth places",
            Self::Reserved => "the name is reserved",
        })
    }
}

/// A name: 3 to 32 characters of `[a-z0-9-]`, neither starting nor ending
/// with `-`, without `--` in its third and fourth places, held inline.
/// Names order by their bytes, as lists sort them.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct Name {
    bytes: [u8; MAX_NAME_BYTES],
    length: u8,
}

impl Name {
    /// Parses a name: its syntax alone, which is what the wire takes. A
    /// [reserved](RESERVED) name parses; [`Name::registrable`] refuses it.
    ///
    /// # Errors
    ///
    /// The text is not a name.
    pub fn parse(text: &str) -> Result<Self, NameError> {
        let raw = text.as_bytes();
        let length = u8::try_from(raw.len()).map_err(|_| NameError::Length)?;
        if !SIZE.contains(raw.len()) {
            return Err(NameError::Length);
        }
        if !ALPHABET.contains_all(raw) {
            return Err(NameError::Character);
        }
        if raw.first() == Some(&b'-') || raw.last() == Some(&b'-') {
            return Err(NameError::Hyphen);
        }
        if raw.get(2..4) == Some(b"--") {
            return Err(NameError::Encoded);
        }
        let mut bytes = [0u8; MAX_NAME_BYTES];
        bytes
            .get_mut(..raw.len())
            .ok_or(NameError::Length)?
            .copy_from_slice(raw);
        Ok(Self { bytes, length })
    }

    /// Parses a name an owner may register: one that is not
    /// [reserved](RESERVED).
    ///
    /// # Errors
    ///
    /// The text is not a name, or the name is reserved.
    pub fn registrable(text: &str) -> Result<Self, NameError> {
        let name = Self::parse(text)?;
        if name.is_reserved() {
            return Err(NameError::Reserved);
        }
        Ok(name)
    }

    /// Whether no owner may register it.
    #[must_use]
    pub fn is_reserved(&self) -> bool {
        RESERVED.binary_search(&self.as_str()).is_ok()
    }

    /// The name.
    #[must_use]
    pub fn as_str(&self) -> &str {
        // Only ASCII is ever stored.
        core::str::from_utf8(self.as_bytes()).unwrap_or_default()
    }

    /// The name's bytes.
    #[must_use]
    pub fn as_bytes(&self) -> &[u8] {
        self.bytes
            .get(..usize::from(self.length))
            .unwrap_or_default()
    }
}

impl Ord for Name {
    fn cmp(&self, other: &Self) -> Ordering {
        self.as_bytes().cmp(other.as_bytes())
    }
}

impl PartialOrd for Name {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl fmt::Display for Name {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl fmt::Debug for Name {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Name({})", self.as_str())
    }
}

/// Reads a name: an `IA5String (SIZE (3..32))` of the name's alphabet, which
/// must also be a name.
pub(crate) fn read_name(reader: &mut Reader<'_>) -> Result<Name, DecodeError> {
    let offset = reader.offset();
    let text = reader.read_ascii(SIZE, ALPHABET)?;
    Name::parse(text).map_err(|_| DecodeError::new(offset, DecodeErrorKind::InvalidCharacter))
}

/// Writes a name.
pub(crate) fn write_name(writer: &mut Writer<'_>, name: &Name) {
    writer.write_ascii(name.as_str(), SIZE, ALPHABET);
}

/// The encoded length of a name.
pub(crate) fn name_len(name: &Name) -> usize {
    size::prefixed(name.as_bytes().len())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_dns_labels() {
        for name in ["alice", "a-b", "abc", "x1-2", "0day", &"a".repeat(32)] {
            assert_eq!(Name::parse(name).unwrap().as_str(), name);
        }
        let cases = [
            ("ab", NameError::Length),
            ("", NameError::Length),
            (&*"a".repeat(33), NameError::Length),
            (&*"a".repeat(300), NameError::Length),
            ("Alice", NameError::Character),
            ("al_ce", NameError::Character),
            ("al.ce", NameError::Character),
            ("alïce", NameError::Character),
            ("-alice", NameError::Hyphen),
            ("alice-", NameError::Hyphen),
            ("---", NameError::Hyphen),
            ("xn--bcher-kva", NameError::Encoded),
            ("ab--c", NameError::Encoded),
        ];
        for (text, error) in cases {
            assert_eq!(Name::parse(text), Err(error), "{text}");
        }
        // Hyphens elsewhere are fine.
        assert!(Name::parse("a--b").is_ok());
        assert!(Name::parse("abc--d").is_ok());
    }

    // No name is as long as a node label, so neither is read as the other.
    #[test]
    fn no_name_is_a_node_label() {
        let label = dassie_identity::NodeKey::from_bytes(&[1; 32])
            .node_id()
            .label()
            .unwrap();
        assert_eq!(Name::parse(label.as_str()), Err(NameError::Length));
    }

    #[test]
    fn reserved_names_parse_but_are_not_registrable() {
        assert!(RESERVED.windows(2).all(|pair| pair[0] < pair[1]));
        for reserved in RESERVED {
            let name = Name::parse(reserved).unwrap();
            assert!(name.is_reserved(), "{reserved}");
            assert_eq!(Name::registrable(reserved), Err(NameError::Reserved));
        }
        assert!(!Name::parse("alice").unwrap().is_reserved());
        assert_eq!(Name::registrable("alice"), Name::parse("alice"));
        assert_eq!(Name::registrable("Www"), Err(NameError::Character));
    }

    #[test]
    fn names_order_by_their_bytes() {
        let names =
            ["a-b", "aaa", "aaaa", "ab0", "abc", "b00", "zzz"].map(|n| Name::parse(n).unwrap());
        assert!(names.windows(2).all(|pair| pair[0] < pair[1]));
    }
}
