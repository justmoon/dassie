//! The v1 wire format: v0's layouts where v1 keeps them (checked against
//! the original's vectors, spec/original/03-peer-network.md#55-test-vectors),
//! the v1 changes (spec/areas/peer-protocol.md#versions), and decoders that
//! never panic.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic,
    clippy::unreachable
)]

mod support;

use std::collections::BTreeMap;

use dassie_identity::{NodeId, NodeKey, PublicKey, Signature};
use dassie_oer::{Decode, Encode};
use dassie_peer::names::{Commitment, MAX_NAME_BYTES, Name, Salt};
use dassie_peer::wire::{
    Authentication, Content, EXTENSION_HUB, EXTENSION_HUBS, EXTENSION_METHODS, Entry, Envelope,
    EnvelopeError, HmacAuthentication, HubList, HubListing, HubMembership, InvalidNodeInfo, Kind,
    LinkStateResponse, MAX_COMMITMENT_BYTES, MAX_HUBS, MAX_NAME_LIST, MAX_NAME_LIST_BYTES,
    MAX_URL_BYTES, MAX_VERSIONS, MethodResponse, NameHolding, NameListResponse, NameReceipt,
    NodeInfo, NodeListResponse, PeeringRequest, PeeringResponse, RefusalReason, ResponseEnvelope,
    SchemeId, SignedNodeInfo, TerminationReason, decode_expiry, decode_hash, decode_octets,
    decode_settling, encode_envelope, encode_expiry, encode_octets, encode_response,
    encode_settling, hub_list_bytes, is_valid_url, peek_kind, sign_node_info,
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

fn node_a() -> NodeKey {
    NodeKey::from_bytes(&[1; 32])
}

fn node_b() -> NodeKey {
    NodeKey::from_bytes(&[3; 32])
}

fn stub() -> SchemeId {
    SchemeId::parse("stub").unwrap()
}

/// Node A's info as in the v0 vector: the same fields, signed as v1.
fn info_a() -> NodeInfo<'static> {
    NodeInfo {
        node_id: node_a().node_id(),
        sequence: 1_700_000_000_000,
        url: "https://a.example",
        alias: "Node A",
        public_key: *node_a().public_key().as_bytes(),
        entries: vec![Entry::Neighbor(node_b().node_id()), Entry::Scheme(stub())],
    }
}

// v1 keeps v0's node-info layout; only the OID's last arc changes, so a
// v0-signed info cannot pass as a v1 link state.
#[test]
fn node_info_keeps_the_v0_layout_under_a_new_oid() {
    let v0 = vector("nodeInfo");
    let v1 = info_a().to_vec();
    assert_eq!(v1.len(), v0.len());
    assert_eq!(v0[13], 0x02);
    assert_eq!(v1[13], 0x03);
    assert_eq!(v1[..13], v0[..13]);
    assert_eq!(v1[14..], v0[14..]);
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
    let bytes = sign_node_info(&node_a(), &info_a());
    let signed = SignedNodeInfo::decode_exact(&bytes).unwrap();
    assert_eq!(signed.bytes, &bytes[..]);
    assert_eq!(signed.info, info_a());
    assert_eq!(signed.verify().unwrap(), node_a().public_key());
    assert_eq!(
        signed.info.neighbors().collect::<Vec<_>>(),
        [node_b().node_id()]
    );
    assert_eq!(signed.info.schemes().collect::<Vec<_>>(), [stub()]);
}

#[test]
fn a_link_state_must_be_its_nodes_and_well_formed() {
    let check = |info: NodeInfo<'_>, key: &NodeKey| {
        let bytes = sign_node_info(key, &info);
        SignedNodeInfo::decode_exact(&bytes).unwrap().verify()
    };
    let mut other = info_a();
    other.node_id = node_b().node_id();
    assert_eq!(check(other, &node_a()), Err(InvalidNodeInfo::NodeId));
    let mut path = info_a();
    path.url = "https://a.example/peer";
    assert_eq!(check(path, &node_a()), Err(InvalidNodeInfo::Url));
    assert_eq!(check(info_a(), &node_b()), Err(InvalidNodeInfo::Signature));
}

