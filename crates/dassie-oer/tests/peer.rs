//! The peer-protocol structures (spec/areas/peer-protocol.md#encoding),
//! built on the codec as
//! a message crate would build them. They exercise the features no other
//! vector covers: OID constants, CHOICE tags, SEQUENCE OF, CONTAINING and
//! capturing the raw bytes of a nested value for signature checks. The
//! vectors were produced by the original's `peer-schema.ts`.

mod support;

use dassie_oer::{
    Alphabet, Decode, DecodeError, DecodeErrorKind, Encode, Reader, Size, Writer, oid, size,
};
use support::{hex, str_field, to_hex, vectors};

const PEER_NODE_INFO_OID: [u8; 14] = oid::encode(&[1, 3, 6, 1, 4, 1, 54392, 5, 1399, 1, 2]);
const NODE_ID_SIZE: Size = Size::range(2, 45);
const NODE_ID_ALPHABET: Alphabet = Alphabet::EMPTY
    .with_range(b'a', b'z')
    .with_range(b'A', b'Z')
    .with_range(b'0', b'9')
    .with_chars(b"-_");
const SCHEME_SIZE: Size = Size::range(2, 128);
const SCHEME_ALPHABET: Alphabet = Alphabet::EMPTY
    .with_range(b'a', b'z')
    .with_range(b'0', b'9')
    .with_chars(b"-");
const MAX_ENTRIES: usize = 64;

#[derive(Debug, PartialEq)]
enum Entry<'a> {
    Neighbor(&'a str),
    SettlementScheme(&'a str),
}

impl Encode for Entry<'_> {
    fn encoded_len(&self) -> usize {
        match self {
            Self::Neighbor(id) | Self::SettlementScheme(id) => {
                size::tag(0) + size::prefixed(id.len())
            }
        }
    }

    fn encode_to(&self, writer: &mut Writer<'_>) {
        match self {
            Self::Neighbor(id) => {
                writer.write_context_tag(0);
                writer.write_ascii(id, NODE_ID_SIZE, NODE_ID_ALPHABET);
            }
            Self::SettlementScheme(id) => {
                writer.write_context_tag(1);
                writer.write_ascii(id, SCHEME_SIZE, SCHEME_ALPHABET);
            }
        }
    }
}

impl<'a> Decode<'a> for Entry<'a> {
    type Error = DecodeError;

    fn decode(reader: &mut Reader<'a>) -> Result<Self, DecodeError> {
        match reader.read_context_tag()? {
            0 => Ok(Self::Neighbor(
                reader.read_ascii(NODE_ID_SIZE, NODE_ID_ALPHABET)?,
            )),
            1 => Ok(Self::SettlementScheme(
                reader.read_ascii(SCHEME_SIZE, SCHEME_ALPHABET)?,
            )),
            _ => Err(reader.error(DecodeErrorKind::UnexpectedTag)),
        }
    }
}

#[derive(Debug, PartialEq)]
struct NodeInfo<'a> {
    node_id: &'a str,
    sequence: u64,
    url: &'a str,
    alias: &'a str,
    public_key: &'a [u8],
    entries: Vec<Entry<'a>>,
}

impl Encode for NodeInfo<'_> {
    fn encoded_len(&self) -> usize {
        PEER_NODE_INFO_OID.len()
            + size::prefixed(self.node_id.len())
            + 8
            + size::prefixed(self.url.len())
            + size::prefixed(self.alias.len())
            + size::prefixed(self.public_key.len())
            + size::quantity(self.entries.len())
            + self.entries.iter().map(Encode::encoded_len).sum::<usize>()
    }

    fn encode_to(&self, writer: &mut Writer<'_>) {
        writer.write_bytes(&PEER_NODE_INFO_OID);
        writer.write_ascii(self.node_id, NODE_ID_SIZE, NODE_ID_ALPHABET);
        writer.write_u64(self.sequence);
        writer.write_ascii(self.url, Size::ANY, Alphabet::VISIBLE);
        writer.write_utf8(self.alias, Size::ANY);
        writer.write_octets(self.public_key, Size::ANY);
        writer.write_quantity(self.entries.len());
        for entry in &self.entries {
            entry.encode_to(writer);
        }
    }
}

impl<'a> Decode<'a> for NodeInfo<'a> {
    type Error = DecodeError;

    fn decode(reader: &mut Reader<'a>) -> Result<Self, DecodeError> {
        reader.expect_bytes(&PEER_NODE_INFO_OID)?;
        let node_id = reader.read_ascii(NODE_ID_SIZE, NODE_ID_ALPHABET)?;
        let sequence = reader.read_u64()?;
        let url = reader.read_ascii(Size::ANY, Alphabet::VISIBLE)?;
        let alias = reader.read_utf8(Size::ANY)?;
        let public_key = reader.read_octets(Size::ANY)?;
        let count = reader.read_quantity(MAX_ENTRIES)?;
        let entries = (0..count)
            .map(|_| Entry::decode(reader))
            .collect::<Result<_, _>>()?;
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

/// SignedPeerNodeInfo: the node info as an OCTET STRING CONTAINING it, whose
/// raw bytes are kept for verifying the signature over them.
struct SignedNodeInfo<'a> {
    signed_bytes: &'a [u8],
    node_info: NodeInfo<'a>,
    signature: &'a [u8; 64],
}

