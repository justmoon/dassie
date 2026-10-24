//! The original's XRPL wallet, from the node's settlement entropy
//! (spec/areas/xrpl.md#the-wallet), account IDs, and the keys the ledger
//! knows: Ed25519 and secp256k1, with their signatures
//! (spec/areas/xrpl.md#keys-and-signatures).
//!
//! The derivation must match exactly: the same phrase must find the same
//! account and its funds (spec/overview.md#frozen-surfaces).

use alloc::string::String;
use core::fmt;

use ecdsa::signature::hazmat::{PrehashSigner, PrehashVerifier};
use ed25519_dalek::{Signer, SigningKey, Verifier, VerifyingKey};
use k256::ecdsa as secp256k1;
use ripemd::Ripemd160;
use sha2::{Digest, Sha256, Sha512};

use crate::{base58, hex_upper};

/// An account's address version byte.
const ACCOUNT_VERSION: [u8; 1] = [0x00];

/// A node public key's version byte: validators' keys read `n…`.
const NODE_PUBLIC_VERSION: [u8; 1] = [0x1C];

/// An Ed25519 family seed's version bytes: seeds read `sEd…`.
const ED25519_SEED_VERSION: [u8; 3] = [0x01, 0xE1, 0x4B];

/// The prefix of an Ed25519 public key on the ledger.
const ED25519_PREFIX: u8 = 0xED;

/// The longest signature the ledger carries: a DER-encoded ECDSA signature
/// of two 33-byte integers. An Ed25519 signature is 64 bytes.
pub const MAX_SIGNATURE_BYTES: usize = 72;

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

/// The kinds of key the ledger knows, told apart by a public key's first
/// byte.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KeyType {
    /// Ed25519: `0xED` and the key. It signs the message whole.
    Ed25519,
    /// ECDSA on secp256k1: a compressed point, `0x02` or `0x03` and its x.
    /// It signs the SHA-512Half of the message, in strict DER, with a low S.
    Secp256k1,
}

/// A public key as the ledger writes it: 33 bytes, of either kind.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct PublicKey([u8; 33]);

impl PublicKey {
    /// The key in `bytes`, if its first byte names a kind the ledger knows.
    /// Whether it is a point on its curve is checked when it verifies.
    #[must_use]
    pub const fn from_bytes(bytes: [u8; 33]) -> Option<Self> {
        match bytes[0] {
            ED25519_PREFIX | 0x02 | 0x03 => Some(Self(bytes)),
            _ => None,
        }
    }

    /// A key built in, such as a list publisher's: one of a kind the ledger
    /// knows, or the build fails.
    #[must_use]
    pub const fn built_in(bytes: [u8; 33]) -> Self {
        assert!(
            matches!(bytes[0], ED25519_PREFIX | 0x02 | 0x03),
            "a built-in key of a kind the ledger knows"
        );
        Self(bytes)
    }

    /// Its bytes, as the ledger writes them.
    #[must_use]
    pub const fn as_bytes(&self) -> &[u8; 33] {
        &self.0
    }

    /// Its kind.
    #[must_use]
    pub const fn key_type(&self) -> KeyType {
        if self.0[0] == ED25519_PREFIX {
            KeyType::Ed25519
        } else {
            KeyType::Secp256k1
        }
    }

    /// The account whose master key it is.
    #[must_use]
    pub fn account(&self) -> AccountId {
        AccountId::from_public_key(&self.0)
    }

    /// It as a node public key, as validators' keys are written: `nH…` for
    /// Ed25519, `n9…` for secp256k1.
    #[must_use]
    pub fn to_node_public(&self) -> String {
        base58::encode_check(&NODE_PUBLIC_VERSION, &self.0)
    }

    /// The key a node public key names, if it is one of a kind the ledger
    /// knows.
    #[must_use]
    pub fn from_node_public(text: &str) -> Option<Self> {
        let payload = base58::decode_check(text, &NODE_PUBLIC_VERSION)?;
        Self::from_bytes(payload.try_into().ok()?)
    }

