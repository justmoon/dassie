//! The v1 wire format (spec/areas/peer-protocol.md#encoding): OER, in v0's
//! layouts except where that section says otherwise.
//!
//! Decoding borrows from the input and never panics; encoding writes what
//! the types allow, so it cannot produce something the decoder refuses.

mod content;
mod node_info;
mod response;
mod scheme;
mod url;

use dassie_identity::NodeId;
use dassie_oer::{Decode, DecodeError, DecodeErrorKind, Encode, Reader, Writer, oid, size};

pub use content::{Content, PeeringRequest, TerminationReason};
pub use node_info::{
    Entry, HubDetails, InvalidNodeInfo, NodeInfo, SettlementOffer, SignedNodeInfo,
    max_node_info_bytes, sign_node_info,
};
pub use response::{
    LinkStateResponse, MethodResponse, NameHolding, NameListResponse, NameReceipt,
    NodeListResponse, PeeringResponse, RefusalReason, decode_expiry, decode_hash, decode_octets,
    decode_settling, encode_expiry, encode_link_states, encode_octets, encode_settling,
};
pub use scheme::{MAX_SCHEME_ID, SchemeId};
pub use url::{Endpoint, is_valid_url};

/// The OID that starts a `PeerNodeInfo`. v0 used `…1399.1.2`, the layout
/// with the node ID beside the key `…1399.1.3`, and the layout with the
/// Ed25519 key alone `…1399.1.5`; a new one keeps node info signed under
/// any of them from passing as a link state of this layout, which carries
/// the SLH-DSA key beside the Ed25519 key.
pub const NODE_INFO_OID: [u8; 14] = oid::encode(&[1, 3, 6, 1, 4, 1, 54392, 5, 1399, 1, 6]);

/// Most bytes in a request body.
pub const MAX_MESSAGE_BYTES: usize = 64 * 1024;

/// Most bytes in a link state's URL.
pub const MAX_URL_BYTES: usize = 256;

/// Most bytes in a link state's alias.
pub const MAX_ALIAS_BYTES: usize = 64;

/// Most entries in a link state: its neighbors and extensions.
pub const MAX_ENTRIES: usize = 64;

/// Most settlement offers in a link state
/// (spec/areas/peer-protocol.md#settlement-offers): a bound of the
/// protocol, which every node decodes, above the schemes a node of this
/// release peers on, so that a later release may offer more without a new
/// version.
pub const MAX_OFFERS: usize = 32;

/// Most bytes of an offer's details: what a scheme says of itself in a
/// link state. A hub's take [`HubDetails::ENCODED_BYTES`]; the bound leaves
/// room for schemes of later releases, such as one that settles directly,
/// to say more without a new version.
pub const MAX_OFFER_DETAILS: usize = 256;

/// Most bytes of a signed node info, the part the signature covers.
pub const MAX_NODE_INFO_BYTES: usize = 4096;

/// Most bytes in a link-state extension's value.
pub const MAX_EXTENSION_BYTES: usize = 2048;

/// The link-state extension that says a node is a periphery node: it has
/// no URL, dials out to its uplinks and is reached only over the
/// connections it opened (spec/areas/peer-protocol.md#link-states). Its
/// value is empty. Only with it may a link state's URL be empty. Type 1 is
/// kept for a post-quantum key, and the types in [`RESERVED_EXTENSIONS`]
/// are reserved.
pub const EXTENSION_PERIPHERY: u16 = 3;

/// The link-state extension that names the UDP port of a core node's rate
/// feed, at its URL's host (spec/areas/rate-feed.md#finding-a-feed). Its
/// value is the port, a `UInt16`, never 0. A node without a feed, and a
/// periphery node, which has no URL, leave it out.
pub const EXTENSION_FEED: u16 = 4;

