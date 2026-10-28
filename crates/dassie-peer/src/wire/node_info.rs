//! Node info, the signed link state (spec/areas/peer-protocol.md#link-states).

use alloc::vec::Vec;

use dassie_identity::{
    NodeId, NodeIdentity, NodeKey, PublicKey, SLH_DSA_PUBLIC_KEY_BYTES, Signature, SlhDsaPublicKey,
};
use dassie_oer::{
    Alphabet, Decode, DecodeError, DecodeErrorKind, Encode, Reader, Size, Writer, size,
};

use super::{
    MAX_ALIAS_BYTES, MAX_ENTRIES, MAX_EXTENSION_BYTES, MAX_NODE_INFO_BYTES, MAX_OFFER_DETAILS,
    MAX_OFFERS, MAX_URL_BYTES, NODE_INFO_OID, SchemeId, expect_tag, is_valid_url, node_id_len,
    read_fixed, read_node_id, write_node_id,
};

/// An entry of a link state.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Entry<'a> {
    /// `neighbor [0]`: a node this one is peered with and advertises.
    Neighbor(NodeId),
    /// `extension [1]`: anything later versions add, such as a post-quantum
    /// key ([`EXTENSION_POST_QUANTUM`](super::EXTENSION_POST_QUANTUM)).
    /// Unknown types are kept, since the signature covers them, and
    /// otherwise ignored.
    Extension {
        /// What it is.
        kind: u16,
        /// Its value.
        value: &'a [u8],
    },
}

impl Encode for Entry<'_> {
    fn encoded_len(&self) -> usize {
        size::tag(0)
            + match self {
                Self::Neighbor(id) => node_id_len(id),
                Self::Extension { value, .. } => 2 + size::prefixed(value.len()),
            }
    }

    fn encode_to(&self, writer: &mut Writer<'_>) {
        match self {
            Self::Neighbor(id) => {
                writer.write_context_tag(0);
                write_node_id(writer, id);
            }
            Self::Extension { kind, value } => {
                writer.write_context_tag(1);
                writer.write_u16(*kind);
                writer.write_octets(value, Size::range(0, MAX_EXTENSION_BYTES));
            }
        }
    }
}

impl<'a> Decode<'a> for Entry<'a> {
    type Error = DecodeError;

    fn decode(reader: &mut Reader<'a>) -> Result<Self, DecodeError> {
        let offset = reader.offset();
        match reader.read_context_tag()? {
            0 => Ok(Self::Neighbor(read_node_id(reader)?)),
            1 => Ok(Self::Extension {
                kind: reader.read_u16()?,
                value: reader.read_octets(Size::range(0, MAX_EXTENSION_BYTES))?,
            }),
            _ => Err(DecodeError::new(offset, DecodeErrorKind::UnexpectedTag)),
        }
    }
}

/// `SettlementOffer`: a scheme a node takes peers on, with the scheme's own
/// details in their canonical OER
/// (spec/areas/peer-protocol.md#settlement-offers). What the details say is
/// the scheme's: a node reads them only on a scheme it knows, and keeps and
/// forwards every other offer unread, since the signature covers it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SettlementOffer<'a> {
    /// The scheme.
    pub scheme: SchemeId,
    /// The scheme's details, at most [`MAX_OFFER_DETAILS`] bytes: a
    /// [`HubDetails`] on a scheme over a settlement hub, and nothing on the
    /// stub.
    pub details: &'a [u8],
}

impl Encode for SettlementOffer<'_> {
    fn encoded_len(&self) -> usize {
        self.scheme.encoded_len() + size::prefixed(self.details.len())
    }

    fn encode_to(&self, writer: &mut Writer<'_>) {
        self.scheme.write(writer);
        writer.write_octets(self.details, Size::range(0, MAX_OFFER_DETAILS));
    }
}

impl<'a> Decode<'a> for SettlementOffer<'a> {
    type Error = DecodeError;

    fn decode(reader: &mut Reader<'a>) -> Result<Self, DecodeError> {
        Ok(Self {
            scheme: SchemeId::read(reader)?,
            details: reader.read_octets(Size::range(0, MAX_OFFER_DETAILS))?,
        })
    }
}

