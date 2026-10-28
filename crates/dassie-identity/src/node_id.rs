//! Node IDs: the short, self-certifying name of a node
//! (spec/areas/identity.md#node-id).
//!
//! An ID is the first 24 bytes of a SHA-256 of the node's two public keys:
//! its Ed25519 key and its SLH-DSA key
//! (spec/decisions/post-quantum-node-id.md). Its text is those bytes in
//! lowercase base32 (RFC 4648, no padding): 39 characters that are at once
//! an ILP address segment, a DNS label and what the owner reads, in either
//! case. On the wire it is the 24 bytes.

use core::fmt;

use dassie_oer::{Decode, DecodeError, DecodeErrorKind, Encode, Reader, Size, Writer, oid};
use sha2::{Digest, Sha256};

use crate::error::Error;
use crate::slh_dsa::SLH_DSA_PUBLIC_KEY_BYTES;

/// The `dassieNodeFingerprintV2Id` object identifier, which versions the
/// fingerprint (spec/areas/identity.md#object-identifiers). The V1 OID,
/// `.2.1`, hashed the Ed25519 key alone.
const FINGERPRINT_V2_OID: [u8; 14] = oid::encode(&[1, 3, 6, 1, 4, 1, 54392, 5, 1399, 2, 2]);

/// Length of the fingerprint input: the OID, then each key with a one-byte
/// length determinant ahead of it.
pub const HASH_INPUT_LENGTH: usize =
    FINGERPRINT_V2_OID.len() + 1 + 32 + 1 + SLH_DSA_PUBLIC_KEY_BYTES;

/// Digest bytes a node ID keeps: what it is on the wire.
pub const NODE_ID_BYTES: usize = 24;

/// Characters of a node ID's text: [`NODE_ID_BYTES`] in base32.
pub const NODE_ID_LENGTH: usize = 39;

const _: () = assert!(NODE_ID_LENGTH == (NODE_ID_BYTES * 8).div_ceil(5));

/// RFC 4648 §6, lowercase.
const ALPHABET: &[u8; 32] = b"abcdefghijklmnopqrstuvwxyz234567";

/// A node ID, held as its text, so that it can be copied, compared and
/// shown without allocating.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct NodeId {
    text: [u8; NODE_ID_LENGTH],
}

impl NodeId {
    /// The node ID of a node's Ed25519 public key and SLH-DSA public key:
    /// the first [`NODE_ID_BYTES`] of `SHA-256(input)`, where `input` is
    /// [`hash_input`](Self::hash_input). A [`NodeIdentity`](crate::NodeIdentity)
    /// holds the two and its ID.
    #[must_use]
    pub fn from_keys(public_key: &[u8; 32], slh_dsa: &[u8; SLH_DSA_PUBLIC_KEY_BYTES]) -> Self {
        let digest = Sha256::digest(Self::hash_input(public_key, slh_dsa));
        let mut bytes = [0u8; NODE_ID_BYTES];
        bytes.copy_from_slice(digest.get(..NODE_ID_BYTES).unwrap_or(&[0; NODE_ID_BYTES]));
        Self::from_bytes(&bytes)
    }

    /// The fingerprint input: the canonical OER of
    /// `SEQUENCE { versionPrefix OBJECT IDENTIFIER, nodePublicKey OCTET STRING,
    /// slhDsaPublicKey OCTET STRING }` with the V2 fingerprint OID, which is
    /// `0d 2b0601040183a878058a770202 20 <key> 20 <SLH-DSA key>`.
    #[must_use]
    pub fn hash_input(
        public_key: &[u8; 32],
        slh_dsa: &[u8; SLH_DSA_PUBLIC_KEY_BYTES],
    ) -> [u8; HASH_INPUT_LENGTH] {
        let mut input = [0u8; HASH_INPUT_LENGTH];
        let mut writer = Writer::new(&mut input);
        writer.write_bytes(&FINGERPRINT_V2_OID);
        writer.write_octets(public_key, Size::ANY);
        writer.write_octets(slh_dsa, Size::ANY);
        assert!(
            writer.position() == HASH_INPUT_LENGTH,
            "fingerprint input length"
        );
        input
    }

