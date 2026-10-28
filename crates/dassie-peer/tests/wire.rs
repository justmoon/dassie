//! The v1 wire format: v0's layouts where v1 keeps them (checked against
//! the original's vectors, from testdata/original, with each node ID as v1
//! writes it), the v1 changes (spec/areas/peer-protocol.md#versions), and
//! decoders that never panic.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic,
    clippy::unreachable
)]

mod support;

use std::collections::BTreeMap;

use dassie_identity::{IdentityKey, NodeId, PublicKey, Signature};
use dassie_oer::{Decode, Encode};
use dassie_peer::names::{Commitment, MAX_NAME_BYTES, Name, Salt};
use dassie_peer::wire::{
    Content, EXTENSION_FEED, EXTENSION_PERIPHERY, EXTENSION_POST_QUANTUM, Entry, HubDetails,
    InvalidNodeInfo, Kind, LinkStateResponse, MAX_COMMITMENT_BYTES, MAX_NAME_LIST,
    MAX_NAME_LIST_BYTES, MAX_OFFER_DETAILS, MAX_OFFERS, MAX_SNAPSHOT_BYTES, MAX_URL_BYTES,
    MethodResponse, NameHolding, NameListResponse, NameReceipt, NodeInfo, NodeListResponse,
    PeeringRequest, PeeringResponse, RefusalReason, SchemeId, SettlementOffer, SignedNodeInfo,
    SubscriptionResponse, TerminationReason, decode_expiry, decode_hash, decode_octets,
    decode_settling, encode_expiry, encode_octets, encode_settling, is_valid_url, peek_kind,
    sign_node_info,
};
use proptest::prelude::*;
use sha2::{Digest, Sha256};
use support::inputs::{KINDS, content, mutate, mutations};

fn vectors() -> serde_json::Value {
    let text = include_str!("../../dassie-oer/tests/data/vectors.json");
    serde_json::from_str::<serde_json::Value>(text).unwrap()["peer"].clone()
}

fn hex(text: &str) -> Vec<u8> {
    (0..text.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&text[i..i + 2], 16).unwrap())
        .collect()
}

fn to_hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn vector(name: &str) -> Vec<u8> {
    let vectors = vectors();
    let value = &vectors[name];
    hex(value
        .as_str()
        .or_else(|| value["encoded"].as_str())
        .unwrap())
}

/// The v0 node IDs of nodes A and B, base64url after a `d`.
const V0_IDS: [&str; 2] = [
    "dagvMuaJff2H5DrnJw8llmK-WZ83WrSzY",
    "dRLajGQUsQnlsol-B7F0Uf_og05hZo4C3",
];

/// A v0 vector with each node ID as v1 writes it: its 24 digest bytes in
/// place of v0's length and base64url text (spec/areas/identity.md#node-id).
fn with_v1_ids(v0: &[u8]) -> Vec<u8> {
    let mut bytes = v0.to_vec();
    for (text, key) in V0_IDS.iter().zip([node_a(), node_b()]) {
        let mut old = vec![u8::try_from(text.len()).unwrap()];
        old.extend(text.as_bytes());
        let new = key.node_id().to_bytes();
        let mut replaced = Vec::new();
        let mut rest = bytes.as_slice();
        while let Some(at) = rest.windows(old.len()).position(|window| window == old) {
            replaced.extend(&rest[..at]);
            replaced.extend(new);
            rest = &rest[at + old.len()..];
        }
        replaced.extend(rest);
        bytes = replaced;
    }
    bytes
}

/// Node A's key, made once: its SLH-DSA key takes a key generation.
fn node_a() -> &'static IdentityKey {
    static KEY: std::sync::OnceLock<IdentityKey> = std::sync::OnceLock::new();
    KEY.get_or_init(|| IdentityKey::from_bytes(&[1; 32]))
}

/// Node B's key, made once.
fn node_b() -> &'static IdentityKey {
    static KEY: std::sync::OnceLock<IdentityKey> = std::sync::OnceLock::new();
    KEY.get_or_init(|| IdentityKey::from_bytes(&[3; 32]))
}

fn stub() -> SchemeId {
    SchemeId::parse("stub").unwrap()
}

/// Node A's info as in the v0 vector: the same fields, but its ID, and its
/// scheme offered in the settlement list rather than listed as an entry.
fn info_a() -> NodeInfo<'static> {
    NodeInfo {
        sequence: 1_700_000_000_000,
        url: "https://a.example",
        alias: "Node A",
        public_key: *node_a().public_key().as_bytes(),
        slh_dsa_public_key: *node_a().identity().slh_dsa().as_bytes(),
        settlement: vec![SettlementOffer {
            scheme: stub(),
            details: &[],
        }],
        entries: vec![Entry::Neighbor(node_b().node_id())],
    }
}

// The node-info layout is v0's, with each node ID as its 24 bytes, but
// without the node's own ID, which derives from the keys sent instead,
// with the SLH-DSA key's 32 bytes after the Ed25519 key, and with the
// schemes in a list of offers after the keys rather than among the
// entries, under a new OID, so that an info signed under another layout
// cannot pass as a link state of this one.
#[test]
fn node_info_is_the_v0_layout_without_the_id_and_with_offers() {
    assert!(NodeInfo::decode_exact(&vector("nodeInfo")).is_err());
    let v0 = with_v1_ids(&vector("nodeInfo"));
    let v1 = info_a().to_vec();
    let id = node_a().node_id().to_bytes();
    let key = node_a().public_key();
    let slh_dsa = node_a().identity().slh_dsa();
    assert_eq!(v0[13], 0x02);
    assert_eq!(v1[13], 0x06);
    assert_eq!(v1[..13], v0[..13]);
    assert_eq!(v0[14..14 + id.len()], id[..]);
    // v0's entries: a neighbor, `neighbor [0]`, and a scheme,
    // `settlementScheme [1]`, two of them.
    let neighbor = [&[0x80][..], &node_b().node_id().to_bytes()].concat();
    let entries = [&[0x01, 0x02][..], &neighbor, &[0x81, 0x04], b"stub"].concat();
    assert!(v0.ends_with(&entries));
    let fields = &v0[14 + id.len()..v0.len() - entries.len()];
    assert!(fields.ends_with(key.as_bytes()));
    // v1: the fields up to the Ed25519 key, the SLH-DSA key, the one offer,
    // on `stub` with no details, and the one entry left, the neighbor.
    let offers = [&[0x01, 0x01, 0x04][..], b"stub", &[0x00]].concat();
    let expected = [
        &v0[..13],
        &[0x06],
        fields,
        slh_dsa.as_bytes(),
        &offers,
        &[0x01, 0x01],
        &neighbor,
    ]
    .concat();
    assert_eq!(v1, expected);
    assert!(NodeInfo::decode_exact(&v0).is_err());
}

