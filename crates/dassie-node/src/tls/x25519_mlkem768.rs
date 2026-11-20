//! The hybrid key exchange X25519MLKEM768 (draft-ietf-tls-ecdhe-mlkem) on
//! ring's X25519 and RustCrypto's ML-KEM-768, which rustls has only with
//! aws-lc-rs (spec/decisions/hybrid-key-exchange.md).
//!
//! A port of rustls's own (`crypto/aws_lc_rs/pq/hybrid.rs`), for the one
//! group. ML-KEM comes first in each share and in the secret: the client
//! sends its encapsulation key and its X25519 key, the server its ciphertext
//! and its X25519 key, and the secret is ML-KEM's shared key and X25519's.

use ml_kem::array::Array;
use ml_kem::ml_kem_768::{Ciphertext, DecapsulationKey, EncapsulationKey};
use ml_kem::{Decapsulate as _, KeyExport as _, SharedKey};
use ring::rand::{SecureRandom as _, SystemRandom};
use rustls::crypto::ring::kx_group::X25519;
use rustls::crypto::{ActiveKeyExchange, CompletedKeyExchange, SharedSecret, SupportedKxGroup};
use rustls::ffdhe_groups::FfdheGroup;
use rustls::{Error, NamedGroup, PeerMisbehaved, ProtocolVersion};
use zeroize::Zeroizing;

/// The group, first in [`super::provider`]'s list.
pub static X25519MLKEM768: &dyn SupportedKxGroup = &Hybrid;

/// Bytes of an X25519 public key.
const X25519_BYTES: usize = 32;
/// Bytes of an ML-KEM-768 encapsulation key: the post-quantum half of the
/// client's share.
pub const ENCAPSULATION_KEY_BYTES: usize = 1_184;
/// Bytes of an ML-KEM-768 ciphertext: the post-quantum half of the server's
/// share.
pub const CIPHERTEXT_BYTES: usize = 1_088;
/// Bytes of the client's share.
pub const CLIENT_SHARE_BYTES: usize = ENCAPSULATION_KEY_BYTES + X25519_BYTES;
/// Bytes of the server's share.
pub const SERVER_SHARE_BYTES: usize = CIPHERTEXT_BYTES + X25519_BYTES;
/// Bytes of the shared secret: ML-KEM's 32 and X25519's 32.
pub const SECRET_BYTES: usize = 64;

const INVALID_KEY_SHARE: Error = Error::PeerMisbehaved(PeerMisbehaved::InvalidKeyShare);

/// A decapsulation key from its 64-byte seed, `d` then `z` (FIPS 203,
/// `ML-KEM.KeyGen_internal`), and its encapsulation key's bytes.
#[must_use]
pub fn keygen(seed: &[u8; 64]) -> (DecapsulationKey, Vec<u8>) {
    let key = DecapsulationKey::from_seed(Array::from(*seed));
    let encapsulation_key = key.encapsulation_key().to_bytes().to_vec();
    (key, encapsulation_key)
}

/// Encapsulates to `encapsulation_key` with the 32 random bytes `m` (FIPS
/// 203, `ML-KEM.Encaps_internal`): the ciphertext and the shared key, or
/// `None` for a key of the wrong length or one that fails the standard's
/// modulus check.
#[must_use]
pub fn encapsulate(encapsulation_key: &[u8], m: &[u8; 32]) -> Option<(Vec<u8>, SharedKey)> {
    let bytes = Array::try_from(encapsulation_key).ok()?;
    let key = EncapsulationKey::new(&bytes).ok()?;
    let (ciphertext, shared) = key.encapsulate_deterministic(&Array::from(*m));
    Some((ciphertext.to_vec(), shared))
}

/// Decapsulates `ciphertext`: the shared key, or a pseudorandom one for a
/// ciphertext not made for `key` (implicit rejection); `None` for one of the
/// wrong length.
#[must_use]
pub fn decapsulate(key: &DecapsulationKey, ciphertext: &[u8]) -> Option<SharedKey> {
    let ciphertext = Ciphertext::try_from(ciphertext).ok()?;
    Some(key.decapsulate(&ciphertext))
}

