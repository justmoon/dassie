//! Writes the hub's golden vectors (spec/areas/hubs.md#golden-vectors) to
//! standard output, from the fixed keys of `tests/support/vectors.rs`.
//! Ed25519 and the XRP Ledger's signatures are deterministic, so the output
//! is the same every time. From the repository root:
//!
//! ```text
//! cargo run -p dassie-hub --example hub_vectors > testdata/hub/vectors.json
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
    println!("{}", serde_json::to_string_pretty(&vectors::all()).unwrap());
}
