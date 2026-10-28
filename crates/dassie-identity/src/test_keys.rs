//! The SLH-DSA keys of the seeds the workspace's tests use, which tests
//! look up rather than generate (spec/areas/testing.md#slh-dsa-keys-in-tests).
//!
//! A key generation takes milliseconds optimized and most of a second
//! unoptimized, and the tests make node keys by the hundred. The table,
//! `test_keys.txt` beside this file, holds what FIPS 205's
//! `slh_keygen_internal` gives for each seed, made by
//! `examples/slh_dsa_test_keys.rs` with the real key generation, which a
//! unit test here checks every entry against, so the table cannot drift
//! from the crate. A test passes [`TableKeygen`] where a node takes its
//! [`SlhDsaKeygen`]; any seed the table lacks panics, naming the command
//! that adds it, so a test never generates a key and never takes a key
//! from a random seed.
//!
//! Nothing in a node passes it: a node's key generation is
//! [`Fips205Keygen`](crate::Fips205Keygen), and the table is linked only
//! into what uses it.

use crate::node_key::NodeKey;
use crate::secret::Hex;
use crate::slh_dsa::{
    IdentityKey, SLH_DSA_PUBLIC_KEY_BYTES, SLH_DSA_SEED_BYTES, SlhDsaKeygen, SlhDsaPublicKey,
};

/// The table: a line per key, `SK.seed SK.prf PK.seed public-key` in hex,
/// and comments after `#`.
const TABLE: &str = include_str!("test_keys.txt");

/// The command that adds a key to the table, given its seeds.
const ADD: &str = "cargo run --release -p dassie-identity --example slh_dsa_test_keys --";

/// The SLH-DSA key generation of tests: the public keys of the tests'
/// seeds, from a table of real keys. It panics on a seed the table lacks,
/// with the command that adds it.
#[derive(Clone, Copy, Debug, Default)]
pub struct TableKeygen;

impl TableKeygen {
    /// The node key of Ed25519 seed `seed`, with its identity from the
    /// table: a test's node key.
    #[must_use]
    pub fn identity_key(seed: &[u8; 32]) -> IdentityKey {
        IdentityKey::with_keygen(NodeKey::from_bytes(seed), &Self)
    }
}

impl SlhDsaKeygen for TableKeygen {
    /// # Panics
    ///
    /// If the table has no key of these seeds, with the command that adds
    /// it.
    #[allow(clippy::panic)] // A test's seed missing from the table fails the test.
    fn public_key(
        &self,
        sk_seed: &[u8; SLH_DSA_SEED_BYTES],
        sk_prf: &[u8; SLH_DSA_SEED_BYTES],
        pk_seed: &[u8; SLH_DSA_SEED_BYTES],
    ) -> SlhDsaPublicKey {
        match entries().find(|entry| {
            entry.sk_seed == *sk_seed && entry.sk_prf == *sk_prf && entry.pk_seed == *pk_seed
        }) {
            Some(entry) => entry.public,
            None => panic!(
                "no precomputed SLH-DSA key for this seed (dassie_identity::TableKeygen): add it \
                 with `{ADD} {:?}{:?}{:?}`",
                Hex(sk_seed),
                Hex(sk_prf),
                Hex(pk_seed),
            ),
        }
    }
}

/// A table entry: the three seeds, and the public key they generate.
struct Entry {
    sk_seed: [u8; SLH_DSA_SEED_BYTES],
    sk_prf: [u8; SLH_DSA_SEED_BYTES],
    pk_seed: [u8; SLH_DSA_SEED_BYTES],
    public: SlhDsaPublicKey,
}

/// The table's entries.
///
/// # Panics
///
/// On a line that is not an entry: the table is the crate's own.
#[allow(clippy::panic)] // A malformed table fails every test that reads it.
fn entries() -> impl Iterator<Item = Entry> {
    TABLE
        .lines()
        .map(|line| line.split('#').next().unwrap_or("").trim())
        .filter(|line| !line.is_empty())
        .map(|line| {
            entry(line).unwrap_or_else(|| panic!("not an entry of the SLH-DSA test keys: {line}"))
        })
}