    /// Whether `signature` is this key's on `message`, as the ledger checks
    /// it: for secp256k1, only a fully canonical signature (strict DER, S
    /// at most half the group order).
    #[must_use]
    pub fn verify(&self, message: &[u8], signature: &Signature) -> bool {
        match self.key_type() {
            KeyType::Ed25519 => verify_ed25519(&self.0, message, signature.as_bytes()),
            KeyType::Secp256k1 => verify_secp256k1(&self.0, message, signature.as_bytes()),
        }
    }
}

impl fmt::Debug for PublicKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "PublicKey({})", hex_upper(&self.0))
    }
}

/// A signature as the ledger carries it: 64 bytes of Ed25519, or an ECDSA
/// signature in DER, at most [`MAX_SIGNATURE_BYTES`].
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct Signature {
    bytes: [u8; MAX_SIGNATURE_BYTES],
    length: u8,
}

impl Signature {
    /// The signature in `bytes`, if it is no longer than any the ledger
    /// carries and not empty. Whether it is valid is checked when it is
    /// verified.
    #[must_use]
    pub fn from_slice(bytes: &[u8]) -> Option<Self> {
        if bytes.is_empty() || bytes.len() > MAX_SIGNATURE_BYTES {
            return None;
        }
        let mut signature = Self {
            bytes: [0; MAX_SIGNATURE_BYTES],
            length: u8::try_from(bytes.len()).ok()?,
        };
        signature
            .bytes
            .get_mut(..bytes.len())?
            .copy_from_slice(bytes);
        Some(signature)
    }

    /// Its bytes.
    #[must_use]
    pub fn as_bytes(&self) -> &[u8] {
        self.bytes
            .get(..usize::from(self.length))
            .unwrap_or_default()
    }
}

impl fmt::Debug for Signature {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Signature({})", hex_upper(self.as_bytes()))
    }
}

/// A key that signs for the ledger: a [`Wallet`]'s Ed25519 key, or a
/// [`Secp256k1Key`].
pub trait Key {
    /// Its public key.
    fn public_key(&self) -> PublicKey;

    /// Its signature on `message`, as the ledger checks it
    /// ([`PublicKey::verify`]).
    fn sign(&self, message: &[u8]) -> Signature;

    /// The account whose master key it is.
    fn account(&self) -> AccountId {
        self.public_key().account()
    }
}

/// An Ed25519 wallet: its seed, keys and account.
pub struct Wallet {
    seed: [u8; 16],
    key: SigningKey,
    public_key: PublicKey,
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
        let mut bytes = [ED25519_PREFIX; 33];
        bytes
            .get_mut(1..)
            .unwrap_or(&mut [])
            .copy_from_slice(key.verifying_key().as_bytes());
        Self {
            seed,
            key,
            account: AccountId::from_public_key(&bytes),
            public_key: PublicKey(bytes),
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
    pub const fn public_key(&self) -> PublicKey {
        self.public_key
    }

    /// Its account.
    #[must_use]
    pub const fn account(&self) -> AccountId {
        self.account
    }

    /// Signs `message` whole, as the ledger's Ed25519 does.
    #[must_use]
    pub fn sign(&self, message: &[u8]) -> Signature {
        let signature = self.key.sign(message).to_bytes();
        let signature = Signature::from_slice(&signature);
        assert!(signature.is_some(), "an Ed25519 signature is 64 bytes");
        signature.unwrap_or(Signature {
            bytes: [0; MAX_SIGNATURE_BYTES],
            length: 0,
        })
    }
}

impl Key for Wallet {
    fn public_key(&self) -> PublicKey {
        Self::public_key(self)
    }

    fn sign(&self, message: &[u8]) -> Signature {
        Self::sign(self, message)
    }

    fn account(&self) -> AccountId {
        Self::account(self)
    }
}

impl fmt::Debug for Wallet {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Wallet")
            .field("account", &self.account)
            .finish_non_exhaustive()
    }
}

/// A secp256k1 key, from its secret scalar: how it was derived is its
/// owner's.
pub struct Secp256k1Key {
    key: secp256k1::SigningKey,
    public_key: PublicKey,
}