/// The details of an offer on a scheme over a settlement hub
/// ([`SettlementOffer::details`]): the hub the node is a member of
/// (spec/designs/settlement-hubs.md#11-choosing-a-hub), and its account
/// there.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct HubDetails {
    /// The hub's ID: the hash of its rules.
    pub hub_id: [u8; 32],
    /// The node's account number in the hub, or
    /// [`NO_ACCOUNT`](Self::NO_ACCOUNT).
    pub account: u32,
}

impl HubDetails {
    /// The account number of a member the hub has no account for yet, to
    /// be paid in by a peer with one
    /// (spec/designs/settlement-hubs.md#5-joining): a hub never gives out 0.
    pub const NO_ACCOUNT: u32 = 0;

    /// The bytes of its encoding: the ID and the account.
    pub const ENCODED_BYTES: usize = 32 + 4;
}

impl Encode for HubDetails {
    fn encoded_len(&self) -> usize {
        Self::ENCODED_BYTES
    }

    fn encode_to(&self, writer: &mut Writer<'_>) {
        writer.write_bytes(&self.hub_id);
        writer.write_u32(self.account);
    }
}

impl<'a> Decode<'a> for HubDetails {
    type Error = DecodeError;

    fn decode(reader: &mut Reader<'a>) -> Result<Self, DecodeError> {
        Ok(Self {
            hub_id: read_fixed::<32>(reader)?,
            account: reader.read_u32()?,
        })
    }
}

/// `PeerNodeInfo`: what a node says about itself. Its ID is not sent: it
/// derives from its two keys ([`NodeInfo::node_id`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NodeInfo<'a> {
    /// Its sequence number, a timestamp in ms that only goes up.
    pub sequence: u64,
    /// Its HTTPS origin.
    pub url: &'a str,
    /// A name for people, unverified.
    pub alias: &'a str,
    /// Its Ed25519 public key, which signs the link state.
    pub public_key: [u8; 32],
    /// Its SLH-DSA public key, which signs nothing yet, and from which its
    /// ID derives with the Ed25519 key (spec/areas/identity.md#node-id).
    pub slh_dsa_public_key: [u8; SLH_DSA_PUBLIC_KEY_BYTES],
    /// The schemes it takes peers on, each with its details, in its order
    /// of preference: at most [`MAX_OFFERS`]
    /// (spec/areas/peer-protocol.md#settlement-offers).
    pub settlement: Vec<SettlementOffer<'a>>,
    /// Its neighbors and extensions.
    pub entries: Vec<Entry<'a>>,
}

/// The most bytes the signed part of a node info takes with `neighbors`
/// neighbors, `offers` settlement offers whose details take
/// `details_bytes` together, and `extensions` extensions whose values take
/// `extension_bytes` together, at the longest URL, alias, node IDs and
/// scheme IDs: what a node checks its own link state against, so it never
/// signs one that others cannot decode.
#[must_use]
pub const fn max_node_info_bytes(
    neighbors: usize,
    offers: usize,
    details_bytes: usize,
    extensions: usize,
    extension_bytes: usize,
) -> usize {
    let entries = neighbors.saturating_add(extensions);
    let fixed = NODE_INFO_OID.len()
        + 8
        + size::prefixed(MAX_URL_BYTES)
        + size::prefixed(MAX_ALIAS_BYTES)
        + size::prefixed(32)
        + SLH_DSA_PUBLIC_KEY_BYTES
        + size::quantity(offers)
        + size::quantity(entries);
    // Each offer: its scheme and its details' length, at most as long as
    // the longest details'.
    let offer = size::prefixed(super::MAX_SCHEME_ID) + size::length_determinant(MAX_OFFER_DETAILS);
    let neighbor = size::tag(0) + dassie_identity::NODE_ID_BYTES;
    // Each extension: its tag, its type and its value's length, at most as
    // long as the longest value's.
    let extension = size::tag(0) + 2 + size::length_determinant(MAX_EXTENSION_BYTES);
    fixed
        .saturating_add(offers.saturating_mul(offer))
        .saturating_add(details_bytes)
        .saturating_add(neighbors.saturating_mul(neighbor))
        .saturating_add(extensions.saturating_mul(extension))
        .saturating_add(extension_bytes)
}

