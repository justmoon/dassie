//! Dassie node identity: everything a node is, derived from one BIP-39
//! phrase.
//!
//! ```text
//! phrase ──PBKDF2──▶ master seed ──HMAC "node"──▶ node key (Ed25519 seed)
//!                        │                          ├─ public key ─▶ node ID
//!                        └─ dev/session             ├─ node/login (login proof)
//!                                                   ├─ node/settlement-module/…
//!                                                   └─ X25519 secret
//! ```
//!
//! The byte-level derivations are the compatibility contract with the
//! original implementation (spec/areas/identity.md#derivation-chain): the
//! same phrase must give the same node ID and the same ledger accounts. The
//! API around them is new and fixes the original's flaws: phrases are
//! validated, seed paths are bounded and reject empty segments, the
//! login proof is compared in constant time, the public-key DER has its
//! unused-bits byte, and non-contributory X25519 results are rejected.
//!
//! The crate is deterministic (spec/decisions/tigerstyle.md): it never reads
//! a clock or the OS RNG, and randomness comes from an RNG the caller passes
//! in. It does not allocate. Secrets are wiped on drop, are not `Copy`, and
//! print as `<redacted>`.

#![no_std]

#[cfg(test)]
extern crate std;

mod error;
mod label;
mod mnemonic;
mod node_id;
mod node_key;
mod secret;
mod seed;
mod wordlist;
mod x25519;

pub use error::Error;
pub use label::{NODE_LABEL_LENGTH, NodeLabel};
pub use mnemonic::{MAX_PHRASE_INPUT_BYTES, MAX_PHRASE_LENGTH, Mnemonic, Phrase, WordCount, Words};
pub use node_id::{HASH_INPUT_LENGTH, MAX_NODE_ID_LENGTH, NodeId};
pub use node_key::{
    NodeKey, PKCS8_DER_LENGTH, PrivateKeyDer, PublicKey, SPKI_DER_LENGTH, Signature,
};
pub use seed::{
    DEV_SESSION_PATH, DerivedSecret, LOGIN_PATH, LoginSecret, MAX_SEED_PATH_BYTES,
    MAX_SEED_PATH_SEGMENTS, MasterSeed, NODE_PATH, SeedPath,
};
pub use x25519::{SharedSecret, X25519Public, X25519Secret};
