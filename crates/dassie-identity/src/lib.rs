//! Dassie node identity: everything a node is, derived from one recovery
//! phrase.
//!
//! ```text
//! phrase (kind, created, secret, check words)
//!   └─ secret ──HKDF, realm──▶ master seed ──HMAC "node"──▶ node key (Ed25519 seed)
//!                                                          ├─ public key ──────────────┐
//!                                                          ├─ node/slh-dsa-sha2-128s/… │
//!                                                          │    └─ SLH-DSA public key ─┴─▶ node ID
//!                                                          └─ node/… (one NodeSecret each:
//!                                                              login, ledger accounts, hubs, …)
//! ```
//!
//! A phrase is 20 words that say its realm and when it was made, and
//! correct their own errors (spec/decisions/phrase-format.md); a correction
//! is never taken silently. The byte-level derivations are frozen
//! (spec/areas/identity.md#derivation-chain): the same phrase must give the
//! same node ID and the same ledger accounts. The API around them fixes the
//! original's flaws: every path below the node key is a variant of
//! [`NodeSecret`], so no caller writes a path of its own, the login proof is
//! compared in constant time, and the public-key DER has its unused-bits
//! byte.
//!
//! The crate is deterministic (spec/decisions/tigerstyle.md): it never reads
//! a clock or the OS RNG: randomness comes from an RNG the caller passes
//! in, and the time a phrase is made from the caller's `now_ms`. It does
//! not allocate. Secrets are wiped on drop, are not `Copy`, and
//! print as `<redacted>`.

#![no_std]

#[cfg(test)]
extern crate std;

mod error;
mod gf;
mod node_id;
mod node_key;
mod paths;
mod phrase;
mod reed_solomon;
mod secret;
mod seed;
mod slh_dsa;
mod wordlist;

pub use error::Error;
pub use node_id::{HASH_INPUT_LENGTH, NODE_ID_BYTES, NODE_ID_LENGTH, NodeId};
pub use node_key::{
    NodeKey, PKCS8_DER_LENGTH, PrivateKeyDer, PublicKey, SPKI_DER_LENGTH, Signature,
};
pub use paths::{HubKey, MAX_SEGMENT_BYTES, NodeSecret, QuicEndpoint, Segment, SlhDsaSeed};
pub use phrase::{
    Correction, Created, MAX_PHRASE_INPUT_BYTES, MAX_PHRASE_LENGTH, PHRASE_WORDS, Phrase,
    PhraseSecret, PhraseText, Positions, Reading, Realm, Words, word_named,
};
pub use seed::{DerivedSecret, LoginSecret, MasterSeed};
pub use slh_dsa::{
    IdentityKey, NodeIdentity, SLH_DSA_PUBLIC_KEY_BYTES, SLH_DSA_SEED_BYTES, SlhDsaPublicKey,
};
