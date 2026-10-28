//! The node's Ed25519 key, its public key, signatures and their DER forms.

use core::fmt;

use ed25519_dalek::{Signer, SigningKey, VerifyingKey};

use crate::error::Error;
use crate::secret::{Hex, secret_bytes};

/// PKCS#8 `PrivateKeyInfo` for an Ed25519 seed (RFC 8410 §7), up to the
/// seed: version 0, algorithm 1.3.101.112, and an OCTET STRING holding an
/// OCTET STRING of 32 bytes.
const PKCS8_PREFIX: [u8; 16] = [
    0x30, 0x2e, 0x02, 0x01, 0x00, 0x30, 0x05, 0x06, 0x03, 0x2b, 0x65, 0x70, 0x04, 0x22, 0x04, 0x20,
];

/// `SubjectPublicKeyInfo` for an Ed25519 public key (RFC 8410 §4), up to the
/// key: algorithm 1.3.101.112 and a BIT STRING of 33 bytes, the first being
/// the count of unused bits (zero). A prefix without that byte would read
/// the key as 33 bytes starting with a stray `00`.
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
/// original (spec/areas/identity.md#node-key).
pub struct NodeKey {
    signing: SigningKey,
    public: PublicKey,
}

impl NodeKey {
    /// The key with Ed25519 seed `seed`.
    #[must_use]
    pub fn from_bytes(seed: &[u8; 32]) -> Self {
        let signing = SigningKey::from_bytes(seed);
        // The same derivation (RFC 8032) in the verifying library, so that
        // no fallible decoding of our own key is needed.
        let zip215 = ed25519_zebra::VerificationKey::from(&ed25519_zebra::SigningKey::from(*seed));
        let public = PublicKey {
            key: signing.verifying_key(),
            zip215,
        };
        Self { signing, public }
    }

    /// The Ed25519 seed: the raw private key the node database stores.
    #[must_use]
    pub fn as_bytes(&self) -> &[u8; 32] {
        self.signing.as_bytes()
    }

    /// The public key.
    #[must_use]
    pub fn public_key(&self) -> PublicKey {
        self.public
    }

    /// Signs `message`.
    #[must_use]
    pub fn sign(&self, message: &[u8]) -> Signature {
        Signature(self.signing.sign(message).to_bytes())
    }

    /// The PKCS#8 DER encoding (RFC 8410): what TLS takes the key in, and
    /// what a front door's key file holds inside a `PRIVATE KEY` PEM block.
    /// The node database stores the raw seed.
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
            .field("public_key", &Hex(self.public.as_bytes()))
            .finish_non_exhaustive()
    }
}

/// An Ed25519 public key, decompressed once so that verifying many messages
/// from the same peer does not repeat the work.
#[derive(Clone, Copy)]
pub struct PublicKey {
    key: VerifyingKey,
    /// The same key for ZIP-215 verification.
    zip215: ed25519_zebra::VerificationKey,
}

impl PartialEq for PublicKey {
    fn eq(&self, other: &Self) -> bool {
        self.as_bytes() == other.as_bytes()
    }
}

impl Eq for PublicKey {}

impl core::hash::Hash for PublicKey {
    fn hash<H: core::hash::Hasher>(&self, state: &mut H) {
        self.as_bytes().hash(state);
    }
}

impl PublicKey {
    /// Parses a 32-byte compressed Edwards point. Non-canonical encodings
    /// are accepted, as ZIP-215 accepts them; the node ID is computed over
    /// the bytes as given.
    pub fn from_bytes(bytes: &[u8; 32]) -> Result<Self, Error> {
        let key = VerifyingKey::from_bytes(bytes).map_err(|_| Error::InvalidPublicKey)?;
        let zip215 = ed25519_zebra::VerificationKey::try_from(*bytes)
            .map_err(|_| Error::InvalidPublicKey)?;
        Ok(Self { key, zip215 })
    }

    /// Whether the key is a point of small order. Under ZIP-215 a signature
    /// with `s = 0` and a small-order `R` verifies for every message under
    /// such a key, so it proves nothing; link states refuse these keys.
    #[must_use]
    pub fn is_weak(&self) -> bool {
        self.key.is_weak()
    }

    /// The 32 bytes as received, which is what node IDs and signatures are
    /// computed over. A node ID takes the node's SLH-DSA key too
    /// ([`NodeIdentity`](crate::NodeIdentity)).
    #[must_use]
    pub fn as_bytes(&self) -> &[u8; 32] {
        self.key.as_bytes()
    }

    /// Verifies `signature` over `message` by the ZIP-215 rules
    /// (spec/areas/identity.md#node-key): cofactored, accepting non-canonical
    /// encodings of `R` and of the key, requiring a canonical `s`. These rules are exactly specified, so
    /// every implementation can agree on every signature, edge cases
    /// included; ed25519-dalek's own `verify` is cofactorless and would
    /// refuse some signatures that ZIP-215 accepts.
    pub fn verify(&self, message: &[u8], signature: &Signature) -> Result<(), Error> {
        let signature = ed25519_zebra::Signature::from_bytes(&signature.0);
        self.zip215
            .verify(&signature, message)
            .map_err(|_| Error::InvalidSignature)
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