    /// The ID of the digest bytes `bytes`.
    #[must_use]
    pub fn from_bytes(bytes: &[u8; NODE_ID_BYTES]) -> Self {
        let mut id = Self {
            text: [0; NODE_ID_LENGTH],
        };
        let mut slots = id.text.iter_mut();
        let mut buffer: u16 = 0;
        let mut bits: u32 = 0;
        for &byte in bytes {
            buffer = (buffer << 8) | u16::from(byte);
            bits += 8;
            while bits >= 5 {
                bits -= 5;
                if let Some(slot) = slots.next() {
                    *slot = symbol(buffer >> bits);
                }
            }
            buffer &= (1 << bits) - 1;
        }
        if bits > 0
            && let Some(slot) = slots.next()
        {
            *slot = symbol(buffer << (5 - bits));
        }
        assert!(slots.next().is_none(), "an ID has 39 characters");
        id
    }

    /// The digest bytes the ID is.
    #[must_use]
    pub fn to_bytes(&self) -> [u8; NODE_ID_BYTES] {
        let mut bytes = [0u8; NODE_ID_BYTES];
        let mut slots = bytes.iter_mut();
        let mut buffer: u16 = 0;
        let mut bits: u32 = 0;
        for &character in &self.text {
            buffer = (buffer << 5) | u16::from(value(character));
            bits += 5;
            if bits >= 8 {
                bits -= 8;
                // The low byte after the shift: the next eight bits.
                let [byte, _] = (buffer >> bits).to_le_bytes();
                if let Some(slot) = slots.next() {
                    *slot = byte;
                }
                buffer &= (1 << bits) - 1;
            }
        }
        assert!(slots.next().is_none(), "an ID holds 24 bytes");
        bytes
    }

    /// Parses a node ID's text, in either case, since DNS ignores case.
    ///
    /// Whether the ID belongs to a node's keys is a separate question,
    /// answered by comparing it with [`from_keys`](Self::from_keys).
    ///
    /// # Errors
    ///
    /// [`Error::InvalidNodeId`] unless it is 39 base32 characters whose
    /// unused last bits are zero, so that each ID has one text.
    pub fn parse(text: &str) -> Result<Self, Error> {
        let characters: &[u8; NODE_ID_LENGTH] = text
            .as_bytes()
            .try_into()
            .map_err(|_| Error::InvalidNodeId)?;
        let mut id = Self {
            text: [0; NODE_ID_LENGTH],
        };
        for (slot, &character) in id.text.iter_mut().zip(characters) {
            let lower = character.to_ascii_lowercase();
            if !ALPHABET.contains(&lower) {
                return Err(Error::InvalidNodeId);
            }
            *slot = lower;
        }
        // 39 characters carry 195 bits, 3 more than the digest's 192.
        let last = id.text.last().copied().map_or(0, value);
        if last & 0b111 != 0 {
            return Err(Error::InvalidNodeId);
        }
        Ok(id)
    }

    /// The ID as text: 39 characters of `[a-z2-7]`.
    #[must_use]
    // Panics only on a broken invariant: only the alphabet is ever stored.
    #[allow(clippy::panic)]
    pub fn as_str(&self) -> &str {
        let Ok(text) = core::str::from_utf8(&self.text) else {
            panic!("node IDs hold only ASCII");
        };
        text
    }
}

/// The character of the low five bits of `quintet`.
fn symbol(quintet: u16) -> u8 {
    let index = usize::from(quintet & 0b1_1111);
    ALPHABET.get(index).copied().unwrap_or(b'a')
}

/// The five bits of a character of the alphabet; zero for any other,
/// which [`NodeId::parse`] never stores.
fn value(character: u8) -> u8 {
    match character {
        b'a'..=b'z' => character - b'a',
        b'2'..=b'7' => character - b'2' + 26,
        _ => 0,
    }
}

/// On the wire, an ID is its digest bytes, `OCTET STRING (SIZE (24))`,
/// which OER writes without a length: every 24 bytes are an ID, and each ID
/// has one encoding.
impl Encode for NodeId {
    fn encoded_len(&self) -> usize {
        NODE_ID_BYTES
    }