// The original's signature over the v0 info checks out, now that it is
// verified at all.
#[test]
fn the_v0_signature_verifies() {
    let signature: [u8; 64] = vector("signature").try_into().unwrap();
    let key = PublicKey::from_bytes(node_a().public_key().as_bytes()).unwrap();
    assert_eq!(
        key.verify(&vector("nodeInfo"), &Signature::from_bytes(signature)),
        Ok(())
    );
}

#[test]
fn signed_node_info_round_trips_and_verifies() {
    let bytes = sign_node_info(node_a(), &info_a());
    let signed = SignedNodeInfo::decode_exact(&bytes).unwrap();
    assert_eq!(signed.bytes, &bytes[..]);
    assert_eq!(signed.info, info_a());
    assert_eq!(signed.verify().unwrap(), node_a().identity());
    assert_eq!(signed.node_id, node_a().node_id());
    assert_eq!(signed.info.node_id(), node_a().node_id());
    assert_eq!(
        signed.info.neighbors().collect::<Vec<_>>(),
        [node_b().node_id()]
    );
    assert_eq!(
        signed
            .info
            .settlement
            .iter()
            .map(|offer| offer.scheme)
            .collect::<Vec<_>>(),
        [stub()]
    );
}

#[test]
fn a_link_state_must_be_its_nodes_and_well_formed() {
    let check = |info: NodeInfo<'_>, key: &IdentityKey| {
        let bytes = sign_node_info(key, &info);
        SignedNodeInfo::decode_exact(&bytes).unwrap().verify()
    };
    let mut path = info_a();
    path.url = "https://a.example/peer";
    assert_eq!(check(path, node_a()), Err(InvalidNodeInfo::Url));
    assert_eq!(check(info_a(), node_b()), Err(InvalidNodeInfo::Signature));
}

// Under ZIP-215, R of small order and s = 0 verify any message under a key
// of small order: such keys are refused, or anyone could sign for them.
#[test]
fn keys_of_small_order_are_refused() {
    let mut weak = [0u8; 32];
    weak[0] = 1;
    let info = NodeInfo {
        public_key: weak,
        ..info_a()
    };
    let signed = info.to_vec();
    let mut signature = [0u8; 64];
    signature[0] = 1;
    assert_eq!(
        PublicKey::from_bytes(&weak)
            .unwrap()
            .verify(&signed, &Signature::from_bytes(signature)),
        Ok(())
    );
    let mut bytes = Vec::new();
    bytes.extend(encoded_length(signed.len()));
    bytes.extend(&signed);
    bytes.push(0x80);
    bytes.extend(signature);
    let decoded = SignedNodeInfo::decode_exact(&bytes).unwrap();
    assert_eq!(decoded.verify(), Err(InvalidNodeInfo::PublicKey));
}

fn encoded_length(length: usize) -> Vec<u8> {
    if length < 128 {
        vec![u8::try_from(length).unwrap()]
    } else {
        vec![0x81, u8::try_from(length).unwrap()]
    }
}

#[test]
fn node_lists_and_their_hashes_match_the_original() {
    let list = NodeListResponse(vec![node_a().node_id(), node_b().node_id()]);
    assert_eq!(list.to_vec(), with_v1_ids(&vector("nodeList")));
    let vectors = vectors();
    let empty = NodeListResponse(Vec::new());
    assert_eq!(empty.to_vec(), vector("emptyNodeList"));
    assert_eq!(
        to_hex(&Sha256::digest(empty.to_vec())),
        vectors["emptyNodeList"]["sha256"].as_str().unwrap()
    );
    assert_eq!(
        NodeListResponse::decode_exact(&with_v1_ids(&vector("nodeList"))).unwrap(),
        list
    );
}

// An `interledgerPacket` is v0's without the length v0 wrapped its fields
// in: flat, as a `methodPacket` is.
#[test]
fn interledger_packets_are_flat() {
    let content = Content::InterledgerPacket {
        request_id: 42,
        packet: &[0x0c],
    };
    let v0 = vector("interledgerContent");
    assert_eq!(usize::from(v0[1]), v0.len() - 2);
    let flat: Vec<u8> = [v0[0]].into_iter().chain(v0[2..].iter().copied()).collect();
    assert_eq!(content.to_vec(), flat);
    assert_eq!(Content::decode_exact(&flat).unwrap(), content);
    assert!(Content::decode_exact(&v0).is_err());
}

// A request is its content: no version byte, since the connection's ALPN
// names the layouts (spec/decisions/protocol-upgrades.md). A v0 envelope,
// which starts with one, is no message.
#[test]
fn a_request_is_its_content() {
    assert!(Content::decode_exact(&vector("nodeListHashRequest")).is_err());
    let body = Content::NodeListHashRequest.to_vec();
    assert_eq!(
        Content::decode_exact(&body).unwrap(),
        Content::NodeListHashRequest
    );
    assert_eq!(peek_kind(&body), Some(Kind::NodeListHashRequest));
}

// The v1 encodings, pinned: a change to them is a change to the protocol.
#[test]
fn v1_encodings_are_pinned() {
    // The content alone: who sent it is the connection's, and which
    // version its ALPN.
    assert_eq!(to_hex(&Content::NodeListRequest.to_vec()), "86");
    assert!(Content::decode_exact(&[]).is_err(), "no content");
    assert_eq!(to_hex(&PeeringResponse::Accepted.to_vec()), "80");
    assert_eq!(
        to_hex(&PeeringResponse::Refused(RefusalReason::Capacity).to_vec()),
        "8103"
    );
}

#[test]
fn peering_requests_round_trip() {
    let info = sign_node_info(node_a(), &info_a());
    let request = Content::PeeringRequest(PeeringRequest {
        scheme: stub(),
        node_info: SignedNodeInfo::decode_exact(&info).unwrap(),
        data: &[1, 2, 3],
    });
    let bytes = request.to_vec();
    assert_eq!(bytes[0], 0x80);
    assert_eq!(Content::decode_exact(&bytes).unwrap(), request);
    // Nothing follows the scheme's data.
    let mut trailing = bytes.clone();
    trailing.push(0);
    assert!(Content::decode_exact(&trailing).is_err());
}

