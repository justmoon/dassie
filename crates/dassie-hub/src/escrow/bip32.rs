//! BIP 32 derivation on secp256k1, non-hardened only, for the keys a
//! validator signs a payout set with
//! (spec/designs/settlement-hubs.md#9-shutdown).
//!
//! From the public parent in the rules, anyone derives the key of every
//! validator for every payout set, which is what puts them on the escrow's
//! new signer list; only the validator, which holds the private parent,
//! can sign with them. With non-hardened derivation one leaked child and
//! the public parent give away every child, so a validator guards its
//! payout key as it guards its escrow key.

use hmac::{Hmac, KeyInit, Mac};
use k256::elliptic_curve::ff::PrimeField;
use k256::elliptic_curve::sec1::ToSec1Point;
use k256::{FieldBytes, ProjectivePoint, PublicKey, Scalar};
use sha2::Sha512;

/// The first index of hardened derivation, which is not used here.
pub const HARDENED: u32 = 1 << 31;

/// A public key with its chain code: what the rules publish.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ExtendedPublic {
    /// The compressed point.
    pub key: [u8; 33],
    /// The chain code.
    pub chain: [u8; 32],
}

/// A secret scalar with its chain code: what the validator keeps.
#[derive(Clone)]
pub struct ExtendedPrivate {
    secret: [u8; 32],
    chain: [u8; 32],
}

impl Drop for ExtendedPrivate {
    fn drop(&mut self) {
        use zeroize::Zeroize;
        self.secret.zeroize();
        self.chain.zeroize();
    }
}

impl core::fmt::Debug for ExtendedPrivate {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str("ExtendedPrivate(<redacted>)")
    }
}

/// HMAC-SHA512 of `data` under `key`, split into its halves.
fn hmac(key: &[u8], data: &[&[u8]]) -> ([u8; 32], [u8; 32]) {
    let Ok(mut mac) = Hmac::<Sha512>::new_from_slice(key) else {
        // HMAC takes keys of any length.
        return ([0; 32], [0; 32]);
    };
    for part in data {
        mac.update(part);
    }
    let mut out: [u8; 64] = mac.finalize().into_bytes().into();
    let mut left = [0; 32];
    let mut right = [0; 32];
    let (l, r) = out.split_at(32);
    left.copy_from_slice(l);
    right.copy_from_slice(r);
    zeroize::Zeroize::zeroize(&mut out);
    (left, right)
}

/// The scalar `bytes` encodes, if it is below the group order.
fn scalar(bytes: &[u8; 32]) -> Option<Scalar> {
    Scalar::from_repr(FieldBytes::from(*bytes)).into_option()
}

fn compressed(point: &PublicKey) -> Option<[u8; 33]> {
    point.to_sec1_point(true).as_bytes().try_into().ok()
}

impl ExtendedPrivate {
    /// The master key of `seed` (BIP 32's "Bitcoin seed" HMAC), if the
    /// seed gives a valid one.
    #[must_use]
    pub fn master(seed: &[u8]) -> Option<Self> {
        let (secret, chain) = hmac(b"Bitcoin seed", &[seed]);
        let value = scalar(&secret)?;
        bool::from(!value.is_zero()).then_some(Self { secret, chain })
    }

    /// Its secret scalar, big-endian.
    #[must_use]
    pub const fn secret(&self) -> &[u8; 32] {
        &self.secret
    }

    /// Its public key and chain code.
    #[must_use]
    pub fn public(&self) -> Option<ExtendedPublic> {
        let value = scalar(&self.secret)?;
        let point =
            PublicKey::from_affine((ProjectivePoint::GENERATOR * value).to_affine()).ok()?;
        Some(ExtendedPublic {
            key: compressed(&point)?,
            chain: self.chain,
        })
    }

    /// The non-hardened child at `index`, if `index` is below
    /// [`HARDENED`] and the child is valid (BIP 32 skips the rare index
    /// that is not).
    #[must_use]
    pub fn child(&self, index: u32) -> Option<Self> {
        if index >= HARDENED {
            return None;
        }
        let parent = self.public()?;
        let (tweak, chain) = hmac(&self.chain, &[&parent.key, &index.to_be_bytes()]);
        let sum = scalar(&tweak)? + scalar(&self.secret)?;
        if bool::from(sum.is_zero()) {
            return None;
        }
        Some(Self {
            secret: sum.to_repr().into(),
            chain,
        })
    }
}

impl ExtendedPublic {
    /// The non-hardened child at `index`, from the public parent alone.
    #[must_use]
    pub fn child(&self, index: u32) -> Option<Self> {
        if index >= HARDENED {
            return None;
        }
        let parent = PublicKey::from_sec1_bytes(&self.key).ok()?;
        let (tweak, chain) = hmac(&self.chain, &[&self.key, &index.to_be_bytes()]);
        let point = ProjectivePoint::GENERATOR * scalar(&tweak)? + parent.to_projective();
        let child = PublicKey::from_affine(point.to_affine()).ok()?;
        Some(Self {
            key: compressed(&child)?,
            chain,
        })
    }
}

/// The two indices under which a payout set for the block at `height`
/// derives its keys: 31 bits each, so that every height below 2^62 has
/// its own (a hub at a block a millisecond would take a hundred million
/// years to pass it).
#[must_use]
pub fn payout_path(height: u64) -> (u32, u32) {
    let mask = u64::from(HARDENED - 1);
    let index = |bits: u64| u32::try_from(bits & mask).unwrap_or(0);
    (index(height >> 31), index(height))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hex32(text: &str) -> [u8; 32] {
        let mut out = [0; 32];
        for (i, byte) in out.iter_mut().enumerate() {
            *byte = u8::from_str_radix(&text[i * 2..i * 2 + 2], 16).unwrap();
        }
        out
    }

    /// BIP 32's test vector 1: from m/0H, the non-hardened child 1, both
    /// from the private and from the public parent.
    #[test]
    fn test_vector_one() {
        let parent = ExtendedPrivate {
            secret: hex32("edb2e14f9ee77d26dd93b4ecede8d16ed408ce149b6cd80b0715a2d911a0afea"),
            chain: hex32("47fdacbd0f1097043b78c63c20c34ef4ed9a111d980047ad16282c7ae6236141"),
        };
        let child = parent.child(1).unwrap();
        assert_eq!(
            child.secret,
            hex32("3c6cb8d0f6a264c91ea8b5030fadaa8e538b020f0a387421a12de9319dc93368")
        );
        assert_eq!(
            child.chain,
            hex32("2a7857631386ba23dacac34180dd1983734e444fdbf774041578e9b6adb37c19")
        );
        let public = parent.public().unwrap().child(1).unwrap();
        assert_eq!(public, child.public().unwrap());
        assert_eq!(public.key[0], 0x03);
        assert_eq!(
            public.key[1..],
            hex32("501e454bf00751f24b1b489aa925215d66af2234e3891c3b21a52bedb3cd711c")
        );
    }

    #[test]
    fn the_public_parent_derives_what_the_private_one_signs_with() {
        let master = ExtendedPrivate::master(&[7; 32]).unwrap();
        for height in [0, 1, 1 << 31, (1 << 62) - 1] {
            let (high, low) = payout_path(height);
            let private = master.child(high).and_then(|k| k.child(low)).unwrap();
            let public = master
                .public()
                .unwrap()
                .child(high)
                .and_then(|k| k.child(low))
                .unwrap();
            assert_eq!(private.public().unwrap(), public, "height {height}");
        }
        assert!(master.child(HARDENED).is_none());
    }
}
