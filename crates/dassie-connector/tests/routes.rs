//! Peers and the route table (M4): packets to other nodes go to a peer the
//! routes name, never back where they came from; below our own address
//! only children count; a packet that surely did not reach its peer is
//! rejected at once.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

mod support;

use dassie_connector::{Config, EndpointId, Outcome, Responded, RoutesError};
use dassie_ilp::{ErrorCode, Fulfill, Response, ildcp};
use dassie_oer::{Decode, Encode};
use support::{Harness, NOW, OWN, decode_prepare, prepare};

const BTP: EndpointId = EndpointId::Btp(1);
const ONE: EndpointId = EndpointId::Peer(1);
const TWO: EndpointId = EndpointId::Peer(2);
const TRUST: u64 = 1_100_000_000;

fn harness() -> Harness {
    let mut harness = Harness::new(Config::DEFAULT, 10_000_000_000);
    harness.add_peer(1, "done", TRUST, 7_000);
    harness.add_peer(2, "dtwo", TRUST, 0);
    harness
}

fn set(harness: &mut Harness, routes: &[(&str, &[EndpointId])]) {
    harness
        .connector
        .set_routes(routes.iter().map(|(prefix, hops)| (*prefix, *hops)))
        .unwrap();
}

fn forwarded_to(outcome: Outcome) -> EndpointId {
    match outcome {
        Outcome::Forward(forwarded) => forwarded.to,
        other => panic!("not forwarded: {other:?}"),
    }
}

fn rejected(harness: &Harness, outcome: Outcome) -> ErrorCode {
    match outcome {
        Outcome::Respond(length) => harness.reject_written(length).0,
        other => panic!("not answered: {other:?}"),
    }
}

#[test]
fn packets_follow_the_longest_route() {
    let mut harness = harness();
    set(
        &mut harness,
        &[("test.das.dfar", &[ONE]), ("test.das.dfar.child", &[TWO])],
    );
    let to_far = prepare("test.das.dfar.x", 10, NOW + 20_000, [1; 32]);
    assert_eq!(forwarded_to(harness.prepare(NOW, BTP, 1, &to_far)), ONE);
    let to_child = prepare("test.das.dfar.child.y", 10, NOW + 20_000, [2; 32]);
    assert_eq!(forwarded_to(harness.prepare(NOW, BTP, 2, &to_child)), TWO);
    // Whole segments only (NET-33).
    let lookalike = prepare("test.das.dfarther", 10, NOW + 20_000, [3; 32]);
    let outcome = harness.prepare(NOW, BTP, 3, &lookalike);
    assert_eq!(rejected(&harness, outcome), ErrorCode::F02_UNREACHABLE);
}

// A packet never goes straight back to the peer it came from: the next
// option is used, and with none, F02.
#[test]
fn packets_do_not_bounce_back() {
    let mut harness = harness();
    set(&mut harness, &[("test.das.dfar", &[ONE, TWO])]);
    let packet = prepare("test.das.dfar", 10, NOW + 20_000, [1; 32]);
    assert_eq!(forwarded_to(harness.prepare(NOW, ONE, 1, &packet)), TWO);
    set(&mut harness, &[("test.das.dfar", &[ONE])]);
    let outcome = harness.prepare(NOW, ONE, 2, &packet);
    assert_eq!(rejected(&harness, outcome), ErrorCode::F02_UNREACHABLE);
}

// Below our own address only children are routes; our own address itself
// has none (NET-34), and no route may claim our subtree.
#[test]
fn our_own_address_is_ours() {
    let mut harness = harness();
    assert_eq!(
        harness.connector.set_routes([(OWN, &[ONE][..])]),
        Err(RoutesError::InvalidPrefix)
    );
    set(&mut harness, &[("test.das", &[ONE])]);
    let own = prepare(OWN, 10, NOW + 20_000, [1; 32]);
    let outcome = harness.prepare(NOW, TWO, 1, &own);
    assert_eq!(rejected(&harness, outcome), ErrorCode::F02_UNREACHABLE);
    let missing = prepare(&format!("{OWN}.nobody"), 10, NOW + 20_000, [1; 32]);
    let outcome = harness.prepare(NOW, TWO, 2, &missing);
    assert_eq!(rejected(&harness, outcome), ErrorCode::F02_UNREACHABLE);
    let child = prepare(&format!("{OWN}.alice"), 10, NOW + 20_000, [1; 32]);
    assert_eq!(forwarded_to(harness.prepare(NOW, TWO, 3, &child)), BTP);
}

