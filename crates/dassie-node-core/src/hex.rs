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

/// Text that is nearly `2 * bytes` hex digits, for the properties of every
/// parser that reads hex: the right number of digits in mixed case, one too
/// few or too many, or one digit replaced by any other character.
#[cfg(test)]
pub(crate) fn near_hex(bytes: usize) -> proptest::strategy::BoxedStrategy<String> {
    use alloc::format;
    use proptest::prelude::*;

    let digits =
        |count: usize| proptest::string::string_regex(&format!("[0-9a-fA-F]{{{count}}}")).unwrap();
    let length = 2 * bytes;
    let junk = prop_oneof![
        "[ -/:-@G-Zg-z~\\x00-\\x1f]",
        any::<char>().prop_map(String::from)
    ];
    let replaced =
        (digits(length), any::<prop::sample::Index>(), junk).prop_map(|(text, at, junk)| {
            if text.is_empty() {
                return junk;
            }
            let at = at.index(text.len());
            format!("{}{junk}{}", &text[..at], &text[at + 1..])
        });
    prop_oneof![
        digits(length),
        digits(length.saturating_sub(1)),
        digits(length + 1),
        replaced,
    ]
    .boxed()
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

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

    fn check_canonical<const N: usize>(text: &str) -> Result<(), TestCaseError> {
        if let Some(bytes) = decode_exact::<N>(text) {
            prop_assert_eq!(encode(&bytes), text.to_ascii_lowercase());
        }
        Ok(())
    }

    proptest! {
        // Whatever decodes encodes back to the text, up to case: the only
        // other spelling of a value is in other case.
        #[test]
        fn decoding_never_panics(
            text in prop_oneof![any::<String>(), near_hex(0), near_hex(1), near_hex(4), near_hex(32)],
        ) {
            check_canonical::<0>(&text)?;
            check_canonical::<1>(&text)?;
            check_canonical::<4>(&text)?;
            check_canonical::<32>(&text)?;
        }

        #[test]
        fn encoding_round_trips(bytes: [u8; 32]) {
            let text = encode(&bytes);
            prop_assert_eq!(decode_exact::<32>(&text), Some(bytes));
            prop_assert_eq!(decode_exact::<32>(&text.to_ascii_uppercase()), Some(bytes));
        }
    }
}
