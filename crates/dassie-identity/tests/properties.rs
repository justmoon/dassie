//! Properties: phrases and node IDs round-trip, damaged phrases are
//! restored within the code's bound and refused past it, every parser and
//! decoder never panics on arbitrary input, and whatever they accept is
//! canonical.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use curve25519_dalek::Scalar;
use curve25519_dalek::constants::EIGHT_TORSION;
use dassie_identity::{
    Created, Error, LoginSecret, NODE_ID_BYTES, NODE_ID_LENGTH, NodeId, NodeKey, NodeSecret,
    PHRASE_WORDS, PKCS8_DER_LENGTH, Phrase, PhraseSecret, Positions, PublicKey, Reading, Realm,
    SPKI_DER_LENGTH, Segment, Signature,
};
use proptest::prelude::*;

fn realm() -> impl Strategy<Value = Realm> {
    prop_oneof![Just(Realm::Test), Just(Realm::Live)]
}

/// Creation hours: the field's ends, and anything between.
fn hours() -> impl Strategy<Value = u32> {
    prop_oneof![Just(0), Just(Created::MAX_HOURS), 0..=Created::MAX_HOURS]
}

fn phrase_of(realm: Realm, hours: u32, secret: [u8; 16]) -> Phrase {
    let created = Created::from_hours(hours).unwrap();
    Phrase::new(realm, created, &PhraseSecret::from_bytes(secret))
}

/// Wrong and unreadable positions, apart, for each `(wrong, unreadable)`
/// count in `counts`.
fn damage(counts: Vec<(usize, usize)>) -> impl Strategy<Value = (Vec<usize>, Vec<usize>)> {
    (
        prop::sample::select(counts),
        Just((0..PHRASE_WORDS).collect::<Vec<_>>()).prop_shuffle(),
    )
        .prop_map(|((wrong, unreadable), order)| {
            (
                order[..wrong].to_vec(),
                order[wrong..wrong + unreadable].to_vec(),
            )
        })
}

/// Every count within the bound: at most two wrong, and `2 × wrong +
/// unreadable` at most five.
fn within_the_bound() -> Vec<(usize, usize)> {
    (0..=5usize)
        .flat_map(|unreadable| {
            (0..=2.min((5 - unreadable) / 2)).map(move |wrong| (wrong, unreadable))
        })
        .collect()
}

/// Counts past the bound for which no other phrase is within it either:
/// `wrong + unreadable + (5 − unreadable) / 2` stays below the code's
/// distance of seven, so they must be refused, never corrected.
const PAST_THE_BOUND: [(usize, usize); 9] = [
    (3, 0),
    (4, 0),
    (3, 1),
    (2, 2),
    (3, 2),
    (2, 3),
    (1, 4),
    (1, 5),
    (0, 6),
];

/// How an unreadable word is written: as `?`, as a word not on the list,
/// or left as it was and marked by its position.
#[derive(Clone, Copy, Debug)]
enum Mark {
    Question,
    Unknown,
    Position,
}

fn mark() -> impl Strategy<Value = Mark> {
    prop_oneof![
        Just(Mark::Question),
        Just(Mark::Unknown),
        Just(Mark::Position)
    ]
}

