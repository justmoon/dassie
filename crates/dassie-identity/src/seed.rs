//! The master seed and the HMAC-SHA256 seed-path tree hanging off it.
//!
//! Every secret a node has is a node of this tree
//! (spec/areas/identity.md#seed-paths): the key of a path `a/b/c` is
//! `HMAC(HMAC(HMAC(seed, "a"), "b"), "c")`, with the previous key as the
//! HMAC key and the segment as the message. The node key is the path
//! `node`, and every secret below it is one of [`crate::NodeSecret`]. The
//! derivation is frozen (spec/areas/identity.md#derivation-chain): the same
//! phrase must recover the same node and the same ledger accounts.

use hmac::{Hmac, KeyInit, Mac};
use sha2::Sha256;
use subtle::ConstantTimeEq;

use crate::node_key::NodeKey;
use crate::secret::secret_bytes;

/// The path of the node key.
const NODE_SEGMENT: &str = "node";

/// The path of the deterministic development session token. It is a sibling
/// of `node`, so only the holder of the master seed can derive it.
pub const DEV_SESSION_PATH: &str = "dev/session";

secret_bytes!(
    /// The 64-byte BIP-39 seed: the root of every secret of a node.
    ///
    /// Only the browser holds it during setup and login. The node itself
    /// keeps only the [`NodeKey`] derived from it.
    MasterSeed,
    64
);

secret_bytes!(
    /// A 32-byte secret derived at some seed path.
    DerivedSecret,
    32
);

secret_bytes!(
    /// The login proof secret at `node/login`.
    ///
    /// The browser derives it from the phrase and sends it as proof, and the
    /// node derives it from its key and compares. The comparison is
    /// [`verify_proof`](Self::verify_proof), which runs in constant time.
    LoginSecret,
    32
);

impl MasterSeed {
    /// The node key at `node`.
    #[must_use]
    pub fn node_key(&self) -> NodeKey {
        let seed = hmac_sha256(&self.0, NODE_SEGMENT.as_bytes());
        NodeKey::from_bytes(seed.as_bytes())
    }

    /// The development session token at `dev/session`.
    #[must_use]
    pub fn dev_session_token(&self) -> DerivedSecret {
        fold(&self.0, DEV_SESSION_PATH.split('/'))
    }
}

impl LoginSecret {
    /// Whether `candidate` equals this secret, compared in constant time.
    ///
    /// A comparison that stops at the first differing byte would tell an
    /// attacker how much of a guess was right. A candidate of the wrong
    /// length is rejected straight away; the length is public.
    #[must_use]
    pub fn verify_proof(&self, candidate: &[u8]) -> bool {
        self.0.as_slice().ct_eq(candidate).into()
    }
}

/// HMAC-SHA256 of `message` under `key`.
// HMAC takes keys of any length, so this cannot panic.
#[allow(clippy::panic)]
fn hmac_sha256(key: &[u8], message: &[u8]) -> DerivedSecret {
    let Ok(mut mac) = Hmac::<Sha256>::new_from_slice(key) else {
        panic!("HMAC accepts keys of any length");
    };
    mac.update(message);
    DerivedSecret(mac.finalize().into_bytes().into())
}

/// Folds HMAC-SHA256 over `segments`, starting from `key`.
///
/// # Panics
///
/// If there are no segments; every path has at least one.
// Panics only on a broken invariant: every path has a segment.
#[allow(clippy::panic)]
pub(crate) fn fold<'s>(key: &[u8], segments: impl Iterator<Item = &'s str>) -> DerivedSecret {
    let mut segments = segments;
    let Some(first) = segments.next() else {
        panic!("a seed path has at least one segment");
    };
    let mut current = hmac_sha256(key, first.as_bytes());
    for segment in segments {
        // Assigning drops, and so wipes, the previous key.
        current = hmac_sha256(&current.0, segment.as_bytes());
    }
    current
}

#[cfg(test)]
mod tests {
    use super::*;

    // The browser derives below `node` from the master seed, and the node
    // from its key: the same secret.
    #[test]
    fn the_node_key_continues_the_master_seeds_tree() {
        let seed = MasterSeed::from_bytes([0x5a; 64]);
        let from_seed = fold(seed.as_bytes(), ["node", "login"].into_iter());
        let login = seed.node_key().login_secret();
        assert_eq!(from_seed.as_bytes(), login.as_bytes());
    }

    #[test]
    fn debug_output_is_redacted() {
        let seed = MasterSeed::from_bytes([0x5a; 64]);
        assert_eq!(std::format!("{seed:?}"), "MasterSeed(<redacted>)");
        let login = NodeKey::from_bytes(&[7; 32]).login_secret();
        assert_eq!(std::format!("{login:?}"), "LoginSecret(<redacted>)");
    }
}