impl NodeInfo<'_> {
    /// Its ID, the one its two keys derive.
    #[must_use]
    pub fn node_id(&self) -> NodeId {
        NodeId::from_keys(&self.public_key, &self.slh_dsa_public_key)
    }

    /// The neighbors it advertises.
    pub fn neighbors(&self) -> impl Iterator<Item = NodeId> + '_ {
        self.entries.iter().filter_map(|entry| match entry {
            Entry::Neighbor(id) => Some(*id),
            _ => None,
        })
    }

    /// Whether it is a periphery node's: one without a URL, reached only
    /// over the connections it opened to its uplinks.
    #[must_use]
    pub fn is_periphery(&self) -> bool {
        self.has_extension(super::EXTENSION_PERIPHERY)
    }

    /// The UDP port it answers requests for its snapshot on, at its URL's
    /// host, if it names one: the first feed extension's, when that is a
    /// port. One that is not, of another length or 0, names none; it is
    /// still kept and forwarded, as the signature covers it.
    #[must_use]
    pub fn feed_port(&self) -> Option<u16> {
        let value = self.entries.iter().find_map(|entry| match entry {
            Entry::Extension { kind, value } if *kind == super::EXTENSION_FEED => Some(*value),
            _ => None,
        })?;
        let port = u16::from_be_bytes(*<&[u8; 2]>::try_from(value).ok()?);
        (port != 0).then_some(port)
    }

    fn has_extension(&self, wanted: u16) -> bool {
        self.entries
            .iter()
            .any(|entry| matches!(entry, Entry::Extension { kind, .. } if *kind == wanted))
    }
}

impl Encode for NodeInfo<'_> {
    fn encoded_len(&self) -> usize {
        NODE_INFO_OID.len()
            + 8
            + size::prefixed(self.url.len())
            + size::prefixed(self.alias.len())
            + size::prefixed(32)
            + SLH_DSA_PUBLIC_KEY_BYTES
            + size::quantity(self.settlement.len())
            + self
                .settlement
                .iter()
                .map(Encode::encoded_len)
                .sum::<usize>()
            + size::quantity(self.entries.len())
            + self.entries.iter().map(Encode::encoded_len).sum::<usize>()
    }

    fn encode_to(&self, writer: &mut Writer<'_>) {
        writer.write_bytes(&NODE_INFO_OID);
        writer.write_u64(self.sequence);
        writer.write_ascii(self.url, Size::range(0, MAX_URL_BYTES), Alphabet::VISIBLE);
        writer.write_utf8(self.alias, Size::range(0, MAX_ALIAS_BYTES));
        writer.write_octets(&self.public_key, Size::exact(32));
        SlhDsaPublicKey::from_bytes(self.slh_dsa_public_key).encode_to(writer);
        writer.write_quantity(self.settlement.len());
        for offer in &self.settlement {
            offer.encode_to(writer);
        }
        writer.write_quantity(self.entries.len());
        for entry in &self.entries {
            entry.encode_to(writer);
        }
    }
}

impl<'a> Decode<'a> for NodeInfo<'a> {
    type Error = DecodeError;

    fn decode(reader: &mut Reader<'a>) -> Result<Self, DecodeError> {
        reader.expect_bytes(&NODE_INFO_OID)?;
        let sequence = reader.read_u64()?;
        let url = reader.read_ascii(Size::range(0, MAX_URL_BYTES), Alphabet::VISIBLE)?;
        let alias = reader.read_utf8(Size::range(0, MAX_ALIAS_BYTES))?;
        let key = reader.read_octets(Size::exact(32))?;
        let mut public_key = [0u8; 32];
        public_key.copy_from_slice(key);
        let slh_dsa_public_key = *SlhDsaPublicKey::decode(reader)?.as_bytes();
        let count = reader.read_quantity(MAX_OFFERS)?;
        let mut settlement = Vec::with_capacity(count);
        for _ in 0..count {
            settlement.push(SettlementOffer::decode(reader)?);
        }
        let count = reader.read_quantity(MAX_ENTRIES)?;
        let mut entries = Vec::with_capacity(count);
        for _ in 0..count {
            entries.push(Entry::decode(reader)?);
        }
        Ok(Self {
            sequence,
            url,
            alias,
            public_key,
            slh_dsa_public_key,
            settlement,
            entries,
        })
    }
}