    fn encode_to(&self, writer: &mut Writer<'_>) {
        writer.write_bytes(&self.to_bytes());
    }
}

impl Decode<'_> for NodeId {
    type Error = DecodeError;

    fn decode(reader: &mut Reader<'_>) -> Result<Self, DecodeError> {
        let offset = reader.offset();
        let bytes = reader
            .read_array::<NODE_ID_BYTES>()
            .map_err(|_| DecodeError::new(offset, DecodeErrorKind::UnexpectedEnd))?;
        Ok(Self::from_bytes(bytes))
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
    #![allow(clippy::unwrap_used)]

    use super::*;

    #[test]
    fn the_oid_constant_is_the_v2_fingerprint() {
        let expected = [
            0x0d, 0x2b, 0x06, 0x01, 0x04, 0x01, 0x83, 0xa8, 0x78, 0x05, 0x8a, 0x77, 0x02, 0x02,
        ];
        assert_eq!(FINGERPRINT_V2_OID, expected);
        assert_eq!(HASH_INPUT_LENGTH, 80);
    }

    /// Bytes and their text, computed apart with Python's base64 module:
    /// a digest that ends in zeros, and the edges of the alphabet.
    const VECTORS: &[(&str, &str)] = &[
        (
            "34a6f038c0b0d034d3a0f338b72c950125b133834fdcc296",
            "gstpaogawdidju5a6m4lolevaes3cm4dj7omffq",
        ),
        (
            "c9b39caec750aec469ab758fa5023a918709a4ab74ff3600",
            "zgzzzlwhkcxmi2nlowh2kar2sgdqtjflot7tmaa",
        ),
        (
            "000000000000000000000000000000000000000000000000",
            "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        ),
        (
            "ffffffffffffffffffffffffffffffffffffffffffffffff",
            "77777777777777777777777777777777777777y",
        ),
    ];

    fn bytes(hex: &str) -> [u8; NODE_ID_BYTES] {
        let mut bytes = [0; NODE_ID_BYTES];
        for (index, byte) in bytes.iter_mut().enumerate() {
            *byte = u8::from_str_radix(&hex[index * 2..index * 2 + 2], 16).unwrap();
        }
        bytes
    }

    #[test]
    fn vectors_both_ways() {
        for &(hex, text) in VECTORS {
            let id = NodeId::from_bytes(&bytes(hex));
            assert_eq!(id.as_str(), text);
            assert_eq!(id.to_bytes(), bytes(hex));
            assert_eq!(NodeId::parse(text), Ok(id));
            assert_eq!(NodeId::parse(&text.to_ascii_uppercase()), Ok(id));
        }
    }

    #[test]
    fn malformed_ids_are_refused() {
        for text in [
            "",
            "aaaa",
            // One character short and one too many.
            "gstpaogawdidju5a6m4lolevaes3cm4dj7omff",
            "gstpaogawdidju5a6m4lolevaes3cm4dj7omffqa",
            // Outside the alphabet.
            "gstpaogawdidju5a6m4lolevaes3cm4dj7omff1",
            "gstpaogawdidju5a6m4lolevaes3cm4dj7omff=",
            // Unused bits set: a second spelling of the first vector.
            "gstpaogawdidju5a6m4lolevaes3cm4dj7omffr",
            // The old form, base64url after a `d`.
            "dNKbwOMCw0DTToPM4tyyVASWxM4NP3MKW",
        ] {
            assert_eq!(NodeId::parse(text), Err(Error::InvalidNodeId), "{text:?}");
        }
    }

    #[test]
    fn the_wire_carries_the_bytes() {
        let id = NodeId::from_bytes(&bytes(VECTORS[0].0));
        let mut buffer = [0; NODE_ID_BYTES + 1];
        assert_eq!(id.encode(&mut buffer), NODE_ID_BYTES);
        assert_eq!(buffer.get(..NODE_ID_BYTES), Some(&bytes(VECTORS[0].0)[..]));
        let mut reader = Reader::new(&buffer);
        assert_eq!(NodeId::decode(&mut reader), Ok(id));
        let short = [0; NODE_ID_BYTES - 1];
        assert!(NodeId::decode(&mut Reader::new(&short)).is_err());
    }
}
