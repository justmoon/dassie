//! What the identity crate refuses, each as a test: a phrase it cannot read
//! or would have to correct without asking, DER that is not exactly the
//! encoding it writes, seed paths with empty segments; and the login proof,
//! which it compares in constant time.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use dassie_identity::{
    Created, Error, NodeKey, PKCS8_DER_LENGTH, Phrase, PhraseSecret, Positions, PublicKey, Reading,
    Realm, SPKI_DER_LENGTH, Segment,
};

/// The BIP-39 phrase of sixteen zero bytes.
const ZERO_BIP39_PHRASE: &str =
    "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about";

/// A reader's clock past every phrase's creation time.
const AFTER_EVERY_PHRASE: u64 = u64::MAX;

fn phrase(realm: Realm) -> Phrase {
    let created = Created::from_hours(7_000).unwrap();
    Phrase::new(realm, created, &PhraseSecret::from_bytes([0x42; 16]))
}

fn words(phrase: &Phrase) -> Vec<&'static str> {
    phrase.words().collect()
}

/// A wrong word is restored, but never silently: the reading is a
/// correction, which gives up the phrase only when confirmed, and a reader
/// with no one to confirm it is told which positions, never which words.
#[test]
fn a_wrong_word_is_corrected_only_with_confirmation() {
    let phrase = phrase(Realm::Test);
    let mut typed = words(&phrase);
    typed[7] = if typed[7] == "zoo" { "abandon" } else { "zoo" };
    let reading = Phrase::read(
        &typed.join(" "),
        Positions::NONE,
        Realm::Test,
        AFTER_EVERY_PHRASE,
    )
    .unwrap();
    let Reading::Corrected(correction) = reading else {
        panic!("a wrong word is a correction");
    };
    assert_eq!(correction.changed(), Positions::NONE.with(7));
    assert_eq!(correction.text().as_str(), phrase.text().as_str());

    let reading = Phrase::read(
        &typed.join(" "),
        Positions::NONE,
        Realm::Test,
        AFTER_EVERY_PHRASE,
    )
    .unwrap();
    let error = reading.without_correction().unwrap_err();
    assert_eq!(
        error,
        Error::NeedsConfirmation {
            changed: Positions::NONE.with(7)
        }
    );
    assert_eq!(
        error.to_string(),
        "the phrase needs correcting at word 8, which only its owner can confirm"
    );
}

/// A word not on the list, or one marked unreadable, is an erasure: its
/// place is known, so it costs half what a wrong word does.
#[test]
fn unknown_and_marked_words_are_erasures() {
    let phrase = phrase(Realm::Live);
    let mut typed = words(&phrase);
    for position in [0, 4, 9, 14, 19] {
        typed[position] = "?";
    }
    let reading = Phrase::read(
        &typed.join(" "),
        Positions::NONE,
        Realm::Live,
        AFTER_EVERY_PHRASE,
    )
    .unwrap();
    let Reading::Corrected(correction) = reading else {
        panic!("five unreadable words are restored");
    };
    assert_eq!(correction.changed().len(), 5);
    assert_eq!(correction.confirm().text().as_str(), phrase.text().as_str());

    // A sixth is one too many, though the code could restore six: one
    // check word is kept back to catch a mistake.
    typed[1] = "?";
    assert_eq!(
        Phrase::read(
            &typed.join(" "),
            Positions::NONE,
            Realm::Live,
            AFTER_EVERY_PHRASE
        )
        .map(|_| ()),
        Err(Error::TooDamaged)
    );

    // Marking a word that reads fine makes it an erasure too.
    let unreadable = Positions::NONE.with(3).with(11);
    let Ok(Reading::Corrected(correction)) = Phrase::read(
        phrase.text().as_str(),
        unreadable,
        Realm::Live,
        AFTER_EVERY_PHRASE,
    ) else {
        panic!("marked words are restored");
    };
    assert_eq!(correction.changed(), unreadable);
}

/// Three wrong words are refused rather than corrected, even though the
/// code's distance would allow it: one random phrase in 2.4 × 10⁸ would
/// pass.
#[test]
fn three_wrong_words_are_too_many() {
    let phrase = phrase(Realm::Test);
    let mut typed = words(&phrase);
    for position in [2, 10, 17] {
        typed[position] = if typed[position] == "zoo" {
            "abandon"
        } else {
            "zoo"
        };
    }
    assert_eq!(
        Phrase::read(
            &typed.join(" "),
            Positions::NONE,
            Realm::Test,
            AFTER_EVERY_PHRASE
        )
        .map(|_| ()),
        Err(Error::TooDamaged)
    );
}