#[test]
fn link_state_requests_and_answers_are_bounded() {
    let ids: Vec<NodeId> = (0..17).map(|_| node_a().node_id()).collect();
    let request = Content::LinkStateRequest(ids.clone());
    assert!(Content::decode_exact(&request.to_vec()).is_err());
    let request = Content::LinkStateRequest(ids[..16].to_vec());
    assert_eq!(Content::decode_exact(&request.to_vec()).unwrap(), request);
    let info = sign_node_info(node_a(), &info_a());
    let answer = LinkStateResponse(vec![
        None,
        Some(SignedNodeInfo::decode_exact(&info).unwrap()),
    ]);
    assert_eq!(
        LinkStateResponse::decode_exact(&answer.to_vec()).unwrap(),
        answer
    );
}

// Settlements carry their sequence, and tag 9 is `peeringTermination`
// (spec/areas/peer-protocol.md#message-set).
#[test]
fn settlements_and_terminations_are_pinned() {
    let settlement = Content::Settlement {
        scheme: stub(),
        amount: 1_100_000_000,
        data: &[0xab],
        sequence: 7,
    };
    assert_eq!(
        to_hex(&settlement.to_vec()),
        "84047374756200000000419\
         0ab0001ab0000000000000007"
    );
    let termination = Content::PeeringTermination(TerminationReason::Owner);
    assert_eq!(to_hex(&termination.to_vec()), "8901");
    assert_eq!(
        Content::decode_exact(&[0x89, 0x03]).unwrap(),
        Content::PeeringTermination(TerminationReason::NotPeered)
    );
    // Unknown reasons read as unspecified.
    assert_eq!(
        Content::decode_exact(&[0x89, 0x63]).unwrap(),
        Content::PeeringTermination(TerminationReason::Unspecified)
    );
    assert_eq!(Kind::from_tag(9), Some(Kind::PeeringTermination));
    // spec/decisions/own-fulfills.md: a settlement request names its scheme
    // alone.
    let request = Content::SettlementRequest {
        scheme: SchemeId::parse("stub").unwrap(),
    };
    assert_eq!(to_hex(&request.to_vec()), "8a0473747562");
    assert_eq!(Kind::from_tag(10), Some(Kind::SettlementRequest));
    assert!(Content::decode_exact(&[0x8a]).is_err());
    assert_eq!(
        to_hex(&PeeringResponse::Refused(RefusalReason::Terminating).to_vec()),
        "8108"
    );
}

proptest! {
    #[test]
    fn decoders_never_panic(bytes in proptest::collection::vec(any::<u8>(), 0..512)) {
        let _ = Content::decode_exact(&bytes);
        let _ = SignedNodeInfo::decode_exact(&bytes);
        let _ = PeeringResponse::decode_exact(&bytes);
        let _ = LinkStateResponse::decode_exact(&bytes);
        let _ = NodeListResponse::decode_exact(&bytes);
        let _ = NameReceipt::decode_exact(&bytes);
        let _ = NameListResponse::decode_exact(&bytes);
        let _ = SubscriptionResponse::decode_exact(&bytes);
        let _ = peek_kind(&bytes);
    }

    #[test]
    fn settlements_round_trip(
        amount in any::<u64>(),
        data in proptest::collection::vec(any::<u8>(), 0..300),
        sequence in any::<u64>(),
    ) {
        let content = Content::Settlement { scheme: stub(), amount, data: &data, sequence };
        let bytes = content.to_vec();
        prop_assert_eq!(Content::decode_exact(&bytes).unwrap(), content);
    }
}

/// Node A's info with an offer over a hub and an entry of every kind,
/// signed.
fn signed_full() -> Vec<u8> {
    let mut info = info_a();
    info.settlement.push(SettlementOffer {
        scheme: SchemeId::parse("hub-xrp").unwrap(),
        details: &[0x11; HubDetails::ENCODED_BYTES],
    });
    info.entries.push(Entry::Extension {
        kind: 1,
        value: &[7; 40],
    });
    sign_node_info(node_a(), &info)
}

/// Decodes content as the node does, and checks a link state it carries.
/// What decodes encodes to something that decodes the same.
fn decode_content(bytes: &[u8]) -> Result<(), TestCaseError> {
    let Ok(content) = Content::decode_exact(bytes) else {
        return Ok(());
    };
    prop_assert_eq!(peek_kind(bytes), Some(content.kind()));
    if let Content::PeeringRequest(PeeringRequest {
        node_info: info, ..
    })
    | Content::LinkStateUpdate(info)
    | Content::Registration(info) = &content
    {
        let _ = info.verify();
    }
    let encoded = content.to_vec();
    prop_assert_eq!(Content::decode_exact(&encoded), Ok(content));
    Ok(())
}

/// How many kinds of response body there are.
const RESPONSES: usize = 11;

/// A valid response body of kind `kind`.
fn response(kind: usize) -> Vec<u8> {
    let signed = signed_full();
    match kind {
        0 => PeeringResponse::Accepted.to_vec(),
        1 => PeeringResponse::Refused(RefusalReason::Capacity).to_vec(),
        2 => LinkStateResponse(vec![
            None,
            Some(SignedNodeInfo::decode_exact(&signed).unwrap()),
        ])
        .to_vec(),
        3 => NodeListResponse(vec![node_a().node_id(), node_b().node_id()]).to_vec(),
        4 => vec![7; 32],
        5 => encode_expiry(1_800_000_000_000),
        6 => encode_octets(b"peering info"),
        7 => encode_settling(433_793_000),
        8 => NameReceipt {
            position: 7,
            window_end_ms: 1_700_000_000_000,
        }
        .to_vec(),
        9 => name_list().to_vec(),
        10 => MethodResponse::Accepted.to_vec(),
        _ => unreachable!(),
    }
}

