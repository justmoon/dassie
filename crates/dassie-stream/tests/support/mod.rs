//! Helpers shared by the integration tests.

#![allow(
    dead_code,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use serde_json::Value;

/// Decodes lowercase or uppercase hex. Panics on malformed input, which in a
/// test means a broken vector file.
pub fn hex(text: &str) -> Vec<u8> {
    assert!(text.len().is_multiple_of(2), "odd-length hex: {text}");
    (0..text.len())
        .step_by(2)
        .map(|index| u8::from_str_radix(&text[index..index + 2], 16).expect("hex digit"))
        .collect()
}

/// Encodes bytes as lowercase hex.
pub fn to_hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// Loads a vector file from `tests/data`.
pub fn vectors(name: &str) -> Value {
    let path = format!("{}/tests/data/{name}", manifest_dir());
    let text = std::fs::read_to_string(&path).expect("vector file");
    let value: Value = serde_json::from_str(&text).expect("vector JSON");
    assert_eq!(
        value["source"], "e962ab78",
        "vectors must come from the original"
    );
    value
}

/// A string field of a vector.
pub fn str_field<'a>(value: &'a Value, key: &str) -> &'a str {
    value[key]
        .as_str()
        .unwrap_or_else(|| panic!("missing string field {key}"))
}

pub mod arbitrary;

/// The package's directory, as cargo and nextest name it to a test they
/// run, never `env!`'s from when the test compiled: a build is reused in
/// other checkouts of the repository, whose files are their own.
#[allow(clippy::disallowed_methods, clippy::expect_used)] // What the test runner says, not configuration.
fn manifest_dir() -> String {
    std::env::var("CARGO_MANIFEST_DIR").expect("cargo and nextest name the package's directory")
}
