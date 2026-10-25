//! The endpoint's connection IDs: a random nonce, signed with an HMAC under
//! a key derived from the node key, so that the node knows the IDs it made
//! before a restart as its own (spec/areas/peer-protocol.md#quic).
//!
//! quinn's own `HashedConnectionIdGenerator` signs with FxHash, which is no
//! MAC: two IDs give its key away. With a key that outlives the process,
//! anyone holding it could tell every ID of the node for good, across
//! restarts and networks. An HMAC gives its key away to nobody, so the IDs
//! stay as unlinkable as random ones to anyone without it.

use std::time::Duration;

use quinn_proto::{ConnectionId, ConnectionIdGenerator, InvalidCid};
use ring::hmac;

use rand_core::UnwrapErr;

/// Random bytes at the front of an ID, as quinn's generator has.
const NONCE_BYTES: usize = 3;
/// Bytes of the HMAC after them. A stranger's ID passes for one of ours
/// once in 2^40, which only costs a packet's handling.
const TAG_BYTES: usize = 5;
/// An ID's length: quinn's default.
const ID_BYTES: usize = NONCE_BYTES + TAG_BYTES;

/// Makes and recognises the endpoint's connection IDs.
pub struct SignedIds {
    key: hmac::Key,
    /// The OS RNG, which panics if it fails, as the node's does
    /// (`crate::clock::os_rng`): quinn asks for an ID it cannot be refused.
    rng: UnwrapErr<getrandom::SysRng>,
}

impl SignedIds {
    /// IDs signed with `secret`, a key that hangs at a path of its own.
    #[must_use]
    pub fn new(secret: &[u8]) -> Self {
        Self {
            key: hmac::Key::new(hmac::HMAC_SHA256, secret),
            rng: UnwrapErr(getrandom::SysRng),
        }
    }

    fn tag(&self, nonce: &[u8]) -> [u8; TAG_BYTES] {
        let mut tag = [0; TAG_BYTES];
        for (byte, signed) in tag.iter_mut().zip(hmac::sign(&self.key, nonce).as_ref()) {
            *byte = *signed;
        }
        tag
    }
}

impl ConnectionIdGenerator for SignedIds {
    fn generate_cid(&mut self) -> ConnectionId {
        let mut id = [0; ID_BYTES];
        rand_core::Rng::fill_bytes(&mut self.rng, &mut id[..NONCE_BYTES]);
        let tag = self.tag(&id[..NONCE_BYTES]);
        id[NONCE_BYTES..].copy_from_slice(&tag);
        ConnectionId::new(&id)
    }

    fn validate(&self, id: &ConnectionId) -> Result<(), InvalidCid> {
        if id.len() != ID_BYTES {
            return Err(InvalidCid);
        }
        let (nonce, tag) = id.split_at(NONCE_BYTES);
        // Not in constant time: a tag that fails costs the sender only a
        // dropped packet, and one that passes only a stateless reset.
        if self.tag(nonce)[..] == tag[..] {
            Ok(())
        } else {
            Err(InvalidCid)
        }
    }

    fn cid_len(&self) -> usize {
        ID_BYTES
    }

    fn cid_lifetime(&self) -> Option<Duration> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_are_known_only_under_their_key() {
        let mut ours = SignedIds::new(&[1; 32]);
        let again = SignedIds::new(&[1; 32]);
        let other = SignedIds::new(&[2; 32]);
        for _ in 0..64 {
            let id = ours.generate_cid();
            assert_eq!(id.len(), ID_BYTES);
            assert!(again.validate(&id).is_ok());
            assert!(other.validate(&id).is_err());
        }
        assert!(again.validate(&ConnectionId::new(&[0; 4])).is_err());
    }
}
