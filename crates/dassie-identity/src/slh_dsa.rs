//! The node's SLH-DSA key (FIPS 205, SLH-DSA-SHA2-128s), and the node's
//! identity: the two public keys its node ID commits to
//! (spec/decisions/post-quantum-node-id.md).
//!
//! Nothing signs with the SLH-DSA key yet. It is in the node ID so that,
//! once Ed25519 can no longer be trusted, a node can prove that it is the
//! node of its ID with a signature that a quantum computer cannot forge,
//! under the same ID. Its seeds derive from the node key at frozen paths
//! below `node`, so the node, which holds only its node key, has it too.

use core::fmt;

use dassie_oer::{Decode, DecodeError, DecodeErrorKind, Encode, Reader, Writer};
use slh_dsa::{Sha2_128s, SigningKey};

use crate::node_id::NodeId;
use crate::node_key::{NodeKey, PublicKey};
use crate::paths::{NodeSecret, SlhDsaSeed};
use crate::secret::Hex;
use crate::seed::DerivedSecret;

/// Bytes of an SLH-DSA-SHA2-128s public key: `PK.seed` and `PK.root`.
pub const SLH_DSA_PUBLIC_KEY_BYTES: usize = 32;

/// Bytes of each of the three seeds key generation takes: the parameter
/// set's `n`.
pub const SLH_DSA_SEED_BYTES: usize = SLH_DSA_PUBLIC_KEY_BYTES / 2;

/// An SLH-DSA-SHA2-128s public key: `PK.seed` followed by `PK.root`.
///
/// Every 32 bytes are a public key: FIPS 205 has no invalid one, so there
/// is nothing to check when one is received. Whether it is a node's is
/// what its node ID says.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SlhDsaPublicKey([u8; SLH_DSA_PUBLIC_KEY_BYTES]);

impl SlhDsaPublicKey {
    /// The key of these bytes.
    #[must_use]
    pub const fn from_bytes(bytes: [u8; SLH_DSA_PUBLIC_KEY_BYTES]) -> Self {
        Self(bytes)
    }

    /// The key's bytes, which the node ID is computed over.
    #[must_use]
    pub const fn as_bytes(&self) -> &[u8; SLH_DSA_PUBLIC_KEY_BYTES] {
        &self.0
    }

    /// The public key of the key FIPS 205's `slh_keygen_internal` generates
    /// from these seeds. A node's seeds are its node key's
    /// ([`NodeKey::slh_dsa_public_key`]); taking any is what lets the
    /// standard's known-answer vectors pin the generation.
    ///
    /// This is the real key generation, which [`Fips205Keygen`] runs and
    /// nothing can replace: the known-answer vectors call it directly.
    #[must_use]
    pub fn from_seeds(
        sk_seed: &[u8; SLH_DSA_SEED_BYTES],
        sk_prf: &[u8; SLH_DSA_SEED_BYTES],
        pk_seed: &[u8; SLH_DSA_SEED_BYTES],
    ) -> Self {
        let key = SigningKey::<Sha2_128s>::slh_keygen_internal(sk_seed, sk_prf, pk_seed);
        let public: &slh_dsa::VerifyingKey<Sha2_128s> = key.as_ref();
        Self(public.to_bytes().into())
    }
}

impl fmt::Debug for SlhDsaPublicKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("SlhDsaPublicKey")
            .field(&Hex(&self.0))
            .finish()
    }
}

/// On the wire, a key is its 32 bytes, `OCTET STRING (SIZE (32))`, which
/// OER writes without a length.
impl Encode for SlhDsaPublicKey {
    fn encoded_len(&self) -> usize {
        SLH_DSA_PUBLIC_KEY_BYTES
    }

    fn encode_to(&self, writer: &mut Writer<'_>) {
        writer.write_bytes(&self.0);
    }
}

impl Decode<'_> for SlhDsaPublicKey {
    type Error = DecodeError;

    fn decode(reader: &mut Reader<'_>) -> Result<Self, DecodeError> {
        let offset = reader.offset();
        let bytes = reader
            .read_array::<SLH_DSA_PUBLIC_KEY_BYTES>()
            .map_err(|_| DecodeError::new(offset, DecodeErrorKind::UnexpectedEnd))?;
        Ok(Self(*bytes))
    }
}

