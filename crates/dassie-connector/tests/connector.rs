//! The connector's behaviour, each D13 fix with a test that fails against
//! the original (spec/original/README.md#conventions).

#![allow(clippy::panic)]

use dassie_connector::sim::*;
use dassie_connector::{Config, EndpointId, Forwarded, Outcome, PacketState, RATE_PRECISION};
use dassie_ilp::{AmountTooLarge, ErrorCode, Response, ildcp};
use dassie_ledger::{
    AccountName, AccountPath, JournalEntry, LedgerId, Name, Reference, TransferState,
};
use dassie_oer::{Decode, Encode};

const ALICE: EndpointId = EndpointId::Btp(1);
const BOB: EndpointId = EndpointId::Btp(2);
const UNIT: u64 = 1_000_000_000;

fn forwarded(outcome: Outcome) -> Forwarded {
    match outcome {
        Outcome::Forward(forwarded) => forwarded,
        other => panic!("expected a forward, got {other:?}"),
    }
}

fn answered(outcome: Outcome) -> usize {
    match outcome {
        Outcome::Respond(length) => length,
        other => panic!("expected an answer, got {other:?}"),
    }
}

/// Sends a packet from Alice to the receiver and returns where it went.
fn pay(h: &mut Harness, request_id: u32, amount: u64, fulfillment: [u8; 32]) -> Forwarded {
    let packet = prepare(
        &format!("{OWN}.stream.abc"),
        amount,
        NOW + 20_000,
        fulfillment,
    );
    forwarded(h.prepare(NOW, ALICE, request_id, &packet))
}

// ILP-7, NET-24 and NET-25: ILDCP answered XRP/9 to everyone, and an address
// that was not the one routed.
#[test]
fn ildcp_answers_the_routed_address_and_the_ledgers_asset() {
    let mut h = Harness::new(Config::DEFAULT, 0);
    let request = ildcp::request(dassie_ilp::Timestamp::from_unix_ms(NOW)).to_vec();
    let length = answered(h.prepare(NOW, ALICE, 7, &request));
    let Response::Fulfill(fulfill) = Response::decode_exact(&h.buffer[..length]).unwrap() else {
        panic!("ILDCP was rejected");
    };
    assert_eq!(fulfill.fulfillment, ildcp::FULFILLMENT);
    let config = ildcp::Response::decode_exact(fulfill.data).unwrap();
    assert_eq!(config.address.as_str(), format!("{OWN}.alice"));
    assert_eq!((config.asset_code, config.asset_scale), ("USD", 9));
    // And the address is routed to Alice.
    let packet = prepare(config.address.as_str(), 0, NOW + 20_000, [1; 32]);
    assert_eq!(forwarded(h.prepare(NOW, BOB, 1, &packet)).to, ALICE);
}

#[test]
fn ildcp_carries_no_money() {
    let mut h = Harness::new(Config::DEFAULT, UNIT);
    let mut request = ildcp::request(dassie_ilp::Timestamp::from_unix_ms(NOW));
    request.amount = 5;
    let length = answered(h.prepare(NOW, ALICE, 1, &request.to_vec()));
    assert_eq!(h.reject_written(length).0, ErrorCode::F01_INVALID_PACKET);
    let wrong_condition = prepare("peer.config", 0, NOW + 20_000, [9; 32]);
    let length = answered(h.prepare(NOW, ALICE, 2, &wrong_condition));
    assert_eq!(h.reject_written(length).0, ErrorCode::F01_INVALID_PACKET);
}

// ILP-3: malformed packets threw instead of getting F01.
#[test]
fn malformed_packets_get_f01() {
    let mut h = Harness::new(Config::DEFAULT, 0);
    for garbage in [&b""[..], b"\x0c", b"\x0c\x05hello", &[0xff; 40]] {
        let length = answered(h.prepare(NOW, ALICE, 1, garbage));
        assert_eq!(h.reject_written(length).0, ErrorCode::F01_INVALID_PACKET);
    }
}

// ILP-3 and NET-33: our own address resolved to an empty route and threw;
// prefixes matched across segment boundaries.
#[test]
fn unroutable_destinations_get_f02() {
    let mut h = Harness::new(Config::DEFAULT, 0);
    for destination in [
        OWN.to_string(),
        format!("{OWN}.carol"),
        format!("{OWN}.alicex"),
        "test.elsewhere".to_string(),
    ] {
        let packet = prepare(&destination, 0, NOW + 20_000, [1; 32]);
        let length = answered(h.prepare(NOW, BOB, 1, &packet));
        assert_eq!(
            h.reject_written(length).0,
            ErrorCode::F02_UNREACHABLE,
            "{destination}"
        );
    }
    let packet = prepare(
        &format!("{OWN}.alice.sub.address"),
        0,
        NOW + 20_000,
        [1; 32],
    );
    assert_eq!(forwarded(h.prepare(NOW, BOB, 1, &packet)).to, ALICE);
}