/// Decodes a response body of kind `kind` as the node does.
fn decode_response(kind: usize, body: &[u8]) -> Result<(), TestCaseError> {
    match kind {
        0 | 1 => {
            if let Ok(response) = PeeringResponse::decode_exact(body) {
                let encoded = response.to_vec();
                prop_assert_eq!(PeeringResponse::decode_exact(&encoded), Ok(response));
            }
        }
        2 => {
            if let Ok(response) = LinkStateResponse::decode_exact(body) {
                for info in response.0.iter().flatten() {
                    let _ = info.verify();
                }
                prop_assert_eq!(response.to_vec(), body);
            }
        }
        3 => {
            if let Ok(list) = NodeListResponse::decode_exact(body) {
                prop_assert_eq!(list.to_vec(), body);
            }
        }
        4 => prop_assert_eq!(decode_hash(body).is_ok(), body.len() == 32),
        5 => prop_assert_eq!(decode_expiry(body).is_ok(), body.len() == 8),
        6 => {
            if let Ok(data) = decode_octets(body) {
                prop_assert_eq!(encode_octets(data), body);
            }
        }
        7 => prop_assert_eq!(decode_settling(body).is_ok(), body.len() == 8),
        8 => prop_assert_eq!(
            NameReceipt::decode_exact(body).is_ok(),
            body.len() == NameReceipt::ENCODED_BYTES
        ),
        9 => {
            if let Ok(list) = NameListResponse::decode_exact(body) {
                prop_assert_eq!(list.to_vec(), body);
            }
        }
        10 => {
            if let Ok(response) = MethodResponse::decode_exact(body) {
                let encoded = response.to_vec();
                prop_assert_eq!(MethodResponse::decode_exact(&encoded), Ok(response));
            }
        }
        _ => unreachable!(),
    }
    Ok(())
}

/// The node info a link state signs.
fn signed_part(signed: &[u8]) -> &[u8] {
    SignedNodeInfo::decode_exact(signed).unwrap().signed
}

/// Text that is, or nearly is, a node URL: hostnames, IP literals, ports.
fn url() -> impl Strategy<Value = String> {
    prop_oneof![
        any::<String>(),
        r"https://[a-zA-Z0-9.:@/?%\[\]-]{0,40}",
        r"https://[a-z0-9-]{0,70}(\.[a-z0-9-]{0,70}){0,4}(:[0-9]{0,6})?",
        r"https://[0-9.]{0,20}(:[0-9]{0,6})?",
        r"https://\[[0-9a-fA-F:.]{0,45}\](:[0-9]{0,6})?",
    ]
}

// Random bytes rarely get past the first fields: these break valid
// encodings instead, so that every field, and the link states and entries
// inside them, see bad input. Each case breaks every kind the same way, so
// that each kind sees every case.
proptest! {
    #[test]
    fn broken_contents_never_panic(mutations in mutations()) {
        let signed = signed_full();
        for tag in 0..KINDS {
            decode_content(&mutate(&content(tag, &signed), &mutations))?;
        }
    }

    // A broken link state verifies only if what it signs is untouched.
    #[test]
    fn broken_link_states_never_panic(mutations in mutations()) {
        let signed = signed_full();
        let bytes = mutate(&signed, &mutations);
        if let Ok(info) = SignedNodeInfo::decode_exact(&bytes)
            && info.verify().is_ok()
        {
            prop_assert_eq!(info.signed, signed_part(&signed));
        }
        let unsigned = mutate(signed_part(&signed), &mutations);
        if let Ok(info) = NodeInfo::decode_exact(&unsigned) {
            let encoded = info.to_vec();
            prop_assert_eq!(NodeInfo::decode_exact(&encoded), Ok(info));
        }
    }

    #[test]
    fn broken_entries_never_panic(mutations in mutations()) {
        let signed = signed_full();
        for entry in SignedNodeInfo::decode_exact(&signed).unwrap().info.entries {
            let bytes = mutate(&entry.to_vec(), &mutations);
            if let Ok(entry) = Entry::decode_exact(&bytes) {
                prop_assert_eq!(entry.to_vec(), bytes);
            }
        }
    }

    #[test]
    fn broken_responses_never_panic(mutations in mutations()) {
        for kind in 0..RESPONSES {
            decode_response(kind, &mutate(&response(kind), &mutations))?;
        }
    }

    #[test]
    fn response_bodies_never_panic(bytes in proptest::collection::vec(any::<u8>(), 0..80)) {
        for kind in 0..RESPONSES {
            decode_response(kind, &bytes)?;
        }
    }

    #[test]
    fn urls_never_panic(url in url()) {
        if is_valid_url(&url) {
            prop_assert!(url.starts_with("https://") && url.len() <= MAX_URL_BYTES);
        }
    }

    #[test]
    fn scheme_ids_never_panic(text in prop_oneof![any::<String>(), "[a-z0-9-]{0,70}"]) {
        if let Some(scheme) = SchemeId::parse(&text) {
            prop_assert_eq!(scheme.as_str(), text.as_str());
        }
    }

    // Any key, ID, URL, signed bytes and signature: checking them never
    // panics, and a signature nobody made never verifies.
    #[test]
    fn link_state_checks_never_panic(
        public_key in prop_oneof![Just(*node_a().public_key().as_bytes()), any::<[u8; 32]>()],
        own_id in any::<bool>(),
        url in prop_oneof![Just("https://a.example".to_owned()), url()],
        signed in prop_oneof![
            Just(info_a().to_vec()),
            proptest::collection::vec(any::<u8>(), 0..300),
        ],
        signature in proptest::collection::vec(any::<u8>(), 64),
    ) {
        let node_id = if own_id {
            NodeId::from_keys(&public_key, node_a().identity().slh_dsa().as_bytes())
        } else {
            node_b().node_id()
        };
        let info = SignedNodeInfo {
            node_id,
            bytes: &[],
            signed: &signed,
            info: NodeInfo {
                url: &url,
                public_key,
                ..info_a()
            },
            signature: signature.try_into().unwrap(),
        };
        prop_assert!(info.verify().is_err());
    }
}

// The answer to a settlement request is the amount on its way, eight bytes
// big-endian, and nothing else.
#[test]
fn a_settlement_requests_answer_is_the_amount_on_its_way() {
    assert_eq!(
        encode_settling(0x0102_0304_0506_0708),
        [1, 2, 3, 4, 5, 6, 7, 8]
    );
    assert_eq!(
        decode_settling(&[1, 2, 3, 4, 5, 6, 7, 8]),
        Ok(0x0102_0304_0506_0708)
    );
    assert!(decode_settling(&[]).is_err());
    assert!(decode_settling(&[1, 2, 3]).is_err());
}

