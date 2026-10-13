//! Lowercase hexadecimal, as the original's setup and login API uses it.

use alloc::string::String;

const DIGITS: &[u8; 16] = b"0123456789abcdef";

/// `bytes` as lowercase hex.
#[must_use]
pub fn encode(bytes: &[u8]) -> String {
    let mut text = String::with_capacity(bytes.len().saturating_mul(2));
    for byte in bytes {
        for nibble in [byte >> 4, byte & 0x0f] {
            if let Some(&digit) = DIGITS.get(usize::from(nibble)) {
                text.push(char::from(digit));
            }
        }
    }
    text
}

/// Exactly `N` bytes from `2 * N` hex digits of either case, or `None`.
#[must_use]
pub fn decode_exact<const N: usize>(text: &str) -> Option<[u8; N]> {
    let digits = text.as_bytes();
    if digits.len() != N.checked_mul(2)? {
        return None;
    }
    let mut bytes = [0_u8; N];
    for (byte, [high, low]) in bytes.iter_mut().zip(digits.as_chunks::<2>().0) {
        *byte = (nibble(*high)? << 4) | nibble(*low)?;
    }
    Some(bytes)
}

fn nibble(digit: u8) -> Option<u8> {
    match digit {
        b'0'..=b'9' => Some(digit - b'0'),
        b'a'..=b'f' => Some(digit - b'a' + 10),
        b'A'..=b'F' => Some(digit - b'A' + 10),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips() {
        let bytes = [0x00, 0x1f, 0xa0, 0xff];
        assert_eq!(encode(&bytes), "001fa0ff");
        assert_eq!(decode_exact::<4>("001fa0ff"), Some(bytes));
        assert_eq!(decode_exact::<4>("001FA0FF"), Some(bytes));
    }

    #[test]
    fn rejects_wrong_length_and_digits() {
        assert_eq!(decode_exact::<4>("001fa0f"), None);
        assert_eq!(decode_exact::<4>("001fa0fff0"), None);
        assert_eq!(decode_exact::<4>("001fa0fg"), None);
        assert_eq!(decode_exact::<1>("+1"), None);
    }
}
