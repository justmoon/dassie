//! The master seed and the HMAC-SHA256 seed-path tree hanging off it.
//!
//! Every secret a node has is a node of this tree (spec 3 §2.1, spec 5
//! §4.5): the key of a path `a/b/c` is `HMAC(HMAC(HMAC(seed, "a"), "b"),
//! "c")`, with the previous key as the HMAC key and the segment as the
//! message. The derivation is the compatibility contract with the original
//! (spec 8 §2): the same phrase must recover the same node and the same
//! ledger accounts, so it must not change.

use dassie_oer::{Alphabet, Size};
use hmac::{Hmac, KeyInit, Mac};
use sha2::Sha256;
use subtle::ConstantTimeEq;

use crate::error::Error;
use crate::node_key::NodeKey;
use crate::secret::secret_bytes;

/// Longest seed path accepted, in bytes. Real paths are a few dozen bytes;
/// the bound keeps derivation cost bounded for paths that arrive from
/// settlement modules.
pub const MAX_SEED_PATH_BYTES: usize = 256;

/// Most segments a seed path may have.
pub const MAX_SEED_PATH_SEGMENTS: usize = 16;

/// The path of the node key.
pub const NODE_PATH: &str = "node";

/// The path of the login proof secret, under the node key.
pub const LOGIN_PATH: &str = "node/login";

/// The path of the deterministic development session token. It is a sibling
/// of `node`, so only the holder of the master seed can derive it.
pub const DEV_SESSION_PATH: &str = "dev/session";

/// The first segment under `node/` of every settlement-module path.
const SETTLEMENT_MODULE_SEGMENT: &str = "settlement-module";

/// Characters of a path segment: printable ASCII without the space, which
/// would only ever be a typo in a path. The separator `/` is in the range but
/// never reaches a segment, since segments are split on it.
const SEGMENT_ALPHABET: Alphabet = Alphabet::EMPTY.with_range(0x21, 0x7e);

/// The peer wire schema of a settlement scheme ID (spec 5 §3).
const SCHEME_ID_SIZE: Size = Size::range(2, 128);
const SCHEME_ID_ALPHABET: Alphabet = Alphabet::EMPTY
    .with_range(b'a', b'z')
    .with_range(b'0', b'9')
    .with_chars(b"-");

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
        let seed = hmac_sha256(&self.0, NODE_PATH.as_bytes());
        NodeKey::from_bytes(seed.as_bytes())
    }

    /// The secret at `path`, folding every segment from the master seed.
    #[must_use]
    pub fn derive(&self, path: &SeedPath<'_>) -> DerivedSecret {
        fold(&self.0, path.segments())
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
    /// The original compared hex strings with `!==`, which exits at the first
    /// differing character (spec 2 §8.3). A candidate of the wrong length is
    /// rejected straight away; the length is public.
    #[must_use]
    pub fn verify_proof(&self, candidate: &[u8]) -> bool {
        self.0.as_slice().ct_eq(candidate).into()
    }
}

/// A validated seed path: one to [`MAX_SEED_PATH_SEGMENTS`] non-empty
/// segments of printable ASCII separated by `/`.
///
/// The original accepted any string, so `node//login` or a trailing slash
/// silently derived an HMAC over an empty segment.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SeedPath<'a> {
    text: &'a str,
}

impl<'a> SeedPath<'a> {
    /// Validates `text` as a seed path.
    pub fn parse(text: &'a str) -> Result<Self, Error> {
        if text.is_empty() || text.len() > MAX_SEED_PATH_BYTES {
            return Err(Error::InvalidSeedPath);
        }
        let mut segments = 0;
        for segment in text.split('/') {
            segments += 1;
            let valid = !segment.is_empty() && SEGMENT_ALPHABET.contains_all(segment.as_bytes());
            if !valid || segments > MAX_SEED_PATH_SEGMENTS {
                return Err(Error::InvalidSeedPath);
            }
        }
        Ok(Self { text })
    }

    /// The path as text.
    #[must_use]
    pub const fn as_str(&self) -> &'a str {
        self.text
    }

    /// The segments in derivation order.
    pub fn segments(&self) -> impl Iterator<Item = &'a str> + use<'a> {
        self.text.split('/')
    }
}

impl NodeKey {
    /// The secret at `path`, derived from the node key the way the node
    /// derives it (spec 5 §4.5).
    ///
    /// The node key sits at `node`, so `node` returns the key itself and
    /// `node/a/b` folds `a` and `b` from it. Every other path hangs off the
    /// master seed, which the node does not have, and is an error.
    pub fn derive(&self, path: &SeedPath<'_>) -> Result<DerivedSecret, Error> {
        let mut segments = path.segments();
        if segments.next() != Some(NODE_PATH) {
            return Err(Error::NotUnderNode);
        }
        let mut rest = segments.peekable();
        if rest.peek().is_none() {
            return Ok(DerivedSecret(*self.as_bytes()));
        }
        Ok(fold(self.as_bytes(), rest))
    }