/// A BIP-39 phrase, another wallet's or a Dassie phrase from before this
/// format, is refused as that, and other counts as no phrase.
#[test]
fn bip39_phrases_are_refused_as_such() {
    assert_eq!(
        Phrase::read(
            ZERO_BIP39_PHRASE,
            Positions::NONE,
            Realm::Test,
            AFTER_EVERY_PHRASE
        )
        .map(|_| ()),
        Err(Error::Bip39Phrase { words: 12 })
    );
    let eleven = ZERO_BIP39_PHRASE.replacen("abandon ", "", 1);
    assert_eq!(
        Phrase::read(&eleven, Positions::NONE, Realm::Test, AFTER_EVERY_PHRASE).map(|_| ()),
        Err(Error::WrongWordCount { words: 11 })
    );
    let long = " ".repeat(dassie_identity::MAX_PHRASE_INPUT_BYTES + 1);
    assert_eq!(
        Phrase::read(&long, Positions::NONE, Realm::Test, AFTER_EVERY_PHRASE).map(|_| ()),
        Err(Error::PhraseTooLong)
    );
}

/// The realm is checked when reading and again when deriving, and before
/// any correction is shown, so the owner never confirms a phrase that is
/// refused next.
#[test]
fn a_phrase_of_the_other_realm_is_refused_before_correction() {
    let live = phrase(Realm::Live);
    assert_eq!(
        Phrase::read(
            live.text().as_str(),
            Positions::NONE,
            Realm::Test,
            AFTER_EVERY_PHRASE
        )
        .map(|_| ()),
        Err(Error::OtherRealm { realm: Realm::Live })
    );
    let mut typed = words(&live);
    typed[5] = "?";
    assert_eq!(
        Phrase::read(
            &typed.join(" "),
            Positions::NONE,
            Realm::Test,
            AFTER_EVERY_PHRASE
        )
        .map(|_| ()),
        Err(Error::OtherRealm { realm: Realm::Live })
    );
    assert_eq!(
        live.master_seed(Realm::Test).map(|_| ()),
        Err(Error::OtherRealm { realm: Realm::Live })
    );
    // The same secret in the other realm is another node.
    let test = phrase(Realm::Test);
    let live_key = live.master_seed(Realm::Live).unwrap().node_key();
    let test_key = test.master_seed(Realm::Test).unwrap().node_key();
    assert_eq!(live.secret().as_bytes(), test.secret().as_bytes());
    assert_ne!(live_key.identity().node_id(), test_key.identity().node_id());
}

/// A phrase made more than a day after the reader's clock is refused, and
/// before any correction is shown; the refusal names the phrase's date and
/// says how to read it anyway: by setting the clock past that date.
#[test]
fn a_phrase_from_the_future_is_refused_with_its_date() {
    // 2026-02-19T13:00Z, the decision's example of a date.
    let made = Created::from_hours((31 + 18) * 24 + 13).unwrap();
    let phrase = Phrase::new(Realm::Test, made, &PhraseSecret::from_bytes([1; 16]));
    let day_ms = 86_400_000;
    let behind = made.unix_ms() - day_ms - 1;
    let error = Phrase::read(phrase.text().as_str(), Positions::NONE, Realm::Test, behind)
        .map(|_| ())
        .unwrap_err();
    assert_eq!(error, Error::CreatedInFuture { created: made });
    let text = error.to_string();
    assert!(text.contains("made on 2026-02-19"), "{text}");
    assert!(text.contains("no phrase is made in the future"), "{text}");
    assert!(
        text.contains("setting this clock past 2026-02-19"),
        "{text}"
    );

    // Damaged, it is refused all the same, and no correction is shown.
    let mut typed = words(&phrase);
    typed[4] = "?";
    assert_eq!(
        Phrase::read(&typed.join(" "), Positions::NONE, Realm::Test, behind).map(|_| ()),
        Err(Error::CreatedInFuture { created: made })
    );

    // A day ahead is no more than a clock that is off.
    let Ok(Reading::Exact(read)) = Phrase::read(
        phrase.text().as_str(),
        Positions::NONE,
        Realm::Test,
        made.unix_ms() - day_ms,
    ) else {
        panic!("read as written");
    };
    assert!(read.master_seed(Realm::Test).is_ok());
}

