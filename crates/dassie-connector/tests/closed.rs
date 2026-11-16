//! A closed connector, while the node drains before it stops
//! (spec/areas/node.md#shutdown): no new packet goes in flight, from any
//! source, while those in flight finish.

#![allow(clippy::panic, clippy::unwrap_used, clippy::expect_used)]

use dassie_connector::sim::*;
use dassie_connector::{Config, EndpointId, Forwarded, Outcome};
use dassie_ilp::{ErrorCode, Response, Timestamp, ildcp};
use dassie_ledger::LedgerId;
use dassie_oer::{Decode, Encode};

const ALICE: EndpointId = EndpointId::Btp(1);
const PEER: EndpointId = EndpointId::Peer(1);
const SWAP: EndpointId = EndpointId::Local(5);
const TRUST: u64 = 1_000_000_000_000;

fn forwarded(outcome: Outcome) -> Forwarded {
    match outcome {
        Outcome::Forward(forwarded) => forwarded,
        other => panic!("expected a forward, got {other:?}"),
    }
}

/// The code and message a Prepare was refused with.
fn refused(h: &Harness, outcome: Outcome) -> (ErrorCode, String) {
    let Outcome::Respond(length) = outcome else {
        panic!("expected a reject, got {outcome:?}");
    };
    match Response::decode_exact(&h.written(length)).unwrap() {
        Response::Reject(reject) => (reject.code, reject.message.to_owned()),
        Response::Fulfill(_) => panic!("expected a reject"),
    }
}

/// A node with a BTP client, a peer on USD and XRP that it routes
/// `test.das.dpeer` to, and a swap endpoint on USD.
fn harness() -> Harness {
    let mut h = Harness::new(Config::DEFAULT, TRUST);
    h.open_books(LedgerId::StubUsd);
    h.fund(LedgerId::XrplTestnetXrp, TRUST);
    h.open_books(LedgerId::XrplTestnetXrp);
    h.add_peer(1, "dpeer", TRUST, 0);
    h.add_slot(LedgerId::XrplTestnetXrp, 1, "dpeer", TRUST);
    h.connector
        .set_routes([("test.das.dpeer", &[PEER][..])])
        .unwrap();
    h.add_swap(5, LedgerId::StubUsd);
    h
}

/// A packet from each kind of source: a BTP client, a peer, the node's own
/// endpoint, and the node's own swap.
fn send_one_of_each(h: &mut Harness) -> [Outcome; 4] {
    let to_receiver = prepare(&format!("{OWN}.stream.x"), 1, NOW + 20_000, [3; 32]);
    let to_peer = prepare("test.das.dpeer.x", 1, NOW + 20_000, [3; 32]);
    let to_alice = prepare(&format!("{OWN}.alice"), 1, NOW + 20_000, [3; 32]);
    let swap = prepare("peer.swap.xrpl-testnet-xrp", 1, NOW + 20_000, [3; 32]);
    [
        h.prepare(NOW, ALICE, 1, &to_peer),
        h.prepare(NOW, PEER, 1, &to_receiver),
        h.prepare(NOW, RECEIVER, 1, &to_alice),
        h.send_swap(NOW, (SWAP, 1), PEER, &swap),
    ]
}

// Every source is refused with T03, which senders take as temporary: a
// BTP client, a peer, the node's own endpoint and its own swap, each of
// which goes while the connector is open.
#[test]
fn a_closed_connector_refuses_every_new_packet_with_t03() {
    let mut open = harness();
    for outcome in send_one_of_each(&mut open) {
        assert!(matches!(outcome, Outcome::Forward(_)), "{outcome:?}");
    }
    let mut h = harness();
    h.connector.set_closed(true);
    assert!(h.connector.is_closed());
    for outcome in send_one_of_each(&mut h) {
        let (code, message) = refused(&h, outcome);
        assert_eq!(code, ErrorCode::T03_CONNECTOR_BUSY);
        assert_eq!(message, "the node is stopping");
    }
    assert_eq!(h.connector.pending_count(), 0);
    assert_eq!(h.ledger.pending_transfers().count(), 0);
    h.check();
}

// Packets in flight when it closes still finish, and are booked.
#[test]
fn packets_in_flight_finish_after_it_closes() {
    let mut h = harness();
    let packet = prepare("test.das.dpeer.x", 7, NOW + 20_000, [3; 32]);
    let sent = forwarded(h.prepare(NOW, ALICE, 1, &packet));
    h.connector.set_closed(true);
    let upstream = h
        .respond(NOW + 5, PEER, sent.request_id, &fulfill([3; 32]))
        .unwrap();
    assert_eq!(upstream.to, ALICE);
    assert_eq!(h.connector.pending_count(), 0);
    let alice = h.connector.account_of(ALICE).unwrap();
    assert_eq!(h.net(alice), -7, "Alice paid");
    h.check();
}

// Opened again, as when the owner cancels the quit, it takes packets.
#[test]
fn opened_again_it_takes_packets() {
    let mut h = harness();
    h.connector.set_closed(true);
    h.connector.set_closed(false);
    let packet = prepare("test.das.dpeer.x", 1, NOW + 20_000, [3; 32]);
    assert_eq!(forwarded(h.prepare(NOW, ALICE, 1, &packet)).to, PEER);
}

// ILDCP puts nothing in flight, so a client still learns its address.
#[test]
fn ildcp_is_still_answered() {
    let mut h = harness();
    h.connector.set_closed(true);
    let request = ildcp::request(Timestamp::from_unix_ms(NOW)).to_vec();
    let Outcome::Respond(length) = h.prepare(NOW, ALICE, 1, &request) else {
        panic!("ILDCP was not answered");
    };
    assert!(matches!(
        Response::decode_exact(&h.written(length)).unwrap(),
        Response::Fulfill(_)
    ));
}

// The latest deadline is the latest expiry of the packets in flight.
#[test]
fn the_latest_deadline_is_the_latest_expiry() {
    let mut h = harness();
    assert_eq!(h.connector.latest_deadline(NOW), None);
    let window = Config::DEFAULT.message_window_ms;
    let early = prepare("test.das.dpeer.x", 1, NOW + 10_000, [3; 32]);
    let late = prepare("test.das.dpeer.x", 1, NOW + 20_000, [4; 32]);
    forwarded(h.prepare(NOW, ALICE, 1, &late));
    forwarded(h.prepare(NOW, ALICE, 2, &early));
    assert_eq!(h.connector.poll_timeout(), Some(NOW + 10_000 - window));
    assert_eq!(
        h.connector.latest_deadline(NOW),
        Some(NOW + 20_000 - window)
    );
}

// A packet over a hub may wait for the hub past its own expiry, until its
// backstop: that is the latest it is answered by, before it expires too.
#[test]
fn the_latest_deadline_counts_a_hub_packets_backstop() {
    let mut h = harness();
    assert!(h.connector.set_hub(PEER, LedgerId::StubUsd, true));
    let packet = prepare("test.das.dpeer.x", 1, NOW + 120_000, [3; 32]);
    forwarded(h.prepare(NOW, ALICE, 1, &packet));
    let config = Config::DEFAULT;
    let expires = NOW + config.max_hold_ms;
    assert_eq!(h.connector.poll_timeout(), Some(expires));
    assert_eq!(
        h.connector.latest_deadline(NOW),
        Some(expires + config.message_window_ms)
    );
}
