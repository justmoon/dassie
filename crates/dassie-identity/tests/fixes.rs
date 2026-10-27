//! What the identity crate refuses, each as a test: a phrase that is not
//! BIP-39, DER that is not exactly the encoding it writes, seed paths with
//! empty segments; and the login proof, which it compares in constant time.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use dassie_identity::{
    Error, Mnemonic, NodeKey, PKCS8_DER_LENGTH, PublicKey, SPKI_DER_LENGTH, Segment,
};

const ZERO_PHRASE: &str =
    "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about";

/// A phrase with a swapped or mistyped word fails its checksum, rather
/// than silently deriving a different node.
#[test]
fn a_phrase_with_a_bad_checksum_is_rejected() {
    assert!(Mnemonic::parse(ZERO_PHRASE).is_ok());
    let wrong_last_word = ZERO_PHRASE.replace("about", "abandon");
    assert_eq!(
        Mnemonic::parse(&wrong_last_word).map(|_| ()),
        Err(Error::InvalidChecksum)
    );
    let swapped = "about abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon";
    assert_eq!(
        Mnemonic::parse(swapped).map(|_| ()),
        Err(Error::InvalidChecksum)
    );
}

/// Unknown words are reported with their position so the UI can point at
/// them.
#[test]
fn an_unknown_word_is_rejected() {
    let typo = ZERO_PHRASE.replacen("abandon", "abandn", 1);
    assert_eq!(
        Mnemonic::parse(&typo).map(|_| ()),
        Err(Error::UnknownWord { position: 0 })
    );
    // A four-letter prefix identifies a word uniquely, but it is not the
    // word, and the seed is computed over the full word.
    let prefix = ZERO_PHRASE.replace("about", "abou");
    assert_eq!(
        Mnemonic::parse(&prefix).map(|_| ()),
        Err(Error::UnknownWord { position: 11 })
    );
    let eleven = ZERO_PHRASE.replacen("abandon ", "", 1);
    assert_eq!(
        Mnemonic::parse(&eleven).map(|_| ()),
        Err(Error::InvalidWordCount)
    );
}

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
