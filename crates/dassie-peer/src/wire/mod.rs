//! The v1 wire format (spec/areas/peer-protocol.md#encoding): OER, as v0's
//! (spec/original/03-peer-network.md#5-wire-schemas) except where the
//! former says otherwise.
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
use dassie_oer::{Alphabet, DecodeError, DecodeErrorKind, Reader, Size, Writer, oid, size};

pub use content::{Content, PeeringRequest, TerminationReason};
pub use envelope::{Authentication, Envelope, EnvelopeError, HmacAuthentication, encode_envelope};
pub use node_info::{
    Entry, HubMembership, InvalidNodeInfo, NodeInfo, SignedNodeInfo, max_node_info_bytes,
    sign_node_info,
};
pub use response::{
    LinkStateResponse, NameHolding, NameListResponse, NameReceipt, NodeListResponse,
    PeeringResponse, RefusalReason, ResponseEnvelope, decode_expiry, decode_hash, decode_octets,
    decode_settling, encode_expiry, encode_link_states, encode_octets, encode_response,
    encode_settling,
};
pub use scheme::{MAX_SCHEME_ID, SchemeId};
pub use url::is_valid_url;

/// The envelope version this crate speaks.
pub const VERSION: u8 = 1;

/// The OID that starts a v1 `PeerNodeInfo`. v0 used `…1399.1.2`; a new one
/// keeps node info signed under v0 from passing as a v1 link state.
pub const NODE_INFO_OID: [u8; 14] = oid::encode(&[1, 3, 6, 1, 4, 1, 54392, 5, 1399, 1, 3]);

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

/// The link-state extension that says a node takes the peer protocol over
/// QUIC, on UDP at its URL's host and port (spec/areas/peer-protocol.md#transport).
/// Its value is empty. Type 1 is kept for a post-quantum key.
pub const EXTENSION_QUIC: u16 = 2;

/// The link-state extension that says a node is a periphery node: it has
/// no URL, dials out to its uplinks and is reached only over the
/// connections it opened (spec/areas/peer-protocol.md#link-states). Its
/// value is empty. Only with it may a link state's URL be empty.
pub const EXTENSION_PERIPHERY: u16 = 3;

/// The link-state extension that names the UDP port of a core node's rate
/// feed, at its URL's host (spec/areas/rate-feed.md#finding-a-feed). Its
/// value is the port, a `UInt16`, never 0. A node without a feed, and a
/// periphery node, which has no URL, leave it out.
pub const EXTENSION_FEED: u16 = 4;

/// The link-state extension that says which settlement hub a node is a
/// member of (spec/designs/settlement-hubs.md#11-choosing-a-hub): a
/// [`HubMembership`] in canonical OER. A node in no hub leaves it out.
pub const EXTENSION_HUB: u16 = 5;

/// Most bytes in the commitment a `hubPacket` carries with its Prepare: a
/// signature and what it covers, well within this.
pub const MAX_COMMITMENT_BYTES: usize = 256;

/// Most bytes after the verdict in the reply to a Fulfill on a peering
/// over a hub: the payer's claim. More is ignored.
pub const MAX_CLAIM_BYTES: usize = 256;

/// Most scheme versions in a peering request.
pub const MAX_VERSIONS: usize = 16;

/// Most node IDs in one link-state request.
pub const MAX_LINK_STATE_REQUEST_IDS: usize = 16;

/// Most node IDs in a node list.
pub const MAX_NODE_LIST: usize = 4096;

/// Most names in a registrar's name list
/// (spec/designs/names.md#5-holding-a-name).
pub const MAX_NAME_LIST: usize = 2048;

/// Most bytes of a name list: [`MAX_NAME_LIST`] entries of the longest
/// name and node ID, which the transport's bound on a response must hold.
pub const MAX_NAME_LIST_BYTES: usize = size::quantity(MAX_NAME_LIST)
    + MAX_NAME_LIST
        * (size::prefixed(crate::names::MAX_NAME_BYTES)
            + size::prefixed(dassie_identity::MAX_NODE_ID_LENGTH)
            + 8);

/// Node IDs: 2 to 45 characters of base64url.
const NODE_ID_SIZE: Size = Size::range(2, dassie_identity::MAX_NODE_ID_LENGTH);

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
    /// `peeringTermination [10]`.
    PeeringTermination,
    /// `settlementRequest [11]`.
    SettlementRequest,
    /// `hubPacket [12]`.
    HubPacket,
    /// `nameCommit [13]`.
    NameCommit,
    /// `nameUnseal [14]`.
    NameUnseal,
    /// `nameRenew [15]`.
    NameRenew,
    /// `nameListHashRequest [16]`.
    NameListHashRequest,
    /// `nameListRequest [17]`.
    NameListRequest,
}

impl Kind {
    /// The kind with CHOICE tag `tag`.
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
            10 => Self::PeeringTermination,
            11 => Self::SettlementRequest,
            12 => Self::HubPacket,
            13 => Self::NameCommit,
            14 => Self::NameUnseal,
            15 => Self::NameRenew,
            16 => Self::NameListHashRequest,
            17 => Self::NameListRequest,
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
            Self::PeeringTermination => 10,
            Self::SettlementRequest => 11,
            Self::HubPacket => 12,
            Self::NameCommit => 13,
            Self::NameUnseal => 14,
            Self::NameRenew => 15,
            Self::NameListHashRequest => 16,
            Self::NameListRequest => 17,
        }
    }

    /// Whether a message of this kind must be authenticated
    /// (spec/areas/peer-protocol.md#which-messages-need-authentication).
    #[must_use]
    pub const fn requires_authentication(self) -> bool {
        matches!(
            self,
            Self::PeeringRequest
                | Self::InterledgerPacket
                | Self::Settlement
                | Self::SettlementMessage
                | Self::Registration
                | Self::PeeringTermination
                | Self::SettlementRequest
                | Self::HubPacket
                | Self::NameCommit
                | Self::NameUnseal
                | Self::NameRenew
        )
    }

    /// Whether a message of this kind carries an ILP packet: its bounds are
    /// the connector's, not those of the protocol's own requests.
    #[must_use]
    pub const fn carries_packet(self) -> bool {
        matches!(self, Self::InterledgerPacket | Self::HubPacket)
    }

    /// Whether a message of this kind only reads: it changes nothing at the
    /// recipient, and its answer is public, signed data or the recipient's
    /// own. A replay of one gets nothing that asking does not, so a sender
    /// the recipient keeps no replay state for gets none from it
    /// (spec/areas/peer-protocol.md#replay-protection).
    #[must_use]
    pub const fn is_read_only(self) -> bool {
        matches!(
            self,
            Self::LinkStateRequest
                | Self::NodeListHashRequest
                | Self::NodeListRequest
                | Self::PeeringInfoRequest
                | Self::NameListHashRequest
                | Self::NameListRequest
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
