//! Stateless server credentials, compatible with `ilp-protocol-stream`.
//!
//! A server hands each client an address `<server>.<token>` and a shared
//! secret. The token is the connection's parameters encrypted under the
//! server secret, and the shared secret is an HMAC of the token, so the
//! server needs no table of issued credentials: they survive restarts and
//! never accumulate. The original kept credentials in memory, never expiring
//! and without tags (spec 4 §11.4); this is the upstream scheme instead.

use core::fmt;

use dassie_oer::{DecodeError, Reader, Size, Writer, size};
use rand_core::CryptoRng;

use crate::crypto::{Cipher, OVERHEAD, hmac_sha256};
use crate::receipt::ReceiptSecret;

/// Random bytes that make each token unique.
pub const TOKEN_NONCE_LENGTH: usize = 18;
/// Longest connection tag. Upstream sets no limit; tags are short labels.
pub const MAX_CONNECTION_TAG_LENGTH: usize = 128;
/// Longest encrypted token.
pub const MAX_TOKEN_LENGTH: usize = OVERHEAD
    + TOKEN_NONCE_LENGTH
    + size::prefixed(MAX_CONNECTION_TAG_LENGTH)
    + size::prefixed(16)
    + size::prefixed(32);

const SHARED_SECRET_LABEL: &[u8] = b"ilp_stream_shared_secret";

/// The server's master secret, from which every connection's shared secret
/// is derived.
pub struct ServerSecret {
    key: [u8; 32],
    cipher: Cipher,
}

/// What a token carries.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TokenContents<'a> {
    /// An application label for the connection, for example the incoming
    /// payment it belongs to. At most [`MAX_CONNECTION_TAG_LENGTH`] octets.
    pub connection_tag: &'a [u8],
    /// Receipt parameters, if the receiver should issue receipts.
    pub receipt: Option<ReceiptSecret>,
}

/// The token was not issued by this server, or is malformed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct InvalidToken;

impl From<DecodeError> for InvalidToken {
    fn from(_: DecodeError) -> Self {
        Self
    }
}

impl ServerSecret {
    /// Wraps a 32-byte server secret.
    #[must_use]
    pub fn new(key: [u8; 32]) -> Self {
        Self {
            key,
            cipher: Cipher::new(&key),
        }
    }

    /// Issues a token carrying `contents` into `output` and returns its
    /// length.
    ///
    /// # Panics
    ///
    /// If the connection tag is longer than [`MAX_CONNECTION_TAG_LENGTH`].
    pub fn issue_token(
        &self,
        rng: &mut (impl CryptoRng + ?Sized),
        contents: &TokenContents<'_>,
        output: &mut [u8; MAX_TOKEN_LENGTH],
    ) -> usize {
        assert!(
            contents.connection_tag.len() <= MAX_CONNECTION_TAG_LENGTH,
            "connection tag too long"
        );
        let (nonce, secret): (&[u8], &[u8]) = match &contents.receipt {
            Some(receipt) => (&receipt.nonce, &receipt.secret),
            None => (&[], &[]),
        };
        let plaintext_len = TOKEN_NONCE_LENGTH
            + size::prefixed(contents.connection_tag.len())
            + size::prefixed(nonce.len())
            + size::prefixed(secret.len());
        let (_, body) = output.split_at_mut(OVERHEAD);
        let mut writer = Writer::new(body);
        let mut token_nonce = [0u8; TOKEN_NONCE_LENGTH];
        rng.fill_bytes(&mut token_nonce);
        writer.write_bytes(&token_nonce);
        writer.write_octets(contents.connection_tag, Size::ANY);
        writer.write_octets(nonce, Size::ANY);
        writer.write_octets(secret, Size::ANY);
        assert!(writer.position() == plaintext_len, "token length mismatch");
        self.cipher.encrypt(rng, plaintext_len, output)
    }

    /// The shared secret of the connection that `token` identifies.
    #[must_use]
    pub fn shared_secret(&self, token: &[u8]) -> [u8; 32] {
        let generator = hmac_sha256(&self.key, SHARED_SECRET_LABEL);
        hmac_sha256(&generator, token)
    }

    /// Decrypts `token`, using `scratch` for the plaintext the result
    /// borrows from.
    pub fn open_token<'b>(
        &self,
        token: &[u8],
        scratch: &'b mut [u8; MAX_TOKEN_LENGTH],
    ) -> Result<TokenContents<'b>, InvalidToken> {
        if token.len() > MAX_TOKEN_LENGTH {
            return Err(InvalidToken);
        }
        let plaintext = self
            .cipher
            .decrypt(token, scratch)
            .map_err(|_| InvalidToken)?;
        let mut reader = Reader::new(plaintext);
        reader.read_bytes(TOKEN_NONCE_LENGTH)?;
        let connection_tag = reader.read_octets(Size::range(0, MAX_CONNECTION_TAG_LENGTH))?;
        let nonce = reader.read_octets(Size::ANY)?;
        let secret = reader.read_octets(Size::ANY)?;
        reader.finish()?;
        let receipt = match (nonce.len(), secret.len()) {
            (0, 0) => None,
            (16, 32) => Some(ReceiptSecret {
                nonce: nonce.try_into().map_err(|_| InvalidToken)?,
                secret: secret.try_into().map_err(|_| InvalidToken)?,
            }),
            _ => return Err(InvalidToken),
        };
        Ok(TokenContents {
            connection_tag,
            receipt,
        })
    }
}

impl fmt::Debug for ServerSecret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("ServerSecret(..)")
    }
}