// spec/areas/peer-protocol.md#versions: `dassie/2` numbers its messages
// from 0 without gaps, and a tag past the last is no message, so a node
// refuses it as malformed, whatever follows it.
#[test]
fn message_tags_have_no_gaps() {
    for tag in 0..u64::try_from(KINDS).unwrap() {
        assert_eq!(Kind::from_tag(tag).unwrap().tag(), tag);
    }
    let past = u64::try_from(KINDS).unwrap();
    assert_eq!(Kind::from_tag(past), None);
    assert!(Content::decode_exact(&[0x80 | u8::try_from(past).unwrap()]).is_err());
    assert!(Content::decode_exact(&hex("9300000007010c02abab")).is_err());
    assert!(Kind::MethodPacket.carries_packet() && Kind::InterledgerPacket.carries_packet());
    assert!(!Kind::Settlement.carries_packet());
}

/// A signed link state of node A with these settlement offers.
fn with_offers(offers: &[(&str, &[u8])]) -> Vec<u8> {
    let mut info = info_a();
    info.settlement = offers
        .iter()
        .map(|(scheme, details)| SettlementOffer {
            scheme: SchemeId::parse(scheme).unwrap(),
            details,
        })
        .collect();
    sign_node_info(node_a(), &info)
}

// spec/areas/peer-protocol.md#settlement-offers: an offer is its scheme
// and the scheme's details, in their canonical OER; over a hub, the hub's
// ID and the node's account in it, nothing else.
#[test]
fn offers_and_hub_details_are_pinned() {
    let hub = HubDetails {
        hub_id: [0x11; 32],
        account: 0x0102_0304,
    };
    let details = hub.to_vec();
    assert_eq!(details.len(), HubDetails::ENCODED_BYTES);
    assert_eq!(to_hex(&details), format!("{}01020304", "11".repeat(32)));
    assert_eq!(HubDetails::decode_exact(&details), Ok(hub));
    let offer = SettlementOffer {
        scheme: SchemeId::parse("hub-a").unwrap(),
        details: &details,
    };
    assert_eq!(
        to_hex(&offer.to_vec()),
        format!("056875622d6124{}01020304", "11".repeat(32))
    );
    let stub_offer = SettlementOffer {
        scheme: stub(),
        details: &[],
    };
    assert_eq!(to_hex(&stub_offer.to_vec()), "047374756200");
    // Short or long details are no hub's.
    for broken in [&details[..35], &[details.as_slice(), &[0]].concat(), &[]] {
        assert!(HubDetails::decode_exact(broken).is_err());
    }
    // Offers of any scheme and details within the bounds are kept as they
    // came, and the link state verifies: what the details say is for a
    // node that knows the scheme to read.
    let bytes = with_offers(&[("hub-a", &details), ("other", &[1, 2, 3]), ("hub-a", &[])]);
    let signed = SignedNodeInfo::decode_exact(&bytes).unwrap();
    assert!(signed.verify().is_ok());
    assert_eq!(signed.info.settlement.len(), 3);
    assert_eq!(signed.info.settlement[1].details, [1, 2, 3]);
    assert_eq!(signed.info.to_vec(), signed.signed);
}

// spec/areas/peer-protocol.md#settlement-offers: every node decodes the
// same bounds, `MAX_OFFERS` offers of at most `MAX_OFFER_DETAILS` bytes of
// details each; a link state past either is malformed.
#[test]
fn offers_are_bounded() {
    let longest = vec![0xab; MAX_OFFER_DETAILS];
    let bytes = with_offers(&[("other", &longest)]);
    assert!(SignedNodeInfo::decode_exact(&bytes).is_ok());
    // An offer one byte past: `other`, then the details' length, 257, and
    // the details.
    let mut too_long = [&[0x05][..], b"other", &[0x82, 0x01, 0x01]].concat();
    too_long.extend([0xab; MAX_OFFER_DETAILS + 1]);
    assert!(SettlementOffer::decode_exact(&too_long).is_err());
    too_long[8] = 0x00;
    too_long.pop();
    assert!(SettlementOffer::decode_exact(&too_long).is_ok());
    let names: Vec<String> = (0..=MAX_OFFERS).map(|n| format!("s{n}")).collect();
    let most: Vec<(&str, &[u8])> = names[..MAX_OFFERS]
        .iter()
        .map(|name| (name.as_str(), &[][..]))
        .collect();
    assert!(SignedNodeInfo::decode_exact(&with_offers(&most)).is_ok());
    let too_many: Vec<(&str, &[u8])> = names.iter().map(|name| (name.as_str(), &[][..])).collect();
    assert!(SignedNodeInfo::decode_exact(&with_offers(&too_many)).is_err());
}

// spec/areas/peer-protocol.md#link-states: an entry is a neighbor (tag 0)
// or an extension (tag 1), and a link state with any other is malformed.
#[test]
fn entries_are_neighbors_and_extensions() {
    let mut info = info_a().to_vec();
    // The last entry, the neighbor, with tag 2 in place of 0.
    let at = info.len() - 1 - dassie_identity::NODE_ID_BYTES;
    assert_eq!(info[at], 0x80);
    assert!(NodeInfo::decode_exact(&info).is_ok());
    info[at] = 0x82;
    assert!(NodeInfo::decode_exact(&info).is_err());
}

/// A signed link state of node A with these extensions.
fn with_extensions(extensions: &[(u16, &[u8])]) -> Vec<u8> {
    let mut info = info_a();
    info.entries.extend(
        extensions
            .iter()
            .map(|(kind, value)| Entry::Extension { kind: *kind, value }),
    );
    sign_node_info(node_a(), &info)
}

/// The largest offers a node makes: `MAX_SCHEMES` of them at the longest
/// scheme IDs, the first `MAX_OWN_HUBS` over hubs, at the largest accounts,
/// and the others with no details, as the stub's.
fn largest_offers() -> Vec<dassie_peer::Offer> {
    let offers: Vec<dassie_peer::Offer> = (0..dassie_peer::MAX_SCHEMES)
        .map(|n| {
            let text = format!("{n:02}{}", "a".repeat(dassie_peer::wire::MAX_SCHEME_ID - 2));
            let scheme = SchemeId::parse(&text).unwrap();
            if n < dassie_peer::MAX_OWN_HUBS {
                let hub = HubDetails {
                    hub_id: [u8::try_from(n).unwrap(); 32],
                    account: u32::MAX,
                };
                dassie_peer::Offer::over_hub(scheme, hub)
            } else {
                dassie_peer::Offer::plain(scheme)
            }
        })
        .collect();
    assert!(dassie_peer::offers_keep_the_rules(&offers));
    offers
}

