//! `PeerMessageContent`: what a message says
//! (spec/areas/peer-protocol.md#message-set).

use alloc::vec::Vec;

use dassie_identity::NodeId;
use dassie_oer::{Decode, DecodeError, DecodeErrorKind, Encode, Reader, Size, Writer, size};

use super::{
    Kind, MAX_COMMITMENT_BYTES, MAX_LINK_STATE_REQUEST_IDS, MAX_VERSIONS, SchemeId, SignedNodeInfo,
    node_id_len, read_fixed, read_node_id, write_node_id,
};
use crate::names::{
    COMMITMENT_BYTES, Commitment, Name, SALT_BYTES, Salt, name_len, read_name, write_name,
};

/// The size of a `hubPacket`'s commitment: never empty, and bounded.
const COMMITMENT_SIZE: Size = Size::range(1, MAX_COMMITMENT_BYTES);

/// The size of a `methodPacket`'s commitment: empty on a method that is not
/// over a hub, and bounded as a `hubPacket`'s.
const METHOD_COMMITMENT_SIZE: Size = Size::range(0, MAX_COMMITMENT_BYTES);

/// A peering request: the requester's signed link state, the scheme it
/// wants to settle with and the versions of that scheme it speaks.
/// It says nothing of the credit the requester extends: a node knows only
/// the credit it extends (spec/decisions/own-fulfills.md).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PeeringRequest<'a> {
    /// The scheme.
    pub scheme: SchemeId,
    /// The requester's link state, which must verify and name the sender.
    pub node_info: SignedNodeInfo<'a>,
    /// The scheme's own data.
    pub data: &'a [u8],
    /// The scheme versions the requester speaks, at least one.
    pub versions: Vec<u16>,
}

