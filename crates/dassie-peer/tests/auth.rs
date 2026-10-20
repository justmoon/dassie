//! v1 message authentication (D2, spec/areas/peer-protocol.md#keys-and-macs).
//! Each test is a way the original's scheme could be fooled (NET-8) or a
//! property the amended scheme promises.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic,
    clippy::unreachable
)]

use std::sync::Arc;

use dassie_identity::NodeKey;
use dassie_node_core::testing::SeededRng;
use dassie_oer::Encode;
use dassie_peer::auth::{AuthConfig, AuthError, Authenticator, Pending};
use dassie_peer::wire::{
    Authentication, Content, Envelope, Kind, ResponseEnvelope, SchemeId, encode_envelope,
    encode_response,
};
use dassie_peer::{Network, PeerBatch};

const NOW: u64 = 1_800_000_000_000;

fn node(seed: u8) -> Arc<NodeKey> {
    Arc::new(NodeKey::from_bytes(&[seed; 32]))
}

fn authenticator(key: &Arc<NodeKey>) -> Authenticator {
    Authenticator::new(Arc::clone(key), AuthConfig::DEFAULT, NOW - 1)
}

/// A request from `from` to `to`, authenticated at `at`: one that changes
/// state, so its replay state is kept.
fn request(from: &mut Authenticator, to: &NodeKey, at: u64) -> (Vec<u8>, Pending) {
    let content = Content::SettlementRequest {
        scheme: SchemeId::parse("stub").unwrap(),
    };
    request_with(from, to, at, &content)
}

/// A request with `content` from `from` to `to`, authenticated at `at`.
fn request_with(
    from: &mut Authenticator,
    to: &NodeKey,
    at: u64,
    content: &Content<'_>,
) -> (Vec<u8>, Pending) {
    let content = content.to_vec();
    let mut rng = SeededRng::new(1);
    let (hmac, pending) = from
        .authenticate(at, &mut rng, &to.node_id(), &to.public_key(), &content)
        .unwrap();
    let body = encode_envelope(from.own(), &Authentication::Hmac(hmac), &content);
    (body, pending)
}

fn accept(at: &mut Authenticator, body: &[u8], now: u64) -> Result<(), AuthError> {
    accept_from(at, body, now, None)
}

/// Accepts a request of the kind [`request`] makes, from `network`.
fn accept_from(
    at: &mut Authenticator,
    body: &[u8],
    now: u64,
    network: Option<Network>,
) -> Result<(), AuthError> {
    let envelope = Envelope::decode(body).unwrap();
    at.accept(now, &envelope, Kind::SettlementRequest, network)
        .map(|_| ())
}

#[test]
fn requests_and_responses_verify_end_to_end() {
    let (a, b) = (node(1), node(3));
    let (mut at_a, mut at_b) = (authenticator(&a), authenticator(&b));
    let (body, pending) = request(&mut at_a, &b, NOW);
    let envelope = Envelope::decode(&body).unwrap();
    let accepted = at_b
        .accept(NOW, &envelope, Kind::SettlementRequest, None)
        .unwrap();
    assert_eq!(accepted.sender, a.node_id());
    assert_eq!(accepted.key, Some(a.public_key()));
    let answer = b"node list";
    let response = encode_response(accepted.response_mac(answer).as_ref(), answer);
    assert_eq!(
        pending.check(&ResponseEnvelope::decode(&response).unwrap()),
        Ok(())
    );
    // A response from anyone else, or an unauthenticated one, does not.
    let forged = encode_response(Some(&[0; 32]), answer);
    assert_eq!(
        pending.check(&ResponseEnvelope::decode(&forged).unwrap()),
        Err(AuthError::BadMac)
    );
    let bare = encode_response(None, answer);
    assert_eq!(
        pending.check(&ResponseEnvelope::decode(&bare).unwrap()),
        Err(AuthError::Missing)
    );
}