// spec/areas/peer-protocol.md#link-states: the bound a node checks its own
// link state against holds for the largest it can sign, at the longest URL,
// alias and scheme IDs, with every offer and extension; and the default
// configuration's link state fits what every node decodes.
#[test]
fn the_largest_own_link_state_fits_its_bound() {
    let neighbors = dassie_peer::Config::DEFAULT.max_peers;
    let key = IdentityKey::from_bytes(&[7; 32]);
    let url = "a".repeat(MAX_URL_BYTES);
    let alias = "\u{e9}".repeat(dassie_peer::wire::MAX_ALIAS_BYTES / 2);
    let ids: Vec<NodeId> = (0..neighbors)
        .map(|n| IdentityKey::from_bytes(&[u8::try_from(n).unwrap() + 8; 32]).node_id())
        .collect();
    let offers = largest_offers();
    let details: Vec<Vec<u8>> = offers.iter().map(dassie_peer::Offer::details).collect();
    let port = 4433u16.to_be_bytes();
    let extensions: [(u16, &[u8]); 1] = [(dassie_peer::wire::EXTENSION_FEED, &port)];
    let info = NodeInfo {
        sequence: u64::MAX,
        url: &url,
        alias: &alias,
        public_key: *key.public_key().as_bytes(),
        slh_dsa_public_key: *key.identity().slh_dsa().as_bytes(),
        settlement: offers
            .iter()
            .zip(&details)
            .map(|(offer, details)| SettlementOffer {
                scheme: offer.scheme,
                details,
            })
            .collect(),
        entries: ids
            .iter()
            .map(|id| Entry::Neighbor(*id))
            .chain(
                extensions
                    .iter()
                    .map(|(kind, value)| Entry::Extension { kind: *kind, value }),
            )
            .collect(),
    };
    let bound = dassie_peer::wire::max_node_info_bytes(
        neighbors,
        dassie_peer::MAX_SCHEMES,
        details.iter().map(Vec::len).sum(),
        extensions.len(),
        port.len(),
    );
    assert!(
        info.encoded_len() <= bound,
        "{} > {bound}",
        info.encoded_len()
    );
    assert!(bound <= dassie_peer::wire::MAX_NODE_INFO_BYTES);
    let bytes = sign_node_info(&key, &info);
    assert_eq!(SignedNodeInfo::decode_exact(&bytes).unwrap().info, info);
}

fn name(text: &str) -> Name {
    Name::parse(text).unwrap()
}

/// A name list of two names, node A holding `alice` and node B `bob`.
fn name_list() -> NameListResponse {
    NameListResponse(BTreeMap::from([
        (
            name("bob"),
            NameHolding {
                holder: node_b().node_id(),
                expires_at_ms: 1_700_000_000_000,
            },
        ),
        (
            name("alice"),
            NameHolding {
                holder: node_a().node_id(),
                expires_at_ms: 1_800_000_000_000,
            },
        ),
    ]))
}

// spec/areas/peer-protocol.md#message-set: the name messages are tags 11
// to 15. Committing, unsealing and renewing change state and need
// authentication; the list requests only read.
#[test]
fn name_messages_are_pinned() {
    let commit = Content::NameCommit(Commitment([0xab; 32]));
    let unseal = Content::NameUnseal {
        name: name("alice"),
        salt: Salt([7; 32]),
    };
    let renew = Content::NameRenew(name("alice"));
    let pinned = [
        (commit, format!("8b{}", "ab".repeat(32))),
        (unseal, format!("8c05616c696365{}", "07".repeat(32))),
        (renew, "8d05616c696365".to_owned()),
        (Content::NameListHashRequest, "8e".to_owned()),
        (Content::NameListRequest, "8f".to_owned()),
    ];
    for (tag, (content, hex)) in (11..).zip(pinned) {
        assert_eq!(to_hex(&content.to_vec()), hex);
        assert_eq!(Content::decode_exact(&content.to_vec()).unwrap(), content);
        assert_eq!(Kind::from_tag(tag), Some(content.kind()));
        assert_eq!(content.kind().tag(), tag);
    }
    assert_eq!(Kind::from_tag(16), Some(Kind::MethodPacket));
    for kind in [Kind::NameCommit, Kind::NameUnseal, Kind::NameRenew] {
        assert!(!kind.is_read_only());
    }
    for kind in [Kind::NameListHashRequest, Kind::NameListRequest] {
        assert!(kind.is_read_only());
    }
    for kind in (11..=15).filter_map(Kind::from_tag) {
        assert!(!kind.carries_packet());
    }
}

// A commitment and a salt are exactly 32 bytes, and a name is read by its
// syntax (spec/designs/names.md#7-names), whether or not it is reserved.
#[test]
fn name_messages_are_bounded() {
    let commit = hex(&format!("8b{}", "ab".repeat(32)));
    assert!(Content::decode_exact(&commit[..32]).is_err());
    let mut long = commit.clone();
    long.push(0xab);
    assert!(Content::decode_exact(&long).is_err());
    let unseal = hex(&format!("8c05616c696365{}", "07".repeat(32)));
    assert!(Content::decode_exact(&unseal[..unseal.len() - 1]).is_err());
    let mut long = unseal.clone();
    long.push(7);
    assert!(Content::decode_exact(&long).is_err());
    let renew = |text: &str| {
        let mut bytes = vec![0x8d, u8::try_from(text.len()).unwrap()];
        bytes.extend(text.as_bytes());
        Content::decode_exact(&bytes).map(|content| content.to_vec())
    };
    let longest = "a".repeat(MAX_NAME_BYTES);
    for valid in ["www", "a-b", longest.as_str()] {
        assert_eq!(
            renew(valid),
            Ok(Content::NameRenew(name(valid)).to_vec()),
            "{valid}"
        );
    }
    let too_long = "a".repeat(MAX_NAME_BYTES + 1);
    for invalid in [
        "ab",
        "Alice",
        "-alice",
        "alice-",
        "xn--alice",
        "al.ce",
        &too_long,
    ] {
        assert!(renew(invalid).is_err(), "{invalid}");
    }
}

