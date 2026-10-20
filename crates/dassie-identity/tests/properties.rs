//! Properties: phrases and node IDs round-trip, every parser and decoder
//! never panics on arbitrary input, and whatever they accept is canonical.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use curve25519_dalek::Scalar;
use curve25519_dalek::constants::EIGHT_TORSION;
use dassie_identity::{
    Error, LoginSecret, Mnemonic, NODE_LABEL_LENGTH, NodeId, NodeKey, NodeLabel, PKCS8_DER_LENGTH,
    PublicKey, SPKI_DER_LENGTH, SeedPath, Signature, WordCount, X25519Public, X25519Secret,
    base64url,
};
use proptest::prelude::*;

fn entropy() -> impl Strategy<Value = Vec<u8>> {
    prop::sample::select(vec![16usize, 20, 24, 28, 32])
        .prop_flat_map(|length| prop::collection::vec(any::<u8>(), length))
}

/// ℓ, the order of the base point, little-endian.
const ORDER: [u8; 32] = [
    0xed, 0xd3, 0xf5, 0x5c, 0x1a, 0x63, 0x12, 0x58, 0xd6, 0x9c, 0xf7, 0xa2, 0xde, 0xf9, 0xde, 0x14,
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0x10,
];

/// `p + offset`, little-endian, where `p = 2^255 - 19`.
fn p_plus(offset: u8) -> [u8; 32] {
    let mut bytes = [0xff; 32];
    bytes[0] = 0xed + offset;
    bytes[31] = 0x7f;
    bytes
}

/// Compressed Edwards points a peer might send: anything, all zeros, the
/// eight points of small order, and non-canonical encodings (`y ≥ p`) with
/// either sign.
fn edwards_encoding() -> impl Strategy<Value = [u8; 32]> {
    let small_order: Vec<[u8; 32]> = EIGHT_TORSION
        .iter()
        .map(|point| point.compress().to_bytes())
        .collect();
    prop_oneof![
        any::<[u8; 32]>(),
        Just([0; 32]),
        prop::sample::select(small_order),
        (0u8..19, any::<bool>()).prop_map(|(offset, negative)| {
            let mut bytes = p_plus(offset);
            if negative {
                bytes[31] |= 0x80;
            }
            bytes
        }),
    ]
}

/// Signature scalars: anything, zero, `ℓ + k`, and values of 2^253 or more.
/// ZIP-215 refuses every one at or above ℓ.
fn signature_scalar() -> impl Strategy<Value = [u8; 32]> {
    prop_oneof![
        any::<[u8; 32]>(),
        Just([0; 32]),
        any::<u8>().prop_map(|k| {
            let mut scalar = ORDER;
            let (low, carry) = scalar[0].overflowing_add(k);
            scalar[0] = low;
            scalar[1] += u8::from(carry);
            scalar
        }),
        any::<[u8; 32]>().prop_map(|mut scalar| {
            scalar[31] |= 0x20;
            scalar
        }),
    ]
}

/// The `u` of X25519 keys that give the all-zero result whatever the secret
/// (RFC 7748 §6.1): the images of the eight points of small order, `p - 1`,
/// and the non-canonical `p` and `p + 1`, each also with the top bit set,
/// which X25519 ignores.
fn low_order_u() -> impl Strategy<Value = [u8; 32]> {
    let mut points: Vec<[u8; 32]> = EIGHT_TORSION
        .iter()
        .map(|point| point.to_montgomery().to_bytes())
        .collect();
    let mut minus_one = p_plus(0);
    minus_one[0] = 0xec;
    points.extend([minus_one, p_plus(0), p_plus(1)]);
    (prop::sample::select(points), any::<bool>()).prop_map(|(mut u, top)| {
        if top {
            u[31] |= 0x80;
        }
        u
    })
}

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

