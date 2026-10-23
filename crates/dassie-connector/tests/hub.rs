//! Packets to a peer over a settlement hub
//! (spec/designs/settlement-hubs.md#4-peering-over-a-hub): when one times
//! out, its Reject upstream waits for the hub, since the payee may still
//! win a claim for it.

#![allow(clippy::panic, clippy::unwrap_used, clippy::expect_used)]

use dassie_connector::sim::*;
use dassie_connector::{Config, EndpointId, HubWait, Outcome, Responded};
use dassie_ilp::{ErrorCode, Response};
use dassie_oer::Decode;

const BTP: EndpointId = EndpointId::Btp(1);
const HUB_PEER: EndpointId = EndpointId::Peer(1);
const TRUST: u64 = 1_100_000_000;

/// A Prepare from BTP that expires at `NOW + 20 s` upstream, so at
/// `NOW + 19 s` toward the hub peer.
fn send(harness: &mut Harness, request_id: u32, fulfillment: [u8; 32]) -> u32 {
    let packet = prepare("test.das.dhub.x", 10, NOW + 20_000, fulfillment);
    match harness.prepare(NOW, BTP, request_id, &packet) {
        Outcome::Forward(forwarded) => {
            assert_eq!(forwarded.to, HUB_PEER);
            forwarded.request_id
        }
        other => panic!("not forwarded: {other:?}"),
    }
}

fn harness() -> Harness {
    let mut harness = Harness::new(Config::DEFAULT, 10_000_000_000);
    harness.add_peer(1, "dhub", TRUST, 0);
    assert!(harness.connector.set_hub(HUB_PEER, true));
    harness
        .connector
        .set_routes([("test.das.dhub", &[HUB_PEER][..])])
        .unwrap();
    harness
}

fn wait(harness: &mut Harness) -> HubWait {
    harness
        .connector
        .poll_hub_wait()
        .expect("a packet waits for the hub")
}

#[test]
fn a_timed_out_packet_waits_for_the_hub_until_its_incoming_expiry() {
    let mut harness = harness();
    let request_id = send(&mut harness, 1, [3; 32]);
    assert!(harness.timeout(NOW + 19_000).is_none(), "rejected at once");
    let waiting = wait(&mut harness);
    assert_eq!(
        (waiting.destination, waiting.request_id),
        (HUB_PEER, request_id)
    );
    assert_eq!(waiting.backstop_ms, NOW + 20_000);
    assert_eq!(harness.connector.poll_timeout(), Some(NOW + 20_000));
    // Unanswered by the backstop, it is rejected.
    let upstream = harness.timeout(NOW + 20_000).unwrap();
    assert_eq!(upstream.to, BTP);
    assert_eq!(
        harness.reject_written(upstream.length).0,
        ErrorCode::R00_TRANSFER_TIMED_OUT
    );
    assert!(harness.connector.poll_hub_wait().is_none());
}

#[test]
fn the_hub_decides_a_waiting_packet() {
    let mut harness = harness();
    let won = send(&mut harness, 1, [3; 32]);
    let lost = send(&mut harness, 2, [4; 32]);
    harness.timeout(NOW + 19_000);
    harness.check();
    // The payee won a claim for the first: it is fulfilled upstream.
    let fulfilled = harness.connector.settle_hub(
        NOW + 19_400,
        HUB_PEER,
        won,
        Some([3; 32]),
        &mut harness.ledger,
        &mut harness.buffer,
    );
    harness.check();
    let Responded::Resolved {
        upstream: Some(upstream),
        posted: Some(_),
    } = fulfilled
    else {
        panic!("{fulfilled:?}");
    };
    let written = harness.written(upstream.length);
    let Ok(Response::Fulfill(sent)) = Response::decode_exact(&written) else {
        panic!("not a Fulfill");
    };
    assert_eq!(sent.fulfillment, [3; 32]);
    // A fulfillment that is not the packet's changes nothing.
    let wrong = harness.connector.settle_hub(
        NOW + 19_400,
        HUB_PEER,
        lost,
        Some([9; 32]),
        &mut harness.ledger,
        &mut harness.buffer,
    );
    assert_eq!(wrong, Responded::Unmatched);
    // The second has no claim by the final block: rejected.
    let rejected = harness.connector.settle_hub(
        NOW + 19_500,
        HUB_PEER,
        lost,
        None,
        &mut harness.ledger,
        &mut harness.buffer,
    );
    harness.check();
    let Responded::Resolved {
        upstream: Some(upstream),
        posted: None,
    } = rejected
    else {
        panic!("{rejected:?}");
    };
    assert_eq!(
        harness.reject_written(upstream.length).0,
        ErrorCode::R00_TRANSFER_TIMED_OUT
    );
    assert_eq!(harness.connector.pending_count(), 0);
}

