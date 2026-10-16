//! Properties: phrases and node IDs round-trip, every parser and decoder
//! never panics on arbitrary input, and whatever they accept is canonical.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use dassie_identity::{
    Error, Mnemonic, NodeId, NodeKey, PublicKey, SeedPath, Signature, WordCount, base64url,
};
use proptest::prelude::*;

fn entropy() -> impl Strategy<Value = Vec<u8>> {
    prop::sample::select(vec![16usize, 20, 24, 28, 32])
        .prop_flat_map(|length| prop::collection::vec(any::<u8>(), length))
}

/// Joins words with random runs of ASCII whitespace and random case, as a
/// person might type them.
fn mangle(words: &[&str], separators: &[String], upper: &[bool]) -> String {
    let mut text = String::new();
    for (index, word) in words.iter().enumerate() {
        text.push_str(&separators[index % separators.len()]);
        if upper[index % upper.len()] {
            text.push_str(&word.to_ascii_uppercase());
        } else {
            text.push_str(word);
        }
    }
    text
}

proptest! {
    #[test]
    fn mnemonics_round_trip(entropy in entropy()) {
        let mnemonic = Mnemonic::from_entropy(&entropy).unwrap();
        let count = mnemonic.word_count();
        prop_assert_eq!(WordCount::from_entropy_bytes(entropy.len()), Some(count));
        prop_assert_eq!(mnemonic.words().count(), count.words());

        let phrase = mnemonic.phrase();
        let parsed = Mnemonic::parse(phrase.as_str()).unwrap();
        prop_assert_eq!(parsed.entropy(), entropy.as_slice());
        let reparsed = parsed.phrase();
        prop_assert_eq!(reparsed.as_str(), phrase.as_str());
    }

    #[test]
    fn sloppy_phrases_parse_to_the_same_entropy(
        entropy in entropy(),
        separators in prop::collection::vec("[ \t\r\n]{1,3}", 1..5),
        upper in prop::collection::vec(any::<bool>(), 1..5),
    ) {
        let mnemonic = Mnemonic::from_entropy(&entropy).unwrap();
        let words: Vec<&str> = mnemonic.words().collect();
        let text = mangle(&words, &separators, &upper);
        let parsed = Mnemonic::parse(&text).unwrap();
        prop_assert_eq!(parsed.entropy(), entropy.as_slice());
    }

    #[test]
    fn generated_phrases_are_deterministic(seed: [u8; 32], words in 0usize..5) {
        let count = [
            WordCount::Twelve,
            WordCount::Fifteen,
            WordCount::Eighteen,
            WordCount::TwentyOne,
            WordCount::TwentyFour,
        ][words];
        let first = Mnemonic::generate(&mut CountingRng(seed), count);
        let second = Mnemonic::generate(&mut CountingRng(seed), count);
        prop_assert_eq!(first.entropy(), second.entropy());
        prop_assert_eq!(first.word_count(), count);
    }

    #[test]
    fn mnemonic_parse_never_panics(text in any::<String>()) {
        if let Ok(mnemonic) = Mnemonic::parse(&text) {
            let phrase = mnemonic.phrase();
            let reparsed = Mnemonic::parse(phrase.as_str()).unwrap();
            prop_assert_eq!(reparsed.entropy(), mnemonic.entropy());
        }
    }

    /// Phrases of real words in random order mostly fail the checksum, and
    /// must never panic whatever their length.
    #[test]
    fn word_salad_never_panics(entropy in entropy(), picks in prop::collection::vec(any::<prop::sample::Index>(), 0..30)) {
        let mnemonic = Mnemonic::from_entropy(&entropy).unwrap();
        let words: Vec<&str> = mnemonic.words().collect();
        let salad: Vec<&str> = picks.iter().map(|pick| *pick.get(&words)).collect();
        match Mnemonic::parse(&salad.join(" ")) {
            Ok(parsed) => {
                let phrase = parsed.phrase();
                prop_assert_eq!(phrase.as_str(), salad.join(" "));
            }
            Err(error) => prop_assert!(matches!(error, Error::InvalidChecksum | Error::InvalidWordCount)),
        }
    }

    #[test]
    fn node_id_parse_never_panics(text in any::<String>()) {
        if let Ok(id) = NodeId::parse(&text) {
            prop_assert_eq!(id.as_str(), text.as_str());
        }
    }

    #[test]
    fn valid_node_ids_re_render_identically(text in "[A-Za-z0-9_-]{2,45}") {
        let id = NodeId::parse(&text).unwrap();
        prop_assert_eq!(id.as_str(), text.as_str());
        prop_assert_eq!(id.to_string(), text.clone());
        prop_assert_eq!(id.len(), text.len());
    }

    #[test]
    fn computed_node_ids_are_valid(key: [u8; 32]) {
        let id = NodeId::from_public_key(&key);
        prop_assert!(id.as_str().starts_with('d'));
        prop_assert!(id.len() <= 33);
        prop_assert!(!id.as_str().ends_with('A'));
        prop_assert_eq!(NodeId::parse(id.as_str()), Ok(id));
    }

    #[test]
    fn base64url_round_trips(bytes in prop::collection::vec(any::<u8>(), 0..64)) {
        let mut encoded = vec![0; base64url::encoded_len(bytes.len())];
        let length = base64url::encode(&bytes, &mut encoded);
        prop_assert_eq!(length, encoded.len());
        prop_assert!(!encoded.contains(&b'='));
        let mut decoded = vec![0; base64url::decoded_len(encoded.len()).unwrap()];
        prop_assert_eq!(base64url::decode(&encoded, &mut decoded), Ok(bytes.len()));
        prop_assert_eq!(decoded, bytes);
    }

    #[test]
    fn base64url_decoding_is_canonical(text in "[A-Za-z0-9_=+/-]{0,40}") {
        let input = text.as_bytes();
        let Some(capacity) = base64url::decoded_len(input.len()) else {
            prop_assert_eq!(base64url::decode(input, &mut []), Err(Error::InvalidBase64));
            return Ok(());
        };
        let mut decoded = vec![0; capacity];
        if let Ok(length) = base64url::decode(input, &mut decoded) {
            let mut encoded = vec![0; base64url::encoded_len(length)];
            base64url::encode(&decoded[..length], &mut encoded);
            prop_assert_eq!(encoded.as_slice(), input);
        }
    }

    #[test]
    fn seed_path_parse_never_panics(text in any::<String>()) {
        if let Ok(path) = SeedPath::parse(&text) {
            prop_assert!(path.segments().all(|segment| !segment.is_empty()));
            let key = NodeKey::from_bytes(&[5; 32]);
            let _ = key.derive(&path);
        }
    }

    #[test]
    fn settlement_entropy_never_panics(scheme in any::<String>(), path in any::<String>()) {
        let _ = NodeKey::from_bytes(&[5; 32]).settlement_entropy(&scheme, &path);
    }

    #[test]
    fn der_and_key_decoders_never_panic(bytes in prop::collection::vec(any::<u8>(), 0..64), key: [u8; 32]) {
        let _ = NodeKey::from_pkcs8_der(&bytes);
        let _ = PublicKey::from_spki_der(&bytes);
        let _ = PublicKey::from_bytes(&key);
    }

    #[test]
    fn signatures_verify(seed: [u8; 32], message in prop::collection::vec(any::<u8>(), 0..128), noise: [u8; 32]) {
        let key = NodeKey::from_bytes(&seed);
        let public_key = key.public_key();
        let signature = key.sign(&message);
        prop_assert_eq!(public_key.verify(&message, &signature), Ok(()));

        let mut forged = *signature.as_bytes();
        forged[..32].copy_from_slice(&noise);
        if forged != *signature.as_bytes() {
            prop_assert_eq!(
                public_key.verify(&message, &Signature::from_bytes(forged)),
                Err(Error::InvalidSignature)
            );
        }
    }
}

/// A deterministic RNG for the tests: a counter mixed into a fixed seed.
/// Not random at all, which is the point.
struct CountingRng([u8; 32]);

impl rand_core::TryRng for CountingRng {
    type Error = core::convert::Infallible;

    fn try_next_u32(&mut self) -> Result<u32, Self::Error> {
        let mut bytes = [0; 4];
        self.try_fill_bytes(&mut bytes)?;
        Ok(u32::from_le_bytes(bytes))
    }

    fn try_next_u64(&mut self) -> Result<u64, Self::Error> {
        let mut bytes = [0; 8];
        self.try_fill_bytes(&mut bytes)?;
        Ok(u64::from_le_bytes(bytes))
    }

    fn try_fill_bytes(&mut self, destination: &mut [u8]) -> Result<(), Self::Error> {
        for byte in destination {
            *byte = self.0[0];
            self.0.rotate_left(1);
            self.0[31] = self.0[31].wrapping_add(1);
        }
        Ok(())
    }
}

impl rand_core::TryCryptoRng for CountingRng {}