// Under ZIP-215, R of small order and s = 0 verify any message under a key
// of small order: such keys are refused, or anyone could sign for them.
#[test]
fn keys_of_small_order_are_refused() {
    let mut weak = [0u8; 32];
    weak[0] = 1;
    let info = NodeInfo {
        node_id: NodeId::from_public_key(&weak),
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
    assert_eq!(list.to_vec(), vector("nodeList"));
    let vectors = vectors();
    assert_eq!(
        to_hex(&Sha256::digest(list.to_vec())),
        vectors["nodeList"]["sha256"].as_str().unwrap()
    );
    let empty = NodeListResponse(Vec::new());
    assert_eq!(empty.to_vec(), vector("emptyNodeList"));
    assert_eq!(
        to_hex(&Sha256::digest(empty.to_vec())),
        vectors["emptyNodeList"]["sha256"].as_str().unwrap()
    );
    assert_eq!(
        NodeListResponse::decode_exact(&vector("nodeList")).unwrap(),
        list
    );
}

#[test]
fn interledger_packets_keep_the_v0_layout() {
    let content = Content::InterledgerPacket {
        request_id: 42,
        packet: &[0x0c],
    };
    assert_eq!(content.to_vec(), vector("interledgerContent"));
    assert_eq!(
        Content::decode_exact(&vector("interledgerContent")).unwrap(),
        content
    );
}

// A v0 envelope is told apart from a malformed one, so the node can answer
// "unsupported version".
#[test]
fn a_v0_envelope_is_another_version() {
    assert_eq!(
        Envelope::decode(&vector("nodeListHashRequest")),
        Err(EnvelopeError::Version(0))
    );
    let mut v1 = vector("nodeListHashRequest");
    v1[0] = 1;
    let envelope = Envelope::decode(&v1).unwrap();
    assert_eq!(envelope.sender, node_a().node_id());
    assert_eq!(envelope.authentication, Authentication::None);
    assert_eq!(
        Content::decode_exact(envelope.content).unwrap(),
        Content::NodeListHashRequest
    );
    assert_eq!(peek_kind(&v1), Some(Kind::NodeListHashRequest));
}

fn hmac() -> HmacAuthentication {
    HmacAuthentication {
        sender_key: *node_a().public_key().as_bytes(),
        session_id: [5; 16],
        counter: 7,
        timestamp_ms: 1_700_000_000_123,
        mac: [9; 32],
    }
}

// The v1 encodings, pinned: a change to them is a change to the protocol.
#[test]
fn v1_encodings_are_pinned() {
    let content = Content::NodeListRequest.to_vec();
    let envelope = encode_envelope(&node_a().node_id(), &Authentication::Hmac(hmac()), &content);
    assert_eq!(
        to_hex(&envelope),
        "0121646167764d75614a666632483544726e4a77386c6c6d4b2d575a38335772537a59\
         818a88e3dd7409f195fd52db2d3cba5d72ca6709bf1d94121bf3748801b40f6f5c\
         05050505050505050505050505050505\
         0000000000000007\
         0000018bcfe5687b\
         0909090909090909090909090909090909090909090909090909090909090909\
         87"
    );
    let decoded = Envelope::decode(&envelope).unwrap();
    assert_eq!(decoded.authentication, Authentication::Hmac(hmac()));
    let response = PeeringResponse::Accepted {
        version: 1,
        credit: 1_100_000_000,
        data: &[0xab],
    };
    assert_eq!(to_hex(&response.to_vec()), "800001000000004190ab0001ab");
    assert_eq!(
        to_hex(&PeeringResponse::Refused(RefusalReason::Capacity).to_vec()),
        "8104"
    );
    let wrapped = encode_response(Some(&[2; 32]), &[0x80]);
    assert_eq!(
        ResponseEnvelope::decode(&wrapped).unwrap(),
        ResponseEnvelope {
            mac: Some([2; 32]),
            body: &[0x80],
        }
    );
}

#[test]
fn peering_requests_round_trip() {
    let info = sign_node_info(&node_a(), &info_a());
    let request = Content::PeeringRequest(PeeringRequest {
        scheme: stub(),
        node_info: SignedNodeInfo::decode_exact(&info).unwrap(),
        data: &[1, 2, 3],
        versions: vec![1, 2],
        credit: 1_100_000_000,
    });
    let bytes = request.to_vec();
    assert_eq!(bytes[0], 0x80);
    assert_eq!(Content::decode_exact(&bytes).unwrap(), request);
    // At least one version.
    let none = Content::PeeringRequest(PeeringRequest {
        versions: Vec::new(),
        ..match request {
            Content::PeeringRequest(request) => request,
            _ => unreachable!(),
        }
    });
    assert!(Content::decode_exact(&none.to_vec()).is_err());
}

#[test]
fn link_state_requests_and_answers_are_bounded() {
    let ids: Vec<NodeId> = (0..17).map(|_| node_a().node_id()).collect();
    let request = Content::LinkStateRequest(ids.clone());
    assert!(Content::decode_exact(&request.to_vec()).is_err());
    let request = Content::LinkStateRequest(ids[..16].to_vec());
    assert_eq!(Content::decode_exact(&request.to_vec()).unwrap(), request);
    let info = sign_node_info(&node_a(), &info_a());
    let answer = LinkStateResponse(vec![
        None,
        Some(SignedNodeInfo::decode_exact(&info).unwrap()),
    ]);
    assert_eq!(
        LinkStateResponse::decode_exact(&answer.to_vec()).unwrap(),
        answer
    );
}

// Settlements carry their sequence, and tag 10 is `peeringTermination`
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
    assert_eq!(to_hex(&termination.to_vec()), "8a01");
    assert_eq!(
        Content::decode_exact(&[0x8a, 0x03]).unwrap(),
        Content::PeeringTermination(TerminationReason::NotPeered)
    );
    // Unknown reasons read as unspecified.
    assert_eq!(
        Content::decode_exact(&[0x8a, 0x63]).unwrap(),
        Content::PeeringTermination(TerminationReason::Unspecified)
    );
    assert_eq!(Kind::from_tag(10), Some(Kind::PeeringTermination));
    assert!(Kind::PeeringTermination.requires_authentication());
    // spec/decisions/own-fulfills.md: a settlement request names its scheme
    // alone.
    let request = Content::SettlementRequest {
        scheme: SchemeId::parse("stub").unwrap(),
    };
    assert_eq!(to_hex(&request.to_vec()), "8b0473747562");
    assert_eq!(Kind::from_tag(11), Some(Kind::SettlementRequest));
    assert!(Kind::SettlementRequest.requires_authentication());
    assert!(Content::decode_exact(&[0x8b]).is_err());
    assert!(Content::decode_exact(&[0x8c]).is_err());
    assert_eq!(
        to_hex(&PeeringResponse::Refused(RefusalReason::Terminating).to_vec()),
        "8109"
    );
}