// NET-8: in v0 anyone could send as any peered node, since the MAC proved
// only knowledge of the recipient's public key. Now the key that must be
// named is the claimed sender's own, and the MAC needs its secret.
#[test]
fn nobody_can_send_as_another_node() {
    let (a, b, mallory) = (node(1), node(3), node(5));
    let mut at_b = authenticator(&b);
    let mut at_mallory = authenticator(&mallory);
    let (body, _) = request(&mut at_mallory, &b, NOW);
    let envelope = Envelope::decode(&body).unwrap();
    // Claiming A's ID with Mallory's key: the key is not A's.
    let mut claimed = envelope;
    claimed.sender = a.node_id();
    assert_eq!(
        at_b.accept(NOW, &claimed, Kind::SettlementRequest, None)
            .unwrap_err(),
        AuthError::WrongSender
    );
    // Claiming A's ID and naming A's key: the MAC was not made with it.
    let Authentication::Hmac(mut hmac) = envelope.authentication else {
        unreachable!()
    };
    hmac.sender_key = *a.public_key().as_bytes();
    let content = Content::NodeListRequest.to_vec();
    let forged = encode_envelope(&a.node_id(), &Authentication::Hmac(hmac), &content);
    assert_eq!(accept(&mut at_b, &forged, NOW), Err(AuthError::BadMac));
}

// A request cannot be turned around and sent back to its sender, nor sent
// on to another node: the MAC keys differ by direction and recipient.
#[test]
fn requests_are_bound_to_their_direction_and_recipient() {
    let (a, b, c) = (node(1), node(3), node(7));
    let mut at_a = authenticator(&a);
    let (body, _) = request(&mut at_a, &b, NOW);
    let Authentication::Hmac(hmac) = Envelope::decode(&body).unwrap().authentication else {
        unreachable!()
    };
    // Reflected to A as if from B, with B's key.
    let mut reflected = hmac;
    reflected.sender_key = *b.public_key().as_bytes();
    let content = Content::NodeListRequest.to_vec();
    let back = encode_envelope(&b.node_id(), &Authentication::Hmac(reflected), &content);
    assert_eq!(accept(&mut at_a, &back, NOW), Err(AuthError::BadMac));
    // Delivered to C instead of B.
    let mut at_c = authenticator(&c);
    assert_eq!(accept(&mut at_c, &body, NOW), Err(AuthError::BadMac));
}

#[test]
fn the_mac_covers_the_content() {
    let (a, b) = (node(1), node(3));
    let mut at_a = authenticator(&a);
    let mut at_b = authenticator(&b);
    let (mut body, _) = request(&mut at_a, &b, NOW);
    let last = body.len() - 1;
    body[last] = Content::NodeListHashRequest.to_vec()[0];
    assert_eq!(accept(&mut at_b, &body, NOW), Err(AuthError::BadMac));
}

#[test]
fn a_request_is_accepted_once() {
    let (a, b) = (node(1), node(3));
    let mut at_a = authenticator(&a);
    let mut at_b = authenticator(&b);
    let (body, _) = request(&mut at_a, &b, NOW);
    assert_eq!(accept(&mut at_b, &body, NOW), Ok(()));
    assert_eq!(accept(&mut at_b, &body, NOW + 10), Err(AuthError::Replayed));
    let (second, _) = request(&mut at_a, &b, NOW + 1);
    assert_eq!(accept(&mut at_b, &second, NOW + 10), Ok(()));
}

