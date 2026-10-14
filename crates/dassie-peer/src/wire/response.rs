//! Responses: an authentication field, then the body for the request's kind
//! (spec 3 §5.4, spec 7 §4.7).

use alloc::vec::Vec;

use dassie_identity::NodeId;
use dassie_oer::{Decode, DecodeError, DecodeErrorKind, Encode, Reader, Size, Writer, size};

use super::{
    MAX_LINK_STATE_REQUEST_IDS, MAX_NODE_LIST, SignedNodeInfo, node_id_len, read_fixed,
    read_node_id, write_node_id,
};

/// A response, its body not yet decoded: the MAC covers the body's bytes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ResponseEnvelope<'a> {
    /// The response MAC, present if and only if the request was
    /// authenticated.
    pub mac: Option<[u8; 32]>,
    /// The body.
    pub body: &'a [u8],
}

impl<'a> ResponseEnvelope<'a> {
    /// Reads a response.
    ///
    /// # Errors
    ///
    /// The bytes are not a response.
    pub fn decode(bytes: &'a [u8]) -> Result<Self, DecodeError> {
        let mut reader = Reader::new(bytes);
        let offset = reader.offset();
        let mac = match reader.read_context_tag()? {
            0 => None,
            1 => Some(read_fixed::<32>(&mut reader)?),
            _ => return Err(DecodeError::new(offset, DecodeErrorKind::UnexpectedTag)),
        };
        let body = reader.read_bytes(reader.remaining())?;
        Ok(Self { mac, body })
    }
}

/// Encodes a response.
#[must_use]
pub fn encode_response(mac: Option<&[u8; 32]>, body: &[u8]) -> Vec<u8> {
    let mut bytes = alloc::vec![0u8; size::tag(0) + mac.map_or(0, |_| 32) + body.len()];
    let mut writer = Writer::new(&mut bytes);
    match mac {
        Some(mac) => {
            writer.write_context_tag(1);
            writer.write_bytes(mac);
        }
        None => writer.write_context_tag(0),
    }
    writer.write_bytes(body);
    bytes
}

/// Why a peering request was refused (spec 7 §4.7). Unknown values read
/// as [`RefusalReason::Unspecified`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum RefusalReason {
    /// No reason given.
    Unspecified,
    /// The requester is not in the table: not yet registered or listed.
    UnknownNode,
    /// The scheme is not active here.
    UnsupportedScheme,
    /// No version of the scheme is spoken by both.
    NoCommonVersion,
    /// No room for another peer.
    Capacity,
    /// The two nodes asked each other at once, and the other request won.
    Concurrent,
    /// The scheme refused.
    SchemeRefused,
    /// The requester's URL could not be reached.
    Unreachable,
    /// The requester is banned.
    Banned,
}

impl RefusalReason {
    /// The reason's code.
    #[must_use]
    pub const fn code(self) -> u8 {
        match self {
            Self::Unspecified => 0,
            Self::UnknownNode => 1,
            Self::UnsupportedScheme => 2,
            Self::NoCommonVersion => 3,
            Self::Capacity => 4,
            Self::Concurrent => 5,
            Self::SchemeRefused => 6,
            Self::Unreachable => 7,
            Self::Banned => 8,
        }
    }

    /// The reason with code `code`.
    #[must_use]
    pub const fn from_code(code: u8) -> Self {
        match code {
            1 => Self::UnknownNode,
            2 => Self::UnsupportedScheme,
            3 => Self::NoCommonVersion,
            4 => Self::Capacity,
            5 => Self::Concurrent,
            6 => Self::SchemeRefused,
            7 => Self::Unreachable,
            8 => Self::Banned,
            _ => Self::Unspecified,
        }
    }
}

/// The answer to a peering request.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PeeringResponse<'a> {
    /// `accepted [0]`.
    Accepted {
        /// The scheme version both speak.
        version: u16,
        /// The credit the responder extends.
        credit: u64,
        /// The scheme's own data.
        data: &'a [u8],
    },
    /// `refused [1]`.
    Refused(RefusalReason),
}

impl Encode for PeeringResponse<'_> {
    fn encoded_len(&self) -> usize {
        size::tag(0)
            + match self {
                Self::Accepted { data, .. } => 2 + 8 + size::prefixed(data.len()),
                Self::Refused(_) => 1,
            }
    }

    fn encode_to(&self, writer: &mut Writer<'_>) {
        match self {
            Self::Accepted {
                version,
                credit,
                data,
            } => {
                writer.write_context_tag(0);
                writer.write_u16(*version);
                writer.write_u64(*credit);
                writer.write_octets(data, Size::ANY);
            }
            Self::Refused(reason) => {
                writer.write_context_tag(1);
                writer.write_u8(reason.code());
            }
        }
    }
}