/// `phrase` with the words at `wrong` replaced by others of the list (by
/// the words of `other`, shifted where they are the same), and the words at
/// `unreadable` written as `mark` says. Returns the text and the marked
/// positions.
fn damaged(
    phrase: &Phrase,
    other: &Phrase,
    wrong: &[usize],
    unreadable: &[usize],
    mark: Mark,
) -> (String, Positions) {
    let mut words: Vec<&str> = phrase.words().collect();
    let others: Vec<&str> = other.words().collect();
    let fallback: Vec<&str> = Phrase::new(
        Realm::Live,
        Created::from_hours(1).unwrap(),
        &PhraseSecret::from_bytes([0x99; 16]),
    )
    .words()
    .collect();
    for &position in wrong {
        words[position] = if others[position] == words[position] {
            fallback
                .iter()
                .copied()
                .find(|word| *word != words[position])
                .unwrap()
        } else {
            others[position]
        };
    }
    let mut marked = Positions::NONE;
    for &position in unreadable {
        match mark {
            Mark::Question => words[position] = "?",
            Mark::Unknown => words[position] = "xyzzy",
            Mark::Position => marked = marked.with(position),
        }
    }
    (words.join(" "), marked)
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

/// Joins words with random runs of whitespace and random case, some cut
/// to their first four letters or with letters after them, as a person
/// might type them or a plate keep them.
fn mangle(words: &[&str], separators: &[String], styles: &[u8]) -> String {
    let mut text = String::new();
    for (index, word) in words.iter().enumerate() {
        text.push_str(&separators[index % separators.len()]);
        let style = styles[index % styles.len()];
        let word = match style % 3 {
            0 => (*word).to_owned(),
            1 => word[..word.len().min(4)].to_owned(),
            _ if word.len() >= 4 => format!("{}zq", &word[..4]),
            _ => (*word).to_owned(),
        };
        if style >= 3 {
            text.push_str(&word.to_ascii_uppercase());
        } else {
            text.push_str(&word);
        }
    }
    text
}

proptest! {
    #[test]
    fn phrases_round_trip(realm in realm(), hours in hours(), secret: [u8; 16]) {
        let phrase = phrase_of(realm, hours, secret);
        prop_assert_eq!(phrase.realm(), realm);
        prop_assert_eq!(phrase.created().hours(), hours);
        prop_assert_eq!(*phrase.secret().as_bytes(), secret);
        prop_assert_eq!(phrase.words().count(), PHRASE_WORDS);

        let text = phrase.text();
        let Ok(Reading::Exact(read)) = Phrase::read(text.as_str(), Positions::NONE, realm) else {
            panic!("a phrase reads as written");
        };
        prop_assert_eq!(read.text().as_str().to_owned(), text.as_str());
        let ours = phrase.master_seed(realm).unwrap();
        let again = read.master_seed(realm).unwrap();
        prop_assert_eq!(ours.as_bytes(), again.as_bytes());

        let other = match realm { Realm::Test => Realm::Live, Realm::Live => Realm::Test };
        prop_assert_eq!(
            Phrase::read(text.as_str(), Positions::NONE, other).map(|_| ()),
            Err(Error::OtherRealm { realm })
        );
        // The same secret in the other realm is another node.
        let twin = phrase_of(other, hours, secret);
        let theirs = twin.master_seed(other).unwrap();
        prop_assert_ne!(ours.as_bytes(), theirs.as_bytes());
    }

    #[test]
    fn sloppy_phrases_read_the_same(
        realm in realm(),
        hours in hours(),
        secret: [u8; 16],
        separators in prop::collection::vec("[ \t\r\n\u{a0}\u{3000}]{1,3}", 1..5),
        styles in prop::collection::vec(0u8..6, 1..7),
    ) {
        let phrase = phrase_of(realm, hours, secret);
        let words: Vec<&str> = phrase.words().collect();
        let text = mangle(&words, &separators, &styles);
        let Ok(Reading::Exact(read)) = Phrase::read(&text, Positions::NONE, realm) else {
            panic!("{text} reads as written");
        };
        prop_assert_eq!(read.text().as_str().to_owned(), phrase.text().as_str().to_owned());
    }

    // Every damage within the bound is restored, and the correction names
    // exactly the damaged positions.
    #[test]
    fn damage_within_the_bound_is_corrected(
        realm in realm(),
        hours in hours(),
        secret: [u8; 16],
        noise: [u8; 16],
        (wrong, unreadable) in damage(within_the_bound()),
        mark in mark(),
    ) {
        let phrase = phrase_of(realm, hours, secret);
        let other = phrase_of(Realm::Test, hours ^ 0x5555, noise);
        let (text, marked) = damaged(&phrase, &other, &wrong, &unreadable, mark);
        let expected = wrong.iter().chain(&unreadable).fold(Positions::NONE, |p, &i| p.with(i));
        match Phrase::read(&text, marked, realm).unwrap() {
            Reading::Exact(read) => {
                prop_assert!(expected.is_empty());
                prop_assert_eq!(read.text().as_str().to_owned(), phrase.text().as_str().to_owned());
            }
            Reading::Corrected(correction) => {
                prop_assert_eq!(correction.changed(), expected);
                prop_assert_eq!(correction.realm(), realm);
                prop_assert_eq!(correction.created().hours(), hours);
                prop_assert_eq!(correction.text().as_str().to_owned(), phrase.text().as_str().to_owned());
                prop_assert_eq!(*correction.confirm().secret().as_bytes(), secret);
            }
        }
    }

    // One step and more past the bound, as far as no other phrase is within
    // it, a phrase is refused, never corrected into another.
    #[test]
    fn damage_past_the_bound_is_refused(
        realm in realm(),
        hours in hours(),
        secret: [u8; 16],
        noise: [u8; 16],
        (wrong, unreadable) in damage(PAST_THE_BOUND.to_vec()),
        mark in mark(),
    ) {
        let phrase = phrase_of(realm, hours, secret);
        let other = phrase_of(Realm::Live, hours ^ 0x2aaa, noise);
        let (text, marked) = damaged(&phrase, &other, &wrong, &unreadable, mark);
        prop_assert_eq!(Phrase::read(&text, marked, realm).map(|_| ()), Err(Error::TooDamaged));
    }

    #[test]
    fn generated_phrases_are_deterministic(seed: [u8; 32], realm in realm(), hours in hours(), within_ms in 0u64..3_600_000) {
        let now_ms = Created::from_hours(hours).unwrap().unix_ms() + within_ms;
        let first = Phrase::generate(&mut CountingRng(seed), realm, now_ms).unwrap();
        let second = Phrase::generate(&mut CountingRng(seed), realm, now_ms).unwrap();
        prop_assert_eq!(first.text().as_str().to_owned(), second.text().as_str().to_owned());
        prop_assert_eq!(first.realm(), realm);
        prop_assert_eq!(first.created().hours(), hours);
        prop_assert!(!first.created().is_after(now_ms));
    }

    #[test]
    fn phrase_read_never_panics(text in any::<String>(), bits: u32, realm in realm()) {
        let marked = Positions::from_bits(bits & 0xf_ffff).unwrap();
        match Phrase::read(&text, marked, realm) {
            Ok(Reading::Exact(phrase)) => {
                let text = phrase.text();
                let again = Phrase::read(text.as_str(), Positions::NONE, realm);
                prop_assert!(matches!(again, Ok(Reading::Exact(_))));
            }
            Ok(Reading::Corrected(correction)) => {
                prop_assert!(!correction.changed().is_empty());
                prop_assert!(correction.changed().len() <= 5);
            }
            Err(_) => {}
        }
    }

    /// Twenty words of the list in any order are almost never a phrase,
    /// and what is taken is within two words of what was read: nothing is
    /// marked and every word is on the list, so only wrong words are
    /// corrected.
    #[test]
    fn word_salad_is_refused_or_within_the_bound(
        secrets: [[u8; 16]; 2],
        picks in prop::collection::vec(any::<prop::sample::Index>(), 0..30),
        realm in realm(),
    ) {
        let one = phrase_of(Realm::Test, 1, secrets[0]);
        let two = phrase_of(Realm::Live, 2, secrets[1]);
        let words: Vec<&str> = one.words().chain(two.words()).collect();
        let salad: Vec<&str> = picks.iter().map(|pick| *pick.get(&words)).collect();
        match Phrase::read(&salad.join(" "), Positions::NONE, realm) {
            Ok(Reading::Exact(phrase)) => {
                prop_assert_eq!(phrase.text().as_str().to_owned(), salad.join(" "));
            }
            Ok(Reading::Corrected(correction)) => {
                prop_assert!(correction.changed().len() <= 2);
            }
            Err(error) => {
                let expected = matches!(
                    error,
                    Error::TooDamaged
                        | Error::Bip39Phrase { .. }
                        | Error::WrongWordCount { .. }
                        | Error::InvalidKind
                        | Error::NewerKind { .. }
                        | Error::OtherRealm { .. }
                );
                prop_assert!(expected, "{}", error);
            }
        }
    }

    #[test]
    fn node_id_parse_never_panics(text in "[a-zA-Z2-9=]{37,41}|\\PC{0,48}") {
        if let Ok(id) = NodeId::parse(&text) {
            prop_assert_eq!(id.as_str(), text.to_ascii_lowercase());
        }
    }

    // Every 24 bytes are an ID, whose text, in either case, names it and
    // no other.
    #[test]
    fn node_ids_round_trip(bytes: [u8; NODE_ID_BYTES]) {
        let id = NodeId::from_bytes(&bytes);
        prop_assert_eq!(id.to_bytes(), bytes);
        prop_assert_eq!(id.as_str().len(), NODE_ID_LENGTH);
        prop_assert!(id.as_str().bytes().all(|b| b.is_ascii_lowercase() || (b'2'..=b'7').contains(&b)));
        prop_assert_eq!(NodeId::parse(id.as_str()), Ok(id));
        prop_assert_eq!(NodeId::parse(&id.as_str().to_ascii_uppercase()), Ok(id));
        prop_assert_eq!(id.to_string(), id.as_str());
    }

    #[test]
    fn computed_node_ids_are_valid(key: [u8; 32]) {
        let id = NodeId::from_public_key(&key);
        prop_assert_eq!(NodeId::parse(id.as_str()), Ok(id));
    }

    #[test]
    fn segment_parse_never_panics(text in any::<String>()) {
        if let Ok(ledger) = Segment::parse(&text) {
            prop_assert!(!ledger.as_str().is_empty() && !ledger.as_str().contains('/'));
            let key = NodeKey::from_bytes(&[5; 32]);
            let _ = key.secret(NodeSecret::StreamReceiver { ledger });
        }
    }

    #[test]
    fn der_and_key_decoders_never_panic(bytes in prop::collection::vec(any::<u8>(), 0..64), key: [u8; 32]) {
        let _ = NodeKey::from_pkcs8_der(&bytes);
        let _ = PublicKey::from_spki_der(&bytes);
        let _ = PublicKey::from_bytes(&key);
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
