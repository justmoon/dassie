//! Node IDs: the short, self-certifying name of a node
//! (spec/areas/identity.md#node-id).

use core::cmp::Ordering;
use core::fmt;
use core::hash::{Hash, Hasher};

use dassie_oer::{Alphabet, Size, Writer, oid};
use sha2::{Digest, Sha256};

use crate::base64url;
use crate::error::Error;

/// The `dassieNodeFingerprintV1Id` object identifier, which versions the
/// fingerprint (spec/areas/identity.md#object-identifiers).
const FINGERPRINT_V1_OID: [u8; 14] = oid::encode(&[1, 3, 6, 1, 4, 1, 54392, 5, 1399, 2, 1]);

/// Length of the fingerprint input for a 32-byte key: the OID, a one-byte
/// length determinant and the key.
pub const HASH_INPUT_LENGTH: usize = FINGERPRINT_V1_OID.len() + 1 + 32;

/// Base64url characters of the digest kept in an ID, before stripping.
const DIGEST_CHARACTERS: usize = 32;

/// Digest bytes that encode to exactly [`DIGEST_CHARACTERS`] characters.
const DIGEST_BYTES: usize = DIGEST_CHARACTERS / 4 * 3;

/// The peer wire schema of a node ID (spec/areas/identity.md#node-id).
const NODE_ID_SIZE: Size = Size::range(2, MAX_NODE_ID_LENGTH);
const NODE_ID_ALPHABET: Alphabet = Alphabet::EMPTY
    .with_range(b'a', b'z')
    .with_range(b'A', b'Z')
    .with_range(b'0', b'9')
    .with_chars(b"-_");

/// Longest node ID the wire schema allows. IDs computed from keys are at
/// most 33 characters; the schema leaves room for longer ones.
pub const MAX_NODE_ID_LENGTH: usize = 45;

/// A node ID, held inline as up to [`MAX_NODE_ID_LENGTH`] ASCII characters
/// so that it can be copied and compared without allocating.
#[derive(Clone, Copy)]
pub struct NodeId {
    bytes: [u8; MAX_NODE_ID_LENGTH],
    length: u8,
}

impl NodeId {
    /// The node ID of an Ed25519 public key:
    /// `"d" + base64url(SHA-256(input))[0..32]` with trailing `A`s removed,
    /// where `input` is [`hash_input`](Self::hash_input).
    ///
    /// Stripping the `A`s is a quirk of the original, kept because node IDs
    /// must not change (spec/areas/identity.md#node-id).
    #[must_use]
    pub fn from_public_key(public_key: &[u8; 32]) -> Self {
        let digest = Sha256::digest(Self::hash_input(public_key));
        let mut encoded = [0u8; DIGEST_CHARACTERS];
        let written =
            base64url::encode(digest.get(..DIGEST_BYTES).unwrap_or_default(), &mut encoded);
        assert!(
            written == DIGEST_CHARACTERS,
            "digest prefix encodes to 32 characters"
        );

        let kept = encoded
            .iter()
            .rposition(|&character| character != b'A')
            .map_or(0, |last| last + 1);
        let mut id = Self {
            bytes: [0; MAX_NODE_ID_LENGTH],
            length: 0,
        };
        id.push(b'd');
        for &character in encoded.iter().take(kept) {
            id.push(character);
        }
        assert!(
            id.len() <= 1 + DIGEST_CHARACTERS,
            "computed IDs are at most 33 characters"
        );
        id
    }

    /// The fingerprint input: the canonical OER of
    /// `SEQUENCE { versionPrefix OBJECT IDENTIFIER, nodePublicKey OCTET STRING }`
    /// with the V1 fingerprint OID, which is
    /// `0d 2b0601040183a878058a770201 20 <key>`.
    #[must_use]
    pub fn hash_input(public_key: &[u8; 32]) -> [u8; HASH_INPUT_LENGTH] {
        let mut input = [0u8; HASH_INPUT_LENGTH];
        let mut writer = Writer::new(&mut input);
        writer.write_bytes(&FINGERPRINT_V1_OID);
        writer.write_octets(public_key, Size::ANY);
        assert!(
            writer.position() == HASH_INPUT_LENGTH,
            "fingerprint input length"
        );
        input
    }