/// `N` bytes from ring's generator, the one its X25519 draws from.
fn random<const N: usize>() -> Result<Zeroizing<[u8; N]>, Error> {
    let mut bytes = Zeroizing::new([0_u8; N]);
    SystemRandom::new()
        .fill(bytes.as_mut())
        .map_err(|_| Error::FailedToGetRandomBytes)?;
    Ok(bytes)
}

fn concat(post_quantum: &[u8], classical: &[u8]) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(post_quantum.len() + classical.len());
    bytes.extend_from_slice(post_quantum);
    bytes.extend_from_slice(classical);
    bytes
}

#[derive(Debug)]
struct Hybrid;

impl SupportedKxGroup for Hybrid {
    fn start(&self) -> Result<Box<dyn ActiveKeyExchange>, Error> {
        let classical = X25519.start()?;
        let (key, encapsulation_key) = keygen(&*random::<64>()?);
        let share = concat(&encapsulation_key, classical.pub_key());
        Ok(Box::new(Active {
            classical,
            key,
            share,
        }))
    }

    fn start_and_complete(&self, client_share: &[u8]) -> Result<CompletedKeyExchange, Error> {
        if client_share.len() != CLIENT_SHARE_BYTES {
            return Err(INVALID_KEY_SHARE);
        }
        let (encapsulation_key, classical_share) = client_share.split_at(ENCAPSULATION_KEY_BYTES);
        let (ciphertext, post_quantum) =
            encapsulate(encapsulation_key, &*random::<32>()?).ok_or(INVALID_KEY_SHARE)?;
        let classical = X25519.start_and_complete(classical_share)?;
        Ok(CompletedKeyExchange {
            group: NamedGroup::X25519MLKEM768,
            pub_key: concat(&ciphertext, &classical.pub_key),
            secret: SharedSecret::from(concat(&post_quantum, classical.secret.secret_bytes())),
        })
    }

    fn ffdhe_group(&self) -> Option<FfdheGroup<'static>> {
        None
    }

    fn name(&self) -> NamedGroup {
        NamedGroup::X25519MLKEM768
    }

    fn usable_for_version(&self, version: ProtocolVersion) -> bool {
        version == ProtocolVersion::TLSv1_3
    }
}

/// A client's exchange, waiting for the server's share.
struct Active {
    classical: Box<dyn ActiveKeyExchange>,
    key: DecapsulationKey,
    share: Vec<u8>,
}

impl ActiveKeyExchange for Active {
    fn complete(self: Box<Self>, server_share: &[u8]) -> Result<SharedSecret, Error> {
        if server_share.len() != SERVER_SHARE_BYTES {
            return Err(INVALID_KEY_SHARE);
        }
        let (ciphertext, classical_share) = server_share.split_at(CIPHERTEXT_BYTES);
        let post_quantum = decapsulate(&self.key, ciphertext).ok_or(INVALID_KEY_SHARE)?;
        let classical = self.classical.complete(classical_share)?;
        Ok(SharedSecret::from(concat(
            &post_quantum,
            classical.secret_bytes(),
        )))
    }

    /// The X25519 half, which rustls sends as a share of its own beside
    /// the hybrid one, so that a server without the hybrid picks X25519
    /// without a HelloRetryRequest.
    fn hybrid_component(&self) -> Option<(NamedGroup, &[u8])> {
        Some((self.classical.group(), self.classical.pub_key()))
    }

    fn complete_hybrid_component(
        self: Box<Self>,
        server_share: &[u8],
    ) -> Result<SharedSecret, Error> {
        self.classical.complete(server_share)
    }

    fn pub_key(&self) -> &[u8] {
        &self.share
    }

