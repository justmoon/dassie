//! Node info, the signed link state (spec/areas/peer-protocol.md#link-states).

use alloc::vec::Vec;

use dassie_identity::{NodeId, NodeKey, PublicKey, Signature};
use dassie_oer::{
    Alphabet, Decode, DecodeError, DecodeErrorKind, Encode, Reader, Size, Writer, size,
};

use super::{
    MAX_ALIAS_BYTES, MAX_ENTRIES, MAX_EXTENSION_BYTES, MAX_NODE_INFO_BYTES, MAX_URL_BYTES,
    NODE_INFO_OID, SchemeId, expect_tag, is_valid_url, node_id_len, read_fixed, read_node_id,
    write_node_id,
};

/// An entry of a link state.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Entry<'a> {
    /// `neighbor [0]`: a node this one is peered with and advertises.
    Neighbor(NodeId),
    /// `settlementScheme [1]`: a scheme this node settles with.
    Scheme(SchemeId),
    /// `extension [2]`: anything later versions add, such as a post-quantum
    /// key (type 1). Unknown types are kept, since the signature covers
    /// them, and otherwise ignored.
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
                Self::Scheme(scheme) => scheme.encoded_len(),
                Self::Extension { value, .. } => 2 + size::prefixed(value.len()),
            }
    }

    fn encode_to(&self, writer: &mut Writer<'_>) {
        match self {
            Self::Neighbor(id) => {
                writer.write_context_tag(0);
                write_node_id(writer, id);
            }
            Self::Scheme(scheme) => {
                writer.write_context_tag(1);
                scheme.write(writer);
            }
            Self::Extension { kind, value } => {
                writer.write_context_tag(2);
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
            1 => Ok(Self::Scheme(SchemeId::read(reader)?)),
            2 => Ok(Self::Extension {
                kind: reader.read_u16()?,
                value: reader.read_octets(Size::range(0, MAX_EXTENSION_BYTES))?,
            }),
            _ => Err(DecodeError::new(offset, DecodeErrorKind::UnexpectedTag)),
        }
    }
}

/// The value of a hub extension ([`EXTENSION_HUB`](super::EXTENSION_HUB)):
/// the settlement hub a node is a member of
/// (spec/designs/settlement-hubs.md#11-choosing-a-hub).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct HubMembership {
    /// The hub's ID: the hash of its rules.
    pub hub_id: [u8; 32],
    /// The node's account number in the hub.
    pub account: u32,
    /// Whether it takes new peers over the hub.
    pub takes_peers: bool,
}

impl HubMembership {
    /// The bytes of its encoding: the ID, the account and the flag.
    pub const ENCODED_BYTES: usize = 32 + 4 + 1;
}

impl Encode for HubMembership {
    fn encoded_len(&self) -> usize {
        Self::ENCODED_BYTES
    }

    fn encode_to(&self, writer: &mut Writer<'_>) {
        writer.write_bytes(&self.hub_id);
        writer.write_u32(self.account);
        writer.write_bool(self.takes_peers);
    }
}

impl<'a> Decode<'a> for HubMembership {
    type Error = DecodeError;

    fn decode(reader: &mut Reader<'a>) -> Result<Self, DecodeError> {
        Ok(Self {
            hub_id: read_fixed::<32>(reader)?,
            account: reader.read_u32()?,
            takes_peers: reader.read_bool()?,
        })
    }
}

/// A settlement hub a node is a member of, with the scheme it settles on
/// over the hub, which names its asset: an item of a hub list
/// ([`EXTENSION_HUBS`](super::EXTENSION_HUBS)).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct HubListing {
    /// The scheme.
    pub scheme: SchemeId,
    /// The hub, the node's account in it and whether it takes new peers
    /// over it, as the hub extension names them.
    pub membership: HubMembership,
}

impl HubListing {
    /// The most bytes of its encoding: the longest scheme ID and the
    /// membership.
    pub const MAX_ENCODED_BYTES: usize =
        size::prefixed(super::MAX_SCHEME_ID) + HubMembership::ENCODED_BYTES;
}

impl Encode for HubListing {
    fn encoded_len(&self) -> usize {
        self.scheme.encoded_len() + HubMembership::ENCODED_BYTES
    }

    fn encode_to(&self, writer: &mut Writer<'_>) {
        self.scheme.write(writer);
        self.membership.encode_to(writer);
    }
}

impl<'a> Decode<'a> for HubListing {
    type Error = DecodeError;

    fn decode(reader: &mut Reader<'a>) -> Result<Self, DecodeError> {
        Ok(Self {
            scheme: SchemeId::read(reader)?,
            membership: HubMembership::decode(reader)?,
        })
    }
}

/// The value of a hub list extension ([`EXTENSION_HUBS`](super::EXTENSION_HUBS)):
/// the hubs a node is a member of, in its order, from one to
/// [`MAX_HUBS`](super::MAX_HUBS), no scheme and no hub twice. A node in
/// none leaves the extension out, so an empty list is not one.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HubList(pub Vec<HubListing>);

