//! The node's Ed25519 key, its public key, signatures and their DER forms.

use core::fmt;

use ed25519_dalek::{Signer, SigningKey, Verifier, VerifyingKey};
use sha2::{Digest, Sha512};
use zeroize::Zeroize;

use crate::error::Error;
use crate::node_id::NodeId;
use crate::secret::{Hex, secret_bytes};
use crate::x25519::{X25519Public, X25519Secret};

/// PKCS#8 `PrivateKeyInfo` for an Ed25519 seed (RFC 8410 §7), up to the
/// seed: version 0, algorithm 1.3.101.112, and an OCTET STRING holding an
/// OCTET STRING of 32 bytes.
const PKCS8_PREFIX: [u8; 16] = [
    0x30, 0x2e, 0x02, 0x01, 0x00, 0x30, 0x05, 0x06, 0x03, 0x2b, 0x65, 0x70, 0x04, 0x22, 0x04, 0x20,
];

/// `SubjectPublicKeyInfo` for an Ed25519 public key (RFC 8410 §4), up to the
/// key: algorithm 1.3.101.112 and a BIT STRING of 33 bytes, the first being
/// the count of unused bits (zero). The original's constant left that byte
/// out (spec 3 §2.1), so its 44-byte keys started with a stray `00`.
const SPKI_PREFIX: [u8; 12] = [
    0x30, 0x2a, 0x30, 0x05, 0x06, 0x03, 0x2b, 0x65, 0x70, 0x03, 0x21, 0x00,
];

/// Length of the PKCS#8 DER of a node key.
pub const PKCS8_DER_LENGTH: usize = PKCS8_PREFIX.len() + 32;

/// Length of the SPKI DER of a public key.
pub const SPKI_DER_LENGTH: usize = SPKI_PREFIX.len() + 32;

secret_bytes!(
    /// The PKCS#8 DER encoding of a node key, which contains the seed.
    PrivateKeyDer,
    PKCS8_DER_LENGTH
);

/// The node's Ed25519 key: the 32-byte seed at `node` and the public key
/// computed from it once.
///
/// Signing is pure Ed25519 (RFC 8032, no prehash, no context), as in the
/// original (spec 3 §2.1).
pub struct NodeKey {
    signing: SigningKey,
}

impl NodeKey {
    /// The key with Ed25519 seed `seed`.
    #[must_use]
    pub fn from_bytes(seed: &[u8; 32]) -> Self {
        Self {
            signing: SigningKey::from_bytes(seed),
        }
    }

    /// The Ed25519 seed (the "raw private key" of spec 3 §2.1).
    #[must_use]
    pub fn as_bytes(&self) -> &[u8; 32] {
        self.signing.as_bytes()
    }

    /// The public key.
    #[must_use]
    pub fn public_key(&self) -> PublicKey {
        PublicKey {
            key: self.signing.verifying_key(),
        }
    }

    /// The node ID of the public key.
    #[must_use]
    pub fn node_id(&self) -> NodeId {
        NodeId::from_public_key(self.signing.verifying_key().as_bytes())
    }

    /// Signs `message`.
    #[must_use]
    pub fn sign(&self, message: &[u8]) -> Signature {
        Signature(self.signing.sign(message).to_bytes())
    }

    /// The static X25519 secret used for peer message authentication:
    /// `clamp(SHA-512(seed)[0..32])`, as libsodium's
    /// `crypto_sign_ed25519_sk_to_curve25519` computes it (spec 3 §2.4).
    #[must_use]
    pub fn x25519_secret(&self) -> X25519Secret {
        let mut hash: [u8; 64] = Sha512::digest(self.as_bytes()).into();
        let mut scalar = [0u8; 32];
        if let Some(low) = hash.get(..32) {
            scalar.copy_from_slice(low);
        }
        hash.zeroize();
        let secret = X25519Secret::from_bytes(curve25519_dalek::scalar::clamp_integer(scalar));
        scalar.zeroize();
        secret
    }

    /// The PKCS#8 DER encoding (RFC 8410), as stored in the node database
    /// inside a `PRIVATE KEY` PEM block.
    #[must_use]
    pub fn to_pkcs8_der(&self) -> PrivateKeyDer {
        let mut der = [0u8; PKCS8_DER_LENGTH];
        let (prefix, seed) = der.split_at_mut(PKCS8_PREFIX.len());
        prefix.copy_from_slice(&PKCS8_PREFIX);
        seed.copy_from_slice(self.as_bytes());
        PrivateKeyDer::from_bytes(der)
    }

