//! Peers and the route table: packets to other nodes go to a peer the
//! routes name, never back where they came from; below our own address
//! only children count; a packet that surely did not reach its peer is
//! rejected at once.
//!
//! The harness draws every next hop with 0, which takes a route's first
//! usable option on the packet's incoming ledger (or its first usable one
//! when none is on that ledger) and, with the default exploring share,
//! ignores earnings: these tests are about which options are usable. How a
//! hop is drawn among them is in `next_hops.rs`
//! (spec/decisions/weighted-next-hops.md).

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use dassie_connector::sim::{Harness, NOW, OWN, decode_prepare, fulfill, prepare};
use dassie_connector::{Config, EndpointId, Outcome, Responded, RoutesError};
use dassie_ilp::{ErrorCode, Fulfill, Response, ildcp};
use dassie_oer::{Decode, Encode};

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
    // Whole segments only.
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
// has none, and no route may claim our subtree.
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
    // Each fulfilled before the next, as only a few may be in flight.
    for request_id in 0..11 {
        let packet = prepare(&format!("{OWN}.alice"), 100_000_000, NOW + 20_000, [4; 32]);
        let Outcome::Forward(forwarded) = harness.prepare(NOW, three, request_id, &packet) else {
            panic!("not forwarded")
        };
        assert_eq!(forwarded.to, BTP);
        assert!(
            harness
                .respond(NOW, BTP, forwarded.request_id, &fulfill([4; 32]))
                .is_some()
        );
    }
    let packet = prepare(&format!("{OWN}.alice"), 100_000_000, NOW + 20_000, [4; 32]);
    let outcome = harness.prepare(NOW, three, 11, &packet);
    assert_eq!(
        rejected(&harness, outcome),
        ErrorCode::T04_INSUFFICIENT_LIQUIDITY
    );
}

// spec/decisions/packets-in-flight.md: money in flight with a peer stays
// within two and a half of the largest packets each way, whatever the credit:
// a crash loses the Fulfills of what was in flight, so this is how far it
// makes two peers' books drift apart. Two of the largest fit, and the half
// left takes smaller ones.
#[test]
fn money_in_flight_with_a_peer_is_capped() {
    let mut harness = harness();
    set(&mut harness, &[("test.das.dfar", &[TWO])]);
    let max = Config::DEFAULT.max_packet_amount;
    let mut ids = Vec::new();
    // From a peer: two of the largest packets, but not a third.
    for request_id in 0..2 {
        let packet = prepare(&format!("{OWN}.alice"), max, NOW + 20_000, [4; 32]);
        let Outcome::Forward(forwarded) = harness.prepare(NOW, ONE, request_id, &packet) else {
            panic!("not forwarded")
        };
        ids.push(forwarded.request_id);
    }
    let packet = prepare(&format!("{OWN}.alice"), max, NOW + 20_000, [4; 32]);
    let outcome = harness.prepare(NOW, ONE, 2, &packet);
    assert_eq!(rejected(&harness, outcome), ErrorCode::T05_RATE_LIMITED);
    // The half packet left takes the first volley of a quote's probes, which
    // need only one of them answered; past it, T05 again.
    for (request_id, amount) in [(10, 1), (11, 1_000), (12, 1_000_000)] {
        let probe = prepare(&format!("{OWN}.alice"), amount, NOW + 20_000, [6; 32]);
        assert_eq!(
            forwarded_to(harness.prepare(NOW, ONE, request_id, &probe)),
            BTP
        );
    }
    let rest = max / 2 - 1_001_001;
    let probe = prepare(&format!("{OWN}.alice"), rest + 1, NOW + 20_000, [6; 32]);
    let outcome = harness.prepare(NOW, ONE, 13, &probe);
    assert_eq!(rejected(&harness, outcome), ErrorCode::T05_RATE_LIMITED);
    let probe = prepare(&format!("{OWN}.alice"), rest, NOW + 20_000, [6; 32]);
    assert_eq!(forwarded_to(harness.prepare(NOW, ONE, 14, &probe)), BTP);
    // Packets without money still pass, and so does money to another peer.
    let empty = prepare(&format!("{OWN}.alice"), 0, NOW + 20_000, [4; 32]);
    assert_eq!(forwarded_to(harness.prepare(NOW, ONE, 4, &empty)), BTP);
    let onward = prepare("test.das.dfar", max, NOW + 20_000, [5; 32]);
    assert_eq!(forwarded_to(harness.prepare(NOW, BTP, 1, &onward)), TWO);
    // A packet resolved makes room again.
    assert!(
        harness
            .respond(NOW, BTP, ids[0], &fulfill([4; 32]))
            .is_some()
    );
    assert_eq!(forwarded_to(harness.prepare(NOW, ONE, 5, &packet)), BTP);

    // To a peer: the same, counted on what goes out.
    for request_id in 2..3 {
        let onward = prepare("test.das.dfar", max, NOW + 20_000, [5; 32]);
        assert_eq!(
            forwarded_to(harness.prepare(NOW, BTP, request_id, &onward)),
            TWO
        );
    }
    let onward = prepare("test.das.dfar", max, NOW + 20_000, [5; 32]);
    let outcome = harness.prepare(NOW, BTP, 4, &onward);
    assert_eq!(rejected(&harness, outcome), ErrorCode::T05_RATE_LIMITED);
    // Expired packets leave nothing in flight.
    while harness.timeout(NOW + 20_000).is_some() {}
    let onward = prepare("test.das.dfar", max, NOW + 40_000, [5; 32]);
    assert_eq!(
        forwarded_to(harness.prepare(NOW + 20_000, BTP, 5, &onward)),
        TWO
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
            &dassie_connector::sim::fulfill([1; 32]),
        )
        .unwrap();
    assert_eq!((upstream.to, upstream.request_id), (BTP, 1));
    assert_eq!(harness.connector.pending_for(ONE), 0);
    assert!(!harness.connector.set_draining(EndpointId::Peer(9), true));
}

