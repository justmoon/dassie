//! Signing as minisign does, for tests only: with an unencrypted secret
//! key (`minisign -G -W`), such as the test key of `testdata/release`.
//!
//! A secret key file is `untrusted comment: …` and the base64 of
//! `Ed ‖ kdf (0 0 when unencrypted) ‖ checksum algorithm ‖ salt (32) ‖
//! opslimit (8) ‖ memlimit (8) ‖ key ID (8) ‖ Ed25519 seed and public key
//! (64) ‖ checksum (32)`.

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use blake2::{Blake2b512, Digest as _};
use ed25519_dalek::{Signer as _, SigningKey};

/// The test key's secret half, as `minisign -G -W` wrote it.
pub const TEST_SECRET_KEY: &str = include_str!("../../../testdata/release/test.key");
/// Its public half.
pub const TEST_PUBLIC_KEY: &str = include_str!("../../../testdata/release/test.pub");

/// Where the key ID starts in a decoded secret key.
const KEY_ID: usize = 2 + 2 + 2 + 32 + 8 + 8;

/// Signs `file` as `minisign -S -t <comment>` would, with an unencrypted
/// secret key.
///
/// # Panics
///
/// If `secret_key` is not such a key: this is for tests.
#[allow(clippy::expect_used, clippy::indexing_slicing)]
pub fn sign(secret_key: &str, file: &[u8], comment: &str) -> String {
    let encoded = secret_key
        .lines()
        .nth(1)
        .expect("a minisign secret key has two lines");
    let bytes = STANDARD.decode(encoded).expect("the secret key is base64");
    assert_eq!(&bytes[..4], b"Ed\0\0", "an unencrypted Ed25519 key");
    let id = &bytes[KEY_ID..KEY_ID + 8];
    let seed: [u8; 32] = bytes[KEY_ID + 8..KEY_ID + 40].try_into().expect("32 bytes");
    let key = SigningKey::from_bytes(&seed);
    let signature = key.sign(Blake2b512::digest(file).as_slice()).to_bytes();
    let mut blob = Vec::from(*b"ED");
    blob.extend_from_slice(id);
    blob.extend_from_slice(&signature);
    let mut global = Vec::from(signature);
    global.extend_from_slice(comment.as_bytes());
    let global = key.sign(&global).to_bytes();
    format!(
        "untrusted comment: signature from minisign secret key\n{}\ntrusted comment: {comment}\n{}\n",
        STANDARD.encode(blob),
        STANDARD.encode(global)
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{PublicKey, Version, trusted_comment, verify_sums};

    #[test]
    fn signs_as_minisign_does() {
        let sums = include_bytes!("../../../testdata/release/SHA256SUMS");
        let version = Version::parse("0.1.0").unwrap_or_else(|e| panic!("{e}"));
        let signature = sign(TEST_SECRET_KEY, sums, &trusted_comment(&version));
        // Ed25519 is deterministic: the same bytes as the minisign CLI's.
        assert_eq!(
            signature,
            include_str!("../../../testdata/release/SHA256SUMS.minisig")
        );
        let key = PublicKey::parse(TEST_PUBLIC_KEY).unwrap_or_else(|e| panic!("{e}"));
        assert!(verify_sums(&key, sums, &signature, &version).is_ok());
    }
}