/// Why a link state does not verify.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InvalidNodeInfo {
    /// The URL is not an HTTPS origin, nor empty in a periphery node's link
    /// state (spec/areas/peer-protocol.md#verification).
    Url,
    /// The public key is not a point, or one of small order.
    PublicKey,
    /// The signature does not verify.
    Signature,
}

/// `SignedPeerNodeInfo`: node info and its signature, with the bytes both
/// were read from, and the ID its key derives.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SignedNodeInfo<'a> {
    /// The node's ID, derived from the key as the state is read.
    pub node_id: NodeId,
    /// The whole encoding, as stored and forwarded.
    pub bytes: &'a [u8],
    /// The node info's own bytes, which the signature covers.
    pub signed: &'a [u8],
    /// The node info.
    pub info: NodeInfo<'a>,
    /// The Ed25519 signature.
    pub signature: [u8; 64],
}

impl SignedNodeInfo<'_> {
    /// Checks the URL syntax, the key, and the signature (ZIP-215,
    /// spec/areas/peer-protocol.md#verification).
    /// A periphery node's link state has no URL, and every other one a
    /// valid one: a URL with the periphery extension would be taken for a
    /// downlink's and dialled, never probed.
    /// Returns the node's identity, its two keys, whose ID is
    /// [`Self::node_id`].
    ///
    /// # Errors
    ///
    /// The first check that fails.
    pub fn verify(&self) -> Result<NodeIdentity, InvalidNodeInfo> {
        let url = &self.info.url;
        let valid = if self.info.is_periphery() {
            url.is_empty()
        } else {
            is_valid_url(url)
        };
        if !valid {
            return Err(InvalidNodeInfo::Url);
        }
        let key =
            PublicKey::from_bytes(&self.info.public_key).map_err(|_| InvalidNodeInfo::PublicKey)?;
        if key.is_weak() {
            return Err(InvalidNodeInfo::PublicKey);
        }
        key.verify(self.signed, &Signature::from_bytes(self.signature))
            .map_err(|_| InvalidNodeInfo::Signature)?;
        let identity = NodeIdentity::new(
            key,
            SlhDsaPublicKey::from_bytes(self.info.slh_dsa_public_key),
        );
        Ok(identity)
    }
}

impl<'a> Decode<'a> for SignedNodeInfo<'a> {
    type Error = DecodeError;

    fn decode(reader: &mut Reader<'a>) -> Result<Self, DecodeError> {
        let start = reader.position();
        let offset = reader.offset();
        let (info, signed) = reader.read_prefixed(|inner| {
            let start = inner.position();
            let info = NodeInfo::decode(inner)?;
            Ok::<_, DecodeError>((info, inner.consumed_since(start)))
        })?;
        if signed.len() > MAX_NODE_INFO_BYTES {
            return Err(DecodeError::new(offset, DecodeErrorKind::SizeOutOfRange));
        }
        expect_tag(reader, 0)?;
        let signature = read_fixed::<64>(reader)?;
        Ok(Self {
            node_id: info.node_id(),
            bytes: reader.consumed_since(start),
            signed,
            info,
            signature,
        })
    }
}

/// Encodes and signs `info`: the `SignedPeerNodeInfo` bytes.
#[must_use]
pub fn sign_node_info(key: &NodeKey, info: &NodeInfo<'_>) -> Vec<u8> {
    let signed = info.to_vec();
    let signature = key.sign(&signed);
    let length = size::prefixed(signed.len()) + size::tag(0) + 64;
    let mut bytes = alloc::vec![0u8; length];
    let mut writer = Writer::new(&mut bytes);
    writer.write_prefixed(signed.len(), |inner| inner.write_bytes(&signed));
    writer.write_context_tag(0);
    writer.write_bytes(signature.as_bytes());
    bytes
}