// A peer whose peering we end, and have not told yet
// (spec/decisions/ending-a-peering.md), has its new Prepares refused as for
// want of liquidity, which tells it nothing more; ours still go to it.
#[test]
fn a_refusing_peer_still_takes_ours() {
    let mut harness = harness();
    set(&mut harness, &[("test.das.dfar", &[ONE, TWO])]);
    assert!(harness.connector.set_refusing(ONE, true));
    let from_it = prepare(&format!("{OWN}.alice"), 10, NOW + 20_000, [3; 32]);
    let outcome = harness.prepare(NOW, ONE, 1, &from_it);
    assert_eq!(
        rejected(&harness, outcome),
        ErrorCode::T04_INSUFFICIENT_LIQUIDITY
    );
    let packet = prepare("test.das.dfar", 10, NOW + 20_000, [1; 32]);
    assert_eq!(forwarded_to(harness.prepare(NOW, BTP, 1, &packet)), ONE);
    assert!(!harness.connector.set_refusing(EndpointId::Peer(9), true));
}

// A peer our messages surely do not reach has its new Prepares refused,
// for a while, as for want of liquidity
// (spec/decisions/unreachable-peers.md); ours still go to it. The refusal is
// its own: it holds while a spec/decisions/ending-a-peering.md refusal is
// lifted, and lifting it leaves a spec/decisions/ending-a-peering.md refusal
// in place.
#[test]
fn an_unreached_peer_still_takes_ours() {
    let mut harness = harness();
    set(&mut harness, &[("test.das.dfar", &[ONE, TWO])]);
    let from_it = |id: u8| prepare(&format!("{OWN}.alice"), 10, NOW + 20_000, [id; 32]);
    assert!(harness.connector.set_unreached(ONE, true));
    let outcome = harness.prepare(NOW, ONE, 1, &from_it(3));
    assert_eq!(
        rejected(&harness, outcome),
        ErrorCode::T04_INSUFFICIENT_LIQUIDITY
    );
    let packet = prepare("test.das.dfar", 10, NOW + 20_000, [1; 32]);
    assert_eq!(forwarded_to(harness.prepare(NOW, BTP, 1, &packet)), ONE);
    assert!(harness.connector.set_refusing(ONE, true));
    assert!(harness.connector.set_refusing(ONE, false));
    let outcome = harness.prepare(NOW, ONE, 2, &from_it(4));
    assert_eq!(
        rejected(&harness, outcome),
        ErrorCode::T04_INSUFFICIENT_LIQUIDITY
    );
    assert!(harness.connector.set_refusing(ONE, true));
    assert!(harness.connector.set_unreached(ONE, false));
    let outcome = harness.prepare(NOW, ONE, 3, &from_it(5));
    assert_eq!(
        rejected(&harness, outcome),
        ErrorCode::T04_INSUFFICIENT_LIQUIDITY
    );
    assert!(harness.connector.set_refusing(ONE, false));
    let onward = prepare("test.das.dfar", 10, NOW + 20_000, [6; 32]);
    assert_eq!(forwarded_to(harness.prepare(NOW, ONE, 4, &onward)), TWO);
    assert!(!harness.connector.set_unreached(EndpointId::Peer(9), true));
}

fn fulfill_bytes(fulfillment: [u8; 32]) -> Vec<u8> {
    Fulfill {
        fulfillment,
        data: b"",
    }
    .to_vec()
}

// A peer's Fulfill that is not the packet's is a resend from before
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

