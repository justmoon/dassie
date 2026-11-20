//! FIPS 203's known-answer vectors for ML-KEM-768, the post-quantum half of
//! the hybrid key exchange X25519MLKEM768
//! (spec/decisions/hybrid-key-exchange.md): cases of NIST's ACVP test
//! groups, taken as they are (`tests/data/ml_kem_768.json` names where
//! from). They pin `ml-kem` through the functions the key exchange calls,
//! with the randomness the vectors give.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use dassie_node::tls::x25519_mlkem768::{
    CIPHERTEXT_BYTES, ENCAPSULATION_KEY_BYTES, decapsulate, encapsulate, keygen,
};
use ml_kem::array::Array;
use ml_kem::ml_kem_768::DecapsulationKey;
use serde_json::Value;

fn vectors() -> Value {
    let path = format!("{}/tests/data/ml_kem_768.json", manifest_dir());
    let vectors: Value = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    assert_eq!(vectors["parameterSet"], "ML-KEM-768");
    vectors
}

fn cases<'a>(vectors: &'a Value, group: &str) -> &'a [Value] {
    let cases = vectors[group].as_array().unwrap();
    assert!(!cases.is_empty(), "{group}");
    cases
}

fn bytes(entry: &Value, field: &str) -> Vec<u8> {
    let text = entry[field].as_str().unwrap();
    (0..text.len())
        .step_by(2)
        .map(|at| u8::from_str_radix(&text[at..at + 2], 16).unwrap())
        .collect()
}

fn array<const N: usize>(entry: &Value, field: &str) -> [u8; N] {
    bytes(entry, field).try_into().unwrap()
}

#[test]
fn key_generation_from_seeds() {
    let vectors = vectors();
    for entry in cases(&vectors, "keyGen") {
        let mut seed = [0; 64];
        seed[..32].copy_from_slice(&array::<32>(entry, "d"));
        seed[32..].copy_from_slice(&array::<32>(entry, "z"));
        let (_, encapsulation_key) = keygen(&seed);
        assert_eq!(encapsulation_key.len(), ENCAPSULATION_KEY_BYTES);
        assert_eq!(encapsulation_key, bytes(entry, "ek"), "{entry}");
    }
}

#[test]
fn encapsulation_with_given_randomness() {
    let vectors = vectors();
    for entry in cases(&vectors, "encapsulation") {
        let (ciphertext, shared) =
            encapsulate(&bytes(entry, "ek"), &array::<32>(entry, "m")).unwrap();
        assert_eq!(ciphertext.len(), CIPHERTEXT_BYTES);
        assert_eq!(ciphertext, bytes(entry, "c"), "{entry}");
        assert_eq!(shared.to_vec(), bytes(entry, "k"), "{entry}");
    }
}

// The vectors give the decapsulation key in the expanded form, which the
// key exchange never holds (it keeps the seed), and the shared key for a
// modified ciphertext too: implicit rejection.
#[test]
#[allow(deprecated)]
fn decapsulation_and_implicit_rejection() {
    let vectors = vectors();
    for entry in cases(&vectors, "decapsulation") {
        let expanded = Array::try_from(bytes(entry, "dk").as_slice()).unwrap();
        let key = DecapsulationKey::from_expanded(&expanded).unwrap();
        let shared = decapsulate(&key, &bytes(entry, "c")).unwrap();
        assert_eq!(shared.to_vec(), bytes(entry, "k"), "{entry}");
    }
}

#[test]
fn the_encapsulation_key_check() {
    let vectors = vectors();
    for entry in cases(&vectors, "encapsulationKeyCheck") {
        let passed = entry["testPassed"].as_bool().unwrap();
        let result = encapsulate(&bytes(entry, "ek"), &[0; 32]);
        assert_eq!(result.is_some(), passed, "{entry}");
    }
}

/// The package's directory, as cargo and nextest name it to a test they
/// run, never `env!`'s from when the test compiled: a build is reused in
/// other checkouts of the repository, whose files are their own.
#[allow(clippy::disallowed_methods, clippy::expect_used)] // What the test runner says, not configuration.
fn manifest_dir() -> String {
    std::env::var("CARGO_MANIFEST_DIR").expect("cargo and nextest name the package's directory")
}
