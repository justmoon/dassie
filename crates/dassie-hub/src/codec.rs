//! What the hub's wire types share: the decode error, and readers and
//! writers for keys, signatures and digests. Every hub message is canonical
//! OER (spec/areas/encoding.md); integers are fixed width, so a SEQUENCE is
//! its fields in order.

use core::fmt;

use dassie_identity::{PublicKey, Signature};
use dassie_oer::{DecodeError, Reader, Writer};

use crate::Digest;

/// Why a hub message could not be decoded.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HubDecodeError {
    /// The OER itself was malformed or not canonical.
    Oer(DecodeError),
    /// A public key is not a valid Ed25519 point, or is weak.
    InvalidKey,
    /// A CHOICE tag names no alternative.
    UnknownChoice(u64),
    /// A value broke a rule of its type, such as a set of validators too
    /// small.
    Invalid(&'static str),
}

impl From<DecodeError> for HubDecodeError {
    fn from(error: DecodeError) -> Self {
        Self::Oer(error)
    }
}

impl fmt::Display for HubDecodeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Oer(error) => write!(f, "malformed OER at {}: {:?}", error.offset, error.kind),
            Self::InvalidKey => f.write_str("invalid or weak public key"),
            Self::UnknownChoice(tag) => write!(f, "unknown choice {tag}"),
            Self::Invalid(why) => f.write_str(why),
        }
    }
}

/// Encoded size of a public key.
pub const KEY_LEN: usize = 32;
/// Encoded size of a signature.
pub const SIGNATURE_LEN: usize = 64;
/// Encoded size of a digest.
pub const DIGEST_LEN: usize = 32;

/// Writes a public key.
pub fn write_key(writer: &mut Writer<'_>, key: &PublicKey) {
    writer.write_bytes(key.as_bytes());
}

/// Reads a public key, refusing weak ones: under ZIP-215 a weak key
/// verifies forged signatures (spec/areas/identity.md#node-key).
pub fn read_key(reader: &mut Reader<'_>) -> Result<PublicKey, HubDecodeError> {
    let key = PublicKey::from_bytes(reader.read_array::<32>()?)
        .map_err(|_| HubDecodeError::InvalidKey)?;
    if key.is_weak() {
        return Err(HubDecodeError::InvalidKey);
    }
    Ok(key)
}

/// Writes a signature.
pub fn write_signature(writer: &mut Writer<'_>, signature: &Signature) {
    writer.write_bytes(signature.as_bytes());
}

/// Reads a signature; whether it verifies is checked where it is used.
pub fn read_signature(reader: &mut Reader<'_>) -> Result<Signature, HubDecodeError> {
    Ok(Signature::from_bytes(*reader.read_array::<64>()?))
}

/// Writes a digest.
pub fn write_digest(writer: &mut Writer<'_>, digest: &Digest) {
    writer.write_bytes(digest.as_bytes());
}

/// Reads a digest.
pub fn read_digest(reader: &mut Reader<'_>) -> Result<Digest, HubDecodeError> {
    Ok(Digest(*reader.read_array::<32>()?))
}

/// Reads a boolean that must be canonical.
pub fn read_flag(reader: &mut Reader<'_>) -> Result<bool, HubDecodeError> {
    Ok(reader.read_bool()?)
}