/// A login secret and a candidate proof: the secret itself, with one bit
/// flipped, cut short, extended, or anything at all.
fn login_proof() -> impl Strategy<Value = ([u8; 32], Vec<u8>)> {
    (
        any::<[u8; 32]>(),
        0u8..5,
        any::<prop::sample::Index>(),
        any::<u8>(),
        prop::collection::vec(any::<u8>(), 0..64),
    )
        .prop_map(|(secret, edit, index, byte, noise)| {
            let mut candidate = secret.to_vec();
            match edit {
                0 => {}
                1 => candidate[index.index(32)] ^= 1 << (byte % 8),
                2 => candidate.truncate(index.index(32)),
                3 => candidate.push(byte),
                _ => candidate = noise,
            }
            (secret, candidate)
        })
}

/// DER with the exact `prefix` and then a tail of any length, mostly the 32
/// bytes that are the only length accepted.
fn der_with_prefix(prefix: Vec<u8>) -> impl Strategy<Value = Vec<u8>> {
    prop_oneof![
        prop::collection::vec(any::<u8>(), 32),
        prop::collection::vec(any::<u8>(), 0..64),
    ]
    .prop_map(move |tail| [prefix.as_slice(), &tail].concat())
}

fn pkcs8_prefix() -> Vec<u8> {
    let der = NodeKey::from_bytes(&[0; 32]).to_pkcs8_der();
    der.as_bytes()[..PKCS8_DER_LENGTH - 32].to_vec()
}

fn spki_prefix() -> Vec<u8> {
    let der = NodeKey::from_bytes(&[0; 32]).public_key().to_spki_der();
    der[..SPKI_DER_LENGTH - 32].to_vec()
}

