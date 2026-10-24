//! Base64 in its standard alphabet, padded, as validator lists carry their
//! blobs and manifests (spec/designs/xrpl-validation.md#5-the-unl-from-its-publishers).
//!
//! Read in its one form only: every group of four characters whole, with
//! `=` only as the last group's padding, and the bits padding leaves over
//! zero. So two texts never read as the same bytes.

use alloc::string::String;
use alloc::vec::Vec;

const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

/// The padding character.
const PAD: u8 = b'=';

/// The value of one character, if it is in the alphabet.
fn value(character: u8) -> Option<u32> {
    let value = match character {
        b'A'..=b'Z' => character - b'A',
        b'a'..=b'z' => character - b'a' + 26,
        b'0'..=b'9' => character - b'0' + 52,
        b'+' => 62,
        b'/' => 63,
        _ => return None,
    };
    Some(u32::from(value))
}

/// The bytes `text` encodes, if it is base64 in its one form.
#[must_use]
pub fn decode(text: &str) -> Option<Vec<u8>> {
    let (groups, rest) = text.as_bytes().as_chunks::<4>();
    if !rest.is_empty() {
        return None;
    }
    let mut out = Vec::with_capacity(groups.len() * 3);
    let last = groups.len().checked_sub(1);
    for (index, group) in groups.iter().enumerate() {
        // Padding only at the end of the last group: `xx==` or `xxx=`.
        let padding = match group {
            [_, _, PAD, PAD] => 2,
            [_, _, _, PAD] => 1,
            _ => 0,
        };
        if padding > 0 && Some(index) != last {
            return None;
        }
        let mut bits = 0_u32;
        for character in group.get(..4 - padding)? {
            bits = bits << 6 | value(*character)?;
        }
        bits <<= 6 * padding;
        let bytes = bits.to_be_bytes();
        let kept = bytes.get(1..4 - padding)?;
        // What padding leaves over is zero, in the one form.
        let dropped = bytes.get(4 - padding..)?;
        if dropped.iter().any(|byte| *byte != 0) {
            return None;
        }
        out.extend_from_slice(kept);
    }
    Some(out)
}

/// `bytes` in base64, padded.
#[must_use]
pub fn encode(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let mut group = [0_u8; 4];
        group
            .get_mut(1..=chunk.len())
            .unwrap_or_default()
            .copy_from_slice(chunk);
        let bits = u32::from_be_bytes(group);
        for position in 0..4 {
            if position > chunk.len() {
                out.push(char::from(PAD));
            } else {
                let index = bits >> (18 - 6 * position) & 0x3f;
                let digit = usize::try_from(index)
                    .ok()
                    .and_then(|index| ALPHABET.get(index));
                out.push(char::from(*digit.unwrap_or(&PAD)));
            }
        }
    }
    out
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    // RFC 4648's vectors.
    #[test]
    fn the_rfc_vectors_round_trip() {
        for (bytes, text) in [
            ("", ""),
            ("f", "Zg=="),
            ("fo", "Zm8="),
            ("foo", "Zm9v"),
            ("foob", "Zm9vYg=="),
            ("fooba", "Zm9vYmE="),
            ("foobar", "Zm9vYmFy"),
        ] {
            assert_eq!(encode(bytes.as_bytes()), text);
            assert_eq!(decode(text).unwrap(), bytes.as_bytes());
        }
        let all: Vec<u8> = (0..=255).collect();
        assert_eq!(decode(&encode(&all)).unwrap(), all);
    }

    #[test]
    fn only_the_one_form_reads() {
        for bad in [
            "Zg", "Zg=", "Zm9", "Z===", "Zh==", "Zm9=", "Zg==Zg==", "Zm 9v", "Zm9v\n", "Zm-v",
            "Zm_v", "====",
        ] {
            assert_eq!(decode(bad), None, "{bad}");
        }
    }
}