// A restart forgets sessions; the floor persisted for a peer refuses what
// it accepted before, including messages stamped ahead of its clock.
#[test]
fn a_restart_does_not_reopen_replays() {
    let (a, b) = (node(1), node(3));
    let mut at_a = authenticator(&a);
    let mut at_b = authenticator(&b);
    let ahead = NOW + 50_000;
    let (body, _) = request(&mut at_a, &b, ahead);
    assert_eq!(accept(&mut at_b, &body, NOW), Ok(()));
    let persisted = at_b.newest(&a.node_id()).unwrap();
    let mut restarted = Authenticator::new(Arc::clone(&b), AuthConfig::DEFAULT, NOW + 5);
    restarted.restore_floor(a.node_id(), persisted);
    assert_eq!(
        accept(&mut restarted, &body, NOW + 5),
        Err(AuthError::Replayed)
    );
    let (fresh, _) = request(&mut at_a, &b, ahead + 1);
    assert_eq!(accept(&mut restarted, &fresh, NOW + 5), Ok(()));
}

#[test]
fn timestamps_far_from_now_are_refused() {
    let (a, b) = (node(1), node(3));
    let mut at_a = authenticator(&a);
    let mut at_b = authenticator(&b);
    let (late, _) = request(&mut at_a, &b, NOW);
    assert_eq!(
        accept(&mut at_b, &late, NOW + 61_000),
        Err(AuthError::Stale)
    );
    let (early, _) = request(&mut at_a, &b, NOW + 61_000);
    assert_eq!(accept(&mut at_b, &early, NOW), Err(AuthError::Stale));
}

#[test]
fn what_must_be_authenticated_is() {
    let (a, b) = (node(1), node(3));
    let mut at_b = authenticator(&b);
    let content = Content::SettlementRequest {
        scheme: SchemeId::parse("stub").unwrap(),
    }
    .to_vec();
    let bare = encode_envelope(&a.node_id(), &Authentication::None, &content);
    let envelope = Envelope::decode(&bare).unwrap();
    assert_eq!(
        at_b.accept(NOW, &envelope, Kind::SettlementRequest, None)
            .unwrap_err(),
        AuthError::Missing
    );
    let content = Content::NodeListRequest.to_vec();
    let bare = encode_envelope(&a.node_id(), &Authentication::None, &content);
    let envelope = Envelope::decode(&bare).unwrap();
    let accepted = at_b
        .accept(NOW, &envelope, Kind::NodeListRequest, None)
        .unwrap();
    assert!(!accepted.is_authenticated());
    assert_eq!(accepted.response_mac(b"x"), None);
    let reserved = encode_envelope(&a.node_id(), &Authentication::Reserved(&[1, 2]), &content);
    let envelope = Envelope::decode(&reserved).unwrap();
    assert_eq!(
        at_b.accept(NOW, &envelope, Kind::NodeListRequest, None)
            .unwrap_err(),
        AuthError::Unsupported
    );
}

// The replay table is bounded: a new sender is refused while every one it
// holds could still replay, and takes the place of one that cannot.
#[test]
fn the_replay_table_is_bounded() {
    let b = node(3);
    let config = AuthConfig {
        max_senders: 1,
        ..AuthConfig::DEFAULT
    };
    let mut at_b = Authenticator::new(Arc::clone(&b), config, NOW - 1);
    let mut at_a = authenticator(&node(1));
    let mut at_c = authenticator(&node(7));
    let (from_a, _) = request(&mut at_a, &b, NOW);
    assert_eq!(accept(&mut at_b, &from_a, NOW), Ok(()));
    let (from_c, _) = request(&mut at_c, &b, NOW);
    assert_eq!(accept(&mut at_b, &from_c, NOW), Err(AuthError::Busy));
    let (later, _) = request(&mut at_c, &b, NOW + 61_000);
    assert_eq!(accept(&mut at_b, &later, NOW + 61_000), Ok(()));
    at_b.check_invariants();
}