    /// Parses a node ID received from elsewhere, applying the wire schema:
    /// 2 to 45 characters of `[A-Za-z0-9_-]`.
    ///
    /// Whether the ID belongs to a key is a separate question, answered by
    /// comparing it with [`from_public_key`](Self::from_public_key).
    pub fn parse(text: &str) -> Result<Self, Error> {
        let valid =
            NODE_ID_SIZE.contains(text.len()) && NODE_ID_ALPHABET.contains_all(text.as_bytes());
        if !valid {
            return Err(Error::InvalidNodeId);
        }
        let mut id = Self {
            bytes: [0; MAX_NODE_ID_LENGTH],
            length: 0,
        };
        for &character in text.as_bytes() {
            id.push(character);
        }
        Ok(id)
    }

    /// The ID as text.
    #[must_use]
    // Panics only on a broken invariant: only ASCII is ever stored.
    #[allow(clippy::panic)]
    pub fn as_str(&self) -> &str {
        let bytes = self.bytes.get(..self.len()).unwrap_or_default();
        let Ok(text) = core::str::from_utf8(bytes) else {
            panic!("node IDs hold only ASCII");
        };
        text
    }

    /// Length in characters.
    #[must_use]
    pub fn len(&self) -> usize {
        usize::from(self.length)
    }

    /// Always false: a node ID has at least two characters. Present because
    /// a `len` without it reads as an oversight.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        false
    }

    // Panics only on a broken invariant: callers check the length first.
    #[allow(clippy::panic)]
    fn push(&mut self, character: u8) {
        assert!(
            NODE_ID_ALPHABET.contains(character),
            "node ID character outside the alphabet"
        );
        let length = self.len();
        let Some(slot) = self.bytes.get_mut(length) else {
            panic!("node ID longer than {MAX_NODE_ID_LENGTH} characters");
        };
        *slot = character;
        self.length += 1;
    }
}

impl PartialEq for NodeId {
    fn eq(&self, other: &Self) -> bool {
        self.as_str() == other.as_str()
    }
}

impl Eq for NodeId {}

impl PartialOrd for NodeId {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for NodeId {
    fn cmp(&self, other: &Self) -> Ordering {
        self.as_str().cmp(other.as_str())
    }
}

impl Hash for NodeId {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.as_str().hash(state);
    }
}

impl AsRef<str> for NodeId {
    fn as_ref(&self) -> &str {
        self.as_str()
    }
}

impl fmt::Display for NodeId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl fmt::Debug for NodeId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("NodeId").field(&self.as_str()).finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_oid_constant_matches_spec_3() {
        let expected = [
            0x0d, 0x2b, 0x06, 0x01, 0x04, 0x01, 0x83, 0xa8, 0x78, 0x05, 0x8a, 0x77, 0x02, 0x01,
        ];
        assert_eq!(FINGERPRINT_V1_OID, expected);
        assert_eq!(HASH_INPUT_LENGTH, 47);
    }

    #[test]
    fn parse_applies_the_wire_schema() {
        assert_eq!(NodeId::parse("d").map(|_| ()), Err(Error::InvalidNodeId));
        assert_eq!(NodeId::parse("d.").map(|_| ()), Err(Error::InvalidNodeId));
        assert_eq!(NodeId::parse("dé").map(|_| ()), Err(Error::InvalidNodeId));
        let longest = "d01234567890123456789012345678901234567890123";
        assert_eq!(NodeId::parse(longest).map(|id| id.len()), Ok(45));
        let too_long = "d012345678901234567890123456789012345678901234";
        assert_eq!(
            NodeId::parse(too_long).map(|_| ()),
            Err(Error::InvalidNodeId)
        );
    }
}
