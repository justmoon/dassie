//! The v1 wire format: v0's layouts where v1 keeps them (checked against
//! the original's vectors, spec 3 §5.5), the v1 changes (spec 7 §4.7), and
//! decoders that never panic.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use dassie_identity::{NodeId, NodeKey, PublicKey, Signature};
use dassie_oer::{Decode, Encode};
use dassie_peer::wire::{
    Authentication, Content, Entry, Envelope, EnvelopeError, HmacAuthentication, InvalidNodeInfo,
    Kind, LinkStateResponse, NodeInfo, NodeListResponse, PeeringRequest, PeeringResponse,
    RefusalReason, ResponseEnvelope, SchemeId, SignedNodeInfo, encode_envelope, encode_response,
    peek_kind, sign_node_info,
};
use proptest::prelude::*;
use sha2::{Digest, Sha256};

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

// Tag 10 is reserved for peering termination, which comes with its
// behaviour (M4b); until then it is not a message.
#[test]
fn tag_ten_is_reserved() {
    assert!(Content::decode_exact(&[0x8a]).is_err());
    assert_eq!(Kind::from_tag(10), None);
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
    fn settlements_round_trip(amount in any::<u64>(), data in proptest::collection::vec(any::<u8>(), 0..300)) {
        let content = Content::Settlement { scheme: stub(), amount, data: &data };
        let bytes = content.to_vec();
        prop_assert_eq!(Content::decode_exact(&bytes).unwrap(), content);
    }
}
