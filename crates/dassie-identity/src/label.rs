//! Node labels: a node ID as a DNS label, which front doors serve
//! Open Payments under, such as `<label>.dassiepay.net`
//! (spec/areas/identity.md#node-label).
//!
//! A node ID is base64url, which DNS cannot carry, since it ignores case.
//! The label is the lowercase base32 (RFC 4648, no padding) of the same
//! 24 digest bytes: the ID's characters after `d`, padded back to 32 with
//! the `A`s the ID stripped, then decoded. That is 39 characters, within
//! DNS's 63.

use core::fmt;

use crate::base64url;
use crate::error::Error;
use crate::node_id::NodeId;

/// Characters of a node label.
pub const NODE_LABEL_LENGTH: usize = 39;

/// Digest bytes a node ID names, and a label encodes.
const DIGEST_BYTES: usize = 24;

/// Base64url characters of those bytes.
const DIGEST_CHARACTERS: usize = 32;

/// RFC 4648 §6, lowercase.
const ALPHABET: &[u8; 32] = b"abcdefghijklmnopqrstuvwxyz234567";

const _: () = assert!(NODE_LABEL_LENGTH == (DIGEST_BYTES * 8).div_ceil(5));
const _: () = assert!(base64url::encoded_len(DIGEST_BYTES) == DIGEST_CHARACTERS);

/// A node ID as a DNS label: 39 characters of `[a-z2-7]`.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct NodeLabel {
    bytes: [u8; NODE_LABEL_LENGTH],
}

impl NodeLabel {
    /// The label of a node ID.
    ///
    /// # Errors
    ///
    /// [`Error::InvalidNodeId`] for an ID no key gives: one that does not
    /// start with `d`, is longer than 33 characters, or ends in `A`, which
    /// an ID computed from a key never does. Such an ID would share its
    /// label with another.
    pub fn of(id: &NodeId) -> Result<Self, Error> {
        let text = id.as_str().as_bytes();
        let digest_text = text.strip_prefix(b"d").ok_or(Error::InvalidNodeId)?;
        if digest_text.len() > DIGEST_CHARACTERS || digest_text.last() == Some(&b'A') {
            return Err(Error::InvalidNodeId);
        }
        let mut padded = [b'A'; DIGEST_CHARACTERS];
        for (slot, &character) in padded.iter_mut().zip(digest_text) {
            *slot = character;
        }
        let mut digest = [0u8; DIGEST_BYTES];
        let written = base64url::decode(&padded, &mut digest).map_err(|_| Error::InvalidNodeId)?;
        assert!(written == DIGEST_BYTES, "32 characters decode to 24 bytes");
        Ok(Self::encode(&digest))
    }

    /// Parses a label, in either case, since DNS ignores case.
    ///
    /// # Errors
    ///
    /// [`Error::InvalidLabel`] unless it is 39 base32 characters whose
    /// unused last bits are zero, so that each ID has one label.
    pub fn parse(text: &str) -> Result<Self, Error> {
        let bytes: &[u8; NODE_LABEL_LENGTH] = text
            .as_bytes()
            .try_into()
            .map_err(|_| Error::InvalidLabel)?;
        let mut label = Self {
            bytes: [0; NODE_LABEL_LENGTH],
        };
        for (slot, &character) in label.bytes.iter_mut().zip(bytes) {
            let lower = character.to_ascii_lowercase();
            if !ALPHABET.contains(&lower) {
                return Err(Error::InvalidLabel);
            }
            *slot = lower;
        }
        // 39 characters carry 195 bits, 3 more than the digest's 192.
        let last = label.bytes.last().copied().map_or(0, value);
        if last & 0b111 != 0 {
            return Err(Error::InvalidLabel);
        }
        Ok(label)
    }

    /// The node ID the label names.
    ///
    /// # Errors
    ///
    /// [`Error::InvalidLabel`] for the label of an all-zero digest, whose ID
    /// would be only `d`.
    pub fn node_id(&self) -> Result<NodeId, Error> {
        let digest = self.decode();
        let mut text = [0u8; 1 + DIGEST_CHARACTERS];
        let (prefix, rest) = text.split_at_mut(1);
        prefix.copy_from_slice(b"d");
        let written = base64url::encode(&digest, rest);
        assert!(written == DIGEST_CHARACTERS, "24 bytes encode to 32");
        let kept = rest
            .iter()
            .rposition(|&character| character != b'A')
            .map_or(0, |last| last + 1);
        let id = text.get(..=kept).unwrap_or_default();
        let id = core::str::from_utf8(id).map_err(|_| Error::InvalidLabel)?;
        NodeId::parse(id).map_err(|_| Error::InvalidLabel)
    }

    /// The label as text.
    #[must_use]
    // Panics only on a broken invariant: only the alphabet is ever stored.
    #[allow(clippy::panic)]
    pub fn as_str(&self) -> &str {
        let Ok(text) = core::str::from_utf8(&self.bytes) else {
            panic!("node labels hold only ASCII");
        };
        text
    }