// The commitment is SHA-256 over the domain OID, the name with its length
// ahead of it, the holder's node ID's 24 bytes and the salt: a golden value
// computed apart from the code.
#[test]
fn a_commitment_is_pinned() {
    let commitment = Commitment::seal(&name("alice"), &node_a().node_id(), &Salt([7; 32]));
    assert_eq!(
        to_hex(&commitment.0),
        "e457d804a7887a42f744b011e446be17b9b22f821857838939620fa1c9ebf06d"
    );
    assert_eq!(
        to_hex(dassie_peer::names::COMMITMENT_OID.as_slice()),
        "0d2b0601040183a878058a770104"
    );
}

// A receipt is the commitment's position in the log and when its window
// closes; a name list is sorted by name, each once, and bounded.
#[test]
fn name_answers_are_pinned_and_bounded() {
    let receipt = NameReceipt {
        position: 7,
        window_end_ms: 1_700_000_000_000,
    };
    assert_eq!(
        to_hex(&receipt.to_vec()),
        "00000000000000070000018bcfe56800"
    );
    assert_eq!(NameReceipt::decode_exact(&receipt.to_vec()), Ok(receipt));
    let id = |key: &IdentityKey| to_hex(&key.node_id().to_bytes());
    let bytes = name_list().to_vec();
    assert_eq!(
        to_hex(&bytes),
        format!(
            "0102\
             05616c696365{}000001a3185c5000\
             03626f62{}0000018bcfe56800",
            id(node_a()),
            id(node_b())
        )
    );
    assert_eq!(NameListResponse::decode_exact(&bytes), Ok(name_list()));
    assert_eq!(to_hex(&NameListResponse::default().to_vec()), "0100");

    // Out of order, or a name twice: malformed.
    let (count, entries) = bytes.split_at(2);
    let (alice, bob) = entries.split_at(1 + 5 + 24 + 8);
    let swapped = [count, bob, alice].concat();
    assert!(NameListResponse::decode_exact(&swapped).is_err());
    let twice = [count, alice, alice].concat();
    assert!(NameListResponse::decode_exact(&twice).is_err());

    // At most MAX_NAME_LIST names, and the largest list fits its bound.
    let longest_id = node_a().node_id();
    let mut largest = NameListResponse(
        (0..=MAX_NAME_LIST)
            .map(|n| {
                let text = format!("{n:0>width$}", width = MAX_NAME_BYTES);
                let holding = NameHolding {
                    holder: longest_id,
                    expires_at_ms: u64::MAX,
                };
                (name(&text), holding)
            })
            .collect(),
    );
    assert!(NameListResponse::decode_exact(&largest.to_vec()).is_err());
    largest.0.pop_last();
    assert_eq!(largest.to_vec().len(), MAX_NAME_LIST_BYTES);
    assert_eq!(
        NameListResponse::decode_exact(&largest.to_vec()),
        Ok(largest)
    );
}

// spec/areas/peer-protocol.md#message-set: the settlement-method messages
// are tags 16 to 18 (spec/decisions/settlement-methods.md). Each changes
// state, or moves money, and needs authentication; a `methodPacket`
// carries a Prepare, with a commitment that is empty on a method not over a
// hub, and bounded.
#[test]
fn settlement_method_messages_are_pinned_and_bounded() {
    let packet = Content::MethodPacket {
        request_id: 7,
        scheme: stub(),
        packet: &[0x0c],
        commitment: &[0xab, 0xab],
    };
    assert_eq!(to_hex(&packet.to_vec()), "90000000070473747562010c02abab");
    let direct = Content::MethodPacket {
        request_id: 7,
        scheme: stub(),
        packet: &[0x0c],
        commitment: &[],
    };
    assert_eq!(to_hex(&direct.to_vec()), "90000000070473747562010c00");
    let add = Content::MethodAdd {
        scheme: stub(),
        data: &[0xab],
    };
    assert_eq!(to_hex(&add.to_vec()), "91047374756201ab");
    let drop = Content::MethodDrop { scheme: stub() };
    assert_eq!(to_hex(&drop.to_vec()), "920473747562");
    for (tag, content) in [(16, &packet), (17, &add), (18, &drop)] {
        assert_eq!(
            Content::decode_exact(&content.to_vec()).as_ref(),
            Ok(content)
        );
        let kind = Kind::from_tag(tag).unwrap();
        assert_eq!((content.kind(), kind.tag()), (kind, tag));
        assert!(!kind.is_read_only());
        assert_eq!(kind.carries_packet(), kind == Kind::MethodPacket);
    }
    assert_eq!(Content::decode_exact(&direct.to_vec()).unwrap(), direct);
    assert_eq!(Kind::from_tag(21), None);
    assert!(Content::decode_exact(&[0x95]).is_err());

    // A commitment past the bound is malformed; one at it is not.
    let at_bound = Content::MethodPacket {
        request_id: 7,
        scheme: stub(),
        packet: &[0x0c],
        commitment: &[0xab; MAX_COMMITMENT_BYTES],
    };
    let mut bytes = at_bound.to_vec();
    assert_eq!(Content::decode_exact(&bytes).unwrap(), at_bound);
    assert_eq!(bytes[12..15], [0x82, 0x01, 0x00]);
    bytes[14] = 0x01;
    bytes.push(0xab);
    assert!(Content::decode_exact(&bytes).is_err());
    // Nothing may follow.
    for content in [&packet, &add, &drop] {
        let mut trailing = content.to_vec();
        trailing.push(0);
        assert!(Content::decode_exact(&trailing).is_err());
    }
}

// The answer to a `methodAdd` is shaped as a peering answer's.
#[test]
fn method_answers_are_pinned() {
    let accepted = MethodResponse::Accepted;
    assert_eq!(to_hex(&accepted.to_vec()), "80");
    let refused = MethodResponse::Refused(RefusalReason::Capacity);
    assert_eq!(to_hex(&refused.to_vec()), "8103");
    for response in [accepted, refused] {
        assert_eq!(
            MethodResponse::decode_exact(&response.to_vec()),
            Ok(response)
        );
    }
    // Unknown reasons, past the last, read as unspecified; unknown tags are
    // malformed.
    for code in [0x0a, 0x63] {
        assert_eq!(
            MethodResponse::decode_exact(&[0x81, code]),
            Ok(MethodResponse::Refused(RefusalReason::Unspecified))
        );
    }
    assert!(MethodResponse::decode_exact(&[0x82, 0x00]).is_err());
}