// spec/areas/connector.md#addresses-and-endpoints: a downlink is a peer at
// its child address below ours. ILDCP tells it that address, and what lies
// below it is routed to it as to a child, from any source; while its
// peering ends, nothing new goes to it.
#[test]
fn downlinks_are_routed_below_our_address() {
    let mut harness = harness();
    let down = EndpointId::Peer(3);
    harness.add_downlink(3, "ddown", TRUST, 0);
    let request = ildcp::request(dassie_ilp::Timestamp::from_unix_ms(NOW + 10_000)).to_vec();
    let Outcome::Respond(length) = harness.prepare(NOW, down, 1, &request) else {
        panic!("ILDCP is answered at once")
    };
    let bytes = harness.written(length);
    let Response::Fulfill(Fulfill { data, .. }) = Response::decode_exact(&bytes).unwrap() else {
        panic!("ILDCP is fulfilled")
    };
    let answer = ildcp::Response::decode_exact(data).unwrap();
    assert_eq!(answer.address.as_str(), format!("{OWN}.ddown"));
    let packet = prepare(&format!("{OWN}.ddown.receiver"), 10, NOW + 20_000, [1; 32]);
    assert_eq!(forwarded_to(harness.prepare(NOW, ONE, 2, &packet)), down);
    harness.connector.set_draining(down, true);
    let outcome = harness.prepare(NOW, ONE, 3, &packet);
    assert_eq!(rejected(&harness, outcome), ErrorCode::F02_UNREACHABLE);
}

// spec/areas/connector.md#a-periphery-node: below the node's address beneath
// an uplink lie its own endpoints, which get the packet written below its
// own address; a peer's packet for anywhere else gets F02, while the
// node's own packets still go out by the routes.
#[test]
fn a_periphery_node_forwards_nothing() {
    let mut harness = harness();
    set(&mut harness, &[("test.das.dfar", &[TWO])]);
    let child = String::from("test.das.done.dnode");
    harness
        .connector
        .set_periphery(Some(core::slice::from_ref(&child)))
        .unwrap();
    let to_alice = prepare(&format!("{child}.alice.x"), 10, NOW + 20_000, [1; 32]);
    let Outcome::Forward(sent) = harness.prepare(NOW, ONE, 1, &to_alice) else {
        panic!("not forwarded")
    };
    assert_eq!(sent.to, BTP);
    let written = harness.written(sent.length);
    assert_eq!(
        decode_prepare(&written).destination.as_str(),
        format!("{OWN}.alice.x")
    );
    let onward = prepare("test.das.dfar.x", 10, NOW + 20_000, [2; 32]);
    let outcome = harness.prepare(NOW, ONE, 2, &onward);
    assert_eq!(rejected(&harness, outcome), ErrorCode::F02_UNREACHABLE);
    assert_eq!(forwarded_to(harness.prepare(NOW, BTP, 3, &onward)), TWO);
    // Its address beneath the uplink is no endpoint's.
    let bare = prepare(&child, 10, NOW + 20_000, [3; 32]);
    let outcome = harness.prepare(NOW, ONE, 4, &bare);
    assert_eq!(rejected(&harness, outcome), ErrorCode::F02_UNREACHABLE);
    assert_eq!(
        harness.connector.set_periphery(Some(&[format!("{OWN}.x")])),
        Err(RoutesError::InvalidPrefix)
    );
}

// spec/areas/connector.md#a-periphery-node: an address beneath each uplink,
// as many as a periphery node may be pinned to, each reaching its own
// endpoints; one more is refused, and changes nothing.
#[test]
fn a_periphery_node_takes_an_address_beneath_every_uplink() {
    let mut harness = harness();
    let addresses: Vec<String> = (0..=dassie_connector::MAX_CHILD_ADDRESSES)
        .map(|n| format!("test.das.dup{n}.dnode"))
        .collect();
    assert_eq!(
        harness.connector.set_periphery(Some(&addresses)),
        Err(RoutesError::TooMany)
    );
    let (all, extra) = addresses.split_at(dassie_connector::MAX_CHILD_ADDRESSES);
    harness.connector.set_periphery(Some(all)).unwrap();
    let last = all.last().unwrap();
    let packet = prepare(&format!("{last}.alice.x"), 10, NOW + 20_000, [1; 32]);
    let Outcome::Forward(sent) = harness.prepare(NOW, ONE, 1, &packet) else {
        panic!("not forwarded")
    };
    assert_eq!(sent.to, BTP);
    let beyond = prepare(&format!("{}.alice.x", extra[0]), 10, NOW + 20_000, [2; 32]);
    let outcome = harness.prepare(NOW, ONE, 2, &beyond);
    assert_eq!(rejected(&harness, outcome), ErrorCode::F02_UNREACHABLE);
}