/// A message's content.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Content<'a> {
    /// `peeringRequest [0]`.
    PeeringRequest(PeeringRequest<'a>),
    /// `linkStateUpdate [1]`.
    LinkStateUpdate(SignedNodeInfo<'a>),
    /// `interledgerPacket [2]`: an ILP packet, correlated by request ID.
    InterledgerPacket {
        /// The request ID, scoped to the peering.
        request_id: u32,
        /// The ILP packet.
        packet: &'a [u8],
    },
    /// `linkStateRequest [3]`: up to 16 node IDs.
    LinkStateRequest(Vec<NodeId>),
    /// `settlement [4]`.
    Settlement {
        /// The scheme.
        scheme: SchemeId,
        /// The amount, in the scheme ledger's units.
        amount: u64,
        /// The scheme's own data.
        data: &'a [u8],
        /// Its number from this sender to this recipient: increasing,
        /// never reset, so a copy is applied once.
        sequence: u64,
    },
    /// `settlementMessage [5]`.
    SettlementMessage {
        /// The scheme.
        scheme: SchemeId,
        /// The scheme's message.
        message: &'a [u8],
    },
    /// `nodeListHashRequest [6]`.
    NodeListHashRequest,
    /// `nodeListRequest [7]`.
    NodeListRequest,
    /// `registration [8]`: the registrant's link state.
    Registration(SignedNodeInfo<'a>),
    /// `peeringInfoRequest [9]`.
    PeeringInfoRequest {
        /// The scheme.
        scheme: SchemeId,
    },
    /// `peeringTermination [10]`.
    PeeringTermination(TerminationReason),
    /// `settlementRequest [11]`: the sender asks the recipient to settle
    /// on `scheme`, leaving the amount to it
    /// (spec/decisions/own-fulfills.md).
    SettlementRequest {
        /// The scheme.
        scheme: SchemeId,
    },
    /// `hubPacket [12]`: an ILP Prepare with the payer's commitment, on a
    /// peering over a settlement hub
    /// (spec/designs/settlement-hubs.md#4-peering-over-a-hub). Only a
    /// Prepare travels in it; its Fulfill or Reject goes back as an
    /// `interledgerPacket`.
    HubPacket {
        /// The request ID, scoped to the peering.
        request_id: u32,
        /// The ILP Prepare.
        packet: &'a [u8],
        /// The commitment, opaque to this crate: the hub's scheme reads
        /// it. Never empty, at most [`MAX_COMMITMENT_BYTES`].
        commitment: &'a [u8],
    },
    /// `nameCommit [13]`: the sender commits to a name sealed, to be held
    /// by itself (spec/designs/names.md#3-registering). Its answer is a
    /// [`NameReceipt`](super::NameReceipt).
    NameCommit(Commitment),
    /// `nameUnseal [14]`: the sender opens a commitment it made, with the
    /// name and salt it sealed; the holder is the sender.
    NameUnseal {
        /// The name.
        name: Name,
        /// The salt.
        salt: Salt,
    },
    /// `nameRenew [15]`: the sender renews a name it holds. Its answer is
    /// the name's new expiry, as a registration's.
    NameRenew(Name),
    /// `nameListHashRequest [16]`.
    NameListHashRequest,
    /// `nameListRequest [17]`: its answer is a
    /// [`NameListResponse`](super::NameListResponse).
    NameListRequest,
    /// `methodPacket [18]`: an ILP Prepare on one settlement method of a
    /// peering, with the payer's commitment when the method is over a hub
    /// (spec/decisions/settlement-methods.md). Only a Prepare travels in
    /// it; its Fulfill or Reject goes back as an `interledgerPacket`, by
    /// its request ID. Only to a node whose link state says it has methods
    /// ([`EXTENSION_METHODS`](super::EXTENSION_METHODS)).
    MethodPacket {
        /// The request ID, scoped to the peering.
        request_id: u32,
        /// The method: the scheme it settles on.
        scheme: SchemeId,
        /// The ILP Prepare.
        packet: &'a [u8],
        /// The commitment, opaque to this crate, as a `hubPacket`'s: empty
        /// on a method not over a hub, at most [`MAX_COMMITMENT_BYTES`].
        commitment: &'a [u8],
    },
    /// `methodAdd [19]`: the sender asks to add a method to the peering,
    /// as a peering request asks for its first. Its answer is a
    /// [`MethodResponse`](super::MethodResponse).
    MethodAdd {
        /// The scheme.
        scheme: SchemeId,
        /// The scheme's own data.
        data: &'a [u8],
        /// The scheme versions the sender speaks, at least one.
        versions: Vec<u16>,
    },
    /// `methodDrop [20]`: the sender drops a method of the peering, after
    /// collecting on it; the recipient drops it at once
    /// (spec/decisions/settlement-methods.md).
    MethodDrop {
        /// The scheme.
        scheme: SchemeId,
    },
}

/// Why a peering is terminated. Unknown codes read as
/// [`TerminationReason::Unspecified`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum TerminationReason {
    /// No reason given.
    Unspecified,
    /// The owner ended it, or banned the peer.
    Owner,
    /// The peer was not heard from for too long.
    Unresponsive,
    /// The sender holds no peering with the recipient: the recipient's is
    /// left over.
    NotPeered,
    /// The recipient's room in the sender's books stayed short of a packet,
    /// without a settlement, for too long
    /// (spec/decisions/peering-lifecycle.md).
    ShortOfRoom,
}

impl TerminationReason {
    /// The reason's code.
    #[must_use]
    pub const fn code(self) -> u8 {
        match self {
            Self::Unspecified => 0,
            Self::Owner => 1,
            Self::Unresponsive => 2,
            Self::NotPeered => 3,
            Self::ShortOfRoom => 4,
        }
    }

    /// The reason with code `code`.
    #[must_use]
    pub const fn from_code(code: u8) -> Self {
        match code {
            1 => Self::Owner,
            2 => Self::Unresponsive,
            3 => Self::NotPeered,
            4 => Self::ShortOfRoom,
            _ => Self::Unspecified,
        }
    }
}

impl Content<'_> {
    /// Its kind.
    #[must_use]
    pub const fn kind(&self) -> Kind {
        match self {
            Self::PeeringRequest(_) => Kind::PeeringRequest,
            Self::LinkStateUpdate(_) => Kind::LinkStateUpdate,
            Self::InterledgerPacket { .. } => Kind::InterledgerPacket,
            Self::LinkStateRequest(_) => Kind::LinkStateRequest,
            Self::Settlement { .. } => Kind::Settlement,
            Self::SettlementMessage { .. } => Kind::SettlementMessage,
            Self::NodeListHashRequest => Kind::NodeListHashRequest,
            Self::NodeListRequest => Kind::NodeListRequest,
            Self::Registration(_) => Kind::Registration,
            Self::PeeringInfoRequest { .. } => Kind::PeeringInfoRequest,
            Self::PeeringTermination(_) => Kind::PeeringTermination,
            Self::SettlementRequest { .. } => Kind::SettlementRequest,
            Self::HubPacket { .. } => Kind::HubPacket,
            Self::NameCommit(_) => Kind::NameCommit,
            Self::NameUnseal { .. } => Kind::NameUnseal,
            Self::NameRenew(_) => Kind::NameRenew,
            Self::NameListHashRequest => Kind::NameListHashRequest,
            Self::NameListRequest => Kind::NameListRequest,
            Self::MethodPacket { .. } => Kind::MethodPacket,
            Self::MethodAdd { .. } => Kind::MethodAdd,
            Self::MethodDrop { .. } => Kind::MethodDrop,
        }
    }

    fn body_len(&self) -> usize {
        match self {
            Self::PeeringRequest(request) => {
                request.scheme.encoded_len()
                    + size::prefixed(request.node_info.bytes.len())
                    + size::prefixed(request.data.len())
                    + versions_len(&request.versions)
            }
            Self::LinkStateUpdate(info) | Self::Registration(info) => info.bytes.len(),
            Self::InterledgerPacket { packet, .. } => {
                size::prefixed(4 + size::prefixed(packet.len()))
            }
            Self::LinkStateRequest(ids) => {
                size::quantity(ids.len()) + ids.iter().map(node_id_len).sum::<usize>()
            }
            Self::Settlement { scheme, data, .. } => {
                scheme.encoded_len() + 8 + size::prefixed(data.len()) + 8
            }
            Self::SettlementMessage { scheme, message } => {
                scheme.encoded_len() + size::prefixed(message.len())
            }
            Self::NodeListHashRequest
            | Self::NodeListRequest
            | Self::NameListHashRequest
            | Self::NameListRequest => 0,
            Self::PeeringInfoRequest { scheme }
            | Self::SettlementRequest { scheme }
            | Self::MethodDrop { scheme } => scheme.encoded_len(),
            Self::PeeringTermination(_) => 1,
            Self::HubPacket {
                packet, commitment, ..
            } => 4 + size::prefixed(packet.len()) + size::prefixed(commitment.len()),
            Self::NameCommit(_) => COMMITMENT_BYTES,
            Self::NameUnseal { name, .. } => name_len(name) + SALT_BYTES,
            Self::NameRenew(name) => name_len(name),
            Self::MethodPacket {
                scheme,
                packet,
                commitment,
                ..
            } => {
                4 + scheme.encoded_len()
                    + size::prefixed(packet.len())
                    + size::prefixed(commitment.len())
            }
            Self::MethodAdd {
                scheme,
                data,
                versions,
            } => scheme.encoded_len() + size::prefixed(data.len()) + versions_len(versions),
        }
    }
}