impl<'a> Decode<'a> for PeeringResponse<'a> {
    type Error = DecodeError;

    fn decode(reader: &mut Reader<'a>) -> Result<Self, DecodeError> {
        let offset = reader.offset();
        match reader.read_context_tag()? {
            0 => Ok(Self::Accepted {
                version: reader.read_u16()?,
                credit: reader.read_u64()?,
                data: reader.read_octets(Size::ANY)?,
            }),
            1 => Ok(Self::Refused(RefusalReason::from_code(reader.read_u8()?))),
            _ => Err(DecodeError::new(offset, DecodeErrorKind::UnexpectedTag)),
        }
    }
}

/// The answer to a link-state request: per ID asked, its link state if
/// known.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LinkStateResponse<'a>(pub Vec<Option<SignedNodeInfo<'a>>>);

impl Encode for LinkStateResponse<'_> {
    fn encoded_len(&self) -> usize {
        size::quantity(self.0.len())
            + self
                .0
                .iter()
                .map(|found| size::tag(0) + found.as_ref().map_or(0, |info| info.bytes.len()))
                .sum::<usize>()
    }

    fn encode_to(&self, writer: &mut Writer<'_>) {
        writer.write_quantity(self.0.len());
        for found in &self.0 {
            match found {
                Some(info) => {
                    writer.write_context_tag(1);
                    writer.write_bytes(info.bytes);
                }
                None => writer.write_context_tag(0),
            }
        }
    }
}

impl<'a> Decode<'a> for LinkStateResponse<'a> {
    type Error = DecodeError;

    fn decode(reader: &mut Reader<'a>) -> Result<Self, DecodeError> {
        let count = reader.read_quantity(MAX_LINK_STATE_REQUEST_IDS)?;
        let mut found = Vec::with_capacity(count);
        for _ in 0..count {
            let offset = reader.offset();
            match reader.read_context_tag()? {
                0 => found.push(None),
                1 => found.push(Some(SignedNodeInfo::decode(reader)?)),
                _ => return Err(DecodeError::new(offset, DecodeErrorKind::UnexpectedTag)),
            }
        }
        Ok(Self(found))
    }
}

/// A node list: `SEQUENCE OF NodeId`, sorted by bytes (NET-32).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NodeListResponse(pub Vec<NodeId>);

impl Encode for NodeListResponse {
    fn encoded_len(&self) -> usize {
        size::quantity(self.0.len()) + self.0.iter().map(node_id_len).sum::<usize>()
    }

    fn encode_to(&self, writer: &mut Writer<'_>) {
        writer.write_quantity(self.0.len());
        for id in &self.0 {
            write_node_id(writer, id);
        }
    }
}

impl Decode<'_> for NodeListResponse {
    type Error = DecodeError;

    fn decode(reader: &mut Reader<'_>) -> Result<Self, DecodeError> {
        let count = reader.read_quantity(MAX_NODE_LIST)?;
        let mut ids = Vec::with_capacity(count);
        for _ in 0..count {
            ids.push(read_node_id(reader)?);
        }
        Ok(Self(ids))
    }
}

/// Encodes a link-state response from stored `SignedPeerNodeInfo` bytes.
#[must_use]
pub fn encode_link_states(found: &[Option<&[u8]>]) -> Vec<u8> {
    let length = size::quantity(found.len())
        + found
            .iter()
            .map(|bytes| size::tag(0) + bytes.map_or(0, <[u8]>::len))
            .sum::<usize>();
    let mut out = alloc::vec![0u8; length];
    let mut writer = Writer::new(&mut out);
    writer.write_quantity(found.len());
    for bytes in found {
        match bytes {
            Some(bytes) => {
                writer.write_context_tag(1);
                writer.write_bytes(bytes);
            }
            None => writer.write_context_tag(0),
        }
    }
    out
}

/// Encodes a body that is one OCTET STRING: the peering info.
#[must_use]
pub fn encode_octets(data: &[u8]) -> Vec<u8> {
    let mut out = alloc::vec![0u8; size::prefixed(data.len())];
    Writer::new(&mut out).write_octets(data, Size::ANY);
    out
}

/// Reads a body that is one OCTET STRING.
///
/// # Errors
///
/// It is not.
pub fn decode_octets(body: &[u8]) -> Result<&[u8], DecodeError> {
    let mut reader = Reader::new(body);
    let data = reader.read_octets(Size::ANY)?;
    reader.finish()?;
    Ok(data)
}

/// Encodes a registration's answer: when it expires.
#[must_use]
pub fn encode_expiry(expires_at_ms: u64) -> Vec<u8> {
    expires_at_ms.to_be_bytes().to_vec()
}

/// Reads a registration's answer.
///
/// # Errors
///
/// It is not eight bytes.
pub fn decode_expiry(body: &[u8]) -> Result<u64, DecodeError> {
    let mut reader = Reader::new(body);
    let expiry = reader.read_u64()?;
    reader.finish()?;
    Ok(expiry)
}

/// Reads a node-list hash.
///
/// # Errors
///
/// It is not 32 bytes.
pub fn decode_hash(body: &[u8]) -> Result<[u8; 32], DecodeError> {
    let mut reader = Reader::new(body);
    let hash = read_fixed::<32>(&mut reader)?;
    reader.finish()?;
    Ok(hash)
}