/// The link-state extension types that are reserved: no extension has
/// them, and none ever will, so that no node reads one as another
/// (spec/areas/peer-protocol.md#versions). Type 2 said that a node takes
/// QUIC, which every node does; 5 named a node's settlement hub and 7
/// listed its hubs, which its settlement offers say now
/// ([`NodeInfo::settlement`]); 6 was to say that a node has settlement
/// methods, which every node of this version has.
pub const RESERVED_EXTENSIONS: [u16; 4] = [2, 5, 6, 7];

/// Most bytes in the commitment a `methodPacket` carries with its Prepare
/// on a method over a hub: a signature and what it covers, well within
/// this.
pub const MAX_COMMITMENT_BYTES: usize = 256;

/// Most bytes after the verdict in the reply to a Fulfill on a peering
/// over a hub: the payer's claim. More is ignored.
pub const MAX_CLAIM_BYTES: usize = 256;

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
        * (size::prefixed(crate::names::MAX_NAME_BYTES) + dassie_identity::NODE_ID_BYTES + 8);

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
    /// `settlement [4]`. Tag 5 follows it unused: it is reserved, and never
    /// given to a message, so that no node reads it as one.
    Settlement,
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
    /// `settlementRequest [11]`. Tag 12 follows it unused: it was
    /// `hubPacket`, which `methodPacket` replaced, and is reserved, never
    /// given to another message.
    SettlementRequest,
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
    /// `methodPacket [18]`.
    MethodPacket,
    /// `methodAdd [19]`.
    MethodAdd,
    /// `methodDrop [20]`.
    MethodDrop,
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
            6 => Self::NodeListHashRequest,
            7 => Self::NodeListRequest,
            8 => Self::Registration,
            9 => Self::PeeringInfoRequest,
            10 => Self::PeeringTermination,
            11 => Self::SettlementRequest,
            13 => Self::NameCommit,
            14 => Self::NameUnseal,
            15 => Self::NameRenew,
            16 => Self::NameListHashRequest,
            17 => Self::NameListRequest,
            18 => Self::MethodPacket,
            19 => Self::MethodAdd,
            20 => Self::MethodDrop,
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
            Self::NodeListHashRequest => 6,
            Self::NodeListRequest => 7,
            Self::Registration => 8,
            Self::PeeringInfoRequest => 9,
            Self::PeeringTermination => 10,
            Self::SettlementRequest => 11,
            Self::NameCommit => 13,
            Self::NameUnseal => 14,
            Self::NameRenew => 15,
            Self::NameListHashRequest => 16,
            Self::NameListRequest => 17,
            Self::MethodPacket => 18,
            Self::MethodAdd => 19,
            Self::MethodDrop => 20,
        }
    }

    /// Whether a message of this kind carries an ILP packet: its bounds are
    /// the connector's, not those of the protocol's own requests.
    #[must_use]
    pub const fn carries_packet(self) -> bool {
        matches!(self, Self::InterledgerPacket | Self::MethodPacket)
    }

    /// Whether a message of this kind only reads: it changes nothing at the
    /// recipient, and its answer is public, signed data or the recipient's
    /// own, which anyone may ask for.
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

/// The kind of the message in `body`, without checking more of it than its
/// tag: enough for the transport to rate-limit by kind.
#[must_use]
pub fn peek_kind(body: &[u8]) -> Option<Kind> {
    let mut reader = Reader::new(body);
    Kind::from_tag(reader.read_context_tag().ok()?)
}

/// Reads a node ID: its 24 bytes, as `dassie-identity` writes them
/// (spec/areas/identity.md#node-id).
pub(crate) fn read_node_id(reader: &mut Reader<'_>) -> Result<NodeId, DecodeError> {
    NodeId::decode(reader)
}

/// Writes a node ID.
pub(crate) fn write_node_id(writer: &mut Writer<'_>, id: &NodeId) {
    id.encode_to(writer);
}

/// The encoded length of a node ID: the same for every one.
pub(crate) const fn node_id_len(_id: &NodeId) -> usize {
    dassie_identity::NODE_ID_BYTES
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