/// The most bytes a hub list of `hubs` hubs takes: what a node budgets for
/// its own.
#[must_use]
pub const fn hub_list_bytes(hubs: usize) -> usize {
    size::quantity(hubs).saturating_add(hubs.saturating_mul(HubListing::MAX_ENCODED_BYTES))
}

impl HubList {
    /// Whether it keeps the list's rules: one to [`MAX_HUBS`](super::MAX_HUBS)
    /// hubs, no scheme and no hub twice, since a hub settles one asset.
    #[must_use]
    pub fn is_valid(&self) -> bool {
        let hubs = &self.0;
        (1..=super::MAX_HUBS).contains(&hubs.len())
            && hubs.iter().enumerate().all(|(index, hub)| {
                hubs.iter().skip(index + 1).all(|other| {
                    other.scheme != hub.scheme && other.membership.hub_id != hub.membership.hub_id
                })
            })
    }
}

impl Encode for HubList {
    fn encoded_len(&self) -> usize {
        size::quantity(self.0.len()) + self.0.iter().map(Encode::encoded_len).sum::<usize>()
    }

    fn encode_to(&self, writer: &mut Writer<'_>) {
        writer.write_quantity(self.0.len());
        for hub in &self.0 {
            hub.encode_to(writer);
        }
    }
}

impl<'a> Decode<'a> for HubList {
    type Error = DecodeError;

    /// Reads a list, refusing one that breaks its rules
    /// ([`HubList::is_valid`]).
    fn decode(reader: &mut Reader<'a>) -> Result<Self, DecodeError> {
        let offset = reader.offset();
        let count = reader.read_quantity(super::MAX_HUBS)?;
        let mut hubs = Vec::with_capacity(count);
        for _ in 0..count {
            hubs.push(HubListing::decode(reader)?);
        }
        let list = Self(hubs);
        if !list.is_valid() {
            return Err(DecodeError::new(offset, DecodeErrorKind::SizeOutOfRange));
        }
        Ok(list)
    }
}

/// `PeerNodeInfo`: what a node says about itself.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NodeInfo<'a> {
    /// Its ID.
    pub node_id: NodeId,
    /// Its sequence number, a timestamp in ms that only goes up.
    pub sequence: u64,
    /// Its HTTPS origin.
    pub url: &'a str,
    /// A name for people, unverified.
    pub alias: &'a str,
    /// Its Ed25519 public key, from which `node_id` derives.
    pub public_key: [u8; 32],
    /// Its neighbors, schemes and extensions.
    pub entries: Vec<Entry<'a>>,
}

/// The most bytes the signed part of a node info takes with `neighbors`
/// neighbors, `schemes` schemes and `extensions` extensions whose values
/// take `extension_bytes` together, at the longest URL, alias, node IDs
/// and scheme IDs: what a node checks its own link state against, so it
/// never signs one that others cannot decode.
#[must_use]
pub const fn max_node_info_bytes(
    neighbors: usize,
    schemes: usize,
    extensions: usize,
    extension_bytes: usize,
) -> usize {
    let node_id = dassie_identity::NODE_ID_BYTES;
    let entries = neighbors.saturating_add(schemes).saturating_add(extensions);
    let fixed = NODE_INFO_OID.len()
        + node_id
        + 8
        + size::prefixed(MAX_URL_BYTES)
        + size::prefixed(MAX_ALIAS_BYTES)
        + size::prefixed(32)
        + size::quantity(entries);
    let neighbor = size::tag(0) + node_id;
    let scheme = size::tag(0) + size::prefixed(super::MAX_SCHEME_ID);
    // Each extension: its tag, its type and its value's length, at most as
    // long as the longest value's.
    let extension = size::tag(0) + 2 + size::length_determinant(MAX_EXTENSION_BYTES);
    fixed
        .saturating_add(neighbors.saturating_mul(neighbor))
        .saturating_add(schemes.saturating_mul(scheme))
        .saturating_add(extensions.saturating_mul(extension))
        .saturating_add(extension_bytes)
}

