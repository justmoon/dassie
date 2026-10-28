//! Writes the table of SLH-DSA keys that tests look up rather than
//! generate (`TableKeygen`, spec/areas/testing.md#slh-dsa-keys-in-tests):
//! `src/test_keys.txt`, with the keys of the seeds given and of every seed
//! in it already, each made by the real key generation.
//!
//! ```text
//! cargo run --release -p dassie-identity --example slh_dsa_test_keys -- [<SK.seed><SK.prf><PK.seed>]...
//! ```
//!
//! Each argument is the three seeds in hex, as a test that lacks its key
//! prints them. Run alone, it generates every key in the table again. The
//! table always has the keys of the node keys of every byte repeated,
//! `[b; 32]`, which the simulations take for their nodes.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use std::collections::BTreeSet;
use std::fmt::Write as _;

use dassie_identity::SlhDsaSeed::{PkSeed, SkPrf, SkSeed};
use dassie_identity::{NodeKey, NodeSecret, SLH_DSA_SEED_BYTES, SlhDsaPublicKey};

type Seeds = [[u8; SLH_DSA_SEED_BYTES]; 3];

const HEADER: &str = "\
# The SLH-DSA-SHA2-128s public keys of the seeds the workspace's tests use,
# which tests look up with dassie-identity's `TableKeygen` rather than generate
# (spec/areas/testing.md#slh-dsa-keys-in-tests). Written by
# `cargo run --release -p dassie-identity --example slh_dsa_test_keys -- <seeds>`,
# which makes each key with the real key generation, and always adds the node
# keys of every byte repeated, which the simulations take. One key a line:
# SK.seed SK.prf PK.seed public-key, in hex.
";

fn main() {
    // As cargo names the package when it runs this, never `env!`'s.
    #[allow(clippy::disallowed_methods)] // What cargo says, not configuration.
    let dir = std::env::var("CARGO_MANIFEST_DIR").expect("run by cargo");
    let path = format!("{dir}/src/test_keys.txt");
    let table = std::fs::read_to_string(&path).unwrap_or_default();
    let mut seeds: BTreeSet<Seeds> = table
        .lines()
        .map(|line| line.split('#').next().unwrap().trim())
        .filter(|line| !line.is_empty())
        .map(|line| {
            let fields: Vec<&str> = line.split_ascii_whitespace().collect();
            assert_eq!(fields.len(), 4, "not an entry: {line}");
            [hex(fields[0]), hex(fields[1]), hex(fields[2])]
        })
        .collect();
    let before = seeds.len();
    for argument in std::env::args().skip(1) {
        assert_eq!(
            argument.len(),
            SLH_DSA_SEED_BYTES * 6,
            "{argument}: the three seeds are {} hex digits",
            SLH_DSA_SEED_BYTES * 6
        );
        let (sk_seed, rest) = argument.split_at(SLH_DSA_SEED_BYTES * 2);
        let (sk_prf, pk_seed) = rest.split_at(SLH_DSA_SEED_BYTES * 2);
        seeds.insert([hex(sk_seed), hex(sk_prf), hex(pk_seed)]);
    }
    // The node keys of every byte repeated, which the simulations take for
    // their nodes, so that a run from any seed finds its keys.
    for byte in 0..=u8::MAX {
        let key = NodeKey::from_bytes(&[byte; 32]);
        seeds.insert([SkSeed, SkPrf, PkSeed].map(|which| {
            let secret = key.secret(NodeSecret::SlhDsa(which));
            secret.as_bytes()[..SLH_DSA_SEED_BYTES].try_into().unwrap()
        }));
    }
    let mut out = String::from(HEADER);
    for [sk_seed, sk_prf, pk_seed] in &seeds {
        let public = SlhDsaPublicKey::from_seeds(sk_seed, sk_prf, pk_seed);
        writeln!(
            out,
            "{} {} {} {}",
            to_hex(sk_seed),
            to_hex(sk_prf),
            to_hex(pk_seed),
            to_hex(public.as_bytes())
        )
        .unwrap();
    }
    std::fs::write(&path, out).unwrap();
    println!("{path}: {} keys, {} new", seeds.len(), seeds.len() - before);
}

fn hex(text: &str) -> [u8; SLH_DSA_SEED_BYTES] {
    assert_eq!(text.len(), SLH_DSA_SEED_BYTES * 2, "{text}: not a seed");
    let mut bytes = [0; SLH_DSA_SEED_BYTES];
    for (index, byte) in bytes.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&text[index * 2..index * 2 + 2], 16)
            .unwrap_or_else(|_| panic!("{text}: not hex"));
    }
    bytes
}

fn to_hex(bytes: &[u8]) -> String {
    bytes.iter().fold(String::new(), |mut text, byte| {
        write!(text, "{byte:02x}").unwrap();
        text
    })
}
