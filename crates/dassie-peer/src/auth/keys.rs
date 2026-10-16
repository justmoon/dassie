//! Keys and MACs of `hmacSha256V1` (spec 7 §4.7).
//!
//! The key between two nodes comes from their static X25519 keys alone: a
//! scheme that only MACs gains nothing from an ephemeral key (D2). It is
//! expanded into four keys, one per direction and per request or response,
//! so that no MAC is valid in any other place.

use dassie_identity::{NodeId, NodeKey, PublicKey};
use hmac::{Hmac, KeyInit, Mac};
use sha2::Sha256;
use subtle::ConstantTimeEq;
use zeroize::Zeroize;

use crate::wire::{HmacAuthentication, VERSION};

/// The HKDF salt of v1.
const SALT: &[u8] = b"dassie peer v1";

/// A 32-byte MAC key.
#[derive(Clone)]
pub struct MacKey([u8; 32]);

impl Drop for MacKey {
    fn drop(&mut self) {
        self.0.zeroize();
    }
}

impl core::fmt::Debug for MacKey {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str("MacKey(..)")
    }
}

/// The keys between this node and one other.
#[derive(Clone, Debug)]
pub struct PairKeys {
    /// The other node's key.
    pub peer: PublicKey,
    /// For our requests to it.
    pub outgoing_request: MacKey,
    /// For its responses to our requests.
    pub outgoing_response: MacKey,
    /// For its requests to us.
    pub incoming_request: MacKey,
    /// For our responses to its requests.
    pub incoming_response: MacKey,
}

/// Why no keys can be made with a node.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NonContributory;

impl PairKeys {
    /// Derives the keys between `own` and `peer`.
    ///
    /// # Errors
    ///
    /// The exchange gives the all-zero secret, which a key of small order
    /// does; such keys are refused before this.
    pub fn derive(own: &NodeKey, peer: &PublicKey) -> Result<Self, NonContributory> {
        let shared = own
            .x25519_secret()
            .diffie_hellman(&peer.to_x25519())
            .map_err(|_| NonContributory)?;
        let mut pair = hmac(SALT, &[shared.as_bytes()]);
        let own_key = own.public_key();
        let (us, them) = (own_key.as_bytes(), peer.as_bytes());
        let keys = Self {
            peer: *peer,
            outgoing_request: MacKey(hmac(&pair, &[b"request", us, them, &[1]])),
            outgoing_response: MacKey(hmac(&pair, &[b"response", us, them, &[1]])),
            incoming_request: MacKey(hmac(&pair, &[b"request", them, us, &[1]])),
            incoming_response: MacKey(hmac(&pair, &[b"response", them, us, &[1]])),
        };
        pair.zeroize();
        Ok(keys)
    }
}

/// HMAC-SHA256 over the concatenation of `parts`. With a part ending in
/// `0x01`, this is HKDF-Expand for one block of 32 bytes; with the salt as
/// key, HKDF-Extract (RFC 5869).
// HMAC takes keys of any length, so this cannot panic.
#[allow(clippy::panic)]
fn hmac(key: &[u8], parts: &[&[u8]]) -> [u8; 32] {
    let Ok(mut mac) = Hmac::<Sha256>::new_from_slice(key) else {
        panic!("HMAC accepts keys of any length");
    };
    for part in parts {
        mac.update(part);
    }
    mac.finalize().into_bytes().into()
}

/// The MAC of a request from `sender` to `recipient`, over everything but
/// the MAC itself.
#[must_use]
pub fn request_mac(
    key: &MacKey,
    sender: &NodeId,
    recipient: &NodeId,
    authentication: &HmacAuthentication,
    content: &[u8],
) -> [u8; 32] {
    let (sender, recipient) = (sender.as_str().as_bytes(), recipient.as_str().as_bytes());
    hmac(
        &key.0,
        &[
            &[VERSION],
            &length(sender),
            sender,
            &length(recipient),
            recipient,
            &authentication.session_id,
            &authentication.counter.to_be_bytes(),
            &authentication.timestamp_ms.to_be_bytes(),
            content,
        ],
    )
}

/// The MAC of a response, bound to its request's MAC.
#[must_use]
pub fn response_mac(key: &MacKey, request_mac: &[u8; 32], body: &[u8]) -> [u8; 32] {
    hmac(&key.0, &[request_mac, body])
}

/// Compares two MACs in constant time (NET-10).
#[must_use]
pub fn macs_match(a: &[u8; 32], b: &[u8; 32]) -> bool {
    a.ct_eq(b).into()
}

/// The OER length determinant of a node ID, which is at most 45 bytes and
/// so one byte.
fn length(id: &[u8]) -> [u8; 1] {
    [u8::try_from(id.len()).unwrap_or(u8::MAX)]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn keys() -> (NodeKey, NodeKey) {
        (NodeKey::from_bytes(&[1; 32]), NodeKey::from_bytes(&[3; 32]))
    }

    // Both ends derive the same keys, each seeing the other's direction.
    #[test]
    fn both_ends_agree() {
        let (a, b) = keys();
        let at_a = PairKeys::derive(&a, &b.public_key()).unwrap();
        let at_b = PairKeys::derive(&b, &a.public_key()).unwrap();
        assert_eq!(at_a.outgoing_request.0, at_b.incoming_request.0);
        assert_eq!(at_a.outgoing_response.0, at_b.incoming_response.0);
        assert_eq!(at_a.incoming_request.0, at_b.outgoing_request.0);
        // No two of the four are the same: a MAC is valid in one place.
        let all = [
            at_a.outgoing_request.0,
            at_a.outgoing_response.0,
            at_a.incoming_request.0,
            at_a.incoming_response.0,
        ];
        for (i, x) in all.iter().enumerate() {
            for y in &all[i + 1..] {
                assert_ne!(x, y);
            }
        }
    }
}
