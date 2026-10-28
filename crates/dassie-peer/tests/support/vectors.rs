//! The golden vectors of the peer protocol's layouts
//! (spec/areas/peer-protocol.md#test-vectors): a link state, every message
//! and every answer, made from fixed keys. `examples/peer_vectors.rs` writes
//! them to `tests/data/vectors.json`, and `tests/vectors.rs` checks that the
//! code still makes and reads exactly those bytes.

use std::collections::BTreeMap;

use dassie_identity::IdentityKey;
use dassie_oer::{Decode, Encode};
use dassie_peer::names::Name;
use dassie_peer::wire::{
    Content, EXTENSION_FEED, Entry, HubDetails, LinkStateResponse, MethodResponse, NameHolding,
    NameListResponse, NameReceipt, NodeInfo, NodeListResponse, PeeringResponse, RefusalReason,
    SchemeId, SettlementOffer, SignedNodeInfo, SubscriptionResponse, encode_expiry, encode_octets,
    encode_settling, sign_node_info,
};

use super::inputs::{KINDS, content};

/// The version the vectors are of, as its ALPN name
/// (spec/decisions/protocol-upgrades.md).
pub const VERSION: &str = "dassie/2";

/// The key of node A: the seed of 32 bytes `01`. Made once, since its
/// SLH-DSA key takes a key generation.
#[must_use]
pub fn node_a() -> &'static IdentityKey {
    static KEY: std::sync::OnceLock<IdentityKey> = std::sync::OnceLock::new();
    KEY.get_or_init(|| IdentityKey::from_bytes(&[1; 32]))
}

/// The key of node B: the seed of 32 bytes `03`.
#[must_use]
pub fn node_b() -> &'static IdentityKey {
    static KEY: std::sync::OnceLock<IdentityKey> = std::sync::OnceLock::new();
    KEY.get_or_init(|| IdentityKey::from_bytes(&[3; 32]))
}

/// Node A's link state: two settlement offers, one over a hub and the
/// stub's, a neighbor and an extension, signed.
#[must_use]
pub fn link_state() -> Vec<u8> {
    let hub = HubDetails {
        hub_id: [0x42; 32],
        account: 7,
    }
    .to_vec();
    let info = NodeInfo {
        sequence: 1_700_000_000_000,
        url: "https://a.example",
        alias: "Node A",
        public_key: *node_a().public_key().as_bytes(),
        slh_dsa_public_key: *node_a().identity().slh_dsa().as_bytes(),
        settlement: vec![
            SettlementOffer {
                scheme: SchemeId::parse("hub-xrpl-testnet-xrp").unwrap(),
                details: &hub,
            },
            SettlementOffer {
                scheme: SchemeId::parse("stub").unwrap(),
                details: &[],
            },
        ],
        entries: vec![
            Entry::Neighbor(node_b().node_id()),
            Entry::Extension {
                kind: EXTENSION_FEED,
                value: &[0x1e, 0x50],
            },
        ],
    };
    sign_node_info(node_a(), &info)
}

/// A message kind's name as the protocol spells it: `peeringRequest`.
fn name_of(content: &Content<'_>) -> String {
    let debug = format!("{:?}", content.kind());
    let mut chars = debug.chars();
    let first = chars.next().unwrap().to_ascii_lowercase();
    core::iter::once(first).chain(chars).collect()
}

/// Every message of the protocol, by name: one of each kind, as the
/// never-panics properties send them, carrying [`link_state`]; and a
/// `methodPacket` on a method not over a hub, with no commitment, beside
/// the one over a hub; and a `settlement` over a hub, a bare notice with no
/// data, beside the stub's.
#[must_use]
pub fn messages() -> BTreeMap<String, Vec<u8>> {
    let state = link_state();
    let mut messages: BTreeMap<String, Vec<u8>> = (0..KINDS)
        .map(|tag| {
            let bytes = content(tag, &state);
            (name_of(&Content::decode_exact(&bytes).unwrap()), bytes)
        })
        .collect();
    let without = Content::MethodPacket {
        request_id: 43,
        scheme: SchemeId::parse("stub").unwrap(),
        packet: &[12, 0x10, 0, 0, 0, 0, 0, 0, 0, 2],
        commitment: &[],
    };
    messages.insert("methodPacketWithoutCommitment".to_owned(), without.to_vec());
    // Over a hub, a settlement message is a bare notice, with no data
    // (spec/areas/settlement.md#settlement-hubs).
    let notice = Content::Settlement {
        scheme: SchemeId::parse("hub-stub").unwrap(),
        amount: 1_000,
        data: &[],
        sequence: 2,
    };
    messages.insert("settlementOverHub".to_owned(), notice.to_vec());
    messages
}

/// Every kind of answer, by name.
#[must_use]
pub fn answers() -> BTreeMap<String, Vec<u8>> {
    let state = link_state();
    let signed = SignedNodeInfo::decode_exact(&state).unwrap();
    let holdings = BTreeMap::from([(
        Name::parse("alice").unwrap(),
        NameHolding {
            holder: node_a().node_id(),
            expires_at_ms: 1_800_000_000_000,
        },
    )]);
    [
        ("peeringAccepted", PeeringResponse::Accepted.to_vec()),
        (
            "peeringRefused",
            PeeringResponse::Refused(RefusalReason::Capacity).to_vec(),
        ),
        ("methodAccepted", MethodResponse::Accepted.to_vec()),
        (
            "methodRefused",
            MethodResponse::Refused(RefusalReason::SchemeRefused).to_vec(),
        ),
        (
            "linkStates",
            LinkStateResponse(vec![Some(signed), None]).to_vec(),
        ),
        (
            "nodeList",
            NodeListResponse(vec![node_a().node_id(), node_b().node_id()]).to_vec(),
        ),
        ("nameList", NameListResponse(holdings).to_vec()),
        (
            "nameReceipt",
            NameReceipt {
                position: 7,
                window_end_ms: 1_700_000_060_000,
            }
            .to_vec(),
        ),
        (
            "subscriptionAccepted",
            SubscriptionResponse::Accepted.to_vec(),
        ),
        (
            "subscriptionRefused",
            SubscriptionResponse::Refused(RefusalReason::NoSwaps).to_vec(),
        ),
        ("registered", encode_expiry(1_700_086_400_000)),
        ("peeringInfo", encode_octets(&[0xab; 20])),
        ("settling", encode_settling(1_000)),
    ]
    .into_iter()
    .map(|(name, bytes)| (name.to_owned(), bytes))
    .collect()
}