/// How a node key's SLH-DSA public key is made from its three seeds.
///
/// A node runs [`Fips205Keygen`], the real key generation, and nothing but
/// a test passes another: [`TableKeygen`](crate::TableKeygen) looks the
/// keys of the tests' seeds up rather than generate them
/// (spec/areas/testing.md#slh-dsa-keys-in-tests). Whatever derives a
/// node's SLH-DSA key from its node key takes one, so that a test can
/// pass it its own.
pub trait SlhDsaKeygen: fmt::Debug + Send + Sync {
    /// The public key of the key FIPS 205's `slh_keygen_internal` makes
    /// from these seeds.
    fn public_key(
        &self,
        sk_seed: &[u8; SLH_DSA_SEED_BYTES],
        sk_prf: &[u8; SLH_DSA_SEED_BYTES],
        pk_seed: &[u8; SLH_DSA_SEED_BYTES],
    ) -> SlhDsaPublicKey;
}

/// FIPS 205's `slh_keygen_internal`, as [`SlhDsaPublicKey::from_seeds`]
/// runs it: the key generation every node uses.
#[derive(Clone, Copy, Debug, Default)]
pub struct Fips205Keygen;

impl SlhDsaKeygen for Fips205Keygen {
    fn public_key(
        &self,
        sk_seed: &[u8; SLH_DSA_SEED_BYTES],
        sk_prf: &[u8; SLH_DSA_SEED_BYTES],
        pk_seed: &[u8; SLH_DSA_SEED_BYTES],
    ) -> SlhDsaPublicKey {
        SlhDsaPublicKey::from_seeds(sk_seed, sk_prf, pk_seed)
    }
}

impl NodeKey {
    /// The public key of the node's SLH-DSA key, made by `keygen` from the
    /// seeds at `node/slh-dsa-sha2-128s/…`: by a node, [`Fips205Keygen`].
    ///
    /// The real key generation builds the top tree of the key's hypertree,
    /// thousands of times the work of an Ed25519 key: about 15 ms of
    /// optimized code, so a caller computes it once, as an
    /// [`IdentityKey`], and keeps it. The seeds are wiped on drop.
    #[must_use]
    pub fn slh_dsa_public_key(&self, keygen: &dyn SlhDsaKeygen) -> SlhDsaPublicKey {
        let sk_seed = self.secret(NodeSecret::SlhDsa(SlhDsaSeed::SkSeed));
        let sk_prf = self.secret(NodeSecret::SlhDsa(SlhDsaSeed::SkPrf));
        let pk_seed = self.secret(NodeSecret::SlhDsa(SlhDsaSeed::PkSeed));
        keygen.public_key(seed_of(&sk_seed), seed_of(&sk_prf), seed_of(&pk_seed))
    }

    /// The node's identity: its public key, and the public key of its
    /// SLH-DSA key, made by `keygen`. A node keeps its own as an
    /// [`IdentityKey`]; there is no `node_id` here, so that no caller pays
    /// for a key generation each time it asks.
    #[must_use]
    pub fn identity(&self, keygen: &dyn SlhDsaKeygen) -> NodeIdentity {
        NodeIdentity::new(self.public_key(), self.slh_dsa_public_key(keygen))
    }
}

/// A node key with the identity it derives, computed once: what a node
/// keeps of itself. It is the node key in every other respect, which it
/// dereferences to.
pub struct IdentityKey {
    key: NodeKey,
    identity: NodeIdentity,
}

impl IdentityKey {
    /// `key` with its identity, by the real key generation
    /// ([`Fips205Keygen`]): what a node keeps of itself.
    #[must_use]
    pub fn new(key: NodeKey) -> Self {
        Self::with_keygen(key, &Fips205Keygen)
    }

    /// `key` with its identity, its SLH-DSA key made by `keygen`: a node's
    /// is [`Fips205Keygen`], and a test's may be another.
    #[must_use]
    pub fn with_keygen(key: NodeKey, keygen: &dyn SlhDsaKeygen) -> Self {
        let identity = key.identity(keygen);
        Self { key, identity }
    }

    /// The node's identity.
    #[must_use]
    pub const fn identity(&self) -> NodeIdentity {
        self.identity
    }

    /// The node's ID.
    #[must_use]
    pub const fn node_id(&self) -> NodeId {
        self.identity.node_id()
    }