// A request that only reads, from a sender whose state is not kept for
// good, is checked and leaves nothing: no place in the pool, and no key in
// the cache. So new identities asking for public data cannot fill the pool
// or keep a place in it fresh.
#[test]
fn requests_that_only_read_leave_no_replay_state() {
    let b = node(3);
    let config = AuthConfig {
        max_senders: 1,
        ..AuthConfig::DEFAULT
    };
    let mut at_b = Authenticator::new(Arc::clone(&b), config, NOW - 1);
    let read = |at_b: &mut Authenticator, from: u8, at: u64| {
        let mut sender = authenticator(&node(from));
        let (body, _) = request_with(&mut sender, &b, at, &Content::NodeListRequest);
        let envelope = Envelope::decode(&body).unwrap();
        at_b.accept(at, &envelope, Kind::NodeListRequest, None)
            .map(|accepted| accepted.is_authenticated())
    };
    for from in 10..20 {
        assert_eq!(read(&mut at_b, from, NOW), Ok(true));
        assert_eq!(at_b.newest(&node(from).node_id()), None);
    }
    // The pool's one place is still free for a request that changes state.
    let mut at_a = authenticator(&node(1));
    let (body, _) = request(&mut at_a, &b, NOW);
    assert_eq!(accept(&mut at_b, &body, NOW), Ok(()));
    // A's reads do not keep its place fresh: it goes idle on its last
    // change, and another sender takes it.
    let (body, _) = request_with(&mut at_a, &b, NOW + 50_000, &Content::NodeListRequest);
    let envelope = Envelope::decode(&body).unwrap();
    assert!(
        at_b.accept(NOW + 50_000, &envelope, Kind::NodeListRequest, None)
            .is_ok()
    );
    assert_eq!(at_b.newest(&node(1).node_id()), Some(NOW));
    let mut at_c = authenticator(&node(7));
    let (later, _) = request(&mut at_c, &b, NOW + 61_000);
    assert_eq!(accept(&mut at_b, &later, NOW + 61_000), Ok(()));
    // Still checked: a stale read is refused.
    let mut sender = authenticator(&node(11));
    let (body, _) = request_with(&mut sender, &b, NOW, &Content::NodeListRequest);
    let envelope = Envelope::decode(&body).unwrap();
    assert_eq!(
        at_b.accept(NOW + 61_000, &envelope, Kind::NodeListRequest, None)
            .map(|_| ()),
        Err(AuthError::Stale)
    );
    at_b.check_invariants();
}

// A network (a /24 or /48) holds only its share of the pool; the rest is
// for everyone else. Once its share is taken, a newcomer from it may take
// only the place of an idle sender from the same network.
#[test]
fn one_network_holds_only_its_share() {
    let b = node(3);
    let config = AuthConfig {
        max_senders: 4,
        max_senders_per_network: 2,
        ..AuthConfig::DEFAULT
    };
    let mut at_b = Authenticator::new(Arc::clone(&b), config, NOW - 1);
    let network = |address: &str| Some(Network::of(address.parse().unwrap()));
    let send = |at_b: &mut Authenticator, from: u8, at: u64, address: &str| {
        let (body, _) = request(&mut authenticator(&node(from)), &b, at);
        accept_from(at_b, &body, at, network(address))
    };
    assert_eq!(send(&mut at_b, 10, NOW, "192.0.2.1"), Ok(()));
    assert_eq!(send(&mut at_b, 11, NOW + 10_000, "192.0.2.200"), Ok(()));
    assert_eq!(send(&mut at_b, 12, NOW, "192.0.2.7"), Err(AuthError::Busy));
    // An IPv4 address as a dual-stack listener sees it is in that network.
    assert_eq!(
        send(&mut at_b, 12, NOW, "::ffff:192.0.2.7"),
        Err(AuthError::Busy)
    );
    assert_eq!(send(&mut at_b, 13, NOW, "198.51.100.1"), Ok(()));
    assert_eq!(send(&mut at_b, 14, NOW + 10_000, "2001:db8:1:2::1"), Ok(()));
    // The pool is full, and the senders of NOW are idle. A newcomer from
    // 192.0.2.0/24 takes the place of the idle one from there.
    assert_eq!(send(&mut at_b, 12, NOW + 61_000, "192.0.2.7"), Ok(()));
    assert_eq!(at_b.newest(&node(10).node_id()), None);
    // With none idle there, the network gets no other's place.
    assert_eq!(
        send(&mut at_b, 16, NOW + 61_000, "192.0.2.9"),
        Err(AuthError::Busy)
    );
    assert_eq!(send(&mut at_b, 15, NOW + 61_000, "203.0.113.1"), Ok(()));
    assert_eq!(at_b.newest(&node(13).node_id()), None);
    at_b.check_invariants();
}