impl Encode for Content<'_> {
    fn encoded_len(&self) -> usize {
        size::tag(self.kind().tag()) + self.body_len()
    }

    fn encode_to(&self, writer: &mut Writer<'_>) {
        writer.write_context_tag(self.kind().tag());
        match self {
            Self::PeeringRequest(request) => write_peering_request(writer, request),
            Self::LinkStateUpdate(info) | Self::Registration(info) => {
                writer.write_bytes(info.bytes);
            }
            Self::InterledgerPacket { request_id, packet } => {
                writer.write_prefixed(4 + size::prefixed(packet.len()), |inner| {
                    inner.write_u32(*request_id);
                    inner.write_octets(packet, Size::ANY);
                });
            }
            Self::LinkStateRequest(ids) => {
                writer.write_quantity(ids.len());
                for id in ids {
                    write_node_id(writer, id);
                }
            }
            Self::Settlement {
                scheme,
                amount,
                data,
                sequence,
            } => {
                scheme.write(writer);
                writer.write_u64(*amount);
                writer.write_octets(data, Size::ANY);
                writer.write_u64(*sequence);
            }
            Self::SettlementMessage { scheme, message } => {
                scheme.write(writer);
                writer.write_octets(message, Size::ANY);
            }
            Self::NodeListHashRequest
            | Self::NodeListRequest
            | Self::NameListHashRequest
            | Self::NameListRequest => {}
            Self::PeeringInfoRequest { scheme }
            | Self::SettlementRequest { scheme }
            | Self::MethodDrop { scheme } => scheme.write(writer),
            Self::PeeringTermination(reason) => writer.write_u8(reason.code()),
            Self::HubPacket {
                request_id,
                packet,
                commitment,
            } => {
                writer.write_u32(*request_id);
                writer.write_octets(packet, Size::ANY);
                writer.write_octets(commitment, COMMITMENT_SIZE);
            }
            Self::NameCommit(commitment) => writer.write_bytes(&commitment.0),
            Self::NameUnseal { name, salt } => {
                write_name(writer, name);
                writer.write_bytes(&salt.0);
            }
            Self::NameRenew(name) => write_name(writer, name),
            Self::MethodPacket {
                request_id,
                scheme,
                packet,
                commitment,
            } => write_method_packet(writer, *request_id, scheme, packet, commitment),
            Self::MethodAdd {
                scheme,
                data,
                versions,
            } => write_method_add(writer, scheme, data, versions),
        }
    }
}

