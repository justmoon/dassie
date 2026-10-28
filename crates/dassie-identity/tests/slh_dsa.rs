//! FIPS 205's known-answer vectors for the key generation of
//! SLH-DSA-SHA2-128s, the node's SLH-DSA key
//! (spec/decisions/post-quantum-node-id.md): NIST's ACVP test group, taken
//! as it is (`tests/data/slh_dsa_keygen.json` names where from). They pin
//! the release candidate of `slh-dsa` the crate depends on, whose public
//! key every node ID commits to.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

mod support;

use dassie_identity::{SLH_DSA_SEED_BYTES, SlhDsaPublicKey};
use support::{str_field, to_hex};

fn seed(entry: &serde_json::Value, field: &str) -> [u8; SLH_DSA_SEED_BYTES] {
    let text = str_field(entry, field);
    let mut seed = [0; SLH_DSA_SEED_BYTES];
    for (index, byte) in seed.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&text[index * 2..index * 2 + 2], 16).unwrap();
    }
    seed
}

#[test]
fn key_generation_from_seeds() {
    let path = format!(
        "{}/tests/data/slh_dsa_keygen.json",
        env!("CARGO_MANIFEST_DIR")
    );
    let vectors: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    assert_eq!(vectors["parameterSet"], "SLH-DSA-SHA2-128s");
    let tests = vectors["tests"].as_array().unwrap();
    assert_eq!(tests.len(), 10);
    for entry in tests {
        let key = SlhDsaPublicKey::from_seeds(
            &seed(entry, "skSeed"),
            &seed(entry, "skPrf"),
            &seed(entry, "pkSeed"),
        );
        assert_eq!(to_hex(key.as_bytes()), str_field(entry, "pk"), "{entry}");
    }
}