    /// The SLH-DSA public key, as computed once. This shadows
    /// [`NodeKey::slh_dsa_public_key`], which the dereference would reach
    /// otherwise and which makes the key again; [`identity`](Self::identity)
    /// shadows [`NodeKey::identity`] the same way.
    #[must_use]
    pub const fn slh_dsa_public_key(&self) -> SlhDsaPublicKey {
        self.identity.slh_dsa()
    }

    /// The node key.
    #[must_use]
    pub const fn key(&self) -> &NodeKey {
        &self.key
    }
}

impl core::ops::Deref for IdentityKey {
    type Target = NodeKey;

    fn deref(&self) -> &NodeKey {
        &self.key
    }
}

impl fmt::Debug for IdentityKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("IdentityKey")
            .field("identity", &self.identity)
            .finish_non_exhaustive()
    }
}

/// A seed: the first [`SLH_DSA_SEED_BYTES`] of a secret derived at its path.
fn seed_of(secret: &DerivedSecret) -> &[u8; SLH_DSA_SEED_BYTES] {
    let (seed, _) = secret
        .as_bytes()
        .split_first_chunk()
        .unwrap_or((&[0; SLH_DSA_SEED_BYTES], &[]));
    seed
}

/// A node's identity: its Ed25519 public key, which signs everything a node
/// signs today, and the public key of its SLH-DSA key, which signs nothing
/// yet. The node ID commits to both, so only the two together name a node,
/// and wherever a node ID is checked against a key, the SLH-DSA key travels
/// beside it (spec/areas/identity.md#node-id).
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct NodeIdentity {
    key: PublicKey,
    slh_dsa: SlhDsaPublicKey,
    /// The ID of the two, computed once.
    id: NodeId,
}

impl NodeIdentity {
    /// The identity of these two keys.
    #[must_use]
    pub fn new(key: PublicKey, slh_dsa: SlhDsaPublicKey) -> Self {
        let id = NodeId::from_keys(key.as_bytes(), slh_dsa.as_bytes());
        Self { key, slh_dsa, id }
    }

    /// The Ed25519 public key, which the node signs with.
    #[must_use]
    pub const fn key(&self) -> PublicKey {
        self.key
    }

    /// The SLH-DSA public key.
    #[must_use]
    pub const fn slh_dsa(&self) -> SlhDsaPublicKey {
        self.slh_dsa
    }

    /// The node ID the two keys hash to.
    #[must_use]
    pub const fn node_id(&self) -> NodeId {
        self.id
    }
}

impl fmt::Debug for NodeIdentity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("NodeIdentity")
            .field("node_id", &self.id.as_str())
            .field("key", &Hex(self.key.as_bytes()))
            .field("slh_dsa", &Hex(self.slh_dsa.as_bytes()))
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_identity_is_the_two_keys() {
        let keygen = crate::TableKeygen;
        let key = NodeKey::from_bytes(&[1; 32]);
        let identity = key.identity(&keygen);
        assert_eq!(identity.key(), key.public_key());
        assert_eq!(identity.slh_dsa(), key.slh_dsa_public_key(&keygen));
        assert_eq!(
            identity.node_id(),
            NodeId::from_keys(key.public_key().as_bytes(), identity.slh_dsa().as_bytes())
        );
        // A held key answers with the identity it computed once, through
        // the methods that would otherwise reach the node key's.
        let held = IdentityKey::with_keygen(NodeKey::from_bytes(&[1; 32]), &keygen);
        assert_eq!(held.identity(), identity);
        assert_eq!(held.slh_dsa_public_key(), identity.slh_dsa());
        assert_eq!(held.node_id(), identity.node_id());
        // Another SLH-DSA key, another node.
        let other = NodeIdentity::new(key.public_key(), SlhDsaPublicKey::from_bytes([0; 32]));
        assert_ne!(other.node_id(), identity.node_id());
    }

    #[test]
    fn the_wire_carries_the_bytes() {
        let key = SlhDsaPublicKey::from_bytes([7; 32]);
        let mut buffer = [0; 33];
        assert_eq!(key.encode(&mut buffer), 32);
        assert_eq!(SlhDsaPublicKey::decode(&mut Reader::new(&buffer)), Ok(key));
        assert!(SlhDsaPublicKey::decode(&mut Reader::new(&[0; 31])).is_err());
    }
}
