//! v1 message authentication (D2, spec 7 §4.7). Each test is a way the
//! original's scheme could be fooled (NET-8) or a property the amended
//! scheme promises.

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
    Authentication, Content, Envelope, ResponseEnvelope, encode_envelope, encode_response,
};

const NOW: u64 = 1_800_000_000_000;

fn node(seed: u8) -> Arc<NodeKey> {
    Arc::new(NodeKey::from_bytes(&[seed; 32]))
}

fn authenticator(key: &Arc<NodeKey>) -> Authenticator {
    Authenticator::new(Arc::clone(key), AuthConfig::DEFAULT, NOW - 1)
}

/// A request from `from` to `to`, authenticated at `at`.
fn request(from: &mut Authenticator, to: &NodeKey, at: u64) -> (Vec<u8>, Pending) {
    let content = Content::NodeListRequest.to_vec();
    let mut rng = SeededRng::new(1);
    let (hmac, pending) = from
        .authenticate(at, &mut rng, &to.node_id(), &to.public_key(), &content)
        .unwrap();
    let body = encode_envelope(from.own(), &Authentication::Hmac(hmac), &content);
    (body, pending)
}

fn accept(at: &mut Authenticator, body: &[u8], now: u64) -> Result<(), AuthError> {
    let envelope = Envelope::decode(body).unwrap();
    at.accept(now, &envelope, true).map(|_| ())
}

#[test]
fn requests_and_responses_verify_end_to_end() {
    let (a, b) = (node(1), node(3));
    let (mut at_a, mut at_b) = (authenticator(&a), authenticator(&b));
    let (body, pending) = request(&mut at_a, &b, NOW);
    let envelope = Envelope::decode(&body).unwrap();
    let accepted = at_b.accept(NOW, &envelope, true).unwrap();
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
        at_b.accept(NOW, &claimed, true).unwrap_err(),
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
    let content = Content::NodeListRequest.to_vec();
    let bare = encode_envelope(&a.node_id(), &Authentication::None, &content);
    let envelope = Envelope::decode(&bare).unwrap();
    assert_eq!(
        at_b.accept(NOW, &envelope, true).unwrap_err(),
        AuthError::Missing
    );
    let accepted = at_b.accept(NOW, &envelope, false).unwrap();
    assert!(!accepted.is_authenticated());
    assert_eq!(accepted.response_mac(b"x"), None);
    let reserved = encode_envelope(&a.node_id(), &Authentication::Reserved(&[1, 2]), &content);
    let envelope = Envelope::decode(&reserved).unwrap();
    assert_eq!(
        at_b.accept(NOW, &envelope, false).unwrap_err(),
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
    // Computed independently from spec 7 §4.7 (X25519, HKDF and HMAC from
    // first principles) before it was pinned here.
    assert_eq!(
        hex,
        "7253c8ea5323692f8b6b7a15cefe82e79a7b6cb77cc49ef480bef76b44920e55"
    );
}