impl NodeInfo<'_> {
    /// The neighbors it advertises.
    pub fn neighbors(&self) -> impl Iterator<Item = NodeId> + '_ {
        self.entries.iter().filter_map(|entry| match entry {
            Entry::Neighbor(id) => Some(*id),
            _ => None,
        })
    }

    /// The schemes it settles with, in its order.
    pub fn schemes(&self) -> impl Iterator<Item = SchemeId> + '_ {
        self.entries.iter().filter_map(|entry| match entry {
            Entry::Scheme(id) => Some(*id),
            _ => None,
        })
    }

    /// Whether it is a periphery node's: one without a URL, reached only
    /// over the connections it opened to its uplinks.
    #[must_use]
    pub fn is_periphery(&self) -> bool {
        self.has_extension(super::EXTENSION_PERIPHERY)
    }

    /// The UDP port of its rate feed, at its URL's host, if it names one:
    /// the first feed extension's, when that is a port. One that is not, of
    /// another length or 0, names none; it is still kept and forwarded, as
    /// the signature covers it.
    #[must_use]
    pub fn feed_port(&self) -> Option<u16> {
        let value = self.entries.iter().find_map(|entry| match entry {
            Entry::Extension { kind, value } if *kind == super::EXTENSION_FEED => Some(*value),
            _ => None,
        })?;
        let port = u16::from_be_bytes(*<&[u8; 2]>::try_from(value).ok()?);
        (port != 0).then_some(port)
    }

    /// The settlement hub it is a member of, if it names one: the first hub
    /// extension's, when that is a canonical [`HubMembership`]. One that is
    /// not names none; it is still kept and forwarded, as the signature
    /// covers it.
    #[must_use]
    pub fn hub(&self) -> Option<HubMembership> {
        let value = self.entries.iter().find_map(|entry| match entry {
            Entry::Extension { kind, value } if *kind == super::EXTENSION_HUB => Some(*value),
            _ => None,
        })?;
        HubMembership::decode_exact(value).ok()
    }

    /// The settlement hubs it is a member of, each with its scheme: the
    /// first hub list extension's, when that is a canonical [`HubList`]
    /// that keeps its rules and whose first is the hub extension's
    /// ([`NodeInfo::hub`]), so that nodes that read the list and nodes that
    /// do not see the same first hub; empty otherwise. A node that names
    /// its hub only in the hub extension is of a version before hub lists,
    /// and settles over it on the hub scheme both nodes run.
    #[must_use]
    pub fn hubs(&self) -> Vec<HubListing> {
        self.entries
            .iter()
            .find_map(|entry| match entry {
                Entry::Extension { kind, value } if *kind == super::EXTENSION_HUBS => Some(*value),
                _ => None,
            })
            .and_then(|value| HubList::decode_exact(value).ok())
            .filter(|list| list.0.first().map(|first| first.membership) == self.hub())
            .map(|list| list.0)
            .unwrap_or_default()
    }

    /// Whether it has settlement methods
    /// (spec/decisions/settlement-methods.md), and so takes `methodPacket`,
    /// `methodAdd` and `methodDrop`: its first methods extension has an
    /// empty value. One with a value breaks the extension's rule, and says
    /// nothing.
    #[must_use]
    pub fn has_methods(&self) -> bool {
        self.entries
            .iter()
            .find_map(|entry| match entry {
                Entry::Extension { kind, value } if *kind == super::EXTENSION_METHODS => {
                    Some(*value)
                }
                _ => None,
            })
            .is_some_and(<[u8]>::is_empty)
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
            + node_id_len(&self.node_id)
            + 8
            + size::prefixed(self.url.len())
            + size::prefixed(self.alias.len())
            + size::prefixed(32)
            + size::quantity(self.entries.len())
            + self.entries.iter().map(Encode::encoded_len).sum::<usize>()
    }

    fn encode_to(&self, writer: &mut Writer<'_>) {
        writer.write_bytes(&NODE_INFO_OID);
        write_node_id(writer, &self.node_id);
        writer.write_u64(self.sequence);
        writer.write_ascii(self.url, Size::range(0, MAX_URL_BYTES), Alphabet::VISIBLE);
        writer.write_utf8(self.alias, Size::range(0, MAX_ALIAS_BYTES));
        writer.write_octets(&self.public_key, Size::exact(32));
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
        let node_id = read_node_id(reader)?;
        let sequence = reader.read_u64()?;
        let url = reader.read_ascii(Size::range(0, MAX_URL_BYTES), Alphabet::VISIBLE)?;
        let alias = reader.read_utf8(Size::range(0, MAX_ALIAS_BYTES))?;
        let key = reader.read_octets(Size::exact(32))?;
        let mut public_key = [0u8; 32];
        public_key.copy_from_slice(key);
        let count = reader.read_quantity(MAX_ENTRIES)?;
        let mut entries = Vec::with_capacity(count);
        for _ in 0..count {
            entries.push(Entry::decode(reader)?);
        }
        Ok(Self {
            node_id,
            sequence,
            url,
            alias,
            public_key,
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
    /// The node ID is not the public key's.
    NodeId,
    /// The signature does not verify.
    Signature,
}

/// `SignedPeerNodeInfo`: node info and its signature, with the bytes both
/// were read from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SignedNodeInfo<'a> {
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
    /// Checks the URL syntax, the key, that the ID is the key's, and the
    /// signature (ZIP-215, spec/areas/peer-protocol.md#verification).
    /// A periphery node's link state has no URL, and every other one a
    /// valid one: a URL with the periphery extension would be taken for a
    /// downlink's and dialled, never probed.
    /// Returns the key.
    ///
    /// # Errors
    ///
    /// The first check that fails.
    pub fn verify(&self) -> Result<PublicKey, InvalidNodeInfo> {
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
        if key.node_id() != self.info.node_id {
            return Err(InvalidNodeInfo::NodeId);
        }
        key.verify(self.signed, &Signature::from_bytes(self.signature))
            .map_err(|_| InvalidNodeInfo::Signature)?;
        Ok(key)
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
