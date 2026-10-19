//! The envelope of a request: version, sender, authentication and content
//! (spec/areas/peer-protocol.md#envelope).

use alloc::vec::Vec;

use dassie_identity::NodeId;
use dassie_oer::{DecodeError, DecodeErrorKind, Reader, Size, Writer, size};

use super::{VERSION, node_id_len, read_fixed, read_node_id, write_node_id};

/// `hmacSha256V1 [1]`: who sends, in which session, and the MAC.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HmacAuthentication {
    /// The sender's Ed25519 public key, which must hash to its node ID.
    pub sender_key: [u8; 32],
    /// The session, chosen by the sender.
    pub session_id: [u8; 16],
    /// The message's number within the session.
    pub counter: u64,
    /// When the sender sent it, by its clock.
    pub timestamp_ms: u64,
    /// The MAC over the message.
    pub mac: [u8; 32],
}

/// How a request is authenticated.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Authentication<'a> {
    /// `none [0]`.
    None,
    /// `hmacSha256V1 [1]`.
    Hmac(HmacAuthentication),
    /// `reserved [2]`: a later scheme, such as one using X-Wing. Parsed, so
    /// that the node can answer that it does not support it.
    Reserved(&'a [u8]),
}

impl Authentication<'_> {
    fn encoded_len(&self) -> usize {
        size::tag(0)
            + match self {
                Self::None => 0,
                Self::Hmac(_) => 32 + 16 + 8 + 8 + 32,
                Self::Reserved(bytes) => size::prefixed(bytes.len()),
            }
    }

    fn write(&self, writer: &mut Writer<'_>) {
        match self {
            Self::None => writer.write_context_tag(0),
            Self::Hmac(hmac) => {
                writer.write_context_tag(1);
                writer.write_bytes(&hmac.sender_key);
                writer.write_bytes(&hmac.session_id);
                writer.write_u64(hmac.counter);
                writer.write_u64(hmac.timestamp_ms);
                writer.write_bytes(&hmac.mac);
            }
            Self::Reserved(bytes) => {
                writer.write_context_tag(2);
                writer.write_octets(bytes, Size::ANY);
            }
        }
    }
}

/// A request, its content not yet decoded: the MAC covers the content's
/// exact bytes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Envelope<'a> {
    /// The claimed sender.
    pub sender: NodeId,
    /// Its authentication.
    pub authentication: Authentication<'a>,
    /// The `PeerMessageContent` bytes, CHOICE tag included.
    pub content: &'a [u8],
}

/// Why a request is not a v1 envelope.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EnvelopeError {
    /// It is another version of the protocol, such as v0.
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
    /// A version other than 1, or a malformed envelope.
    pub fn decode(body: &'a [u8]) -> Result<Self, EnvelopeError> {
        let mut reader = Reader::new(body);
        let version = reader.read_u8()?;
        if version != VERSION {
            return Err(EnvelopeError::Version(version));
        }
        let sender = read_node_id(&mut reader)?;
        let authentication = read_authentication(&mut reader)?;
        let start = reader.position();
        let content = reader.read_bytes(reader.remaining())?;
        if content.is_empty() {
            return Err(EnvelopeError::Malformed(DecodeError::new(
                start,
                DecodeErrorKind::UnexpectedEnd,
            )));
        }
        Ok(Self {
            sender,
            authentication,
            content,
        })
    }
}

fn read_authentication<'a>(reader: &mut Reader<'a>) -> Result<Authentication<'a>, DecodeError> {
    let offset = reader.offset();
    match reader.read_context_tag()? {
        0 => Ok(Authentication::None),
        1 => Ok(Authentication::Hmac(HmacAuthentication {
            sender_key: read_fixed::<32>(reader)?,
            session_id: read_fixed::<16>(reader)?,
            counter: reader.read_u64()?,
            timestamp_ms: reader.read_u64()?,
            mac: read_fixed::<32>(reader)?,
        })),
        2 => Ok(Authentication::Reserved(reader.read_octets(Size::ANY)?)),
        _ => Err(DecodeError::new(offset, DecodeErrorKind::UnexpectedTag)),
    }
}

/// Encodes a request.
#[must_use]
pub fn encode_envelope(
    sender: &NodeId,
    authentication: &Authentication<'_>,
    content: &[u8],
) -> Vec<u8> {
    let length = 1 + node_id_len(sender) + authentication.encoded_len() + content.len();
    let mut bytes = alloc::vec![0u8; length];
    let mut writer = Writer::new(&mut bytes);
    writer.write_u8(VERSION);
    write_node_id(&mut writer, sender);
    authentication.write(&mut writer);
    writer.write_bytes(content);
    bytes
}