    /// The login proof secret at `node/login`.
    #[must_use]
    pub fn login_secret(&self) -> LoginSecret {
        let secret = fold(self.as_bytes(), LOGIN_PATH.split('/').skip(1));
        LoginSecret(secret.0)
    }

    /// The entropy a settlement module gets for `path`, at
    /// `node/settlement-module/<scheme>/<path>`.
    ///
    /// Settlement modules build wallets from it, so this is what keeps the
    /// same phrase pointing at the same funds. `scheme` must be a valid
    /// settlement scheme ID and `path` a non-empty seed path.
    pub fn settlement_entropy(&self, scheme: &str, path: &str) -> Result<DerivedSecret, Error> {
        let scheme_valid = SCHEME_ID_SIZE.contains(scheme.len())
            && SCHEME_ID_ALPHABET.contains_all(scheme.as_bytes());
        if !scheme_valid {
            return Err(Error::InvalidSchemeId);
        }
        let path = SeedPath::parse(path)?;
        let segments = [SETTLEMENT_MODULE_SEGMENT, scheme]
            .into_iter()
            .chain(path.segments());
        Ok(fold(self.as_bytes(), segments))
    }
}

/// HMAC-SHA256 of `message` under `key`.
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
/// If there are no segments; every caller passes a validated path, which has
/// at least one.
fn fold<'s>(key: &[u8], segments: impl Iterator<Item = &'s str>) -> DerivedSecret {
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

    #[test]
    fn seed_paths_are_bounded() {
        let mut long = [b'a'; MAX_SEED_PATH_BYTES + 1];
        let Ok(text) = core::str::from_utf8(&long) else {
            unreachable!()
        };
        assert_eq!(SeedPath::parse(text), Err(Error::InvalidSeedPath));
        if let Some(byte) = long.get_mut(MAX_SEED_PATH_BYTES) {
            *byte = b'/';
        }
        let Ok(text) = core::str::from_utf8(&long) else {
            unreachable!()
        };
        assert_eq!(SeedPath::parse(text), Err(Error::InvalidSeedPath));

        let most = "a/a/a/a/a/a/a/a/a/a/a/a/a/a/a/a";
        assert!(SeedPath::parse(most).is_ok());
        assert_eq!(
            SeedPath::parse("a/a/a/a/a/a/a/a/a/a/a/a/a/a/a/a/a"),
            Err(Error::InvalidSeedPath)
        );
    }

    #[test]
    fn seed_paths_reject_spaces_and_control_characters() {
        for text in ["node/a b", "node/\tx", "node/\u{7f}", "node/é"] {
            assert_eq!(
                SeedPath::parse(text),
                Err(Error::InvalidSeedPath),
                "{text:?}"
            );
        }
    }

    #[test]
    fn node_key_paths_must_start_at_node() {
        let key = NodeKey::from_bytes(&[7; 32]);
        for text in ["dev/session", "nodes/login", "login"] {
            let Ok(path) = SeedPath::parse(text) else {
                unreachable!()
            };
            assert_eq!(
                key.derive(&path).map(|_| ()),
                Err(Error::NotUnderNode),
                "{text}"
            );
        }
        let Ok(node) = SeedPath::parse("node") else {
            unreachable!()
        };
        assert!(matches!(key.derive(&node), Ok(secret) if secret.as_bytes() == &[7; 32]));
    }

    #[test]
    fn settlement_scheme_ids_follow_the_wire_schema() {
        let key = NodeKey::from_bytes(&[7; 32]);
        for scheme in ["x", "XRPL", "xrpl_testnet", "xrpl/testnet", ""] {
            assert_eq!(
                key.settlement_entropy(scheme, "seed").map(|_| ()),
                Err(Error::InvalidSchemeId),
                "{scheme}"
            );
        }
        assert_eq!(
            key.settlement_entropy("xrpl", "").map(|_| ()),
            Err(Error::InvalidSeedPath)
        );
        assert!(key.settlement_entropy("xrpl", "a/b").is_ok());
    }

    #[test]
    fn debug_output_is_redacted() {
        let seed = MasterSeed::from_bytes([0x5a; 64]);
        assert_eq!(std::format!("{seed:?}"), "MasterSeed(<redacted>)");
        let login = NodeKey::from_bytes(&[7; 32]).login_secret();
        assert_eq!(std::format!("{login:?}"), "LoginSecret(<redacted>)");
    }
}
