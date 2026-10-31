//! Writes the hub's golden vectors (spec/areas/hubs.md#golden-vectors) of
//! one version of the hub's protocol to standard output, from the fixed
//! keys of `tests/support/vectors.rs`. Ed25519 and the XRP Ledger's
//! signatures are deterministic, so the output is the same every time. From
//! the repository root, for version 3:
//!
//! ```text
//! cargo run -p dassie-hub --example hub_vectors -- 3 > testdata/hub/vectors-v3.json
//! ```
//!
//! What the vectors pin is frozen (spec/overview.md#frozen-surfaces):
//! running this again must not change the committed file, and
//! `tests/vectors.rs` fails if the code no longer makes or reads those
//! bytes.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::print_stdout
)]

#[path = "../tests/support/vectors.rs"]
mod vectors;

fn main() {
    let version = std::env::args()
        .nth(1)
        .expect("the version of the hub's protocol")
        .parse()
        .expect("a version");
    assert!(dassie_hub::rules::runs(version), "a version this code runs");
    println!(
        "{}",
        serde_json::to_string_pretty(&vectors::all(version)).unwrap()
    );
}
