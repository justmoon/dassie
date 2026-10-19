//! Signature verification follows ZIP-215 (spec/areas/identity.md#node-key),
//! edge cases included. The cases are those of the ZIP-215 conformance
//! tests: every small-order public key and `R`, in canonical and
//! non-canonical encodings, with `s = 0`.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use curve25519_dalek::constants::EIGHT_TORSION;
use dassie_identity::{NodeKey, PublicKey, Signature};
use ed25519_dalek::Verifier;

/// The canonical encodings of the eight points of small order, and
/// non-canonical encodings of the two with `x = 0`: a set sign bit, and
/// `y = 1` written as `p + 1`.
fn small_order_encodings() -> Vec<[u8; 32]> {
    let mut encodings: Vec<[u8; 32]> = EIGHT_TORSION
        .iter()
        .map(|point| point.compress().to_bytes())
        .collect();
    let mut one_negative = [0u8; 32];
    one_negative[0] = 1;
    one_negative[31] = 0x80;
    let mut minus_one_negative = [0xffu8; 32];
    minus_one_negative[0] = 0xec;
    let mut one_plus_p = [0xffu8; 32];
    one_plus_p[0] = 0xee;
    one_plus_p[31] = 0x7f;
    let mut one_plus_p_negative = one_plus_p;
    one_plus_p_negative[31] = 0xff;
    encodings.extend([
        one_negative,
        minus_one_negative,
        one_plus_p,
        one_plus_p_negative,
    ]);
    encodings
}

/// ℓ, the order of the base point, little-endian.
const ORDER: [u8; 32] = [
    0xed, 0xd3, 0xf5, 0x5c, 0x1a, 0x63, 0x12, 0x58, 0xd6, 0x9c, 0xf7, 0xa2, 0xde, 0xf9, 0xde, 0x14,
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0x10,
];

fn signature(r: &[u8; 32], s: &[u8; 32]) -> Signature {
    let mut bytes = [0u8; 64];
    bytes[..32].copy_from_slice(r);
    bytes[32..].copy_from_slice(s);
    Signature::from_bytes(bytes)
}

// Under ZIP-215 the check is [8][s]B = [8]R + [8][k]A, so a small-order
// key and R with s = 0 verify for any message. ed25519-dalek's own verify,
// which the node used before, refuses many of them: two nodes on different
// rules would disagree about which link states are signed.
#[test]
fn small_order_cases_verify_as_zip215_says() {
    let encodings = small_order_encodings();
    let mut refused_by_dalek = 0;
    for a in &encodings {
        let key = PublicKey::from_bytes(a).expect("every small-order encoding decodes");
        assert!(key.is_weak(), "{a:02x?} is of small order");
        for r in &encodings {
            let signature = signature(r, &[0; 32]);
            assert_eq!(
                key.verify(b"Zcash", &signature),
                Ok(()),
                "{a:02x?} {r:02x?}"
            );
            let legacy = ed25519_dalek::VerifyingKey::from_bytes(a).unwrap();
            let legacy_signature = ed25519_dalek::Signature::from_bytes(signature.as_bytes());
            if legacy.verify(b"Zcash", &legacy_signature).is_err() {
                refused_by_dalek += 1;
            }
        }
    }
    assert!(refused_by_dalek > 0, "the cases tell the two rules apart");
}

// ZIP-215 still requires s < ℓ: s and s + ℓ would otherwise both verify.
#[test]
fn a_non_canonical_s_is_refused() {
    let key = NodeKey::from_bytes(&[1; 32]);
    let message = b"link state";
    let honest = key.sign(message);
    assert_eq!(key.public_key().verify(message, &honest), Ok(()));
    let bytes = honest.as_bytes();
    let mut s = [0u8; 32];
    s.copy_from_slice(&bytes[32..]);
    let mut carry = 0u16;
    for (byte, order) in s.iter_mut().zip(&ORDER) {
        let sum = u16::from(*byte) + u16::from(*order) + carry;
        *byte = sum.to_le_bytes()[0];
        carry = sum >> 8;
    }
    let mut r = [0u8; 32];
    r.copy_from_slice(&bytes[..32]);
    let shifted = signature(&r, &s);
    assert!(key.public_key().verify(message, &shifted).is_err());
}

// An honest key is not weak, and its signatures verify only over their
// message.
#[test]
fn honest_keys_are_strong() {
    let key = NodeKey::from_bytes(&[3; 32]);
    let public = key.public_key();
    assert!(!public.is_weak());
    let signature = key.sign(b"one");
    assert_eq!(public.verify(b"one", &signature), Ok(()));
    assert!(public.verify(b"two", &signature).is_err());
    assert_eq!(PublicKey::from_bytes(public.as_bytes()).unwrap(), public);
}
