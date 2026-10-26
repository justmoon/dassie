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
    // A window of one second, whatever the default, so that the times
    // below read plainly.
    let config = Config {
        message_window_ms: 1_000,
        ..Config::DEFAULT
    };
    let mut harness = Harness::new(config, 10_000_000_000);
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

// spec/areas/connector.md#inventory-pricing: a ledger of a settlement hub
// converts as any other with a currency. Transit from a peer on dollars to
// a peer over the hub, in the hub's XRP, gets the curve's price, books its
// conversion on the hub ledger's FX accounts, and comes back the same way.
#[test]
fn transit_converts_into_and_out_of_a_hub_ledger() {
    use dassie_connector::RATE_PRECISION;
    use dassie_ledger::{AccountName, LedgerId};
    const HUB: LedgerId = LedgerId::HubXrplTestnetXrp;
    const DOLLAR_PEER: EndpointId = EndpointId::Peer(2);
    let dollar = 1_000_000_000;
    // A hundred dollars, and two hundred XRP in the hub, worth as much.
    let mut harness = Harness::new(Config::DEFAULT, 100 * dollar);
    harness.fund(HUB, 200 * dollar);
    harness.open_books(LedgerId::StubUsd);
    harness.open_books(HUB);
    harness.rates.set("USD", 10u128.pow(RATE_PRECISION));
    harness.rates.set("XRP", 2 * 10u128.pow(RATE_PRECISION));
    harness.add_peer_on(HUB, 3, "dxrp", TRUST, 0);
    let hub_peer = EndpointId::Peer(3);
    assert!(harness.connector.set_hub(hub_peer, true));
    harness.add_peer(2, "dusd", TRUST, 0);
    harness
        .connector
        .set_routes([
            ("test.das.dxrp", &[hub_peer][..]),
            ("test.das.dusd", &[DOLLAR_PEER][..]),
        ])
        .unwrap();
    let prices = harness.prices(NOW);
    assert!(prices.ledger(HUB).is_some());
    let holdings = prices.live_holdings(&harness.ledger);
    let max_age = Config::DEFAULT.max_rate_age_ms;
    // Half the largest packet.
    let amount = dollar / 20;
    let expected = prices
        .convert(
            LedgerId::StubUsd,
            HUB,
            amount,
            amount,
            &holdings,
            NOW,
            max_age,
        )
        .unwrap();
    // Near target, about twice the dollars in XRP.
    assert!(expected <= 2 * amount && expected > 2 * amount / 100 * 99);
    let packet = prepare("test.das.dxrp.x", amount, NOW + 20_000, [3; 32]);
    let Outcome::Forward(sent) = harness.prepare(NOW, DOLLAR_PEER, 1, &packet) else {
        panic!("not forwarded");
    };
    assert_eq!(sent.to, hub_peer);
    assert_eq!(
        decode_prepare(&harness.buffer[..sent.length]).amount,
        expected
    );
    harness
        .respond(NOW + 1, hub_peer, sent.request_id, &fulfill([3; 32]))
        .unwrap();
    let expected_fx = -i128::from(expected);
    assert_eq!(harness.net_at(HUB, AccountName::ExpensesFx), expected_fx);
    // And back out of the hub, into dollars.
    let packet = prepare("test.das.dusd.x", expected, NOW + 20_000, [4; 32]);
    let Outcome::Forward(back) = harness.prepare(NOW + 2, hub_peer, 2, &packet) else {
        panic!("not forwarded");
    };
    assert_eq!(back.to, DOLLAR_PEER);
    let returned = decode_prepare(&harness.buffer[..back.length]).amount;
    assert!(returned < amount, "a round trip never gains: {returned}");
}