#[test]
fn expiry_is_shortened_and_capped() {
    let mut h = Harness::new(Config::DEFAULT, UNIT);
    let short = prepare(&format!("{OWN}.stream.x"), 1, NOW + 1_999, [1; 32]);
    let length = answered(h.prepare(NOW, ALICE, 1, &short));
    assert_eq!(
        h.reject_written(length).0,
        ErrorCode::R02_INSUFFICIENT_TIMEOUT
    );
    let sent = pay(&mut h, 2, 1, [1; 32]);
    let outgoing = decode_prepare(&h.buffer[..sent.length]).expires_at.unix_ms;
    assert_eq!(outgoing, NOW + 19_000, "the incoming expiry less 1 s");
    let long = prepare(&format!("{OWN}.stream.x"), 1, NOW + 600_000, [1; 32]);
    let sent = forwarded(h.prepare(NOW, ALICE, 3, &long));
    let outgoing = decode_prepare(&h.buffer[..sent.length]).expires_at.unix_ms;
    assert_eq!(outgoing, NOW + 30_000, "at most 30 s from now");
}

// ILP-4: F08 rejects carried no amounts.
#[test]
fn f08_says_how_much_is_allowed() {
    let mut h = Harness::new(Config::DEFAULT, 100 * UNIT);
    let packet = prepare(
        &format!("{OWN}.stream.x"),
        100_000_001,
        NOW + 20_000,
        [1; 32],
    );
    let length = answered(h.prepare(NOW, ALICE, 1, &packet));
    let (code, data) = h.reject_written(length);
    assert_eq!(code, ErrorCode::F08_AMOUNT_TOO_LARGE);
    assert_eq!(
        AmountTooLarge::decode_exact(&data).unwrap(),
        AmountTooLarge {
            received_amount: 100_000_001,
            maximum_amount: 100_000_000
        }
    );
}

#[test]
fn a_fulfilled_packet_posts_its_transfer_with_its_trace() {
    let mut h = Harness::new(Config::DEFAULT, 0);
    let sent = pay(&mut h, 41, 2_500, [3; 32]);
    assert_eq!(sent.to, RECEIVER);
    let alice = h
        .ledger
        .find(&AccountPath::new(
            LedgerId::StubUsd,
            AccountName::EquityOwnerSub(dassie_ledger::OwnerSub::Btp(1)),
        ))
        .unwrap();
    let upstream = h
        .respond(NOW + 10, RECEIVER, sent.request_id, &fulfill([3; 32]))
        .unwrap();
    assert_eq!((upstream.to, upstream.request_id), (ALICE, 41));
    assert_eq!(h.written(upstream.length), fulfill([3; 32]));
    // Within the owner's group: no funds needed, and a journal row.
    assert_eq!(h.net(alice), -2_500);
    assert_eq!(h.net(h.owner), 2_500);
    let record = *h.connector.packets_after(None).last().unwrap();
    assert_eq!(record.state, PacketState::Fulfilled);
    let posted: Vec<_> = h
        .journal
        .iter()
        .filter_map(|entry| match entry {
            JournalEntry::Transfer {
                transfer,
                state: TransferState::Posted,
                ..
            } if transfer.reference == Reference::Packet(record.trace) => Some(transfer.amount),
            _ => None,
        })
        .collect();
    assert_eq!(posted, [2_500]);
}

// ILP-2: a wrong fulfillment was dropped, and the sender waited for R00.
#[test]
fn a_wrong_fulfillment_is_rejected_at_once() {
    let mut h = Harness::new(Config::DEFAULT, 0);
    let sent = pay(&mut h, 1, 10, [3; 32]);
    let upstream = h
        .respond(NOW + 10, RECEIVER, sent.request_id, &fulfill([4; 32]))
        .unwrap();
    assert_eq!(
        h.reject_written(upstream.length).0,
        ErrorCode::F05_WRONG_CONDITION
    );
    assert_eq!(h.net(h.owner), 0);
    assert_eq!(h.ledger.pending_transfers().count(), 0);
    // The packet is gone: the right fulfillment now is too late.
    assert!(
        h.respond(NOW + 20, RECEIVER, sent.request_id, &fulfill([3; 32]))
            .is_none()
    );
}