#[test]
fn a_late_fulfill_from_the_peer_is_taken_while_the_packet_waits() {
    let mut harness = harness();
    let request_id = send(&mut harness, 1, [3; 32]);
    harness.timeout(NOW + 19_000);
    let upstream = harness
        .respond(NOW + 19_200, HUB_PEER, request_id, &fulfill([3; 32]))
        .unwrap();
    assert_eq!(harness.written(upstream.length), fulfill([3; 32]));
}

#[test]
fn only_waiting_packets_are_settled_by_the_hub() {
    let mut harness = harness();
    let request_id = send(&mut harness, 1, [3; 32]);
    let early = harness.connector.settle_hub(
        NOW + 1,
        HUB_PEER,
        request_id,
        None,
        &mut harness.ledger,
        &mut harness.buffer,
    );
    assert_eq!(early, Responded::Unmatched);
    assert_eq!(harness.connector.pending_count(), 1);
}

#[test]
fn packets_to_other_peers_are_rejected_at_their_expiry() {
    let mut harness = harness();
    assert!(harness.connector.set_hub(HUB_PEER, false));
    send(&mut harness, 1, [3; 32]);
    let upstream = harness.timeout(NOW + 19_000).unwrap();
    assert_eq!(
        harness.reject_written(upstream.length).0,
        ErrorCode::R00_TRANSFER_TIMED_OUT
    );
    assert!(
        !harness.connector.set_hub(BTP, true),
        "only peers settle over hubs"
    );
}

#[test]
fn a_fulfill_at_the_expiry_before_the_timer_ran_is_taken() {
    let mut harness = harness();
    let request_id = send(&mut harness, 1, [3; 32]);
    // The Fulfill comes in the step whose timer would start the wait.
    let upstream = harness
        .respond(NOW + 19_000, HUB_PEER, request_id, &fulfill([3; 32]))
        .unwrap();
    assert_eq!(harness.written(upstream.length), fulfill([3; 32]));
    assert!(harness.timeout(NOW + 19_000).is_none());
}

#[test]
fn a_far_incoming_expiry_waits_at_most_one_window() {
    let mut harness = harness();
    // Expires toward the peer at the 30 s cap, far before upstream does.
    let packet = prepare("test.das.dhub.x", 10, NOW + 120_000, [3; 32]);
    let Outcome::Forward(sent) = harness.prepare(NOW, BTP, 1, &packet) else {
        panic!("not forwarded");
    };
    assert_eq!(sent.to, HUB_PEER);
    assert!(harness.timeout(NOW + 30_000).is_none());
    assert_eq!(wait(&mut harness).backstop_ms, NOW + 31_000);
    let upstream = harness.timeout(NOW + 31_000).unwrap();
    assert_eq!(
        harness.reject_written(upstream.length).0,
        ErrorCode::R00_TRANSFER_TIMED_OUT
    );
}

#[test]
fn a_waiting_packet_fails_at_once_when_its_peer_goes() {
    let mut harness = harness();
    send(&mut harness, 1, [3; 32]);
    harness.timeout(NOW + 19_000);
    assert!(harness.connector.remove_endpoint(NOW + 19_100, HUB_PEER));
    harness.check();
    let upstream = harness.timeout(NOW + 19_100).unwrap();
    assert_eq!(
        harness.reject_written(upstream.length).0,
        ErrorCode::T01_PEER_UNREACHABLE
    );
}

#[test]
fn a_waiting_packet_can_still_be_failed() {
    let mut harness = harness();
    let request_id = send(&mut harness, 1, [3; 32]);
    harness.timeout(NOW + 19_000);
    assert!(harness.connector.fail_forwarded(
        NOW + 19_100,
        HUB_PEER,
        request_id,
        ErrorCode::T01_PEER_UNREACHABLE,
        "unreachable",
    ));
    harness.check();
    let upstream = harness.timeout(NOW + 19_100).unwrap();
    assert_eq!(
        harness.reject_written(upstream.length).0,
        ErrorCode::T01_PEER_UNREACHABLE
    );
}
