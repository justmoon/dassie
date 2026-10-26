//! The envelope of a request: the version, then the content. Who sent it
//! is the node whose key the QUIC connection proved, which nothing in the
//! bytes says (spec/areas/peer-protocol.md#authentication).

use alloc::vec::Vec;

use dassie_oer::{DecodeError, DecodeErrorKind, Reader, Writer};

use super::VERSION;

/// A request, its content not yet decoded.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Envelope<'a> {
    /// The `PeerMessageContent` bytes, CHOICE tag included.
    pub content: &'a [u8],
}

/// Why a request is not an envelope of this version.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EnvelopeError {
    /// It is another version of the protocol.
    Version(u8),
    /// It is not a valid envelope.
    Malformed(DecodeError),
}

impl From<DecodeError> for EnvelopeError {
    fn from(error: DecodeError) -> Self {
        Self::Malformed(error)
    }
}

impl<'a> Envelope<'a> {
    /// Reads a request body.
    ///
    /// # Errors
    ///
    /// Another version, or no content.
    pub fn decode(body: &'a [u8]) -> Result<Self, EnvelopeError> {
        let mut reader = Reader::new(body);
        let version = reader.read_u8()?;
        if version != VERSION {
            return Err(EnvelopeError::Version(version));
        }
        let start = reader.position();
        let content = reader.read_bytes(reader.remaining())?;
        if content.is_empty() {
            return Err(EnvelopeError::Malformed(DecodeError::new(
                start,
                DecodeErrorKind::UnexpectedEnd,
            )));
        }
        Ok(Self { content })
    }
}

/// Encodes a request.
#[must_use]
pub fn encode_envelope(content: &[u8]) -> Vec<u8> {
    let mut bytes = alloc::vec![0u8; 1 + content.len()];
    let mut writer = Writer::new(&mut bytes);
    writer.write_u8(VERSION);
    writer.write_bytes(content);
    bytes
}