// A peer's packets debit its account, within the credit it was given:
// eleven packets of 0.1 fit a credit of 1.1, the twelfth gets T04.
#[test]
fn peer_packets_book_against_its_credit() {
    let mut harness = harness();
    let three = EndpointId::Peer(3);
    harness.add_peer(3, "dthree", TRUST, 0);
    for request_id in 0..11 {
        let packet = prepare(&format!("{OWN}.alice"), 100_000_000, NOW + 20_000, [4; 32]);
        assert_eq!(
            forwarded_to(harness.prepare(NOW, three, request_id, &packet)),
            BTP
        );
    }
    let packet = prepare(&format!("{OWN}.alice"), 100_000_000, NOW + 20_000, [4; 32]);
    let outcome = harness.prepare(NOW, three, 11, &packet);
    assert_eq!(
        rejected(&harness, outcome),
        ErrorCode::T04_INSUFFICIENT_LIQUIDITY
    );
}

// Outgoing request IDs to a peer start where the caller said, so after a
// restart they do not collide with IDs the peer remembers.
#[test]
fn peer_request_ids_start_at_random() {
    let mut harness = harness();
    set(&mut harness, &[("test.das.dfar", &[ONE])]);
    let packet = prepare("test.das.dfar", 10, NOW + 20_000, [1; 32]);
    match harness.prepare(NOW, BTP, 1, &packet) {
        Outcome::Forward(forwarded) => assert_eq!(forwarded.request_id, 7_000),
        other => panic!("{other:?}"),
    }
}

// A packet that surely did not reach its peer is rejected at once with
// what the caller says; its transfer is voided.
#[test]
fn undelivered_packets_fail_at_once() {
    let mut harness = harness();
    set(&mut harness, &[("test.das.dfar", &[ONE])]);
    let packet = prepare("test.das.dfar", 10, NOW + 20_000, [1; 32]);
    let Outcome::Forward(forwarded) = harness.prepare(NOW, BTP, 9, &packet) else {
        panic!()
    };
    assert!(harness.connector.fail_forwarded(
        NOW + 5,
        ONE,
        forwarded.request_id,
        ErrorCode::T01_PEER_UNREACHABLE,
        "the peer could not be reached",
    ));
    assert_eq!(harness.connector.poll_timeout(), Some(NOW + 5));
    let upstream = harness.timeout(NOW + 5).unwrap();
    assert_eq!((upstream.to, upstream.request_id), (BTP, 9));
    let (code, _) = harness.reject_written(upstream.length);
    assert_eq!(code, ErrorCode::T01_PEER_UNREACHABLE);
    assert_eq!(harness.ledger.pending_transfers().count(), 0);
    assert!(!harness.connector.fail_forwarded(
        NOW + 6,
        ONE,
        forwarded.request_id,
        ErrorCode::T01_PEER_UNREACHABLE,
        "again"
    ));
}

// A peer asking ILDCP learns the address we gave it and our asset.
#[test]
fn peers_get_their_address_over_ildcp() {
    let mut harness = harness();
    let request = ildcp::request(dassie_ilp::Timestamp::from_unix_ms(NOW)).to_vec();
    let Outcome::Respond(length) = harness.prepare(NOW, ONE, 1, &request) else {
        panic!()
    };
    let written = harness.written(length);
    let Response::Fulfill(Fulfill { data, .. }) = Response::decode_exact(&written).unwrap() else {
        panic!("not fulfilled")
    };
    let answer = ildcp::Response::decode_exact(data).unwrap();
    assert_eq!(answer.address.as_str(), "test.das.done");
    assert_eq!(answer.asset_code, "USD");
}

#[test]
fn the_route_table_is_bounded() {
    let mut harness = Harness::new(
        Config {
            max_routes: 1,
            ..Config::DEFAULT
        },
        0,
    );
    harness.add_peer(1, "done", TRUST, 0);
    let two = [("test.das.da", &[ONE][..]), ("test.das.db", &[ONE][..])];
    assert_eq!(harness.connector.set_routes(two), Err(RoutesError::TooMany));
    assert_eq!(harness.connector.routes().count(), 0);
    let _ = decode_prepare;
}