    /// Decodes the PKCS#8 DER of an Ed25519 key. Only the exact encoding
    /// [`to_pkcs8_der`](Self::to_pkcs8_der) produces is accepted: no
    /// attributes, no embedded public key, no other version.
    pub fn from_pkcs8_der(der: &[u8]) -> Result<Self, Error> {
        let seed = strip_prefix(der, &PKCS8_PREFIX)?;
        Ok(Self::from_bytes(seed))
    }
}

// `SigningKey` wipes its seed on drop (the `zeroize` feature).
impl zeroize::ZeroizeOnDrop for NodeKey {}

impl fmt::Debug for NodeKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // The public half identifies the key without revealing it.
        f.debug_struct("NodeKey")
            .field("public_key", &Hex(self.signing.verifying_key().as_bytes()))
            .finish_non_exhaustive()
    }
}

/// An Ed25519 public key, decompressed once so that verifying many messages
/// from the same peer does not repeat the work.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct PublicKey {
    key: VerifyingKey,
}

impl PublicKey {
    /// Parses a 32-byte compressed Edwards point.
    pub fn from_bytes(bytes: &[u8; 32]) -> Result<Self, Error> {
        VerifyingKey::from_bytes(bytes)
            .map(|key| Self { key })
            .map_err(|_| Error::InvalidPublicKey)
    }

    /// The 32 bytes as received, which is what node IDs and signatures are
    /// computed over.
    #[must_use]
    pub fn as_bytes(&self) -> &[u8; 32] {
        self.key.as_bytes()
    }

    /// The node ID of this key.
    #[must_use]
    pub fn node_id(&self) -> NodeId {
        NodeId::from_public_key(self.as_bytes())
    }

    /// Verifies `signature` over `message`.
    ///
    /// This is the lenient (cofactored, ZIP-215-style) check the original
    /// performed, deliberately not `verify_strict`: being stricter than the
    /// rest of the network could split it on edge-case signatures (spec 8
    /// §2).
    pub fn verify(&self, message: &[u8], signature: &Signature) -> Result<(), Error> {
        let signature = ed25519_dalek::Signature::from_bytes(&signature.0);
        self.key
            .verify(message, &signature)
            .map_err(|_| Error::InvalidSignature)
    }

    /// The X25519 public key: the Montgomery form `u = (1 + y) / (1 - y)` of
    /// this point, as libsodium's `crypto_sign_ed25519_pk_to_curve25519`
    /// computes it (spec 3 §2.4).
    #[must_use]
    pub fn to_x25519(&self) -> X25519Public {
        X25519Public::from_bytes(self.key.to_montgomery().to_bytes())
    }

    /// The `SubjectPublicKeyInfo` DER encoding (RFC 8410).
    #[must_use]
    pub fn to_spki_der(&self) -> [u8; SPKI_DER_LENGTH] {
        let mut der = [0u8; SPKI_DER_LENGTH];
        let (prefix, key) = der.split_at_mut(SPKI_PREFIX.len());
        prefix.copy_from_slice(&SPKI_PREFIX);
        key.copy_from_slice(self.as_bytes());
        der
    }

    /// Decodes the `SubjectPublicKeyInfo` DER of an Ed25519 key, accepting
    /// only the exact encoding [`to_spki_der`](Self::to_spki_der) produces.
    pub fn from_spki_der(der: &[u8]) -> Result<Self, Error> {
        Self::from_bytes(strip_prefix(der, &SPKI_PREFIX)?)
    }
}

impl fmt::Debug for PublicKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("PublicKey")
            .field(&Hex(self.as_bytes()))
            .finish()
    }
}

/// A 64-byte Ed25519 signature.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct Signature([u8; 64]);

impl Signature {
    /// Wraps signature bytes. Whether they are well formed is checked by
    /// [`PublicKey::verify`].
    #[must_use]
    pub const fn from_bytes(bytes: [u8; 64]) -> Self {
        Self(bytes)
    }

    /// The signature bytes.
    #[must_use]
    pub const fn as_bytes(&self) -> &[u8; 64] {
        &self.0
    }
}

impl fmt::Debug for Signature {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("Signature").field(&Hex(&self.0)).finish()
    }
}

/// The 32 bytes after `prefix`, if `der` is exactly `prefix` followed by 32
/// bytes.
fn strip_prefix<'a>(der: &'a [u8], prefix: &[u8]) -> Result<&'a [u8; 32], Error> {
    der.strip_prefix(prefix)
        .and_then(|rest| <&[u8; 32]>::try_from(rest).ok())
        .ok_or(Error::InvalidDer)
}