proptest! {
    #[test]
    fn decoders_never_panic(bytes in proptest::collection::vec(any::<u8>(), 0..512)) {
        let _ = Envelope::decode(&bytes);
        let _ = Content::decode_exact(&bytes);
        let _ = SignedNodeInfo::decode_exact(&bytes);
        let _ = ResponseEnvelope::decode(&bytes);
        let _ = PeeringResponse::decode_exact(&bytes);
        let _ = LinkStateResponse::decode_exact(&bytes);
        let _ = NodeListResponse::decode_exact(&bytes);
        let _ = NameReceipt::decode_exact(&bytes);
        let _ = NameListResponse::decode_exact(&bytes);
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

/// Node A's info with an entry of every kind, signed.
fn signed_full() -> Vec<u8> {
    let mut info = info_a();
    info.entries.push(Entry::Extension {
        kind: 1,
        value: &[7; 40],
    });
    sign_node_info(&node_a(), &info)
}

/// Decodes content as the node does, and checks a link state it carries.
/// What decodes encodes to something that decodes the same.
fn decode_content(bytes: &[u8]) -> Result<(), TestCaseError> {
    let Ok(content) = Content::decode_exact(bytes) else {
        return Ok(());
    };
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
        0 => PeeringResponse::Accepted {
            version: 1,
            credit: 1_100_000_000,
            data: &[0xab; 3],
        }
        .to_vec(),
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
        10 => MethodResponse::Accepted {
            version: 1,
            data: &[0xab; 3],
        }
        .to_vec(),
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
        7 => prop_assert_eq!(
            decode_settling(body).is_ok(),
            body.is_empty() || body.len() == 8
        ),
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

/// Decodes an envelope as the node does, and its content.
fn decode_envelope(body: &[u8]) -> Result<(), TestCaseError> {
    let Ok(envelope) = Envelope::decode(body) else {
        prop_assert_eq!(peek_kind(body), None);
        return Ok(());
    };
    prop_assert_eq!(
        encode_envelope(&envelope.sender, &envelope.authentication, envelope.content),
        body
    );
    if let Ok(content) = Content::decode_exact(envelope.content) {
        prop_assert_eq!(peek_kind(body), Some(content.kind()));
    }
    decode_content(envelope.content)
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

    #[test]
    fn broken_envelopes_never_panic(authentication in 0..3usize, mutations in mutations()) {
        let authentication = [
            Authentication::Hmac(hmac()),
            Authentication::Reserved(&[2; 64]),
            Authentication::None,
        ][authentication];
        let signed = signed_full();
        for tag in 0..KINDS {
            let content = content(tag, &signed);
            let body = encode_envelope(&node_a().node_id(), &authentication, &content);
            decode_envelope(&mutate(&body, &mutations))?;
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
    fn broken_responses_never_panic(wrapped in any::<bool>(), mutations in mutations()) {
        for kind in 0..RESPONSES {
            if wrapped {
                let bytes = encode_response(Some(&[2; 32]), &response(kind));
                if let Ok(envelope) = ResponseEnvelope::decode(&mutate(&bytes, &mutations)) {
                    decode_response(kind, envelope.body)?;
                }
            } else {
                decode_response(kind, &mutate(&response(kind), &mutations))?;
            }
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
            NodeId::from_public_key(&public_key)
        } else {
            node_b().node_id()
        };
        let info = SignedNodeInfo {
            bytes: &[],
            signed: &signed,
            info: NodeInfo {
                node_id,
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
// big-endian; an empty answer, from a node before
// spec/decisions/ending-a-peering.md, says nothing.
#[test]
fn a_settlement_requests_answer_is_the_amount_on_its_way() {
    assert_eq!(
        encode_settling(0x0102_0304_0506_0708),
        [1, 2, 3, 4, 5, 6, 7, 8]
    );
    assert_eq!(
        decode_settling(&[1, 2, 3, 4, 5, 6, 7, 8]),
        Ok(Some(0x0102_0304_0506_0708))
    );
    assert_eq!(decode_settling(&[]), Ok(None));
    assert!(decode_settling(&[1, 2, 3]).is_err());
}

// spec/areas/peer-protocol.md#message-set: `hubPacket [12]` carries a
// Prepare with its commitment, which is never empty and bounded.
#[test]
fn hub_packets_are_pinned_and_bounded() {
    let content = Content::HubPacket {
        request_id: 7,
        packet: &[0x0c],
        commitment: &[0xab, 0xab],
    };
    assert_eq!(to_hex(&content.to_vec()), "8c00000007010c02abab");
    assert_eq!(Content::decode_exact(&content.to_vec()).unwrap(), content);
    assert_eq!(Kind::from_tag(12), Some(Kind::HubPacket));
    assert_eq!(content.kind(), Kind::HubPacket);
    assert!(Kind::HubPacket.requires_authentication());
    assert!(!Kind::HubPacket.is_read_only());
    assert!(Kind::HubPacket.carries_packet() && Kind::InterledgerPacket.carries_packet());
    assert!(!Kind::Settlement.carries_packet());
    // An empty commitment, and one past the bound, are malformed.
    assert!(Content::decode_exact(&hex("8c00000007010c00")).is_err());
    let at_bound = Content::HubPacket {
        request_id: 7,
        packet: &[0x0c],
        commitment: &[0xab; MAX_COMMITMENT_BYTES],
    };
    let mut bytes = at_bound.to_vec();
    assert_eq!(Content::decode_exact(&bytes).unwrap(), at_bound);
    assert_eq!(bytes[7..10], [0x82, 0x01, 0x00]);
    bytes[9] = 0x01;
    bytes.push(0xab);
    assert!(Content::decode_exact(&bytes).is_err());
    // Nothing may follow the commitment.
    let mut trailing = content.to_vec();
    trailing.push(0);
    assert!(Content::decode_exact(&trailing).is_err());
}

/// A signed link state of node A with these extensions.
fn with_extensions(extensions: &[(u16, &[u8])]) -> Vec<u8> {
    let mut info = info_a();
    info.entries.extend(
        extensions
            .iter()
            .map(|(kind, value)| Entry::Extension { kind: *kind, value }),
    );
    sign_node_info(&node_a(), &info)
}

// spec/areas/peer-protocol.md#link-states: extension type 5 names the hub a
// node is a member of, in canonical OER; a value that is not one names none,
// and the link state still verifies.
#[test]
fn the_hub_extension_is_pinned_and_read_strictly() {
    let membership = HubMembership {
        hub_id: [0x11; 32],
        account: 0x0102_0304,
        takes_peers: true,
    };
    let value = membership.to_vec();
    assert_eq!(value.len(), HubMembership::ENCODED_BYTES);
    assert_eq!(to_hex(&value), format!("{}01020304ff", "11".repeat(32)));
    assert_eq!(HubMembership::decode_exact(&value), Ok(membership));
    let bytes = with_extensions(&[(EXTENSION_HUB, &value)]);
    let signed = SignedNodeInfo::decode_exact(&bytes).unwrap();
    assert!(signed.verify().is_ok());
    assert_eq!(signed.info.hub(), Some(membership));
    let closed = HubMembership {
        takes_peers: false,
        ..membership
    };
    assert_eq!(closed.to_vec().last(), Some(&0x00));

    // Not canonical, short or long: no hub, and still a valid link state.
    let mut not_canonical = value.clone();
    *not_canonical.last_mut().unwrap() = 0x01;
    let short = &value[..36];
    let mut long = value.clone();
    long.push(0);
    for broken in [&not_canonical[..], short, &long[..], &[]] {
        let bytes = with_extensions(&[(EXTENSION_HUB, broken)]);
        let signed = SignedNodeInfo::decode_exact(&bytes).unwrap();
        assert!(signed.verify().is_ok());
        assert_eq!(signed.info.hub(), None);
    }
    // The first hub extension is the one read.
    let bytes = with_extensions(&[(EXTENSION_HUB, short), (EXTENSION_HUB, &value)]);
    let signed = SignedNodeInfo::decode_exact(&bytes).unwrap();
    assert_eq!(signed.info.hub(), None);
    let full = signed_full();
    assert_eq!(
        SignedNodeInfo::decode_exact(&full).unwrap().info.hub(),
        None
    );
}

/// The longest hub list a node lists: [`dassie_peer::MAX_OWN_HUBS`] hubs on
/// the first of `schemes`, at the largest accounts.
fn largest_hub_list(schemes: &[SchemeId]) -> HubList {
    let hubs = HubList(
        schemes[..dassie_peer::MAX_OWN_HUBS]
            .iter()
            .zip(0u8..)
            .map(|(scheme, n)| HubListing {
                scheme: *scheme,
                membership: HubMembership {
                    hub_id: [n; 32],
                    account: u32::MAX,
                    takes_peers: true,
                },
            })
            .collect(),
    );
    assert!(hubs.is_valid());
    hubs
}

// spec/areas/peer-protocol.md#link-states: the bound a node checks its own
// link state against holds for the largest it can sign, at the longest URL,
// alias and scheme IDs, with every extension; and the default
// configuration's link state fits what every node decodes.
#[test]
fn the_largest_own_link_state_fits_its_bound() {
    let neighbors = dassie_peer::Config::DEFAULT.max_peers;
    let key = NodeKey::from_bytes(&[7; 32]);
    let url = "a".repeat(MAX_URL_BYTES);
    let alias = "\u{e9}".repeat(dassie_peer::wire::MAX_ALIAS_BYTES / 2);
    let ids: Vec<NodeId> = (0..neighbors)
        .map(|n| NodeKey::from_bytes(&[u8::try_from(n).unwrap() + 8; 32]).node_id())
        .collect();
    let schemes: Vec<SchemeId> = (0..dassie_peer::MAX_SCHEMES)
        .map(|n| {
            let text = format!("{n:02}{}", "a".repeat(dassie_peer::wire::MAX_SCHEME_ID - 2));
            SchemeId::parse(&text).unwrap()
        })
        .collect();
    let hubs = largest_hub_list(&schemes);
    let hub = hubs.0[0].membership.to_vec();
    let hubs = hubs.to_vec();
    assert_eq!(hubs.len(), hub_list_bytes(dassie_peer::MAX_OWN_HUBS));
    let port = 4433u16.to_be_bytes();
    let extensions: [(u16, &[u8]); 5] = [
        (dassie_peer::wire::EXTENSION_QUIC, &[]),
        (dassie_peer::wire::EXTENSION_FEED, &port),
        (EXTENSION_HUB, &hub),
        (EXTENSION_METHODS, &[]),
        (EXTENSION_HUBS, &hubs),
    ];
    let info = NodeInfo {
        node_id: key.node_id(),
        sequence: u64::MAX,
        url: &url,
        alias: &alias,
        public_key: *key.public_key().as_bytes(),
        entries: ids
            .iter()
            .map(|id| Entry::Neighbor(*id))
            .chain(schemes.iter().map(|id| Entry::Scheme(*id)))
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
        extensions.len(),
        port.len() + hub.len() + hubs.len(),
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

// spec/areas/peer-protocol.md#message-set: the name messages are tags 13
// to 17. Committing, unsealing and renewing change state and need
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
        (commit, format!("8d{}", "ab".repeat(32))),
        (unseal, format!("8e05616c696365{}", "07".repeat(32))),
        (renew, "8f05616c696365".to_owned()),
        (Content::NameListHashRequest, "90".to_owned()),
        (Content::NameListRequest, "91".to_owned()),
    ];
    for (tag, (content, hex)) in (13..).zip(pinned) {
        assert_eq!(to_hex(&content.to_vec()), hex);
        assert_eq!(Content::decode_exact(&content.to_vec()).unwrap(), content);
        assert_eq!(Kind::from_tag(tag), Some(content.kind()));
        assert_eq!(content.kind().tag(), tag);
    }
    assert_eq!(Kind::from_tag(18), Some(Kind::MethodPacket));
    for kind in [Kind::NameCommit, Kind::NameUnseal, Kind::NameRenew] {
        assert!(kind.requires_authentication() && !kind.is_read_only());
    }
    for kind in [Kind::NameListHashRequest, Kind::NameListRequest] {
        assert!(!kind.requires_authentication() && kind.is_read_only());
    }
    for kind in (13..=17).filter_map(Kind::from_tag) {
        assert!(!kind.carries_packet());
    }
}

// A commitment and a salt are exactly 32 bytes, and a name is read by its
// syntax (spec/designs/names.md#7-names), whether or not it is reserved.
#[test]
fn name_messages_are_bounded() {
    let commit = hex(&format!("8d{}", "ab".repeat(32)));
    assert!(Content::decode_exact(&commit[..32]).is_err());
    let mut long = commit.clone();
    long.push(0xab);
    assert!(Content::decode_exact(&long).is_err());
    let unseal = hex(&format!("8e05616c696365{}", "07".repeat(32)));
    assert!(Content::decode_exact(&unseal[..unseal.len() - 1]).is_err());
    let mut long = unseal.clone();
    long.push(7);
    assert!(Content::decode_exact(&long).is_err());
    let renew = |text: &str| {
        let mut bytes = vec![0x8f, u8::try_from(text.len()).unwrap()];
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

// The commitment is SHA-256 over the domain OID, the name and the holder's
// node ID, each with its length ahead of it, and the salt: a golden value
// computed apart from the code.
#[test]
fn a_commitment_is_pinned() {
    let commitment = Commitment::seal(&name("alice"), &node_a().node_id(), &Salt([7; 32]));
    assert_eq!(
        to_hex(&commitment.0),
        "b3133351028596cfe0bb902945d506c424bf832decca2c6f5b630b98e2e12afd"
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
    let id = |key: NodeKey| to_hex(key.node_id().as_str().as_bytes());
    let bytes = name_list().to_vec();
    assert_eq!(
        to_hex(&bytes),
        format!(
            "0102\
             05616c69636521{}000001a3185c5000\
             03626f6221{}0000018bcfe56800",
            id(node_a()),
            id(node_b())
        )
    );
    assert_eq!(NameListResponse::decode_exact(&bytes), Ok(name_list()));
    assert_eq!(to_hex(&NameListResponse::default().to_vec()), "0100");

    // Out of order, or a name twice: malformed.
    let (count, entries) = bytes.split_at(2);
    let (alice, bob) = entries.split_at(1 + 5 + 1 + 33 + 8);
    let swapped = [count, bob, alice].concat();
    assert!(NameListResponse::decode_exact(&swapped).is_err());
    let twice = [count, alice, alice].concat();
    assert!(NameListResponse::decode_exact(&twice).is_err());

    // At most MAX_NAME_LIST names, and the largest list fits its bound.
    let longest_id = NodeId::parse(&format!("d{}", "a".repeat(44))).unwrap();
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
// are tags 18 to 20 (spec/decisions/settlement-methods.md). Each changes
// state, or moves money, and needs authentication; a `methodPacket`
// carries a Prepare, with a commitment that is empty on a method not over a
// hub, bounded as a `hubPacket`'s.
#[test]
fn settlement_method_messages_are_pinned_and_bounded() {
    let packet = Content::MethodPacket {
        request_id: 7,
        scheme: stub(),
        packet: &[0x0c],
        commitment: &[0xab, 0xab],
    };
    assert_eq!(to_hex(&packet.to_vec()), "92000000070473747562010c02abab");
    let direct = Content::MethodPacket {
        request_id: 7,
        scheme: stub(),
        packet: &[0x0c],
        commitment: &[],
    };
    assert_eq!(to_hex(&direct.to_vec()), "92000000070473747562010c00");
    let add = Content::MethodAdd {
        scheme: stub(),
        data: &[0xab],
        versions: vec![1, 2],
    };
    assert_eq!(to_hex(&add.to_vec()), "93047374756201ab010200010002");
    let drop = Content::MethodDrop { scheme: stub() };
    assert_eq!(to_hex(&drop.to_vec()), "940473747562");
    for (tag, content) in [(18, &packet), (19, &add), (20, &drop)] {
        assert_eq!(
            Content::decode_exact(&content.to_vec()).as_ref(),
            Ok(content)
        );
        let kind = Kind::from_tag(tag).unwrap();
        assert_eq!((content.kind(), kind.tag()), (kind, tag));
        assert!(kind.requires_authentication() && !kind.is_read_only());
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
    // No versions, or more than MAX_VERSIONS, are malformed.
    for versions in [vec![], vec![1; MAX_VERSIONS + 1]] {
        let add = Content::MethodAdd {
            scheme: stub(),
            data: &[],
            versions,
        };
        assert!(Content::decode_exact(&add.to_vec()).is_err());
    }
    // Nothing may follow.
    for content in [&packet, &add, &drop] {
        let mut trailing = content.to_vec();
        trailing.push(0);
        assert!(Content::decode_exact(&trailing).is_err());
    }
}

// The answer to a `methodAdd` is shaped as a peering answer's, without the
// credit only nodes of 0.1 read.
#[test]
fn method_answers_are_pinned() {
    let accepted = MethodResponse::Accepted {
        version: 1,
        data: &[0xab],
    };
    assert_eq!(to_hex(&accepted.to_vec()), "80000101ab");
    let refused = MethodResponse::Refused(RefusalReason::NoCommonVersion);
    assert_eq!(to_hex(&refused.to_vec()), "8103");
    for response in [accepted, refused] {
        assert_eq!(
            MethodResponse::decode_exact(&response.to_vec()),
            Ok(response)
        );
    }
    // Unknown reasons read as unspecified; unknown tags are malformed.
    assert_eq!(
        MethodResponse::decode_exact(&[0x81, 0x63]),
        Ok(MethodResponse::Refused(RefusalReason::Unspecified))
    );
    assert!(MethodResponse::decode_exact(&[0x82, 0x00]).is_err());
}

// spec/areas/peer-protocol.md#link-states: extension type 6, with an empty
// value, says a node has settlement methods; one with a value says nothing,
// and the link state still verifies.
#[test]
fn the_methods_extension_is_read_strictly() {
    let signed = |extensions: &[(u16, &[u8])]| {
        let bytes = with_extensions(extensions);
        let signed = SignedNodeInfo::decode_exact(&bytes).unwrap();
        assert!(signed.verify().is_ok());
        signed.info.has_methods()
    };
    assert!(signed(&[(EXTENSION_METHODS, &[])]));
    assert!(!signed(&[]));
    assert!(!signed(&[(EXTENSION_METHODS, &[0])]));
    // The first methods extension is the one read.
    assert!(!signed(&[
        (EXTENSION_METHODS, &[0]),
        (EXTENSION_METHODS, &[])
    ]));
    let full = signed_full();
    assert!(
        !SignedNodeInfo::decode_exact(&full)
            .unwrap()
            .info
            .has_methods()
    );
}

fn listing(scheme: &str, hub: u8, account: u32, takes_peers: bool) -> HubListing {
    HubListing {
        scheme: SchemeId::parse(scheme).unwrap(),
        membership: HubMembership {
            hub_id: [hub; 32],
            account,
            takes_peers,
        },
    }
}

// spec/areas/peer-protocol.md#link-states: extension type 7 lists a node's
// hubs, each with its scheme, in canonical OER; a list that is empty, too
// long, or names a scheme or a hub twice names none, and the link state
// still verifies. The hub extension, type 5, is read as before, beside it.
#[test]
fn the_hub_list_is_pinned_and_read_strictly() {
    let first = listing("hub-a", 0x11, 0x0102_0304, true);
    let second = listing("hub-b", 0x22, 5, false);
    let list = HubList(vec![first, second]);
    let value = list.to_vec();
    assert_eq!(
        to_hex(&value),
        format!(
            "0102056875622d61{}01020304ff056875622d62{}0000000500",
            "11".repeat(32),
            "22".repeat(32)
        )
    );
    assert_eq!(HubList::decode_exact(&value), Ok(list.clone()));
    let hub = first.membership.to_vec();
    let bytes = with_extensions(&[(EXTENSION_HUB, &hub), (EXTENSION_HUBS, &value)]);
    let signed = SignedNodeInfo::decode_exact(&bytes).unwrap();
    assert!(signed.verify().is_ok());
    assert_eq!(signed.info.hubs(), [first, second]);
    assert_eq!(signed.info.hub(), Some(first.membership));
    // A node of a version before lists names its hub alone.
    let bytes = with_extensions(&[(EXTENSION_HUB, &hub)]);
    let signed = SignedNodeInfo::decode_exact(&bytes).unwrap();
    assert_eq!(signed.info.hubs(), []);
    assert_eq!(signed.info.hub(), Some(first.membership));

    // Broken lists name none, and leave the hub extension as it is.
    let same_scheme = listing("hub-a", 0x22, 5, false);
    let same_hub = listing("hub-b", 0x11, 5, false);
    let too_many = HubList(
        (0..=MAX_HUBS)
            .map(|n| listing(&format!("hub-{n}"), u8::try_from(n).unwrap(), 1, true))
            .collect(),
    );
    let mut not_canonical = value.clone();
    *not_canonical.last_mut().unwrap() = 0x01;
    let mut trailing = value.clone();
    trailing.push(0);
    let broken = [
        HubList(vec![]).to_vec(),
        HubList(vec![first, same_scheme]).to_vec(),
        HubList(vec![first, same_hub]).to_vec(),
        too_many.to_vec(),
        not_canonical,
        trailing,
        value[..value.len() - 1].to_vec(),
        vec![],
    ];
    for broken in &broken {
        assert!(HubList::decode_exact(broken).is_err());
        let bytes = with_extensions(&[(EXTENSION_HUB, &hub), (EXTENSION_HUBS, broken)]);
        let signed = SignedNodeInfo::decode_exact(&bytes).unwrap();
        assert!(signed.verify().is_ok());
        assert_eq!(signed.info.hubs(), []);
        assert_eq!(signed.info.hub(), Some(first.membership));
    }
    // The first hub list is the one read.
    let bytes = with_extensions(&[(EXTENSION_HUBS, &broken[0]), (EXTENSION_HUBS, &value)]);
    let signed = SignedNodeInfo::decode_exact(&bytes).unwrap();
    assert_eq!(signed.info.hubs(), []);
    // A list of MAX_HUBS is read.
    let most = HubList(too_many.0[..MAX_HUBS].to_vec());
    assert_eq!(HubList::decode_exact(&most.to_vec()), Ok(most));
}

proptest! {
    #[test]
    fn method_messages_round_trip(
        request_id in any::<u32>(),
        scheme in "[a-z0-9-]{2,64}",
        packet in proptest::collection::vec(any::<u8>(), 0..300),
        commitment in proptest::collection::vec(any::<u8>(), 0..=MAX_COMMITMENT_BYTES),
        versions in proptest::collection::vec(any::<u16>(), 1..=MAX_VERSIONS),
    ) {
        let scheme = SchemeId::parse(&scheme).unwrap();
        let contents = [
            Content::MethodPacket { request_id, scheme, packet: &packet, commitment: &commitment },
            Content::MethodAdd { scheme, data: &packet, versions },
            Content::MethodDrop { scheme },
        ];
        for content in contents {
            let bytes = content.to_vec();
            prop_assert_eq!(bytes.len(), content.encoded_len());
            prop_assert_eq!(Content::decode_exact(&bytes), Ok(content));
        }
    }

    // A list decodes when, and only when, it keeps its rules, and then
    // encodes to the bytes it came from.
    #[test]
    fn hub_lists_are_read_by_their_rules(
        listings in proptest::collection::vec(
            ("[a-c]{2}", 0..4u8, any::<u32>(), any::<bool>()),
            0..=MAX_HUBS + 1,
        ),
    ) {
        let list = HubList(
            listings
                .iter()
                .map(|(scheme, hub, account, takes)| listing(scheme, *hub, *account, *takes))
                .collect(),
        );
        let bytes = list.to_vec();
        prop_assert_eq!(bytes.len(), list.encoded_len());
        match HubList::decode_exact(&bytes) {
            Ok(decoded) => {
                prop_assert!(list.is_valid());
                prop_assert_eq!(decoded, list);
            }
            Err(_) => prop_assert!(!list.is_valid()),
        }
    }
}
