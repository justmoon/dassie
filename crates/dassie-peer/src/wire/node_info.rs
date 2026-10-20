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

    /// Whether it takes the peer protocol over QUIC.
    #[must_use]
    pub fn speaks_quic(&self) -> bool {
        self.has_extension(super::EXTENSION_QUIC)
    }

    /// Whether it is a periphery node's: one without a URL, reached only
    /// over the connections it opened to its uplinks.
    #[must_use]
    pub fn is_periphery(&self) -> bool {
        self.has_extension(super::EXTENSION_PERIPHERY)
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
