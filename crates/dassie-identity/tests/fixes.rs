//! One test per flaw of the original that the rewrite fixes. Each would
//! have failed against the original's behaviour
//! (spec/original/README.md#how-the-rewrite-treated-the-gaps).

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use dassie_identity::{
    Error, Mnemonic, NodeKey, PKCS8_DER_LENGTH, PublicKey, SPKI_DER_LENGTH, SeedPath, X25519Public,
    X25519Secret,
};

const ZERO_PHRASE: &str =
    "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about";

/// UI-5: the original fed any text to PBKDF2, so a phrase with a swapped
/// or mistyped word silently produced a different node.
#[test]
fn ui_5_phrase_with_a_bad_checksum_is_rejected() {
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

/// UI-5: unknown words are reported with their position so the UI can
/// point at them.
#[test]
fn ui_5_unknown_word_is_rejected() {
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

/// lib-x509's SPKI constant omitted the BIT STRING's unused-bits byte, so
/// the 44-byte DER it parsed gave a 33-byte "key" starting with `00`
/// (spec/original/03-peer-network.md#21-node-private-key).
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

/// PKCS#8 decoding is strict, and the setup route can no longer store a key
/// of the wrong length (spec/original/02-node-runtime.md#83-post-apisetup,
/// step 3): a node key is 32 bytes by type.
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

/// The original compared the login proof as hex strings with `!==`, which
/// returns at the first differing character
/// (spec/original/02-node-runtime.md#83-post-apisetup,
/// spec/original/02-node-runtime.md#84-post-apilogin). The proof is now
/// checked as bytes with `subtle`'s constant-time equality.
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
/// doubled or trailing slash derived an HMAC over an empty string.
#[test]
fn empty_seed_path_segments_are_rejected() {
    for text in ["", "/", "node/", "/node", "node//login", "node/login/"] {
        assert_eq!(
            SeedPath::parse(text),
            Err(Error::InvalidSeedPath),
            "{text:?}"
        );
    }
    let key = NodeKey::from_bytes(&[9; 32]);
    assert_eq!(
        key.settlement_entropy("xrpl", "seed//mainnet").map(|_| ()),
        Err(Error::InvalidSeedPath)
    );
}

/// The original used X25519 results without the RFC 7748 §6.1 check, so a
/// peer with a small-order key could force an all-zero shared secret.
#[test]
fn non_contributory_x25519_is_rejected() {
    let secret = NodeKey::from_bytes(&[3; 32]).x25519_secret();
    let small_order: [[u8; 32]; 4] = [
        [0; 32],
        {
            let mut one = [0; 32];
            one[0] = 1;
            one
        },
        // A point of order 8.
        hex32("e0eb7a7c3b41b8ae1656e3faf19fc46ada098deb9c32b1fd866205165f49b800"),
        // p - 1, of order 2.
        hex32("ecffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff7f"),
    ];
    for point in small_order {
        assert_eq!(
            secret
                .diffie_hellman(&X25519Public::from_bytes(point))
                .map(|_| ()),
            Err(Error::NonContributory)
        );
    }

    // An Ed25519 key of small order maps to one of them.
    let mut identity = [0; 32];
    identity[0] = 1;
    let peer = PublicKey::from_bytes(&identity).unwrap();
    assert_eq!(
        secret.diffie_hellman(&peer.to_x25519()).map(|_| ()),
        Err(Error::NonContributory)
    );

    // A proper key still works.
    let other = X25519Secret::from_bytes([4; 32]);
    let ours = secret.diffie_hellman(&other.public_key()).unwrap();
    let theirs = other.diffie_hellman(&secret.public_key()).unwrap();
    assert_eq!(ours.as_bytes(), theirs.as_bytes());
}

fn hex32(text: &str) -> [u8; 32] {
    let mut bytes = [0; 32];
    for (index, byte) in bytes.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&text[index * 2..index * 2 + 2], 16).unwrap();
    }
    bytes
}