// spec/decisions/peer-swaps.md: a `subscription` is a BOOLEAN, and a
// `snapshot` an OCTET STRING of one to `MAX_SNAPSHOT_BYTES`, opaque to the
// peer protocol; the answer to a subscription is shaped as a peering
// answer's, and `NoSwaps` is the reason after the last before it.
#[test]
fn subscriptions_and_snapshots_are_pinned() {
    let subscribe = Content::Subscription { subscribe: true };
    let unsubscribe = Content::Subscription { subscribe: false };
    assert_eq!(to_hex(&subscribe.to_vec()), "93ff");
    assert_eq!(to_hex(&unsubscribe.to_vec()), "9300");
    let snapshot = Content::Snapshot(&[0xab, 0xcd]);
    assert_eq!(to_hex(&snapshot.to_vec()), "9402abcd");
    for (tag, content) in [(19, &subscribe), (19, &unsubscribe), (20, &snapshot)] {
        assert_eq!(
            Content::decode_exact(&content.to_vec()).as_ref(),
            Ok(content)
        );
        let kind = Kind::from_tag(tag).unwrap();
        assert_eq!((content.kind(), kind.tag()), (kind, tag));
        assert!(!kind.is_read_only());
        assert!(!kind.carries_packet());
        let mut trailing = content.to_vec();
        trailing.push(0);
        assert!(Content::decode_exact(&trailing).is_err());
    }
    // A BOOLEAN is 0x00 or 0xFF, nothing else.
    for byte in [0x01, 0x7f, 0xfe] {
        assert!(Content::decode_exact(&[0x93, byte]).is_err());
    }
    // A snapshot is never empty, and at most one datagram.
    assert!(Content::decode_exact(&[0x94, 0x00]).is_err());
    let at_bound = Content::Snapshot(&[0x5a; MAX_SNAPSHOT_BYTES]);
    let mut bytes = at_bound.to_vec();
    assert_eq!(bytes.len(), 1 + 3 + MAX_SNAPSHOT_BYTES);
    assert_eq!(Content::decode_exact(&bytes).unwrap(), at_bound);
    // One byte longer, its length says so too.
    bytes[3] += 1;
    bytes.push(0x5a);
    assert!(Content::decode_exact(&bytes).is_err());

    assert_eq!(RefusalReason::NoSwaps.code(), 9);
    assert_eq!(RefusalReason::from_code(9), RefusalReason::NoSwaps);
    let accepted = SubscriptionResponse::Accepted;
    let refused = SubscriptionResponse::Refused(RefusalReason::NoSwaps);
    assert_eq!(to_hex(&accepted.to_vec()), "80");
    assert_eq!(to_hex(&refused.to_vec()), "8109");
    for response in [accepted, refused] {
        assert_eq!(
            SubscriptionResponse::decode_exact(&response.to_vec()),
            Ok(response)
        );
    }
    assert_eq!(
        SubscriptionResponse::decode_exact(&[0x81, 0x0a]),
        Ok(SubscriptionResponse::Refused(RefusalReason::Unspecified))
    );
    assert!(SubscriptionResponse::decode_exact(&[0x82]).is_err());
}

// spec/areas/peer-protocol.md#link-states: `dassie/2` numbers its
// extension types from 1 without gaps, and keeps one of a type it does not
// know, empty or not, as it came: the link state still verifies.
#[test]
fn extension_types_have_no_gaps_and_unknown_ones_are_kept() {
    assert_eq!(
        [EXTENSION_POST_QUANTUM, EXTENSION_PERIPHERY, EXTENSION_FEED],
        [1, 2, 3]
    );
    for kind in [EXTENSION_POST_QUANTUM, 4, 7, u16::MAX] {
        for value in [&[][..], &[0][..], &[0x11; 37][..]] {
            let bytes = with_extensions(&[(kind, value)]);
            let signed = SignedNodeInfo::decode_exact(&bytes).unwrap();
            assert!(signed.verify().is_ok());
            assert!(
                signed
                    .info
                    .entries
                    .contains(&Entry::Extension { kind, value })
            );
            assert!(!signed.info.is_periphery());
            assert_eq!(signed.info.feed_port(), None);
        }
    }
}

proptest! {
    #[test]
    fn method_messages_round_trip(
        request_id in any::<u32>(),
        scheme in "[a-z0-9-]{2,64}",
        packet in proptest::collection::vec(any::<u8>(), 0..300),
        commitment in proptest::collection::vec(any::<u8>(), 0..=MAX_COMMITMENT_BYTES),
    ) {
        let scheme = SchemeId::parse(&scheme).unwrap();
        let contents = [
            Content::MethodPacket { request_id, scheme, packet: &packet, commitment: &commitment },
            Content::MethodAdd { scheme, data: &packet },
            Content::MethodDrop { scheme },
        ];
        for content in contents {
            let bytes = content.to_vec();
            prop_assert_eq!(bytes.len(), content.encoded_len());
            prop_assert_eq!(Content::decode_exact(&bytes), Ok(content));
        }
    }

    // A link state with any offers decodes when, and only when, they are
    // within the bounds, and then encodes to the bytes it came from, kept as
    // they came whatever their schemes and details say. Details past their
    // bound cannot be written: `offers_are_bounded` reads them.
    #[test]
    fn offers_round_trip_within_their_bounds(
        offers in proptest::collection::vec(
            ("[a-z0-9-]{2,64}", proptest::collection::vec(any::<u8>(), 0..64)),
            0..=MAX_OFFERS + 1,
        ),
    ) {
        let offers: Vec<(&str, &[u8])> = offers
            .iter()
            .map(|(scheme, details)| (scheme.as_str(), details.as_slice()))
            .collect();
        let bytes = with_offers(&offers);
        let within = offers.len() <= MAX_OFFERS;
        match SignedNodeInfo::decode_exact(&bytes) {
            Ok(signed) => {
                prop_assert!(within);
                prop_assert_eq!(signed.info.settlement.len(), offers.len());
                prop_assert_eq!(signed.info.to_vec(), signed.signed);
            }
            // Past the bounds, or past the bytes every node decodes.
            Err(_) => prop_assert!(!within || bytes.len() > dassie_peer::wire::MAX_NODE_INFO_BYTES),
        }
    }
}
