//! STREAM cryptography (RFC 29 §4): key derivation, AES-256-GCM and the
//! fulfillments that tie an ILP condition to an encrypted packet.
//!
//! The rest of the crate goes through [`Keys`] and [`Cipher`] and never
//! touches a primitive directly, so a simulation can later swap them for a
//! cheaper implementation in one place (D16).

use core::fmt;

use aes_gcm::Aes256Gcm;
use aes_gcm::aead::{AeadInOut, KeyInit, Nonce, Tag};
use hmac::{Hmac, Mac};
use rand_core::CryptoRng;
use sha2::{Digest, Sha256};

/// AES-GCM nonce length.
pub const IV_LENGTH: usize = 12;
/// AES-GCM tag length.
pub const TAG_LENGTH: usize = 16;
/// Bytes that encryption adds in front of the plaintext: nonce and tag.
pub const OVERHEAD: usize = IV_LENGTH + TAG_LENGTH;

const ENCRYPTION_KEY_LABEL: &[u8] = b"ilp_stream_encryption";
const FULFILLMENT_KEY_LABEL: &[u8] = b"ilp_stream_fulfillment";

/// HMAC-SHA256 of `message` under `key`.
#[must_use]
// HMAC takes keys of any length, so this cannot panic.
#[allow(clippy::unreachable)]
pub fn hmac_sha256(key: &[u8], message: &[u8]) -> [u8; 32] {
    let Ok(mut mac) = <Hmac<Sha256> as hmac::KeyInit>::new_from_slice(key) else {
        unreachable!("HMAC accepts keys of any length");
    };
    mac.update(message);
    mac.finalize().into_bytes().into()
}

/// SHA-256 of `data`.
#[must_use]
pub fn sha256(data: &[u8]) -> [u8; 32] {
    Sha256::digest(data).into()
}

/// The ciphertext was tampered with, truncated, or encrypted under another
/// key. A value rather than an exception (ILP-13).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DecryptionFailure;

/// AES-256-GCM under one key, in the STREAM layout: nonce, tag, ciphertext.
pub struct Cipher {
    cipher: Aes256Gcm,
}

impl Cipher {
    /// A cipher under `key`.
    #[must_use]
    pub fn new(key: &[u8; 32]) -> Self {
        Self {
            cipher: Aes256Gcm::new(&(*key).into()),
        }
    }

    /// Encrypts in place: `buffer[OVERHEAD..OVERHEAD + plaintext_len]` holds
    /// the plaintext, and on return `buffer[..OVERHEAD + plaintext_len]`
    /// holds nonce, tag and ciphertext. Returns that length.
    ///
    /// Callers encode the plaintext straight into the buffer after the
    /// overhead, so a packet is built and encrypted without copies.
    pub fn encrypt(
        &self,
        rng: &mut (impl CryptoRng + ?Sized),
        plaintext_len: usize,
        buffer: &mut [u8],
    ) -> usize {
        let mut iv = [0; IV_LENGTH];
        rng.fill_bytes(&mut iv);
        self.encrypt_with_iv(iv, plaintext_len, buffer)
    }

    /// [`encrypt`](Self::encrypt) with a given nonce. Reusing a nonce under
    /// the same key breaks AES-GCM, so outside tests use `encrypt`.
    // Cannot panic: ILP's data field is far below GCM's plaintext limit.
    #[allow(clippy::unreachable)]
    pub fn encrypt_with_iv(
        &self,
        iv: [u8; IV_LENGTH],
        plaintext_len: usize,
        buffer: &mut [u8],
    ) -> usize {
        let total = OVERHEAD + plaintext_len;
        assert!(buffer.len() >= total, "encryption buffer too small");
        let (header, rest) = buffer.split_at_mut(OVERHEAD);
        let (plaintext, _) = rest.split_at_mut(plaintext_len);
        let nonce = Nonce::<Aes256Gcm>::from(iv);
        // Fails only for plaintexts beyond GCM's 64 GiB limit, which ILP's
        // 32 KiB data field rules out.
        let Ok(tag) = self
            .cipher
            .encrypt_inout_detached(&nonce, &[], plaintext.into())
        else {
            unreachable!("AES-GCM plaintext limit exceeded");
        };
        let (iv_slot, tag_slot) = header.split_at_mut(IV_LENGTH);
        iv_slot.copy_from_slice(&iv);
        tag_slot.copy_from_slice(&tag);
        total
    }

    /// Decrypts `ciphertext` into `output` and returns the plaintext, which
    /// is `ciphertext.len() - OVERHEAD` bytes long.
    pub fn decrypt<'b>(
        &self,
        ciphertext: &[u8],
        output: &'b mut [u8],
    ) -> Result<&'b [u8], DecryptionFailure> {
        let (Some(header), Some(body)) = (ciphertext.get(..OVERHEAD), ciphertext.get(OVERHEAD..))
        else {
            return Err(DecryptionFailure);
        };
        let plaintext = output.get_mut(..body.len()).ok_or(DecryptionFailure)?;
        plaintext.copy_from_slice(body);
        let (iv, tag) = header.split_at(IV_LENGTH);
        let nonce = Nonce::<Aes256Gcm>::try_from(iv).map_err(|_| DecryptionFailure)?;
        let tag = Tag::<Aes256Gcm>::try_from(tag).map_err(|_| DecryptionFailure)?;
        self.cipher
            .decrypt_inout_detached(&nonce, &[], (&mut *plaintext).into(), &tag)
            .map_err(|_| DecryptionFailure)?;
        Ok(plaintext)
    }
}

/// The keys derived from a connection's shared secret.
pub struct Keys {
    /// Encrypts and decrypts the connection's STREAM packets.
    pub cipher: Cipher,
    fulfillment_key: [u8; 32],
}

impl Keys {
    /// Derives the encryption and fulfillment keys from `shared_secret`.
    #[must_use]
    pub fn derive(shared_secret: &[u8]) -> Self {
        Self {
            cipher: Cipher::new(&Self::encryption_key(shared_secret)),
            fulfillment_key: hmac_sha256(shared_secret, FULFILLMENT_KEY_LABEL),
        }
    }

    /// The encryption key for `shared_secret`.
    #[must_use]
    pub fn encryption_key(shared_secret: &[u8]) -> [u8; 32] {
        hmac_sha256(shared_secret, ENCRYPTION_KEY_LABEL)
    }

    /// The fulfillment key, for tests against vectors.
    #[must_use]
    pub const fn fulfillment_key(&self) -> &[u8; 32] {
        &self.fulfillment_key
    }

    /// The fulfillment for an ILP Prepare whose data is `ciphertext`.
    #[must_use]
    pub fn fulfillment(&self, ciphertext: &[u8]) -> [u8; 32] {
        hmac_sha256(&self.fulfillment_key, ciphertext)
    }

    /// The condition for an ILP Prepare whose data is `ciphertext`.
    #[must_use]
    pub fn condition(&self, ciphertext: &[u8]) -> [u8; 32] {
        sha256(&self.fulfillment(ciphertext))
    }
}

impl fmt::Debug for Cipher {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Cipher(..)")
    }
}

impl fmt::Debug for Keys {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Keys(..)")
    }
}