// ILP-1: the original gave up after 5 s although the packet allowed 30.
#[test]
fn a_fulfillment_inside_the_expiry_is_honoured() {
    let mut h = Harness::new(Config::DEFAULT, 0);
    let sent = pay(&mut h, 1, 10, [3; 32]);
    assert_eq!(h.connector.poll_timeout(), Some(NOW + 19_000));
    assert!(h.timeout(NOW + 18_999).is_none());
    let upstream = h
        .respond(NOW + 18_999, RECEIVER, sent.request_id, &fulfill([3; 32]))
        .unwrap();
    assert_eq!(h.written(upstream.length), fulfill([3; 32]));
    assert_eq!(h.net(h.owner), 10);
}

#[test]
fn expired_packets_are_rejected_and_voided() {
    let mut h = Harness::new(Config::DEFAULT, 0);
    let first = pay(&mut h, 1, 10, [3; 32]);
    pay(&mut h, 2, 10, [3; 32]);
    // A response at the deadline, before the timer ran, is the timeout.
    let upstream = h
        .respond(NOW + 19_000, RECEIVER, first.request_id, &fulfill([3; 32]))
        .unwrap();
    assert_eq!(
        h.reject_written(upstream.length).0,
        ErrorCode::R00_TRANSFER_TIMED_OUT
    );
    let upstream = h.timeout(NOW + 19_000).unwrap();
    assert_eq!((upstream.to, upstream.request_id), (ALICE, 2));
    assert_eq!(
        h.reject_written(upstream.length).0,
        ErrorCode::R00_TRANSFER_TIMED_OUT
    );
    assert!(h.timeout(NOW + 19_000).is_none());
    assert_eq!(h.connector.poll_timeout(), None);
    assert_eq!(h.net(h.owner), 0);
}

#[test]
fn rejects_pass_through_unchanged() {
    let mut h = Harness::new(Config::DEFAULT, 0);
    let sent = pay(&mut h, 1, 10, [3; 32]);
    let rejection = reject(ErrorCode::F99_APPLICATION_ERROR);
    let upstream = h
        .respond(NOW + 5, RECEIVER, sent.request_id, &rejection)
        .unwrap();
    assert_eq!(h.written(upstream.length), rejection);
    assert_eq!(h.net(h.owner), 0);
}

// ILP-8: random 32-bit request IDs could collide and overwrite each other.
#[test]
fn request_ids_are_unique_per_endpoint() {
    let mut h = Harness::new(Config::DEFAULT, 0);
    let mut ids = std::collections::BTreeSet::new();
    for request_id in 0..300 {
        let sent = pay(&mut h, request_id, 1, [3; 32]);
        assert!(ids.insert(sent.request_id), "duplicate {}", sent.request_id);
    }
    // Reusing an incoming ID that is still in flight changes nothing.
    let packet = prepare(&format!("{OWN}.stream.x"), 1, NOW + 20_000, [3; 32]);
    assert_eq!(h.prepare(NOW, ALICE, 5, &packet), Outcome::Duplicate);
    assert_eq!(h.connector.stats().duplicates, 1);
}

#[test]
fn the_limits_of_the_ledger_and_the_tables_refuse_packets() {
    let config = Config {
        max_pending: 3,
        max_pending_per_endpoint: 2,
        ..Config::DEFAULT
    };
    let mut h = Harness::new(config, 0);
    pay(&mut h, 1, 1, [3; 32]);
    pay(&mut h, 2, 1, [3; 32]);
    let packet = prepare(&format!("{OWN}.stream.x"), 1, NOW + 20_000, [3; 32]);
    let length = answered(h.prepare(NOW, ALICE, 3, &packet));
    assert_eq!(h.reject_written(length).0, ErrorCode::T05_RATE_LIMITED);
    forwarded(h.prepare(NOW, BOB, 1, &packet));
    let length = answered(h.prepare(NOW, BOB, 2, &packet));
    assert_eq!(h.reject_written(length).0, ErrorCode::T03_CONNECTOR_BUSY);
    // An account with a limit and nothing in it: a peer's, for example.
    let (peer, _) = h
        .ledger
        .open_peer(0, LedgerId::StubUsd, Name::new("dpeer").unwrap(), 0)
        .unwrap();
    let mut h2 = Harness::new(Config::DEFAULT, 0);
    let (peer2, _) = h2
        .ledger
        .open_peer(0, LedgerId::StubUsd, Name::new("dpeer").unwrap(), 0)
        .unwrap();
    assert_eq!(peer, peer2);
    h2.connector
        .add_endpoint(
            EndpointId::Btp(9),
            &format!("{OWN}.dpeer"),
            peer2,
            LedgerId::StubUsd,
        )
        .unwrap();
    let length = answered(h2.prepare(NOW, EndpointId::Btp(9), 1, &packet));
    assert_eq!(
        h2.reject_written(length).0,
        ErrorCode::T04_INSUFFICIENT_LIQUIDITY
    );
    assert_eq!(h2.ledger.pending_transfers().count(), 0);
}