/// The entry of a line of the table.
fn entry(line: &str) -> Option<Entry> {
    let mut fields = line.split_ascii_whitespace();
    let entry = Entry {
        sk_seed: hex(fields.next()?)?,
        sk_prf: hex(fields.next()?)?,
        pk_seed: hex(fields.next()?)?,
        public: SlhDsaPublicKey::from_bytes(hex::<SLH_DSA_PUBLIC_KEY_BYTES>(fields.next()?)?),
    };
    fields.next().is_none().then_some(entry)
}

/// The `N` bytes of `text`, in lowercase hex.
fn hex<const N: usize>(text: &str) -> Option<[u8; N]> {
    let (pairs, rest) = text.as_bytes().as_chunks::<2>();
    if pairs.len() != N || !rest.is_empty() {
        return None;
    }
    let mut bytes = [0; N];
    for (byte, [high, low]) in bytes.iter_mut().zip(pairs) {
        *byte = (digit(*high)? << 4) | digit(*low)?;
    }
    Some(bytes)
}

/// A lowercase hex digit's value.
const fn digit(character: u8) -> Option<u8> {
    match character {
        b'0'..=b'9' => Some(character - b'0'),
        b'a'..=b'f' => Some(character - b'a' + 10),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

    extern crate std;

    use std::vec::Vec;

    use super::*;
    use crate::slh_dsa::Fips205Keygen;

    /// The first, the middle and the last entry are what the real key
    /// generation gives for their seeds. A change to `slh-dsa` that moved
    /// key generation would move every key, which the known-answer and
    /// identity vectors catch; these catch a table written other than by
    /// the generator, which makes each entry with the real key generation.
    #[test]
    fn entries_are_the_real_key_generation() {
        let entries: Vec<Entry> = entries().collect();
        let last = entries.len() - 1;
        for entry in [&entries[0], &entries[last / 2], &entries[last]] {
            let real = Fips205Keygen.public_key(&entry.sk_seed, &entry.sk_prf, &entry.pk_seed);
            assert_eq!(
                real,
                entry.public,
                "the table's key of {:?}{:?}{:?} is not the real one: run `{ADD}`",
                Hex(&entry.sk_seed),
                Hex(&entry.sk_prf),
                Hex(&entry.pk_seed),
            );
        }
    }

    /// The table names each seed once, in order, as the generator writes
    /// it, so that a merge shows what it added.
    #[test]
    fn the_table_is_sorted_and_unique() {
        let seeds: Vec<_> = entries()
            .map(|entry| (entry.sk_seed, entry.sk_prf, entry.pk_seed))
            .collect();
        assert!(seeds.windows(2).all(|pair| pair[0] < pair[1]));
    }

    #[test]
    fn a_seed_in_the_table_gives_its_key() {
        let entry = entries().next().unwrap();
        assert_eq!(
            TableKeygen.public_key(&entry.sk_seed, &entry.sk_prf, &entry.pk_seed),
            entry.public
        );
        // A test's node key, with its identity from the table.
        let key = TableKeygen::identity_key(&[1; 32]);
        assert_eq!(
            key.identity(),
            NodeKey::from_bytes(&[1; 32]).identity(&TableKeygen)
        );
    }

    #[test]
    #[should_panic(expected = "no precomputed SLH-DSA key for this seed")]
    fn another_seed_panics() {
        let _ = TableKeygen.public_key(&[0xff; 16], &[0xff; 16], &[0xff; 16]);
    }

    #[test]
    fn lines_are_read_strictly() {
        let seed = "00".repeat(16);
        let key = "11".repeat(32);
        assert!(entry(&std::format!("{seed} {seed} {seed} {key}")).is_some());
        assert!(entry(&std::format!("{seed} {seed} {seed}")).is_none());
        assert!(entry(&std::format!("{seed} {seed} {seed} {key} {key}")).is_none());
        assert!(entry(&std::format!("{seed} {seed} {seed} {}", "AA".repeat(32))).is_none());
        assert!(entry(&std::format!("{seed} {seed} 0 {key}")).is_none());
    }
}
