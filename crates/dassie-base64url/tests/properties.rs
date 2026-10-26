//! Properties: base64url round-trips, decoding never panics on any input,
//! and whatever it accepts is canonical.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use dassie_base64url::{InvalidBase64, decode, decoded_len, encode, encoded_len};
use proptest::prelude::*;

/// Bytes that are mostly base64url characters, so that some decode and the
/// rest fail at every check: a byte outside the alphabet, a length no
/// encoding has, or leftover bits that are not zero.
fn base64url_like() -> impl Strategy<Value = Vec<u8>> {
    let alphabet = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_".to_vec();
    let byte = prop_oneof![8 => prop::sample::select(alphabet), 1 => any::<u8>()];
    prop_oneof![
        prop::collection::vec(any::<u8>(), 0..64),
        prop::collection::vec(byte, 0..64),
    ]
}

proptest! {
    #[test]
    fn round_trips(bytes in prop::collection::vec(any::<u8>(), 0..200)) {
        let mut encoded = vec![0; encoded_len(bytes.len())];
        let length = encode(&bytes, &mut encoded);
        prop_assert_eq!(length, encoded.len());
        prop_assert!(!encoded.contains(&b'='));
        let mut decoded = vec![0; decoded_len(encoded.len()).unwrap()];
        prop_assert_eq!(decode(&encoded, &mut decoded), Ok(bytes.len()));
        prop_assert_eq!(decoded, bytes);
    }

    #[test]
    fn decoding_is_canonical(text in "[A-Za-z0-9_=+/-]{0,40}") {
        let input = text.as_bytes();
        let Some(capacity) = decoded_len(input.len()) else {
            prop_assert_eq!(decode(input, &mut []), Err(InvalidBase64));
            return Ok(());
        };
        let mut decoded = vec![0; capacity];
        if let Ok(length) = decode(input, &mut decoded) {
            let mut encoded = vec![0; encoded_len(length)];
            encode(&decoded[..length], &mut encoded);
            prop_assert_eq!(encoded.as_slice(), input);
        }
    }

    /// Any output, too short included: the bytes past the decoded length
    /// are left alone, and an output too short is an error.
    #[test]
    fn decode_never_panics(input in base64url_like(), size in 0usize..52) {
        let mut output = vec![0xa5; size];
        match decode(&input, &mut output) {
            Ok(written) => {
                prop_assert_eq!(Some(written), decoded_len(input.len()));
                prop_assert!(output[written..].iter().all(|&byte| byte == 0xa5));
                let mut encoded = vec![0; encoded_len(written)];
                encode(&output[..written], &mut encoded);
                prop_assert_eq!(encoded, input);
            }
            // Refused before decoding anything: a length no encoding has,
            // or an output too short.
            Err(InvalidBase64) if decoded_len(input.len()).is_none_or(|length| length > size) => {
                prop_assert!(output.iter().all(|&byte| byte == 0xa5));
            }
            Err(InvalidBase64) => {}
        }
    }
}
