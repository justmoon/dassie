//! X25519 on the Montgomery form of node keys, for peer message
//! authentication (spec/areas/identity.md#x25519-key).

use core::fmt;

use curve25519_dalek::montgomery::MontgomeryPoint;
use rand_core::CryptoRng;
use subtle::ConstantTimeEq;
use zeroize::Zeroize;

use crate::error::Error;
use crate::secret::{Hex, secret_bytes};

secret_bytes!(
    /// An X25519 secret scalar.
    ///
    /// Any 32 bytes are a valid secret: X25519 clamps them before use (RFC
    /// 7748 §5), so storing them clamped or not gives the same results. The
    /// secret derived from a node key is stored clamped, as libsodium does.
    X25519Secret,
    32
);

secret_bytes!(
    /// The output of an X25519 exchange that passed the contributory check.
    SharedSecret,
    32
);

impl X25519Secret {
    /// A fresh secret from the caller's RNG, for ephemeral exchanges.
    #[must_use]
    pub fn generate<R: CryptoRng + ?Sized>(rng: &mut R) -> Self {
        let mut secret = Self([0; 32]);
        rng.fill_bytes(&mut secret.0);
        secret
    }

    /// The public key: the clamped scalar times the base point.
    #[must_use]
    pub fn public_key(&self) -> X25519Public {
        X25519Public(MontgomeryPoint::mul_base_clamped(self.0).to_bytes())
    }

    /// The X25519 shared secret with `peer`.
    ///
    /// A peer key of small order makes the result all zeros whatever our
    /// secret is, which would let the peer force a shared secret it knows.
    /// RFC 7748 §6.1 says to reject that, and this does; the original did
    /// not check.
    pub fn diffie_hellman(&self, peer: &X25519Public) -> Result<SharedSecret, Error> {
        let mut point = MontgomeryPoint(peer.0).mul_clamped(self.0);
        let shared = SharedSecret(point.to_bytes());
        point.zeroize();
        if bool::from(shared.0.as_slice().ct_eq(&[0; 32])) {
            return Err(Error::NonContributory);
        }
        Ok(shared)
    }
}

/// An X25519 public key (a Montgomery `u` coordinate).
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct X25519Public([u8; 32]);

impl X25519Public {
    /// Wraps a `u` coordinate. Every 32-byte string is accepted, as RFC 7748
    /// requires; weak keys are caught by the contributory check instead.
    #[must_use]
    pub const fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    /// The `u` coordinate.
    #[must_use]
    pub const fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }
}

impl fmt::Debug for X25519Public {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("X25519Public").field(&Hex(&self.0)).finish()
    }
}