// A peer whose peering is ending takes no new Prepares, either way, while
// the packets in flight finish: then nothing is pending with it, and it
// can go with no packet's outcome left unknown.
#[test]
fn draining_peers_finish_what_is_in_flight() {
    let mut harness = harness();
    set(&mut harness, &[("test.das.dfar", &[ONE, TWO])]);
    let packet = prepare("test.das.dfar", 10, NOW + 20_000, [1; 32]);
    let Outcome::Forward(forwarded) = harness.prepare(NOW, BTP, 1, &packet) else {
        panic!()
    };
    assert_eq!(forwarded.to, ONE);
    assert_eq!(harness.connector.pending_for(ONE), 1);
    assert!(harness.connector.set_draining(ONE, true));
    // New packets go around it, and its own are refused.
    let next = prepare("test.das.dfar", 10, NOW + 20_000, [2; 32]);
    assert_eq!(forwarded_to(harness.prepare(NOW, BTP, 2, &next)), TWO);
    let from_it = prepare(&format!("{OWN}.alice"), 10, NOW + 20_000, [3; 32]);
    let outcome = harness.prepare(NOW, ONE, 1, &from_it);
    assert_eq!(rejected(&harness, outcome), ErrorCode::T01_PEER_UNREACHABLE);
    // The packet in flight still completes.
    let upstream = harness
        .respond(
            NOW + 1,
            ONE,
            forwarded.request_id,
            &support::fulfill([1; 32]),
        )
        .unwrap();
    assert_eq!((upstream.to, upstream.request_id), (BTP, 1));
    assert_eq!(harness.connector.pending_for(ONE), 0);
    assert!(!harness.connector.set_draining(EndpointId::Peer(9), true));
}

fn fulfill_bytes(fulfillment: [u8; 32]) -> Vec<u8> {
    Fulfill {
        fulfillment,
        data: b"",
    }
    .to_vec()
}

// D17: a peer's Fulfill that is not the packet's is a resend from before
// a restart, for a request ID used again: it is unmatched, and the packet
// waits for its own response.
#[test]
fn a_peers_stale_fulfill_leaves_the_packet_in_flight() {
    let mut harness = harness();
    set(&mut harness, &[("test.das.dfar", &[ONE])]);
    let packet = prepare("test.das.dfar", 10, NOW + 20_000, [5; 32]);
    let Outcome::Forward(forwarded) = harness.prepare(NOW, TWO, 1, &packet) else {
        panic!("not forwarded");
    };
    let stale = harness.respond_full(NOW + 5, ONE, forwarded.request_id, &fulfill_bytes([6; 32]));
    assert_eq!(stale, Responded::Unmatched);
    assert_eq!(harness.connector.pending_count(), 1);
    // Its own Fulfill posts it, and says what was posted for whom.
    let responded =
        harness.respond_full(NOW + 6, ONE, forwarded.request_id, &fulfill_bytes([5; 32]));
    let Responded::Resolved {
        upstream: Some(upstream),
        posted: Some(posted),
    } = responded
    else {
        panic!("not posted: {responded:?}");
    };
    assert_eq!((upstream.to, upstream.request_id), (TWO, 1));
    assert_eq!((posted.source, posted.source_request_id), (TWO, 1));
    assert_eq!(posted.destination, ONE);
    assert_eq!(posted.condition, dassie_ilp::condition_for(&[5; 32]));
    assert_eq!(posted.transfers[0].unwrap().amount, 10);
    assert!(posted.transfers[1].is_none());
    // Once resolved, the same Fulfill again is unmatched.
    let again = harness.respond_full(NOW + 7, ONE, forwarded.request_id, &fulfill_bytes([5; 32]));
    assert_eq!(again, Responded::Unmatched);
}

// A late Fulfill resolves the packet as expired: nothing is posted.
#[test]
fn a_late_fulfill_posts_nothing() {
    let mut harness = harness();
    set(&mut harness, &[("test.das.dfar", &[ONE])]);
    let packet = prepare("test.das.dfar", 10, NOW + 20_000, [5; 32]);
    let Outcome::Forward(forwarded) = harness.prepare(NOW, TWO, 1, &packet) else {
        panic!("not forwarded");
    };
    let expiry = decode_prepare(&harness.written(forwarded.length))
        .expires_at
        .unix_ms;
    let responded =
        harness.respond_full(expiry, ONE, forwarded.request_id, &fulfill_bytes([5; 32]));
    assert!(matches!(
        responded,
        Responded::Resolved { posted: None, .. }
    ));
}