// A sender let go, such as a node whose registration lapsed, keeps its
// replay state outside the pool until every message accepted from it is
// stale, so that letting it go opens no replay.
#[test]
fn a_sender_let_go_is_forgotten_only_once_stale() {
    let (a, b) = (node(1), node(3));
    let config = AuthConfig {
        max_senders: 1,
        ..AuthConfig::DEFAULT
    };
    let mut at_b = Authenticator::new(Arc::clone(&b), config, NOW - 1);
    at_b.pin(a.node_id());
    let mut at_a = authenticator(&a);
    let (body, _) = request(&mut at_a, &b, NOW);
    assert_eq!(accept(&mut at_b, &body, NOW), Ok(()));
    at_b.unpin(NOW + 1, &a.node_id());
    assert!(!at_b.is_pinned(&a.node_id()));
    assert_eq!(accept(&mut at_b, &body, NOW + 2), Err(AuthError::Replayed));
    // Kept outside the pool: a stranger still finds its place.
    let (other, _) = request(&mut authenticator(&node(7)), &b, NOW + 2);
    assert_eq!(accept(&mut at_b, &other, NOW + 2), Ok(()));
    at_b.check_invariants();
    // What it sends now is recorded only in a place of the pool's, and
    // there is none.
    let (more, _) = request(&mut at_a, &b, NOW + 3);
    assert_eq!(accept(&mut at_b, &more, NOW + 3), Err(AuthError::Busy));
    // Stale, it is forgotten.
    let (later, _) = request(&mut authenticator(&node(8)), &b, NOW + 61_000);
    assert_eq!(accept(&mut at_b, &later, NOW + 61_000), Ok(()));
    assert_eq!(at_b.newest(&a.node_id()), None);
    at_b.check_invariants();
}

// The floors of the senders kept for good are persisted, those of
// strangers are not, so strangers cannot make the node write; a sender let
// go has its floor dropped only once every message from it is stale.
#[test]
fn only_the_floors_of_senders_kept_are_persisted() {
    let (a, b, c) = (node(1), node(3), node(7));
    let mut at_b = Authenticator::new(Arc::clone(&b), AuthConfig::DEFAULT, NOW - 1);
    at_b.pin(a.node_id());
    let (from_a, _) = request(&mut authenticator(&a), &b, NOW + 5);
    let (from_c, _) = request(&mut authenticator(&c), &b, NOW);
    assert_eq!(accept(&mut at_b, &from_a, NOW), Ok(()));
    assert_eq!(accept(&mut at_b, &from_c, NOW), Ok(()));
    let (mut floors, mut forgotten) = (Vec::new(), Vec::new());
    at_b.take_floors(&mut floors, &mut forgotten);
    assert_eq!(floors, vec![(a.node_id(), NOW + 5)]);
    // A stranger pinned for what its request did has its floor persisted
    // with that.
    at_b.pin(c.node_id());
    floors.clear();
    at_b.take_floors(&mut floors, &mut forgotten);
    assert_eq!(floors, vec![(c.node_id(), NOW)]);
    at_b.unpin(NOW + 1, &a.node_id());
    at_b.take_floors(&mut floors, &mut forgotten);
    assert!(forgotten.is_empty(), "not stale yet");
    at_b.unpin(NOW + 61_000, &a.node_id());
    at_b.take_floors(&mut floors, &mut forgotten);
    assert_eq!(forgotten, vec![a.node_id()]);
    at_b.check_invariants();
}

