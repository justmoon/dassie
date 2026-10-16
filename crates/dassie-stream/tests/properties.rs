//! Properties of the codec, allocation and helpers. The decoders never
//! panic on anything a remote can send (D16, spec 7 §7).

mod support;

use dassie_oer::{Reader, Writer};
use dassie_stream::allocate::{Claim, allocate};
use dassie_stream::base64url;
use dassie_stream::crypto::{Cipher, Keys, OVERHEAD, hmac_sha256};
use dassie_stream::packet::{Frames, Packet, PacketType};
use dassie_stream::receipt::{self, RECEIPT_VERSION};
use dassie_stream::token::{MAX_CONNECTION_TAG_LENGTH, MAX_TOKEN_LENGTH, ServerSecret};
use dassie_stream::{Frame, Rate};
use proptest::prelude::*;
use support::arbitrary;

/// Whatever decoded encodes again and decodes to the same frame.
fn assert_frame_round_trips(frame: Frame<'_>) -> Result<(), TestCaseError> {
    let mut buffer = vec![0; frame.encoded_len()];
    frame.encode_to(&mut Writer::new(&mut buffer));
    prop_assert_eq!(Frame::decode(&mut Reader::new(&buffer)), Ok(Some(frame)));
    Ok(())
}

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
    fn mutated_packets_never_panic(
        sequence: u64,
        packet_type in prop::sample::select(vec![
            PacketType::Prepare,
            PacketType::Fulfill,
            PacketType::Reject,
        ]),
        prepare_amount: u64,
        frames in prop::collection::vec(arbitrary::frame(), 0..8),
        mutations in arbitrary::mutations(),
    ) {
        // Random bytes rarely get past the header; a damaged valid packet
        // reaches the frames.
        let mut input = arbitrary::packet(sequence, packet_type, prepare_amount, &frames);
        arbitrary::damage(&mut input, &mutations);
        if let Ok(packet) = Packet::parse(&input) {
            let frames: Vec<_> = packet.frames.iter().collect();
            for &frame in &frames {
                assert_frame_round_trips(frame)?;
            }
            let rebuilt = Packet { frames: Frames::List(&frames), ..packet };
            let mut buffer = vec![0; rebuilt.encoded_len()];
            rebuilt.encode_to(&mut Writer::new(&mut buffer));
            let reparsed = Packet::parse(&buffer).unwrap();
            prop_assert_eq!(reparsed.frames.iter().collect::<Vec<_>>(), frames);
        }
    }

    #[test]
    fn frames_never_panic(input: Vec<u8>) {
        let mut reader = Reader::new(&input);
        while !reader.is_empty() {
            match Frame::decode(&mut reader) {
                Ok(Some(frame)) => assert_frame_round_trips(frame)?,
                Ok(None) => {}
                Err(_) => break,
            }
        }
    }

    #[test]
    fn mutated_frames_never_panic(
        frame in arbitrary::frame(),
        mutations in arbitrary::mutations(),
    ) {
        let mut input = frame;
        arbitrary::damage(&mut input, &mutations);
        if let Ok(Some(frame)) = Frame::decode(&mut Reader::new(&input)) {
            assert_frame_round_trips(frame)?;
        }
    }

    #[test]
    fn tokens_never_panic(
        raw in prop::collection::vec(any::<u8>(), 0..MAX_TOKEN_LENGTH + 8),
        plaintext in arbitrary::token_plaintext(),
    ) {
        let secret = ServerSecret::new([1; 32]);
        let mut scratch = [0u8; MAX_TOKEN_LENGTH];
        // Bytes the server did not encrypt fail authentication.
        prop_assert!(secret.open_token(&raw, &mut scratch).is_err());
        // Encrypted under the server's key, they reach the parser.
        let token = arbitrary::seal(&Cipher::new(&[1; 32]), &plaintext);
        if let Ok(contents) = secret.open_token(&token, &mut scratch) {
            prop_assert!(contents.connection_tag.len() <= MAX_CONNECTION_TAG_LENGTH);
        }
    }

    #[test]
    fn receipts_never_panic(
        raw in prop::collection::vec(any::<u8>(), 0..80),
        version in prop_oneof![Just(RECEIPT_VERSION), any::<u8>()],
        fields: [u8; 25],
        secret: [u8; 32],
    ) {
        prop_assert!(receipt::verify(&raw, &secret).is_err());
        // Signed with the secret, any fields reach the decoding.
        let mut signed = vec![version];
        signed.extend_from_slice(&fields);
        let mut input = signed.clone();
        input.extend_from_slice(&hmac_sha256(&secret, &signed));
        match receipt::verify(&input, &secret) {
            Ok(receipt) => {
                prop_assert_eq!(version, RECEIPT_VERSION);
                prop_assert_eq!(&receipt.nonce[..], &fields[..16]);
                prop_assert_eq!(receipt.stream_id, fields[16]);
                prop_assert_eq!(&receipt.total_received.to_be_bytes()[..], &fields[17..]);
            }
            Err(_) => prop_assert_ne!(version, RECEIPT_VERSION),
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
    fn base64url_decoding_never_panics(
        raw: Vec<u8>,
        text in "[A-Za-z0-9_-]{0,80}",
        mutations in arbitrary::mutations(),
        output_length in 0usize..80,
    ) {
        // Raw bytes mostly fail at the first character; damaged base64url
        // gets further. Output buffers of every length, too short included.
        let mut damaged = text.into_bytes();
        arbitrary::damage(&mut damaged, &mutations);
        for input in [raw, damaged] {
            let mut output = vec![0; output_length];
            if let Ok(length) = base64url::decode(&input, &mut output) {
                prop_assert!(length <= output_length);
                let mut encoded = vec![0; base64url::encoded_len(length)];
                let written = base64url::encode(&output[..length], &mut encoded);
                prop_assert_eq!(&encoded[..written], input.as_slice());
            }
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
