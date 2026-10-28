//! What the hub's wire types share: the decode error, and readers and
//! writers for keys, signatures and digests. Every hub message is canonical
//! OER (spec/areas/encoding.md); integers are fixed width, so a SEQUENCE is
//! its fields in order.

use core::fmt;

use dassie_identity::{
    NodeIdentity, PublicKey, SLH_DSA_PUBLIC_KEY_BYTES, Signature, SlhDsaPublicKey,
};
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

/// Encoded size of a node's identity: its Ed25519 key, then its SLH-DSA
/// key, which the rules name a node by, so that its node ID follows from
/// them (spec/areas/identity.md#node-id).
pub const NODE_LEN: usize = KEY_LEN + SLH_DSA_PUBLIC_KEY_BYTES;

/// Writes a node's identity.
pub fn write_node(writer: &mut Writer<'_>, node: &NodeIdentity) {
    write_key(writer, &node.key());
    writer.write_bytes(node.slh_dsa().as_bytes());
}

/// Reads a node's identity, refusing a weak Ed25519 key as [`read_key`]
/// does. Every 32 bytes are an SLH-DSA key.
pub fn read_node(reader: &mut Reader<'_>) -> Result<NodeIdentity, HubDecodeError> {
    let key = read_key(reader)?;
    let slh_dsa = SlhDsaPublicKey::from_bytes(*reader.read_array::<SLH_DSA_PUBLIC_KEY_BYTES>()?);
    Ok(NodeIdentity::new(key, slh_dsa))
}

/// Writes a signature.
pub fn write_signature(writer: &mut Writer<'_>, signature: &Signature) {
    writer.write_bytes(signature.as_bytes());
}

/// Reads a signature; whether it verifies is checked where it is used.
/// Its error is the OER one, so that the consensus messages, which have no
/// [`HubDecodeError`], read signatures with it too.
pub fn read_signature(reader: &mut Reader<'_>) -> Result<Signature, DecodeError> {
    Ok(Signature::from_bytes(
        *reader.read_array::<SIGNATURE_LEN>()?,
    ))
}

/// Writes a digest.
pub fn write_digest(writer: &mut Writer<'_>, digest: &Digest) {
    writer.write_bytes(digest.as_bytes());
}

/// Reads a digest, with the OER error as [`read_signature`] does.
pub fn read_digest(reader: &mut Reader<'_>) -> Result<Digest, DecodeError> {
    Ok(Digest(*reader.read_array::<DIGEST_LEN>()?))
}