impl<'a> Decode<'a> for Content<'a> {
    type Error = DecodeError;

    fn decode(reader: &mut Reader<'a>) -> Result<Self, DecodeError> {
        let offset = reader.offset();
        let kind = Kind::from_tag(reader.read_context_tag()?)
            .ok_or(DecodeError::new(offset, DecodeErrorKind::UnexpectedTag))?;
        Ok(match kind {
            Kind::PeeringRequest => Self::PeeringRequest(read_peering_request(reader)?),
            Kind::LinkStateUpdate => Self::LinkStateUpdate(SignedNodeInfo::decode(reader)?),
            Kind::InterledgerPacket => {
                let (request_id, packet) = reader.read_prefixed(|inner| {
                    let request_id = inner.read_u32()?;
                    let packet = inner.read_octets(Size::ANY)?;
                    Ok::<_, DecodeError>((request_id, packet))
                })?;
                Self::InterledgerPacket { request_id, packet }
            }
            Kind::LinkStateRequest => {
                let count = reader.read_quantity(MAX_LINK_STATE_REQUEST_IDS)?;
                let mut ids = Vec::with_capacity(count);
                for _ in 0..count {
                    ids.push(read_node_id(reader)?);
                }
                Self::LinkStateRequest(ids)
            }
            Kind::Settlement => Self::Settlement {
                scheme: SchemeId::read(reader)?,
                amount: reader.read_u64()?,
                data: reader.read_octets(Size::ANY)?,
                sequence: reader.read_u64()?,
            },
            Kind::SettlementMessage => Self::SettlementMessage {
                scheme: SchemeId::read(reader)?,
                message: reader.read_octets(Size::ANY)?,
            },
            Kind::NodeListHashRequest => Self::NodeListHashRequest,
            Kind::NodeListRequest => Self::NodeListRequest,
            Kind::Registration => Self::Registration(SignedNodeInfo::decode(reader)?),
            Kind::PeeringInfoRequest => Self::PeeringInfoRequest {
                scheme: SchemeId::read(reader)?,
            },
            Kind::PeeringTermination => {
                Self::PeeringTermination(TerminationReason::from_code(reader.read_u8()?))
            }
            Kind::SettlementRequest => Self::SettlementRequest {
                scheme: SchemeId::read(reader)?,
            },
            Kind::HubPacket => Self::HubPacket {
                request_id: reader.read_u32()?,
                packet: reader.read_octets(Size::ANY)?,
                commitment: reader.read_octets(COMMITMENT_SIZE)?,
            },
            Kind::NameCommit => Self::NameCommit(Commitment(read_fixed(reader)?)),
            Kind::NameUnseal => Self::NameUnseal {
                name: read_name(reader)?,
                salt: Salt(read_fixed(reader)?),
            },
            Kind::NameRenew => Self::NameRenew(read_name(reader)?),
            Kind::NameListHashRequest => Self::NameListHashRequest,
            Kind::NameListRequest => Self::NameListRequest,
            Kind::MethodPacket | Kind::MethodAdd | Kind::MethodDrop => read_method(kind, reader)?,
        })
    }
}

