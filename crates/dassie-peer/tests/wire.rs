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

use dassie_identity::{NodeId, NodeKey, PublicKey, Signature};
use dassie_oer::{Decode, Encode};
use dassie_peer::wire::{
    Authentication, Content, Entry, Envelope, EnvelopeError, HmacAuthentication, InvalidNodeInfo,
    Kind, LinkStateResponse, MAX_URL_BYTES, NodeInfo, NodeListResponse, PeeringRequest,
    PeeringResponse, RefusalReason, ResponseEnvelope, SchemeId, SignedNodeInfo, TerminationReason,
    decode_expiry, decode_hash, decode_octets, decode_settling, encode_envelope, encode_expiry,
    encode_octets, encode_response, encode_settling, is_valid_url, peek_kind, sign_node_info,
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
    // D23: a settlement request names its scheme alone.
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
const RESPONSES: usize = 8;

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
// big-endian; an empty answer, from a node before D28, says nothing.
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