fn signature(r: &[u8; 32], s: &[u8; 32]) -> Signature {
    let mut bytes = [0u8; 64];
    bytes[..32].copy_from_slice(r);
    bytes[32..].copy_from_slice(s);
    Signature::from_bytes(bytes)
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

    // Every ID a key gives has a label, which names it and no other.
    #[test]
    fn computed_node_ids_round_trip_through_their_labels(key: [u8; 32]) {
        let id = NodeId::from_public_key(&key);
        let label = id.label().unwrap();
        prop_assert_eq!(label.as_str().len(), NODE_LABEL_LENGTH);
        prop_assert!(label.as_str().bytes().all(|b| b.is_ascii_lowercase() || (b'2'..=b'7').contains(&b)));
        prop_assert_eq!(NodeId::from_label(label.as_str()), Ok(id));
        prop_assert_eq!(NodeLabel::parse(&label.as_str().to_ascii_uppercase()), Ok(label));
    }

    #[test]
    fn node_label_parse_never_panics(text in "[a-zA-Z2-9=]{37,41}|\\PC{0,48}") {
        if let Ok(label) = NodeLabel::parse(&text) {
            prop_assert_eq!(label.as_str(), text.to_ascii_lowercase());
            if let Ok(id) = label.node_id() {
                prop_assert_eq!(id.label(), Ok(label));
            }
        }
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
    fn mnemonic_from_entropy_never_panics(entropy in prop::collection::vec(any::<u8>(), 0..48)) {
        match Mnemonic::from_entropy(&entropy) {
            Ok(mnemonic) => prop_assert_eq!(mnemonic.entropy(), entropy.as_slice()),
            Err(error) => {
                prop_assert_eq!(error, Error::InvalidEntropyLength);
                prop_assert_eq!(WordCount::from_entropy_bytes(entropy.len()), None);
            }
        }
    }

    /// Callers size the output with `decoded_len` or larger, so that is the
    /// contract tested; the bytes past the decoded length are left alone.
    #[test]
    fn base64url_decode_never_panics(input in base64url_like(), slack in 0usize..4) {
        let Some(length) = base64url::decoded_len(input.len()) else {
            prop_assert_eq!(base64url::decode(&input, &mut []), Err(Error::InvalidBase64));
            return Ok(());
        };
        let mut output = vec![0xa5; length + slack];
        match base64url::decode(&input, &mut output) {
            Ok(written) => {
                prop_assert_eq!(written, length);
                prop_assert!(output[length..].iter().all(|&byte| byte == 0xa5));
                let mut encoded = vec![0; base64url::encoded_len(length)];
                base64url::encode(&output[..length], &mut encoded);
                prop_assert_eq!(encoded, input);
            }
            Err(error) => prop_assert_eq!(error, Error::InvalidBase64),
        }
    }

    #[test]
    fn pkcs8_der_with_the_prefix_never_panics(der in der_with_prefix(pkcs8_prefix())) {
        let tail = &der[PKCS8_DER_LENGTH - 32..];
        match NodeKey::from_pkcs8_der(&der) {
            Ok(key) => {
                prop_assert_eq!(key.as_bytes().as_slice(), tail);
                prop_assert_eq!(key.to_pkcs8_der().as_bytes().to_vec(), der);
            }
            Err(error) => {
                prop_assert_eq!(error, Error::InvalidDer);
                prop_assert_ne!(tail.len(), 32);
            }
        }
    }

    #[test]
    fn spki_der_with_the_prefix_never_panics(der in der_with_prefix(spki_prefix())) {
        let tail = &der[SPKI_DER_LENGTH - 32..];
        let expected =
            <&[u8; 32]>::try_from(tail).map_or(Err(Error::InvalidDer), PublicKey::from_bytes);
        let decoded = PublicKey::from_spki_der(&der);
        prop_assert_eq!(decoded, expected);
        if let Ok(key) = decoded {
            prop_assert_eq!(key.to_spki_der().to_vec(), der);
        }
    }

    /// An arbitrary key, `R`, `s` and message: whatever decodes as a key
    /// verifies or not without panicking, and an `s` of ℓ or more never
    /// verifies.
    #[test]
    fn arbitrary_signatures_never_panic(
        key in edwards_encoding(),
        r in edwards_encoding(),
        s in signature_scalar(),
        message in prop::collection::vec(any::<u8>(), 0..128),
    ) {
        let Ok(public_key) = PublicKey::from_bytes(&key) else {
            return Ok(());
        };
        let result = public_key.verify(&message, &signature(&r, &s));
        if bool::from(Scalar::from_canonical_bytes(s).is_none()) {
            prop_assert_eq!(result, Err(Error::InvalidSignature));
        } else {
            prop_assert!(matches!(result, Ok(()) | Err(Error::InvalidSignature)));
        }
    }

    #[test]
    fn x25519_never_panics(secret: [u8; 32], peer in prop_oneof![any::<[u8; 32]>(), low_order_u()]) {
        let peer = X25519Public::from_bytes(peer);
        match X25519Secret::from_bytes(secret).diffie_hellman(&peer) {
            Ok(shared) => prop_assert_ne!(shared.as_bytes(), &[0; 32]),
            Err(error) => prop_assert_eq!(error, Error::NonContributory),
        }
    }

    #[test]
    fn x25519_refuses_low_order_peers(secret: [u8; 32], peer in low_order_u()) {
        let peer = X25519Public::from_bytes(peer);
        let result = X25519Secret::from_bytes(secret).diffie_hellman(&peer);
        prop_assert_eq!(result.map(|_| ()), Err(Error::NonContributory));
    }

    /// The path peers take: an Ed25519 key, mapped to X25519. The exchange
    /// fails exactly for the weak keys, which peers refuse before it.
    #[test]
    fn x25519_with_node_keys_fails_only_for_weak_keys(secret: [u8; 32], key in edwards_encoding()) {
        let Ok(public_key) = PublicKey::from_bytes(&key) else {
            return Ok(());
        };
        let result = X25519Secret::from_bytes(secret).diffie_hellman(&public_key.to_x25519());
        prop_assert_eq!(result.is_err(), public_key.is_weak());
    }

    #[test]
    fn verify_proof_never_panics((secret, candidate) in login_proof()) {
        let accepted = LoginSecret::from_bytes(secret).verify_proof(&candidate);
        prop_assert_eq!(accepted, candidate == secret);
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