impl Secp256k1Key {
    /// The key of the secret scalar `secret`, big-endian, if it is one: not
    /// zero, and below the group order.
    #[must_use]
    pub fn from_secret(secret: &[u8; 32]) -> Option<Self> {
        let key = secp256k1::SigningKey::from_slice(secret).ok()?;
        let point = key.verifying_key().to_sec1_point(true);
        let bytes: [u8; 33] = point.as_bytes().try_into().ok()?;
        Some(Self {
            key,
            public_key: PublicKey::from_bytes(bytes)?,
        })
    }

    /// Its public key: the compressed point.
    #[must_use]
    pub const fn public_key(&self) -> PublicKey {
        self.public_key
    }

    /// The account whose master key it is.
    #[must_use]
    pub fn account(&self) -> AccountId {
        self.public_key.account()
    }

    /// Signs the SHA-512Half of `message`, as the ledger's secp256k1 does:
    /// deterministically (RFC 6979), with a low S, in DER.
    #[must_use]
    pub fn sign(&self, message: &[u8]) -> Signature {
        let signed: Result<secp256k1::Signature, _> = self.key.sign_prehash(&sha512_half(message));
        // Signing a 32-byte hash with a valid key does not fail.
        assert!(signed.is_ok(), "ECDSA signs a 32-byte hash");
        let der = signed.map(|s| s.normalize_s().to_der());
        let signature = der
            .ok()
            .and_then(|der| Signature::from_slice(der.as_bytes()));
        assert!(signature.is_some(), "a DER signature fits");
        signature.unwrap_or(Signature {
            bytes: [0; MAX_SIGNATURE_BYTES],
            length: 0,
        })
    }
}

impl Key for Secp256k1Key {
    fn public_key(&self) -> PublicKey {
        Self::public_key(self)
    }

    fn sign(&self, message: &[u8]) -> Signature {
        Self::sign(self, message)
    }
}

impl fmt::Debug for Secp256k1Key {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Secp256k1Key")
            .field("public_key", &self.public_key)
            .finish_non_exhaustive()
    }
}

/// The first half of SHA-512: the ledger's hash.
#[must_use]
pub fn sha512_half(bytes: &[u8]) -> [u8; 32] {
    let digest = Sha512::digest(bytes);
    let mut out = [0; 32];
    out.copy_from_slice(digest.get(..32).unwrap_or(&[0; 32]));
    out
}

fn verify_ed25519(public_key: &[u8; 33], message: &[u8], signature: &[u8]) -> bool {
    let Some((&ED25519_PREFIX, key)) = public_key.split_first() else {
        return false;
    };
    let (Ok(key), Ok(signature)) = (<[u8; 32]>::try_from(key), <[u8; 64]>::try_from(signature))
    else {
        return false;
    };
    let Ok(key) = VerifyingKey::from_bytes(&key) else {
        return false;
    };
    key.verify(message, &ed25519_dalek::Signature::from_bytes(&signature))
        .is_ok()
}

/// As rippled checks a secp256k1 signature: the DER is strict, so it
/// writes back to the same bytes, and S is low, so no one can make a second
/// valid signature, with another hash, of one signed transaction.
fn verify_secp256k1(public_key: &[u8; 33], message: &[u8], signature: &[u8]) -> bool {
    let Ok(key) = secp256k1::VerifyingKey::from_sec1_bytes(public_key) else {
        return false;
    };
    let Ok(parsed) = secp256k1::Signature::from_der(signature) else {
        return false;
    };
    let canonical = parsed.to_der().as_bytes() == signature && parsed.normalize_s() == parsed;
    canonical && key.verify_prehash(&sha512_half(message), &parsed).is_ok()
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::indexing_slicing)]
mod tests {
    use super::*;

    #[test]
    fn a_signature_verifies_and_a_changed_one_does_not() {
        let wallet = Wallet::from_seed([7; 16]);
        let signature = wallet.sign(b"pay");
        assert_eq!(signature.as_bytes().len(), 64);
        let key = wallet.public_key();
        assert_eq!(key.key_type(), KeyType::Ed25519);
        assert!(key.verify(b"pay", &signature));
        assert!(!key.verify(b"pay more", &signature));
        let mut changed = signature.as_bytes().to_vec();
        changed[0] ^= 1;
        assert!(!key.verify(b"pay", &Signature::from_slice(&changed).unwrap()));
    }