// A replayed read is harmless, so a read moves no floor to persist, and
// costs no write; unless the node asks, as it does for a peer, which a
// replayed read would mark as heard from.
#[test]
fn reads_move_no_floor_to_persist() {
    let (a, b) = (node(1), node(3));
    let mut at_b = authenticator(&b);
    at_b.pin(a.node_id());
    let (read, _) = request_with(&mut authenticator(&a), &b, NOW, &Content::NodeListRequest);
    let envelope = Envelope::decode(&read).unwrap();
    at_b.accept(NOW, &envelope, Kind::NodeListRequest, None)
        .unwrap();
    let (mut floors, mut forgotten) = (Vec::new(), Vec::new());
    at_b.take_floors(&mut floors, &mut forgotten);
    assert!(floors.is_empty());
    at_b.keep_floor(&a.node_id());
    at_b.take_floors(&mut floors, &mut forgotten);
    assert_eq!(floors, vec![(a.node_id(), NOW)]);
}

// A sender let go that sends again before its floor is stale takes a
// stranger's place. Its persisted floor goes when that place does, or it
// would stay for as long as the node runs.
#[test]
fn a_sender_let_go_that_takes_a_place_loses_its_floor_with_it() {
    let (a, b) = (node(1), node(3));
    let config = AuthConfig {
        max_senders: 1,
        ..AuthConfig::DEFAULT
    };
    let mut at_b = Authenticator::new(Arc::clone(&b), config, NOW - 1);
    at_b.pin(a.node_id());
    let mut at_a = authenticator(&a);
    let (first, _) = request(&mut at_a, &b, NOW);
    assert_eq!(accept(&mut at_b, &first, NOW), Ok(()));
    let (mut floors, mut forgotten) = (Vec::new(), Vec::new());
    at_b.take_floors(&mut floors, &mut forgotten);
    assert_eq!(floors, vec![(a.node_id(), NOW)]);
    at_b.unpin(NOW + 1, &a.node_id());
    let (again, _) = request(&mut at_a, &b, NOW + 2);
    assert_eq!(accept(&mut at_b, &again, NOW + 2), Ok(()));
    at_b.check_invariants();
    // A stranger takes its place once it is idle.
    let later = NOW + 2 + 61_000;
    let (other, _) = request(&mut authenticator(&node(7)), &b, later);
    assert_eq!(accept(&mut at_b, &other, later), Ok(()));
    assert_eq!(at_b.newest(&a.node_id()), None, "evicted");
    let mut batch = PeerBatch::default();
    let mut floors = Vec::new();
    at_b.take_floors(&mut floors, &mut batch.floors_removed);
    assert_eq!(batch.floors_removed, vec![a.node_id()]);
    at_b.check_invariants();
}

// The MAC of a fixed request, pinned: a second implementation of v1 has to
// produce exactly this.
#[test]
fn the_mac_is_pinned() {
    let (a, b) = (node(1), node(3));
    let keys = dassie_peer::auth::PairKeys::derive(&a, &b.public_key()).unwrap();
    let hmac = dassie_peer::wire::HmacAuthentication {
        sender_key: *a.public_key().as_bytes(),
        session_id: [5; 16],
        counter: 7,
        timestamp_ms: 1_700_000_000_123,
        mac: [0; 32],
    };
    let content = Content::NodeListRequest.to_vec();
    let mac = dassie_peer::auth::request_mac(
        &keys.outgoing_request,
        &a.node_id(),
        &b.node_id(),
        &hmac,
        &content,
    );
    let hex: String = mac.iter().map(|byte| format!("{byte:02x}")).collect();
    // Computed independently from spec/areas/peer-protocol.md#keys-and-macs
    // (X25519, HKDF and HMAC from first principles) before it was pinned
    // here.
    assert_eq!(
        hex,
        "7253c8ea5323692f8b6b7a15cefe82e79a7b6cb77cc49ef480bef76b44920e55"
    );
}