#[test]
fn removing_an_endpoint_fails_its_packets_and_forgets_its_answers() {
    let mut h = Harness::new(Config::DEFAULT, 0);
    let to_bob = prepare(&format!("{OWN}.bob"), 7, NOW + 20_000, [3; 32]);
    forwarded(h.prepare(NOW, ALICE, 1, &to_bob));
    let from_bob = prepare(&format!("{OWN}.stream.x"), 5, NOW + 20_000, [3; 32]);
    let sent = forwarded(h.prepare(NOW, BOB, 9, &from_bob));
    assert!(h.connector.remove_endpoint(NOW + 1, BOB));
    h.check();
    let upstream = h.timeout(NOW + 1).unwrap();
    assert_eq!((upstream.to, upstream.request_id), (ALICE, 1));
    assert_eq!(
        h.reject_written(upstream.length).0,
        ErrorCode::T01_PEER_UNREACHABLE
    );
    // Bob's own packet still settles; the answer has nowhere to go.
    assert!(
        h.respond(NOW + 2, RECEIVER, sent.request_id, &fulfill([3; 32]))
            .is_none()
    );
    assert_eq!(h.net(h.owner), 5);
    assert_eq!(h.prepare(NOW, BOB, 10, &from_bob), Outcome::UnknownSource);
}

// ILP-6: missing rates threw inside the pipeline; there was no staleness.
#[test]
fn cross_ledger_packets_convert_through_the_fx_accounts() {
    let mut h = Harness::new(Config::DEFAULT, UNIT);
    let xrp_owner = h.fund(LedgerId::XrplTestnetXrp, 0);
    h.open_books(LedgerId::StubUsd);
    h.open_books(LedgerId::XrplTestnetXrp);
    let peer = EndpointId::Peer(1);
    h.add_peer(1, "dpeer", UNIT, 0);
    h.connector
        .add_endpoint(
            EndpointId::Local(1),
            &format!("{OWN}.xrp"),
            xrp_owner,
            LedgerId::XrplTestnetXrp,
        )
        .unwrap();
    let packet = prepare(&format!("{OWN}.xrp.x"), 1_000, NOW + 20_000, [3; 32]);
    let length = answered(h.prepare(NOW, peer, 1, &packet));
    assert_eq!(
        h.reject_written(length).0,
        ErrorCode::T00_INTERNAL_ERROR,
        "no rates"
    );
    h.rates.set("USD", 10u128.pow(RATE_PRECISION));
    h.rates.set("XRP", 2 * 10u128.pow(RATE_PRECISION));
    let sent = forwarded(h.prepare(NOW, peer, 2, &packet));
    assert_eq!(decode_prepare(&h.buffer[..sent.length]).amount, 2_000);
    h.respond(NOW + 1, sent.to, sent.request_id, &fulfill([3; 32]))
        .unwrap();
    // The receiver's account shows what arrived; the owner's funds hold
    // it where it is, in USD: on XRP, `expenses/fx` takes it back out.
    assert_eq!(h.net(xrp_owner), 2_000);
    assert_eq!(h.net_at(LedgerId::StubUsd, AccountName::RevenueFx), 1_000);
    assert_eq!(
        h.net_at(LedgerId::XrplTestnetXrp, AccountName::ExpensesFx),
        -2_000
    );
    // The owner's own endpoints never convert
    // (spec/areas/ledger.md#the-owners-accounts).
    let length = answered(h.prepare(NOW, ALICE, 3, &packet));
    assert_eq!(h.reject_written(length).0, ErrorCode::F02_UNREACHABLE);
    let stale = NOW + Config::DEFAULT.max_rate_age_ms + 1;
    let packet = prepare(&format!("{OWN}.xrp.x"), 1_000, stale + 20_000, [3; 32]);
    let length = answered(h.prepare(stale, peer, 4, &packet));
    assert_eq!(
        h.reject_written(length).0,
        ErrorCode::T00_INTERNAL_ERROR,
        "stale rates"
    );
}

#[test]
fn zero_amounts_skip_the_ledger() {
    let mut h = Harness::new(Config::DEFAULT, 0);
    let sent = pay(&mut h, 1, 0, [3; 32]);
    h.respond(NOW + 1, RECEIVER, sent.request_id, &fulfill([3; 32]))
        .unwrap();
    let record = *h.connector.packets_after(None).last().unwrap();
    assert_eq!(record.transfers, [None; 3]);
    assert_eq!(record.state, PacketState::Fulfilled);
}