    #[test]
    fn family_seeds_round_trip() {
        let wallet = Wallet::from_seed([3; 16]);
        let again = Wallet::from_family_seed(&wallet.family_seed()).unwrap();
        assert_eq!(again.account(), wallet.account());
        assert!(wallet.family_seed().starts_with("sEd"));
    }

    #[test]
    fn secp256k1_signatures_are_canonical_and_verify() {
        let key = Secp256k1Key::from_secret(&[0x11; 32]).unwrap();
        let public = key.public_key();
        assert_eq!(public.key_type(), KeyType::Secp256k1);
        assert_eq!(public.account(), key.account());
        let signature = key.sign(b"pay");
        // Deterministic.
        assert_eq!(signature, key.sign(b"pay"));
        assert!(public.verify(b"pay", &signature));
        assert!(!public.verify(b"pay more", &signature));
        // The same signature with a high S verifies as ECDSA, but is not
        // canonical, so the ledger refuses it.
        let parsed = secp256k1::Signature::from_der(signature.as_bytes()).unwrap();
        let (r, s) = parsed.split_scalars();
        let high = secp256k1::Signature::from_scalars(r.to_bytes(), (-*s).to_bytes()).unwrap();
        let high = Signature::from_slice(high.to_der().as_bytes()).unwrap();
        assert!(!public.verify(b"pay", &high));
        // An Ed25519 signature is no secp256k1 one, nor the other way.
        let wallet = Wallet::from_seed([7; 16]);
        assert!(!public.verify(b"pay", &wallet.sign(b"pay")));
        assert!(!wallet.public_key().verify(b"pay", &signature));
    }

    #[test]
    fn node_public_keys_round_trip() {
        // A mainnet validator's master key, and its signing key.
        let master = "nHBWa56Vr7csoFcCnEPzCCKVvnDQw3L28mATgHYQMGtbEfUjuYyB";
        let key = PublicKey::from_node_public(master).unwrap();
        assert_eq!(key.key_type(), KeyType::Ed25519);
        assert_eq!(
            hex_upper(key.as_bytes()),
            "ED13AAFCB6A87BCB5D093C2EF37F04431C291126D674293305152D9776C6ABA4D6"
        );
        assert_eq!(key.to_node_public(), master);
        let signing = "n94a894ARPe5RdcaRgdMBB9gG9ukS5mqsd7q2oNmC1NKqtZqEJnb";
        let key = PublicKey::from_node_public(signing).unwrap();
        assert_eq!(key.key_type(), KeyType::Secp256k1);
        assert_eq!(key.to_node_public(), signing);
        // An address is no node public key, nor the other way round, and a
        // changed one is neither.
        assert!(PublicKey::from_node_public(&key.account().to_address()).is_none());
        assert!(AccountId::parse(signing).is_none());
        let changed = "n94a894ARPe5RdcaRgdMBB9gG9ukS5mqsd7q2oNmC1NKqtZqEJnc";
        assert!(PublicKey::from_node_public(changed).is_none());
    }

    #[test]
    fn keys_of_no_kind_and_secrets_of_no_key_are_refused() {
        assert!(PublicKey::from_bytes([0x04; 33]).is_none());
        assert!(PublicKey::from_bytes([0; 33]).is_none());
        assert!(Secp256k1Key::from_secret(&[0; 32]).is_none());
        assert!(Secp256k1Key::from_secret(&[0xFF; 32]).is_none());
        assert!(Signature::from_slice(&[]).is_none());
        assert!(Signature::from_slice(&[1; MAX_SIGNATURE_BYTES + 1]).is_none());
        // A point not on the curve verifies nothing.
        let mut off = [0x02; 33];
        off[1..].fill(0xFF);
        let key = PublicKey::from_bytes(off).unwrap();
        let signature = Secp256k1Key::from_secret(&[0x11; 32]).unwrap().sign(b"x");
        assert!(!key.verify(b"x", &signature));
    }
}