    fn encode(digest: &[u8; DIGEST_BYTES]) -> Self {
        let mut label = Self {
            bytes: [0; NODE_LABEL_LENGTH],
        };
        let mut slots = label.bytes.iter_mut();
        let mut buffer: u16 = 0;
        let mut bits: u32 = 0;
        for &byte in digest {
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
        assert!(slots.next().is_none(), "a label has 39 characters");
        label
    }

    fn decode(&self) -> [u8; DIGEST_BYTES] {
        let mut digest = [0u8; DIGEST_BYTES];
        let mut bytes = digest.iter_mut();
        let mut buffer: u16 = 0;
        let mut bits: u32 = 0;
        for &character in &self.bytes {
            buffer = (buffer << 5) | u16::from(value(character));
            bits += 5;
            if bits >= 8 {
                bits -= 8;
                // The low byte after the shift: the next eight bits.
                let [byte, _] = (buffer >> bits).to_le_bytes();
                if let Some(slot) = bytes.next() {
                    *slot = byte;
                }
                buffer &= (1 << bits) - 1;
            }
        }
        assert!(bytes.next().is_none(), "a label holds 24 bytes");
        digest
    }
}

/// The character of the low five bits of `quintet`.
fn symbol(quintet: u16) -> u8 {
    let index = usize::from(quintet & 0b1_1111);
    ALPHABET.get(index).copied().unwrap_or(b'a')
}

/// The five bits of a character of the alphabet; zero for any other,
/// which [`NodeLabel::parse`] never stores.
fn value(character: u8) -> u8 {
    match character {
        b'a'..=b'z' => character - b'a',
        b'2'..=b'7' => character - b'2' + 26,
        _ => 0,
    }
}

impl NodeId {
    /// The node's label, as front doors serve it.
    ///
    /// # Errors
    ///
    /// As [`NodeLabel::of`].
    pub fn label(&self) -> Result<NodeLabel, Error> {
        NodeLabel::of(self)
    }

    /// The node ID a label names.
    ///
    /// # Errors
    ///
    /// As [`NodeLabel::parse`] and [`NodeLabel::node_id`].
    pub fn from_label(text: &str) -> Result<Self, Error> {
        NodeLabel::parse(text)?.node_id()
    }
}

impl AsRef<str> for NodeLabel {
    fn as_ref(&self) -> &str {
        self.as_str()
    }
}

impl fmt::Display for NodeLabel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl fmt::Debug for NodeLabel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("NodeLabel").field(&self.as_str()).finish()
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    /// Computed apart, by Node.js's base64url and a base32 of its own.
    const VECTORS: &[(&str, &str)] = &[
        (
            "dNKbwOMCw0DTToPM4tyyVASWxM4NP3MKW",
            "gstpaogawdidju5a6m4lolevaes3cm4dj7omffq",
        ),
        (
            "dyrjUANXiJ5S6JXxwo7jdKKJjdLtK129U",
            "zk4niagv4itzjorfprykhog5fcrgg5f3jllw6va",
        ),
        (
            "dDHq9ng9J1z7luQ64eN4MJ_4_w-KK4FPk",
            "br5l3hqpjhlt5znzb24hrxqme77d7q7crlqfhza",
        ),
        // Digests that ended in `A`s, which the IDs strip.
        (
            "dybOcrsdQrsRpq3WPpQI6kYcJpKt0_zY",
            "zgzzzlwhkcxmi2nlowh2kar2sgdqtjflot7tmaa",
        ),
        ("d_", "7qaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"),
        ("dabc", "ng3qaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"),
        ("dAAAB", "aaaacaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"),
        (
            "d________________________________",
            "77777777777777777777777777777777777777y",
        ),
    ];

    #[test]
    fn vectors_both_ways() {
        for &(id, label) in VECTORS {
            let parsed = NodeId::parse(id).unwrap();
            assert_eq!(
                parsed.label().map(|l| l.as_str() == label),
                Ok(true),
                "{id}"
            );
            assert_eq!(NodeId::from_label(label), Ok(parsed), "{label}");
            let upper = label.to_ascii_uppercase();
            assert_eq!(NodeId::from_label(&upper), Ok(parsed), "{upper}");
        }
    }

    #[test]
    fn ids_no_key_gives_have_no_label() {
        for id in ["dabcA", "xabc", "d012345678901234567890123456789012"] {
            let parsed = NodeId::parse(id).unwrap();
            assert_eq!(
                parsed.label().map(|_| ()),
                Err(Error::InvalidNodeId),
                "{id}"
            );
        }
    }

    #[test]
    fn malformed_labels_are_refused() {
        for label in [
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
            // The all-zero digest, whose ID would be only `d`.
            "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        ] {
            assert_eq!(
                NodeId::from_label(label).map(|_| ()),
                Err(Error::InvalidLabel),
                "{label}"
            );
        }
    }
}