impl<'a> Decode<'a> for SignedNodeInfo<'a> {
    type Error = DecodeError;

    fn decode(reader: &mut Reader<'a>) -> Result<Self, DecodeError> {
        let (node_info, signed_bytes) = reader.read_prefixed(|inner| {
            let start = inner.position();
            let node_info = NodeInfo::decode(inner)?;
            Ok((node_info, inner.consumed_since(start)))
        })?;
        if reader.read_context_tag()? != 0 {
            return Err(reader.error(DecodeErrorKind::UnexpectedTag));
        }
        let signature = reader.read_array::<64>()?;
        Ok(Self {
            signed_bytes,
            node_info,
            signature,
        })
    }
}

#[test]
fn node_info_matches_the_original() {
    let vectors = vectors("vectors.json");
    let peer = &vectors["peer"];
    let encoded = hex(str_field(&peer["nodeInfo"], "encoded"));
    let value = &peer["nodeInfo"]["value"];
    let node_b = str_field(&peer["nodeInfo"], "nodeB");
    let public_key = hex(str_field(value, "publicKey"));

    let node_info = NodeInfo {
        node_id: str_field(value, "nodeId"),
        sequence: str_field(value, "sequence").parse().unwrap(),
        url: str_field(value, "url"),
        alias: str_field(value, "alias"),
        public_key: &public_key,
        entries: vec![Entry::Neighbor(node_b), Entry::SettlementScheme("stub")],
    };
    assert_eq!(to_hex(&node_info.to_vec()), to_hex(&encoded));
    assert_eq!(NodeInfo::decode_exact(&encoded), Ok(node_info));
}

#[test]
fn signed_node_info_captures_the_signed_bytes() {
    let vectors = vectors("vectors.json");
    let peer = &vectors["peer"];
    let signed = hex(str_field(peer, "signedNodeInfo"));
    let node_info = hex(str_field(&peer["nodeInfo"], "encoded"));
    let signature = hex(str_field(peer, "signature"));

    let decoded = SignedNodeInfo::decode_exact(&signed).unwrap();
    assert_eq!(decoded.signed_bytes, node_info.as_slice());
    assert_eq!(decoded.signature.as_slice(), signature.as_slice());
    assert_eq!(decoded.node_info.alias, "Node A");
    // The 157-octet content needs a long-form length determinant.
    assert_eq!(signed[..2], [0x81, 0x9d]);
}

#[test]
fn node_lists_match_the_original() {
    let vectors = vectors("vectors.json");
    let peer = &vectors["peer"];
    for key in ["nodeList", "emptyNodeList"] {
        let encoded = hex(str_field(&peer[key], "encoded"));
        let mut reader = Reader::new(&encoded);
        let count = reader.read_quantity(MAX_ENTRIES).unwrap();
        let ids: Vec<&str> = (0..count)
            .map(|_| reader.read_ascii(NODE_ID_SIZE, NODE_ID_ALPHABET).unwrap())
            .collect();
        reader.finish().unwrap();

        let length = size::quantity(ids.len())
            + ids.iter().map(|id| size::prefixed(id.len())).sum::<usize>();
        let mut buffer = vec![0; length];
        let mut writer = Writer::new(&mut buffer);
        writer.write_quantity(ids.len());
        for id in &ids {
            writer.write_ascii(id, NODE_ID_SIZE, NODE_ID_ALPHABET);
        }
        assert_eq!(buffer, encoded, "{key}");
    }
}

#[test]
fn peer_message_envelopes_decode() {
    // The v0 envelopes of the original's test vectors
    // (`tests/data/vectors.json`, from testdata/original). v1
    // changes the authentication (spec/decisions/peer-protocol-v1.md), but
    // the OER layout is the same, so these still test CHOICE and nested
    // CONTAINING decoding.
    let vectors = vectors("vectors.json");
    let peer = &vectors["peer"];

    let request = hex(str_field(peer, "nodeListHashRequest"));
    let mut reader = Reader::new(&request);
    assert_eq!(reader.read_u8(), Ok(0));
    reader.read_ascii(NODE_ID_SIZE, NODE_ID_ALPHABET).unwrap();
    assert_eq!(reader.read_context_tag(), Ok(0));
    assert_eq!(reader.read_context_tag(), Ok(6));
    reader.finish().unwrap();

    let update = hex(str_field(peer, "linkStateUpdate"));
    let mut reader = Reader::new(&update);
    assert_eq!(reader.read_u8(), Ok(0));
    reader.read_ascii(NODE_ID_SIZE, NODE_ID_ALPHABET).unwrap();
    assert_eq!(reader.read_context_tag(), Ok(0));
    assert_eq!(reader.read_context_tag(), Ok(1));
    let signed = SignedNodeInfo::decode(&mut reader).unwrap();
    reader.finish().unwrap();
    assert_eq!(signed.node_info.sequence, 1_700_000_000_000);

    let authenticated = hex(str_field(peer, "authenticatedInterledger"));
    let mut reader = Reader::new(&authenticated);
    assert_eq!(reader.read_u8(), Ok(0));
    reader.read_ascii(NODE_ID_SIZE, NODE_ID_ALPHABET).unwrap();
    assert_eq!(reader.read_context_tag(), Ok(1));
    let session_key = reader.read_array::<32>().unwrap();
    let mac = reader.read_array::<32>().unwrap();
    assert_eq!(to_hex(mac), str_field(peer, "mac"));
    assert_eq!(to_hex(session_key).len(), 64);
    assert_eq!(reader.read_context_tag(), Ok(2));
    let (request_id, packet) = reader
        .read_prefixed(|inner| {
            Ok::<_, DecodeError>((inner.read_u32()?, inner.read_octets(Size::ANY)?))
        })
        .unwrap();
    reader.finish().unwrap();
    assert_eq!((request_id, packet), (42, &[0x0c][..]));
}