/// Writes the body of a `methodPacket`.
fn write_method_packet(
    writer: &mut Writer<'_>,
    request_id: u32,
    scheme: &SchemeId,
    packet: &[u8],
    commitment: &[u8],
) {
    writer.write_u32(request_id);
    scheme.write(writer);
    writer.write_octets(packet, Size::ANY);
    writer.write_octets(commitment, METHOD_COMMITMENT_SIZE);
}

/// Writes the body of a `methodAdd`.
fn write_method_add(writer: &mut Writer<'_>, scheme: &SchemeId, data: &[u8], versions: &[u16]) {
    scheme.write(writer);
    writer.write_octets(data, Size::ANY);
    write_versions(writer, versions);
}

/// Reads the body of a settlement method's message of kind `kind`.
fn read_method<'a>(kind: Kind, reader: &mut Reader<'a>) -> Result<Content<'a>, DecodeError> {
    Ok(match kind {
        Kind::MethodPacket => Content::MethodPacket {
            request_id: reader.read_u32()?,
            scheme: SchemeId::read(reader)?,
            packet: reader.read_octets(Size::ANY)?,
            commitment: reader.read_octets(METHOD_COMMITMENT_SIZE)?,
        },
        Kind::MethodAdd => Content::MethodAdd {
            scheme: SchemeId::read(reader)?,
            data: reader.read_octets(Size::ANY)?,
            versions: read_versions(reader)?,
        },
        _ => Content::MethodDrop {
            scheme: SchemeId::read(reader)?,
        },
    })
}

fn write_peering_request(writer: &mut Writer<'_>, request: &PeeringRequest<'_>) {
    request.scheme.write(writer);
    writer.write_octets(request.node_info.bytes, Size::ANY);
    writer.write_octets(request.data, Size::ANY);
    write_versions(writer, &request.versions);
}

fn read_peering_request<'a>(reader: &mut Reader<'a>) -> Result<PeeringRequest<'a>, DecodeError> {
    let scheme = SchemeId::read(reader)?;
    let node_info = reader.read_prefixed(|inner| {
        let info = SignedNodeInfo::decode(inner)?;
        inner.finish()?;
        Ok::<_, DecodeError>(info)
    })?;
    let data = reader.read_octets(Size::ANY)?;
    let versions = read_versions(reader)?;
    Ok(PeeringRequest {
        scheme,
        node_info,
        data,
        versions,
    })
}

/// The encoded length of a list of scheme versions.
fn versions_len(versions: &[u16]) -> usize {
    size::quantity(versions.len()) + 2 * versions.len()
}

/// Writes a list of scheme versions.
fn write_versions(writer: &mut Writer<'_>, versions: &[u16]) {
    writer.write_quantity(versions.len());
    for version in versions {
        writer.write_u16(*version);
    }
}

/// Reads a list of scheme versions: at least one, at most
/// [`MAX_VERSIONS`].
fn read_versions(reader: &mut Reader<'_>) -> Result<Vec<u16>, DecodeError> {
    let offset = reader.offset();
    let count = reader.read_quantity(MAX_VERSIONS)?;
    if count == 0 {
        return Err(DecodeError::new(offset, DecodeErrorKind::SizeOutOfRange));
    }
    let mut versions = Vec::with_capacity(count);
    for _ in 0..count {
        versions.push(reader.read_u16()?);
    }
    Ok(versions)
}
