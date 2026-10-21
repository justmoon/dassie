//! Unpadded base64url (RFC 4648 §5), for connection tokens in ILP addresses.
//! Small enough not to justify a dependency (spec/decisions/tigerstyle.md).

const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";

/// Characters needed for `length` bytes.
#[must_use]
pub const fn encoded_len(length: usize) -> usize {
    (length * 4).div_ceil(3)
}

/// Encodes `input` into the start of `output` and returns the length.
///
/// # Panics
///
/// If `output` is shorter than [`encoded_len`] of the input.
pub fn encode(input: &[u8], output: &mut [u8]) -> usize {
    let length = encoded_len(input.len());
    assert!(output.len() >= length, "base64url output too short");
    let mut written = 0;
    for chunk in input.chunks(3) {
        let mut group = [0u8; 3];
        group
            .get_mut(..chunk.len())
            .unwrap_or_default()
            .copy_from_slice(chunk);
        let bits = u32::from_be_bytes([0, group[0], group[1], group[2]]);
        for index in 0..=chunk.len() {
            let shift = 18 - 6 * index;
            let sextet = (bits >> shift) & 0x3f;
            if let (Some(slot), Some(&character)) = (
                output.get_mut(written),
                ALPHABET.get(usize::try_from(sextet).unwrap_or(0)),
            ) {
                *slot = character;
            }
            written += 1;
        }
    }
    assert!(written == length, "base64url length mismatch");
    length
}

fn sextet(character: u8) -> Option<u32> {
    let value = match character {
        b'A'..=b'Z' => character - b'A',
        b'a'..=b'z' => character - b'a' + 26,
        b'0'..=b'9' => character - b'0' + 52,
        b'-' => 62,
        b'_' => 63,
        _ => return None,
    };
    Some(u32::from(value))
}

/// The input is not canonical unpadded base64url, or does not fit.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct InvalidBase64;

/// Decodes `input` into the start of `output` and returns the length.
/// Rejects padding, other alphabets, impossible lengths and non-zero
/// trailing bits, so every byte string has exactly one accepted encoding.
pub fn decode(input: &[u8], output: &mut [u8]) -> Result<usize, InvalidBase64> {
    if input.len() % 4 == 1 {
        return Err(InvalidBase64);
    }
    let length = input.len() * 3 / 4;
    if output.len() < length {
        return Err(InvalidBase64);
    }
    let mut written = 0;
    for chunk in input.chunks(4) {
        let mut bits: u32 = 0;
        for (index, &character) in chunk.iter().enumerate() {
            bits |= sextet(character).ok_or(InvalidBase64)? << (18 - 6 * index);
        }
        let bytes = bits.to_be_bytes();
        let produced = chunk.len() - 1;
        // Bits beyond the produced bytes must be zero for a canonical input.
        let leftover = bits & (0x00ff_ffff >> (8 * produced));
        if leftover != 0 {
            return Err(InvalidBase64);
        }
        let source = bytes.get(1..=produced).ok_or(InvalidBase64)?;
        output
            .get_mut(written..written + produced)
            .ok_or(InvalidBase64)?
            .copy_from_slice(source);
        written += produced;
    }
    Ok(written)
}
