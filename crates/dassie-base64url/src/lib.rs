//! Unpadded base64url (RFC 4648 §5): the encoding of node IDs, of the
//! tokens the node hands out, and of the connection tokens STREAM puts in
//! ILP addresses. One crate of its own, so that STREAM and identity share it
//! without one depending on the other.
//!
//! It is small enough that a dependency would cost more than it saves
//! (spec/decisions/tigerstyle.md). Decoding is strict: no padding, no
//! whitespace, and the unused low bits of the last character must be zero,
//! so every byte string has exactly one accepted encoding. Decoding takes
//! untrusted input, so it never panics: an output too short is an error
//! too. It does not allocate.

#![no_std]

#[cfg(test)]
extern crate std;

use core::fmt;

/// The input is not canonical unpadded base64url, or its bytes do not fit
/// the output.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct InvalidBase64;

impl fmt::Display for InvalidBase64 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("invalid base64url")
    }
}

impl core::error::Error for InvalidBase64 {}

/// Length of the encoding of `input_length` bytes.
#[must_use]
pub const fn encoded_len(input_length: usize) -> usize {
    let tail = match input_length % 3 {
        0 => 0,
        1 => 2,
        _ => 3,
    };
    input_length / 3 * 4 + tail
}

/// Length of the bytes `input_length` characters decode to, or `None` if no
/// encoding has that length.
#[must_use]
pub const fn decoded_len(input_length: usize) -> Option<usize> {
    let tail = match input_length % 4 {
        0 => 0,
        2 => 1,
        3 => 2,
        _ => return None,
    };
    Some(input_length / 4 * 3 + tail)
}

/// Encodes `input` into the start of `output` and returns the length
/// written.
///
/// # Panics
///
/// If `output` is shorter than [`encoded_len`] of the input, which is a
/// programming error.
pub fn encode(input: &[u8], output: &mut [u8]) -> usize {
    let length = encoded_len(input.len());
    assert!(output.len() >= length, "base64url output buffer too small");
    let mut characters = output.iter_mut();
    for chunk in input.chunks(3) {
        let byte = |index: usize| chunk.get(index).copied().unwrap_or(0);
        let (first, second, third) = (byte(0), byte(1), byte(2));
        let sextets = [
            first >> 2,
            ((first & 0x03) << 4) | (second >> 4),
            ((second & 0x0f) << 2) | (third >> 6),
            third & 0x3f,
        ];
        for &sextet in sextets.iter().take(chunk.len() + 1) {
            if let Some(slot) = characters.next() {
                *slot = symbol(sextet);
            }
        }
    }
    length
}

/// Decodes `input` into the start of `output` and returns the length
/// written. An `output` shorter than [`decoded_len`] of the input is an
/// error, and nothing is written then.
pub fn decode(input: &[u8], output: &mut [u8]) -> Result<usize, InvalidBase64> {
    let length = decoded_len(input.len()).ok_or(InvalidBase64)?;
    if output.len() < length {
        return Err(InvalidBase64);
    }
    let mut bytes = output.iter_mut();
    for chunk in input.chunks(4) {
        let mut sextets = [0u8; 4];
        for (sextet, &character) in sextets.iter_mut().zip(chunk) {
            *sextet = value(character).ok_or(InvalidBase64)?;
        }
        let [first, second, third, fourth] = sextets;
        let decoded = [
            (first << 2) | (second >> 4),
            ((second & 0x0f) << 4) | (third >> 2),
            ((third & 0x03) << 6) | fourth,
        ];
        // The bits a short final chunk leaves over must be zero, or two
        // encodings would decode to the same bytes.
        let leftover = match chunk.len() {
            2 => second & 0x0f,
            3 => third & 0x03,
            _ => 0,
        };
        if leftover != 0 {
            return Err(InvalidBase64);
        }
        for &byte in decoded.iter().take(chunk.len() - 1) {
            if let Some(slot) = bytes.next() {
                *slot = byte;
            }
        }
    }
    Ok(length)
}

/// The character for a sextet.
const fn symbol(sextet: u8) -> u8 {
    assert!(sextet < 64, "a sextet has six bits");
    match sextet {
        0..=25 => b'A' + sextet,
        26..=51 => b'a' + (sextet - 26),
        52..=61 => b'0' + (sextet - 52),
        62 => b'-',
        _ => b'_',
    }
}

/// The sextet of a character, if it is in the alphabet.
const fn value(character: u8) -> Option<u8> {
    match character {
        b'A'..=b'Z' => Some(character - b'A'),
        b'a'..=b'z' => Some(character - b'a' + 26),
        b'0'..=b'9' => Some(character - b'0' + 52),
        b'-' => Some(62),
        b'_' => Some(63),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn round_trip(input: &[u8], expected: &str) {
        let mut encoded = [0u8; 16];
        let length = encode(input, &mut encoded);
        assert_eq!(encoded.get(..length), Some(expected.as_bytes()));
        let mut decoded = [0u8; 16];
        let length = decode(expected.as_bytes(), &mut decoded);
        assert_eq!(length.map(|length| decoded.get(..length)), Ok(Some(input)));
    }

    #[test]
    fn rfc_4648_vectors() {
        round_trip(b"", "");
        round_trip(b"f", "Zg");
        round_trip(b"fo", "Zm8");
        round_trip(b"foo", "Zm9v");
        round_trip(b"foob", "Zm9vYg");
        round_trip(b"fooba", "Zm9vYmE");
        round_trip(b"foobar", "Zm9vYmFy");
        round_trip(&[0xfb, 0xff], "-_8");
    }

    #[test]
    fn decoding_is_strict() {
        let mut output = [0u8; 8];
        for text in ["Zg==", "Z", "Zh", "Zm9", "Zm+v", "Zm/v", "Zm9 "] {
            assert_eq!(
                decode(text.as_bytes(), &mut output),
                Err(InvalidBase64),
                "{text}"
            );
        }
    }

    #[test]
    fn an_output_too_short_is_an_error() {
        let mut output = [0xa5u8; 5];
        assert_eq!(decode(b"Zm9vYmFy", &mut output), Err(InvalidBase64));
        assert_eq!(output, [0xa5; 5]);
    }
}
