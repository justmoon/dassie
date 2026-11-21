//! How a node key's signatures are made and checked, where a simulation
//! makes and checks them by the hundred thousand
//! (spec/decisions/hashed-signatures-in-simulations.md).
//!
//! A node signs and checks with Ed25519, [`Ed25519Signatures`]. Whatever
//! signs or checks a kind of message that a simulation sends in such
//! numbers takes a [`NodeSignatures`] instead of calling [`NodeKey::sign`]
//! and [`PublicKey::verify`], so that a simulation can pass
//! [`HashSignatures`]: a hash of the signer's public key and the message,
//! which costs a hundredth of a signature. Nothing in a node passes it.

use core::fmt;

use sha2::{Digest, Sha512};
use subtle::ConstantTimeEq;

use crate::error::Error;
use crate::node_key::{NodeKey, PublicKey, Signature};

/// How a node key signs, and how its signatures are checked.
///
/// A node's is [`Ed25519Signatures`]. Every kind of message that takes one
/// is signed and checked by the same one, wherever it is signed or
/// checked, so the two always agree.
pub trait NodeSignatures: fmt::Debug + Send + Sync {
    /// `key`'s signature over `message`.
    fn sign(&self, key: &NodeKey, message: &[u8]) -> Signature;

    /// Checks `signature` over `message` by `key`.
    ///
    /// # Errors
    ///
    /// [`Error::InvalidSignature`] if it is not `key`'s over `message`.
    fn verify(&self, key: &PublicKey, message: &[u8], signature: &Signature) -> Result<(), Error>;
}

/// Ed25519, as [`NodeKey::sign`] signs and [`PublicKey::verify`] checks:
/// what every node signs and checks with.
#[derive(Clone, Copy, Debug, Default)]
pub struct Ed25519Signatures;

impl NodeSignatures for Ed25519Signatures {
    fn sign(&self, key: &NodeKey, message: &[u8]) -> Signature {
        key.sign(message)
    }

    fn verify(&self, key: &PublicKey, message: &[u8], signature: &Signature) -> Result<(), Error> {
        key.verify(message, signature)
    }
}

/// The signatures of simulations: SHA-512 over a tag, the signer's public
/// key and the message, which the check computes again.
///
/// A signature changed in any bit, over another message or of another key
/// fails, as a real one would; an Ed25519 signature fails too, and a hash
/// fails under Ed25519. Anyone who knows the public key could make one, so
/// it proves nothing outside a simulation, where no code makes one but
/// through [`NodeSignatures::sign`], which takes the signer's key.
#[derive(Clone, Copy, Debug, Default)]
pub struct HashSignatures;

/// What a hash signature hashes first, so that it is no other hash.
const HASH_TAG: &[u8] = b"dassie simulated node signature";

impl HashSignatures {
    fn hash(key: &PublicKey, message: &[u8]) -> [u8; 64] {
        Sha512::new()
            .chain_update(HASH_TAG)
            .chain_update(key.as_bytes())
            .chain_update(message)
            .finalize()
            .into()
    }
}

impl NodeSignatures for HashSignatures {
    fn sign(&self, key: &NodeKey, message: &[u8]) -> Signature {
        Signature::from_bytes(Self::hash(&key.public_key(), message))
    }

    fn verify(&self, key: &PublicKey, message: &[u8], signature: &Signature) -> Result<(), Error> {
        if bool::from(Self::hash(key, message).ct_eq(signature.as_bytes())) {
            Ok(())
        } else {
            Err(Error::InvalidSignature)
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::indexing_slicing)]

    use proptest::prelude::*;

    use super::*;

    #[test]
    fn ed25519_is_the_node_keys_own() {
        let key = NodeKey::from_bytes(&[1; 32]);
        let signature = Ed25519Signatures.sign(&key, b"vote");
        assert_eq!(signature, key.sign(b"vote"));
        assert_eq!(
            Ed25519Signatures.verify(&key.public_key(), b"vote", &signature),
            Ok(())
        );
    }

    #[test]
    fn a_hash_is_no_ed25519_signature() {
        let key = NodeKey::from_bytes(&[1; 32]);
        let public = key.public_key();
        let hash = HashSignatures.sign(&key, b"vote");
        assert_eq!(HashSignatures.verify(&public, b"vote", &hash), Ok(()));
        assert!(Ed25519Signatures.verify(&public, b"vote", &hash).is_err());
        let real = Ed25519Signatures.sign(&key, b"vote");
        assert!(HashSignatures.verify(&public, b"vote", &real).is_err());
    }

    #[test]
    fn a_hash_is_of_its_key() {
        let key = NodeKey::from_bytes(&[1; 32]);
        let other = NodeKey::from_bytes(&[2; 32]).public_key();
        let hash = HashSignatures.sign(&key, b"vote");
        assert_eq!(
            HashSignatures.verify(&other, b"vote", &hash),
            Err(Error::InvalidSignature)
        );
    }

    proptest! {
        /// A hash fails with any bit of it or of its message flipped.
        #[test]
        fn a_flipped_bit_fails(
            message in prop::collection::vec(any::<u8>(), 1..200),
            bit in any::<prop::sample::Index>(),
        ) {
            let key = NodeKey::from_bytes(&[3; 32]);
            let public = key.public_key();
            let hash = HashSignatures.sign(&key, &message);
            prop_assert_eq!(HashSignatures.verify(&public, &message, &hash), Ok(()));

            let mut bytes = *hash.as_bytes();
            let at = bit.index(bytes.len() * 8);
            bytes[at / 8] ^= 1 << (at % 8);
            let flipped = Signature::from_bytes(bytes);
            prop_assert!(HashSignatures.verify(&public, &message, &flipped).is_err());

            let mut changed = message.clone();
            let at = bit.index(changed.len() * 8);
            changed[at / 8] ^= 1 << (at % 8);
            prop_assert!(HashSignatures.verify(&public, &changed, &hash).is_err());
        }
    }
}
