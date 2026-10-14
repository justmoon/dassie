//! The v1 wire format (spec 7 §4.7): OER, as v0's (spec 3 §5) except where
//! that section says otherwise.
//!
//! Decoding borrows from the input and never panics; encoding writes what
//! the types allow, so it cannot produce something the decoder refuses.

mod content;
mod envelope;
mod node_info;
mod response;
mod scheme;
mod url;

use dassie_identity::NodeId;
use dassie_oer::{Alphabet, DecodeError, DecodeErrorKind, Reader, Size, Writer, oid};

pub use content::{Content, PeeringRequest};
pub use envelope::{Authentication, Envelope, EnvelopeError, HmacAuthentication, encode_envelope};
pub use node_info::{Entry, InvalidNodeInfo, NodeInfo, SignedNodeInfo, sign_node_info};
pub use response::{
    LinkStateResponse, NodeListResponse, PeeringResponse, RefusalReason, ResponseEnvelope,
    decode_expiry, decode_hash, decode_octets, encode_expiry, encode_link_states, encode_octets,
    encode_response,
};
pub use scheme::{MAX_SCHEME_ID, SchemeId};
pub use url::is_valid_url;

/// The envelope version this crate speaks.
pub const VERSION: u8 = 1;

/// The OID that starts a v1 `PeerNodeInfo`. v0 used `…1399.1.2`; a new one
/// keeps node info signed under v0 from passing as a v1 link state.
pub const NODE_INFO_OID: [u8; 14] = oid::encode(&[1, 3, 6, 1, 4, 1, 54392, 5, 1399, 1, 3]);

/// The media type of requests, which `Accept` must also name.
pub const MESSAGE_CONTENT_TYPE: &str = "application/dassie-message";

/// The media type of responses.
pub const RESPONSE_CONTENT_TYPE: &str = "application/dassie-peer-response";

/// Most bytes in a request body.
pub const MAX_MESSAGE_BYTES: usize = 64 * 1024;

/// Most bytes in a link state's URL.
pub const MAX_URL_BYTES: usize = 256;

/// Most bytes in a link state's alias.
pub const MAX_ALIAS_BYTES: usize = 64;

/// Most entries in a link state.
pub const MAX_ENTRIES: usize = 64;

/// Most bytes of a signed node info, the part the signature covers.
pub const MAX_NODE_INFO_BYTES: usize = 4096;

/// Most bytes in a link-state extension's value.
pub const MAX_EXTENSION_BYTES: usize = 2048;

/// Most scheme versions in a peering request.
pub const MAX_VERSIONS: usize = 16;

/// Most node IDs in one link-state request.
pub const MAX_LINK_STATE_REQUEST_IDS: usize = 16;

/// Most node IDs in a node list.
pub const MAX_NODE_LIST: usize = 4096;

/// Node IDs: 2 to 45 characters of base64url (spec 3 §2.2).
const NODE_ID_SIZE: Size = Size::range(2, 45);

/// The characters of a node ID.
const NODE_ID_ALPHABET: Alphabet = Alphabet::EMPTY
    .with_range(b'a', b'z')
    .with_range(b'A', b'Z')
    .with_range(b'0', b'9')
    .with_chars(b"-_");

/// The kinds of message, by their CHOICE tag.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Kind {
    /// `peeringRequest [0]`.
    PeeringRequest,
    /// `linkStateUpdate [1]`.
    LinkStateUpdate,
    /// `interledgerPacket [2]`.
    InterledgerPacket,
    /// `linkStateRequest [3]`.
    LinkStateRequest,
    /// `settlement [4]`.
    Settlement,
    /// `settlementMessage [5]`.
    SettlementMessage,
    /// `nodeListHashRequest [6]`.
    NodeListHashRequest,
    /// `nodeListRequest [7]`.
    NodeListRequest,
    /// `registration [8]`.
    Registration,
    /// `peeringInfoRequest [9]`.
    PeeringInfoRequest,
}

impl Kind {
    /// The kind with CHOICE tag `tag`. Tag 10, reserved for
    /// `peeringTermination`, is not one yet.
    #[must_use]
    pub const fn from_tag(tag: u64) -> Option<Self> {
        Some(match tag {
            0 => Self::PeeringRequest,
            1 => Self::LinkStateUpdate,
            2 => Self::InterledgerPacket,
            3 => Self::LinkStateRequest,
            4 => Self::Settlement,
            5 => Self::SettlementMessage,
            6 => Self::NodeListHashRequest,
            7 => Self::NodeListRequest,
            8 => Self::Registration,
            9 => Self::PeeringInfoRequest,
            _ => return None,
        })
    }

    /// Its CHOICE tag.
    #[must_use]
    pub const fn tag(self) -> u64 {
        match self {
            Self::PeeringRequest => 0,
            Self::LinkStateUpdate => 1,
            Self::InterledgerPacket => 2,
            Self::LinkStateRequest => 3,
            Self::Settlement => 4,
            Self::SettlementMessage => 5,
            Self::NodeListHashRequest => 6,
            Self::NodeListRequest => 7,
            Self::Registration => 8,
            Self::PeeringInfoRequest => 9,
        }
    }

    /// Whether a message of this kind must be authenticated (spec 7 §4.7).
    #[must_use]
    pub const fn requires_authentication(self) -> bool {
        matches!(
            self,
            Self::PeeringRequest
                | Self::InterledgerPacket
                | Self::Settlement
                | Self::SettlementMessage
                | Self::Registration
        )
    }
}

/// The kind of the message in `body`, without checking more of it than the
/// envelope's header: enough for the HTTP layer to rate-limit by kind.
#[must_use]
pub fn peek_kind(body: &[u8]) -> Option<Kind> {
    let envelope = Envelope::decode(body).ok()?;
    let mut reader = Reader::new(envelope.content);
    Kind::from_tag(reader.read_context_tag().ok()?)
}

/// Reads a node ID.
pub(crate) fn read_node_id(reader: &mut Reader<'_>) -> Result<NodeId, DecodeError> {
    let offset = reader.offset();
    let text = reader.read_ascii(NODE_ID_SIZE, NODE_ID_ALPHABET)?;
    NodeId::parse(text).map_err(|_| DecodeError::new(offset, DecodeErrorKind::InvalidCharacter))
}

/// Writes a node ID.
pub(crate) fn write_node_id(writer: &mut Writer<'_>, id: &NodeId) {
    writer.write_ascii(id.as_str(), NODE_ID_SIZE, NODE_ID_ALPHABET);
}

/// The encoded length of a node ID.
pub(crate) fn node_id_len(id: &NodeId) -> usize {
    dassie_oer::size::prefixed(id.as_str().len())
}

/// Reads a fixed-size OCTET STRING, which OER writes without a length.
pub(crate) fn read_fixed<const N: usize>(reader: &mut Reader<'_>) -> Result<[u8; N], DecodeError> {
    reader.read_array::<N>().copied()
}

/// Fails unless the next tag is `expected`.
pub(crate) fn expect_tag(reader: &mut Reader<'_>, expected: u64) -> Result<(), DecodeError> {
    let offset = reader.offset();
    if reader.read_context_tag()? == expected {
        Ok(())
    } else {
        Err(DecodeError::new(offset, DecodeErrorKind::UnexpectedTag))
    }
}
