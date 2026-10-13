//! Properties of the codec, allocation and helpers.

mod support;

use dassie_oer::Writer;
use dassie_stream::Rate;
use dassie_stream::allocate::{Claim, allocate};
use dassie_stream::base64url;
use dassie_stream::crypto::{Keys, OVERHEAD};
use dassie_stream::packet::{Frames, Packet};
use proptest::prelude::*;

proptest! {
    #[test]
    fn packets_never_panic(input: Vec<u8>) {
        if let Ok(packet) = Packet::parse(&input) {
            // Whatever parsed can be encoded again, with unknown frames
            // dropped, and parses to the same frames.
            let frames: Vec<_> = packet.frames.iter().collect();
            let rebuilt = Packet { frames: Frames::List(&frames), ..packet };
            let mut buffer = vec![0; rebuilt.encoded_len()];
            rebuilt.encode_to(&mut Writer::new(&mut buffer));
            let reparsed = Packet::parse(&buffer).unwrap();
            prop_assert_eq!(reparsed.frames.iter().collect::<Vec<_>>(), frames);
        }
    }

    #[test]
    fn allocation_hands_out_exactly_the_total_within_capacity(
        claims in prop::collection::vec((0u64..1_000_000, 0u64..1_000_000), 0..12),
        total in 0u64..5_000_000,
    ) {
        let claims: Vec<Claim> = claims
            .into_iter()
            .map(|(shares, capacity)| Claim { shares, capacity })
            .collect();
        let mut output = vec![0; claims.len()];
        if allocate(total, &claims, &mut output).is_ok() {
            prop_assert_eq!(output.iter().sum::<u64>(), total);
            for (amount, claim) in output.iter().zip(&claims) {
                prop_assert!(*amount <= claim.capacity);
            }
        } else {
            // Failure only when the streams that have shares cannot take it.
            let room: u64 = claims.iter().filter(|c| c.shares > 0).map(|c| c.capacity).sum();
            prop_assert!(room < total, "failed although {room} fits {total}");
        }
    }

    #[test]
    fn base64url_round_trips(bytes in prop::collection::vec(any::<u8>(), 0..200)) {
        let mut encoded = vec![0; base64url::encoded_len(bytes.len())];
        let length = base64url::encode(&bytes, &mut encoded);
        let mut decoded = vec![0; bytes.len()];
        prop_assert_eq!(base64url::decode(&encoded[..length], &mut decoded), Ok(bytes.len()));
        prop_assert_eq!(decoded, bytes);
    }

    #[test]
    fn base64url_decoding_is_canonical(text in "[A-Za-z0-9_-]{0,40}") {
        let mut decoded = [0u8; 40];
        if let Ok(length) = base64url::decode(text.as_bytes(), &mut decoded) {
            let mut encoded = [0u8; 60];
            let written = base64url::encode(&decoded[..length], &mut encoded);
            prop_assert_eq!(&encoded[..written], text.as_bytes());
        }
    }

    #[test]
    fn encryption_round_trips(plaintext in prop::collection::vec(any::<u8>(), 0..500), iv: [u8; 12]) {
        let keys = Keys::derive(b"shared");
        let mut buffer = vec![0; OVERHEAD + plaintext.len()];
        buffer[OVERHEAD..].copy_from_slice(&plaintext);
        let length = keys.cipher.encrypt_with_iv(iv, plaintext.len(), &mut buffer);
        let mut output = vec![0; length];
        prop_assert_eq!(keys.cipher.decrypt(&buffer[..length], &mut output), Ok(plaintext.as_slice()));
    }

    #[test]
    fn rates_never_overpay(numerator in 1u64.., denominator in 1u64.., destination: u64) {
        let rate = Rate::new(numerator, denominator).unwrap();
        let source = rate.max_source_amount(destination);
        prop_assert!(rate.destination_amount(source) <= destination);
    }
}