    fn ffdhe_group(&self) -> Option<FfdheGroup<'static>> {
        None
    }

    fn group(&self) -> NamedGroup {
        NamedGroup::X25519MLKEM768
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::indexing_slicing)]

    use proptest::prelude::*;

    use super::*;

    #[test]
    fn client_and_server_agree() {
        let client = X25519MLKEM768.start().unwrap();
        assert_eq!(client.pub_key().len(), CLIENT_SHARE_BYTES);
        assert_eq!(client.group(), NamedGroup::X25519MLKEM768);
        let server = X25519MLKEM768.start_and_complete(client.pub_key()).unwrap();
        assert_eq!(server.group, NamedGroup::X25519MLKEM768);
        assert_eq!(server.pub_key.len(), SERVER_SHARE_BYTES);
        let secret = client.complete(&server.pub_key).unwrap();
        assert_eq!(secret.secret_bytes().len(), SECRET_BYTES);
        assert_eq!(secret.secret_bytes(), server.secret.secret_bytes());
    }

    // The X25519 share rustls sends beside the hybrid one is the hybrid's
    // own X25519 half, and completes against a server of X25519 alone.
    #[test]
    fn the_x25519_half_completes_alone() {
        let client = X25519MLKEM768.start().unwrap();
        let (group, share) = client.hybrid_component().unwrap();
        assert_eq!(group, NamedGroup::X25519);
        assert_eq!(share, &client.pub_key()[ENCAPSULATION_KEY_BYTES..]);
        let server = X25519.start_and_complete(share).unwrap();
        let secret = client.complete_hybrid_component(&server.pub_key).unwrap();
        assert_eq!(secret.secret_bytes(), server.secret.secret_bytes());
    }

    #[test]
    fn shares_of_the_wrong_length_are_refused() {
        let client = X25519MLKEM768.start().unwrap();
        let share = client.pub_key();
        for length in [CLIENT_SHARE_BYTES - 1, CLIENT_SHARE_BYTES + 1] {
            let mut wrong = share.to_vec();
            wrong.resize(length, 0);
            assert_eq!(
                X25519MLKEM768.start_and_complete(&wrong).err(),
                Some(INVALID_KEY_SHARE)
            );
        }
        let server = X25519MLKEM768.start_and_complete(share).unwrap();
        for length in [SERVER_SHARE_BYTES - 1, SERVER_SHARE_BYTES + 1] {
            let client = X25519MLKEM768.start().unwrap();
            let mut wrong = server.pub_key.clone();
            wrong.resize(length, 0);
            assert_eq!(client.complete(&wrong).err(), Some(INVALID_KEY_SHARE));
        }
    }

    // FIPS 203's encapsulation key check: a coefficient of 4095 is not
    // below q = 3329.
    #[test]
    fn an_encapsulation_key_out_of_range_is_refused() {
        let client = X25519MLKEM768.start().unwrap();
        let mut share = client.pub_key().to_vec();
        share[0] = 0xff;
        share[1] |= 0x0f;
        assert_eq!(
            X25519MLKEM768.start_and_complete(&share).err(),
            Some(INVALID_KEY_SHARE)
        );
    }

    // A ciphertext not made for the key gives another secret, not an
    // error: ML-KEM's implicit rejection.
    #[test]
    fn a_changed_ciphertext_gives_another_secret() {
        let client = X25519MLKEM768.start().unwrap();
        let mut server = X25519MLKEM768.start_and_complete(client.pub_key()).unwrap();
        server.pub_key[0] ^= 1;
        let secret = client.complete(&server.pub_key).unwrap();
        assert_ne!(secret.secret_bytes(), server.secret.secret_bytes());
        assert_eq!(
            secret.secret_bytes()[32..],
            server.secret.secret_bytes()[32..]
        );
    }

    proptest! {
        #![proptest_config(ProptestConfig { cases: 64, ..ProptestConfig::default() })]

        // A peer sends any share: it completes or is refused, without a
        // panic (spec/decisions/tigerstyle.md).
        #[test]
        fn any_client_share_is_completed_or_refused(share in prop_oneof![
            proptest::collection::vec(any::<u8>(), 0..1_300),
            proptest::collection::vec(any::<u8>(), CLIENT_SHARE_BYTES),
        ]) {
            let _ = X25519MLKEM768.start_and_complete(&share);
        }

        #[test]
        fn any_server_share_is_completed_or_refused(share in prop_oneof![
            proptest::collection::vec(any::<u8>(), 0..1_200),
            proptest::collection::vec(any::<u8>(), SERVER_SHARE_BYTES),
        ]) {
            let _ = X25519MLKEM768.start().unwrap().complete(&share);
        }
    }
}
