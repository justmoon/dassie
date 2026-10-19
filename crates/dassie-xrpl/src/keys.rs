//! The original's XRPL wallet, from the node's settlement entropy
//! (spec/areas/xrpl.md#the-wallet), and account IDs.
//!
//! The derivation must match exactly: the same phrase must find the same
//! account and its funds (spec/overview.md#compatibility-with-the-original).

use alloc::string::String;
use core::fmt;

use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};
use ripemd::Ripemd160;
use sha2::{Digest, Sha256, Sha512};

use crate::base58;

/// An account's address version byte.
const ACCOUNT_VERSION: [u8; 1] = [0x00];

/// An Ed25519 family seed's version bytes: seeds read `sEd…`.
const ED25519_SEED_VERSION: [u8; 3] = [0x01, 0xE1, 0x4B];

/// The prefix of an Ed25519 public key on the ledger.
const ED25519_PREFIX: u8 = 0xED;

/// An account on the ledger: the RIPEMD-160 of the SHA-256 of its public
/// key.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct AccountId(pub [u8; 20]);

impl AccountId {
    /// The account of `public_key`, as the ledger writes it (33 bytes).
    #[must_use]
    pub fn from_public_key(public_key: &[u8; 33]) -> Self {
        let digest = Ripemd160::digest(Sha256::digest(public_key));
        let mut id = [0; 20];
        id.copy_from_slice(digest.as_slice());
        Self(id)
    }

    /// Its classic address, `r…`.
    #[must_use]
    pub fn to_address(&self) -> String {
        base58::encode_check(&ACCOUNT_VERSION, &self.0)
    }

    /// The account a classic address names, if it is one.
    #[must_use]
    pub fn parse(address: &str) -> Option<Self> {
        if !(25..=35).contains(&address.len()) {
            return None;
        }
        let payload = base58::decode_check(address, &ACCOUNT_VERSION)?;
        Some(Self(payload.try_into().ok()?))
    }
}

impl fmt::Display for AccountId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.to_address())
    }
}

impl fmt::Debug for AccountId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "AccountId({self})")
    }
}

/// An Ed25519 wallet: its seed, keys and account.
pub struct Wallet {
    seed: [u8; 16],
    key: SigningKey,
    public_key: [u8; 33],
    account: AccountId,
}

impl Wallet {
    /// The wallet of the node's settlement entropy for the scheme
    /// (spec/areas/identity.md#seed-paths): its first 16 bytes are the seed.
    #[must_use]
    pub fn from_entropy(entropy: &[u8; 32]) -> Self {
        let mut seed = [0; 16];
        seed.copy_from_slice(entropy.get(..16).unwrap_or(&[0; 16]));
        Self::from_seed(seed)
    }

    /// The wallet of an Ed25519 seed: the private key is the first half of
    /// SHA-512 of the seed, as ripple-keypairs derives it.
    #[must_use]
    pub fn from_seed(seed: [u8; 16]) -> Self {
        let digest = Sha512::digest(seed);
        let mut secret = [0; 32];
        secret.copy_from_slice(digest.get(..32).unwrap_or(&[0; 32]));
        let key = SigningKey::from_bytes(&secret);
        let mut public_key = [ED25519_PREFIX; 33];
        public_key
            .get_mut(1..)
            .unwrap_or(&mut [])
            .copy_from_slice(key.verifying_key().as_bytes());
        Self {
            seed,
            key,
            account: AccountId::from_public_key(&public_key),
            public_key,
        }
    }

    /// The wallet of a family seed, `sEd…`, if it is one.
    #[must_use]
    pub fn from_family_seed(text: &str) -> Option<Self> {
        let payload = base58::decode_check(text, &ED25519_SEED_VERSION)?;
        Some(Self::from_seed(payload.try_into().ok()?))
    }

    /// Its family seed, `sEd…`: a secret.
    #[must_use]
    pub fn family_seed(&self) -> String {
        base58::encode_check(&ED25519_SEED_VERSION, &self.seed)
    }

    /// Its public key as the ledger writes it: `0xED` and the key.
    #[must_use]
    pub const fn public_key(&self) -> &[u8; 33] {
        &self.public_key
    }

    /// Its account.
    #[must_use]
    pub const fn account(&self) -> AccountId {
        self.account
    }

    /// Signs `message` whole, as the ledger's Ed25519 does.
    #[must_use]
    pub fn sign(&self, message: &[u8]) -> [u8; 64] {
        self.key.sign(message).to_bytes()
    }
}

impl fmt::Debug for Wallet {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Wallet")
            .field("account", &self.account)
            .finish_non_exhaustive()
    }
}

/// Whether `signature` is `public_key`'s on `message`, for an Ed25519 key
/// as the ledger writes it.
#[must_use]
pub fn verify(public_key: &[u8; 33], message: &[u8], signature: &[u8; 64]) -> bool {
    let Some((&ED25519_PREFIX, key)) = public_key.split_first() else {
        return false;
    };
    let Ok(key) = <[u8; 32]>::try_from(key) else {
        return false;
    };
    let Ok(key) = VerifyingKey::from_bytes(&key) else {
        return false;
    };
    key.verify(message, &Signature::from_bytes(signature))
        .is_ok()
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::indexing_slicing)]
mod tests {
    use super::*;

    #[test]
    fn a_signature_verifies_and_a_changed_one_does_not() {
        let wallet = Wallet::from_seed([7; 16]);
        let mut signature = wallet.sign(b"pay");
        assert!(verify(wallet.public_key(), b"pay", &signature));
        assert!(!verify(wallet.public_key(), b"pay more", &signature));
        signature[0] ^= 1;
        assert!(!verify(wallet.public_key(), b"pay", &signature));
    }

    #[test]
    fn family_seeds_round_trip() {
        let wallet = Wallet::from_seed([3; 16]);
        let again = Wallet::from_family_seed(&wallet.family_seed()).unwrap();
        assert_eq!(again.account(), wallet.account());
        assert!(wallet.family_seed().starts_with("sEd"));
    }
}