/// A generator makes a phrase only while its clock lies within the field.
#[test]
fn no_phrase_is_made_outside_the_field() {
    let mut rng = Fixed;
    let before = Created::EPOCH_UNIX_MS - 1;
    assert_eq!(
        Phrase::generate(&mut rng, Realm::Test, before).map(|_| ()),
        Err(Error::ClockOutOfRange)
    );
    let after = Created::from_hours(Created::MAX_HOURS).unwrap().unix_ms() + 3_600_000;
    assert_eq!(
        Phrase::generate(&mut rng, Realm::Test, after).map(|_| ()),
        Err(Error::ClockOutOfRange)
    );
    let phrase = Phrase::generate(&mut rng, Realm::Live, after - 1).unwrap();
    assert_eq!(phrase.created().hours(), Created::MAX_HOURS);
    assert_eq!(phrase.realm(), Realm::Live);
    assert_eq!(phrase.secret().as_bytes(), &[7; 16]);
}

/// An RNG that always gives sevens.
struct Fixed;

impl rand_core::TryRng for Fixed {
    type Error = core::convert::Infallible;

    fn try_next_u32(&mut self) -> Result<u32, Self::Error> {
        Ok(0x0707_0707)
    }

    fn try_next_u64(&mut self) -> Result<u64, Self::Error> {
        Ok(0x0707_0707_0707_0707)
    }

    fn try_fill_bytes(&mut self, destination: &mut [u8]) -> Result<(), Self::Error> {
        destination.fill(7);
        Ok(())
    }
}

impl rand_core::TryCryptoRng for Fixed {}

/// The SPKI encoding has the BIT STRING's unused-bits byte, so a 44-byte
/// DER gives a 32-byte key, not a 33-byte one starting with `00`.
#[test]
fn spki_der_has_the_unused_bits_byte() {
    let key = NodeKey::from_bytes(&[1; 32]).public_key();
    let der = key.to_spki_der();
    assert_eq!(der.len(), SPKI_DER_LENGTH);
    assert_eq!(der.len(), 44);
    assert_eq!(
        &der[..12],
        &[
            0x30, 0x2a, 0x30, 0x05, 0x06, 0x03, 0x2b, 0x65, 0x70, 0x03, 0x21, 0x00
        ]
    );
    assert_eq!(&der[12..], key.as_bytes());
    assert_eq!(PublicKey::from_spki_der(&der), Ok(key));

    // The original's prefix without the `00` byte is not a valid encoding.
    let mut buggy = der[..11].to_vec();
    buggy.extend_from_slice(key.as_bytes());
    assert_eq!(PublicKey::from_spki_der(&buggy), Err(Error::InvalidDer));
}

/// PKCS#8 decoding is strict, and setup cannot store a key of the wrong
/// length: a node key is 32 bytes by type.
#[test]
fn pkcs8_der_is_exact() {
    let key = NodeKey::from_bytes(&[1; 32]);
    let der = key.to_pkcs8_der();
    assert_eq!(der.as_bytes().len(), PKCS8_DER_LENGTH);
    let decoded = NodeKey::from_pkcs8_der(der.as_bytes()).unwrap();
    assert_eq!(decoded.as_bytes(), key.as_bytes());

    let bytes = der.as_bytes();
    assert!(NodeKey::from_pkcs8_der(&bytes[..47]).is_err());
    let mut longer = bytes.to_vec();
    longer.push(0);
    assert!(NodeKey::from_pkcs8_der(&longer).is_err());
    let mut version_one = bytes.to_vec();
    version_one[4] = 1;
    assert!(NodeKey::from_pkcs8_der(&version_one).is_err());
}

/// The login proof is checked as bytes with `subtle`'s constant-time
/// equality, so how long a refusal takes says nothing about the guess.
#[test]
fn login_proof_is_checked_in_constant_time() {
    let key = NodeKey::from_bytes(&[9; 32]);
    let secret = key.login_secret();
    let proof = *secret.as_bytes();
    assert!(secret.verify_proof(&proof));

    for index in 0..proof.len() {
        let mut wrong = proof;
        wrong[index] ^= 0x01;
        assert!(!secret.verify_proof(&wrong), "byte {index}");
    }
    assert!(!secret.verify_proof(&proof[..31]));
    assert!(!secret.verify_proof(&[]));
}

/// The original split paths on `/` without looking at the segments, so a
/// doubled or trailing slash derived an HMAC over an empty string. A
/// segment a caller names is never empty and never holds a `/`.
#[test]
fn empty_seed_path_segments_are_rejected() {
    for text in ["", "/", "node/", "/node", "node//login", "node/login/"] {
        assert_eq!(
            Segment::parse(text),
            Err(Error::InvalidSeedPath),
            "{text:?}"
        );
    }
}
