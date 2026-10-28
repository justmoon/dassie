//! Swaps with peers (spec/decisions/peer-swaps.md,
//! spec/areas/connector.md#swaps): a peer's Prepare to
//! `peer.swap.<scheme>` converts as transit and goes back to it on the
//! method it names, before routing, with its destination, condition and
//! data unchanged; the node's own swaps go out from its swap endpoint on a
//! ledger, unconverted, and come back to it on the method they name.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use dassie_connector::sim::{Harness, NOW, decode_prepare, fulfill, prepare, reject};
use dassie_connector::{
    Charges, Config, EndpointId, Forwarded, Outcome, RATE_PRECISION, Responded,
};
use dassie_ilp::{AmountTooLarge, ErrorCode, Prepare, Timestamp, condition_for};
use dassie_ledger::{AccountName, AccountPath, LedgerId};
use dassie_oer::{Decode, Encode};

/// The peer that swaps with us, on USD and XRP.
const ONE: EndpointId = EndpointId::Peer(1);
/// A peer on XRP alone, for transit to compare with.
const TWO: EndpointId = EndpointId::Peer(2);
const ALICE: EndpointId = EndpointId::Btp(1);
/// The node's swap endpoints.
const SWAP_USD: EndpointId = EndpointId::Local(5);
const SWAP_XRP: EndpointId = EndpointId::Local(6);
const TRUST: u64 = 1_000_000_000_000;
const USD: LedgerId = LedgerId::StubUsd;
const XRP: LedgerId = LedgerId::XrplTestnetXrp;
const TO_XRP: &str = "peer.swap.xrpl-testnet-xrp";
const TO_USD: &str = "peer.swap.stub";

const FEE: Charges = Charges {
    fee_ppm: 10_000,
    spread_ppm: 5_000,
};

/// A core node: peer one on USD and XRP, peer two on XRP alone, routed to
/// at `test.das.dtwo`; 1 USD buys 2 XRP, and the owner holds both, with
/// `charges` on transit.
fn harness(charges: Charges) -> Harness {
    let mut h = Harness::new(Config::DEFAULT, TRUST);
    h.open_books(USD);
    h.fund(XRP, TRUST);
    h.open_books(XRP);
    h.add_peer(1, "done", TRUST, 0);
    h.add_slot(XRP, 1, "done", TRUST);
    h.add_peer_on(XRP, 2, "dtwo", TRUST, 0);
    h.connector
        .set_routes([("test.das.dtwo", &[TWO][..])])
        .unwrap();
    h.rates.set_at("USD", 10u128.pow(RATE_PRECISION), NOW);
    h.rates.set_at("XRP", 2 * 10u128.pow(RATE_PRECISION), NOW);
    h.pricing.charges = charges;
    h
}

/// A swap's Prepare to `destination` with data of its own.
fn swap(destination: &str, amount: u64, expires_at: u64, fulfillment: [u8; 32]) -> Vec<u8> {
    Prepare {
        amount,
        expires_at: Timestamp::from_unix_ms(expires_at),
        execution_condition: condition_for(&fulfillment),
        destination: dassie_ilp::Address::parse(destination).unwrap(),
        data: b"the swapper's own",
    }
    .to_vec()
}

fn forwarded(outcome: Outcome) -> Forwarded {
    match outcome {
        Outcome::Forward(forwarded) => forwarded,
        other => panic!("expected a forward, got {other:?}"),
    }
}

/// The code a Prepare was refused with.
fn refused(h: &Harness, outcome: Outcome) -> (ErrorCode, Vec<u8>) {
    match outcome {
        Outcome::Respond(length) => h.reject_written(length),
        other => panic!("expected a reject, got {other:?}"),
    }
}

/// What the owner's group holds on `ledger`.
fn owner(h: &Harness, ledger: LedgerId) -> i128 {
    let root = h
        .ledger
        .find(&AccountPath::new(ledger, AccountName::EquityOwner))
        .unwrap();
    h.ledger.group_balance(root).unwrap().net_credit()
}

/// The balance of peer one's account on `ledger`.
fn peer(h: &Harness, id: EndpointId, ledger: LedgerId) -> i128 {
    h.net(h.connector.account_on(id, ledger).unwrap())
}

// A peer's swap converts as transit does, charged the fee and the spread,
// and goes back to it on the method its address names, with destination,
// condition and data unchanged and its expiry a message window earlier.
// Fulfilled, the peer paid on USD and was paid on XRP.
#[test]
fn a_peers_swap_converts_as_transit_and_goes_back_on_the_method_it_names() {
    // What transit from one on USD to two on XRP gets.
    let mut transit = harness(FEE);
    let packet = prepare("test.das.dtwo.x", 100_000, NOW + 20_000, [1; 32]);
    let sent = forwarded(transit.prepare_on(NOW, (ONE, USD), 1, &packet));
    assert_eq!((sent.to, sent.ledger), (TWO, XRP));
    let transit_out = decode_prepare(&transit.buffer[..sent.length]).amount;

    let mut h = harness(FEE);
    let packet = swap(TO_XRP, 100_000, NOW + 20_000, [1; 32]);
    let sent = forwarded(h.prepare_on(NOW, (ONE, USD), 1, &packet));
    assert_eq!((sent.to, sent.ledger), (ONE, XRP));
    let incoming = decode_prepare(&packet);
    let outgoing = decode_prepare(&h.buffer[..sent.length]);
    assert_eq!(outgoing.amount, transit_out, "priced as transit");
    assert!(outgoing.amount > 0 && outgoing.amount < 200_000);
    assert_eq!(outgoing.destination, incoming.destination);
    assert_eq!(outgoing.execution_condition, incoming.execution_condition);
    assert_eq!(outgoing.data, incoming.data);
    let window = Config::DEFAULT.message_window_ms;
    assert_eq!(outgoing.expires_at.unix_ms, NOW + 20_000 - window);
    let out = outgoing.amount;

    let (usd_before, xrp_before) = (peer(&h, ONE, USD), peer(&h, ONE, XRP));
    let upstream = h.respond(NOW + 1, ONE, sent.request_id, &fulfill([1; 32]));
    let upstream = upstream.unwrap();
    assert_eq!(
        (upstream.to, upstream.ledger, upstream.request_id),
        (ONE, USD, 1)
    );
    // One paid 100 000 on USD and got `out` on XRP.
    assert_eq!(peer(&h, ONE, USD) - usd_before, -100_000);
    assert_eq!(peer(&h, ONE, XRP) - xrp_before, i128::from(out));
    // Its fee counts toward no hop's weight: the swap converts.
    assert_eq!(h.connector.earnings(ONE, USD, NOW + 1), Some(0));
    assert_eq!(h.connector.earnings(ONE, XRP, NOW + 1), Some(0));
    assert_eq!(h.net_at(USD, AccountName::RevenueFees), 1_000);
}

// A Reject from the peer on the way back goes to it on the method the
// swap came on, and leaves the books as they were.
#[test]
fn a_rejected_swap_leaves_nothing() {
    let mut h = harness(FEE);
    let before = (owner(&h, USD), owner(&h, XRP), peer(&h, ONE, USD));
    let packet = swap(TO_XRP, 100_000, NOW + 20_000, [1; 32]);
    let sent = forwarded(h.prepare_on(NOW, (ONE, USD), 7, &packet));
    let back = reject(ErrorCode::F05_WRONG_CONDITION);
    let upstream = h.respond(NOW + 1, ONE, sent.request_id, &back).unwrap();
    assert_eq!(
        (upstream.to, upstream.ledger, upstream.request_id),
        (ONE, USD, 7)
    );
    assert_eq!((owner(&h, USD), owner(&h, XRP), peer(&h, ONE, USD)), before);
}

// What a swap refuses for, in the order of a Prepare's checks.
#[test]
fn a_swap_is_refused_as_transit_is() {
    let refuse =
        |h: &mut Harness, (on, destination, amount, expires_at): (LedgerId, &str, u64, u64)| {
            let packet = swap(destination, amount, expires_at, [1; 32]);
            let outcome = h.prepare_on(NOW, (ONE, on), 1, &packet);
            refused(h, outcome).0
        };
    let mut h = harness(FEE);
    let fine = (USD, TO_XRP, 1_000, NOW + 20_000);
    // A scheme that is not a method of the peering, or nothing like one.
    let hub = "peer.swap.hub-xrpl-testnet-xrp";
    for destination in [hub, "peer.swap.nonesuch", "peer.swap.stub.x", "peer.other"] {
        let code = refuse(&mut h, (USD, destination, 1_000, NOW + 20_000));
        assert_eq!(code, ErrorCode::F02_UNREACHABLE, "{destination}");
    }
    // Too little time.
    let code = refuse(&mut h, (USD, TO_XRP, 1_000, NOW + 1_500));
    assert_eq!(code, ErrorCode::R02_INSUFFICIENT_TIMEOUT);
    // Above the largest packet, told the most.
    let largest = Config::DEFAULT.max_packet_amount;
    let packet = swap(TO_XRP, largest + 1, NOW + 20_000, [1; 32]);
    let outcome = h.prepare_on(NOW, (ONE, USD), 1, &packet);
    let (code, data) = refused(&h, outcome);
    assert_eq!(code, ErrorCode::F08_AMOUNT_TOO_LARGE);
    let told = AmountTooLarge::decode_exact(&data).unwrap();
    assert_eq!(told.maximum_amount, largest);
    // The method named not available.
    h.connector.set_available(ONE, XRP, false);
    assert_eq!(refuse(&mut h, fine), ErrorCode::T04_INSUFFICIENT_LIQUIDITY);
    h.connector.set_available(ONE, XRP, true);
    // No fresh rate.
    let mut stale = harness(FEE);
    stale
        .rates
        .set_at("XRP", 2 * 10u128.pow(RATE_PRECISION), NOW - 2 * 3_600_000);
    assert_eq!(refuse(&mut stale, fine), ErrorCode::T00_INTERNAL_ERROR);
    // The owner holds nothing on the way out.
    let mut poor = Harness::new(Config::DEFAULT, TRUST);
    poor.open_books(USD);
    poor.fund(XRP, 0);
    poor.open_books(XRP);
    poor.add_peer(1, "done", TRUST, 0);
    poor.add_slot(XRP, 1, "done", TRUST);
    poor.rates.set_at("USD", 10u128.pow(RATE_PRECISION), NOW);
    poor.rates
        .set_at("XRP", 2 * 10u128.pow(RATE_PRECISION), NOW);
    assert_eq!(
        refuse(&mut poor, fine),
        ErrorCode::T04_INSUFFICIENT_LIQUIDITY
    );
    // Nothing moved for any of them.
    assert_eq!(h.connector.pending_count(), 0);
}

// Swaps fill the money in flight on the link as any packet does: past the
// cap on the method they go back on, T05.
#[test]
fn a_swap_counts_in_the_money_in_flight() {
    let mut h = harness(Charges::NONE);
    let largest = Config::DEFAULT.max_packet_amount;
    // 1 USD buys 2 XRP: half the largest packet in comes back as about
    // the largest out.
    let mut request_id = 0;
    loop {
        request_id += 1;
        let packet = swap(TO_XRP, largest / 2, NOW + 20_000, [1; 32]);
        match h.prepare_on(NOW, (ONE, USD), request_id, &packet) {
            Outcome::Forward(_) => {}
            outcome => {
                assert_eq!(refused(&h, outcome).0, ErrorCode::T05_RATE_LIMITED);
                break;
            }
        }
        assert!(request_id < 10, "the cap holds");
    }
    let xrp = h
        .connector
        .slots(ONE)
        .find(|slot| slot.ledger == XRP)
        .unwrap();
    assert!(xrp.money_to <= Config::DEFAULT.max_in_flight());
}

// A node that does not offer swaps, or a periphery node, refuses them
// (F02); so is a swap from a child, which has no peering.
#[test]
fn only_a_core_node_that_offers_swaps_takes_them() {
    let fine = swap(TO_XRP, 1_000, NOW + 20_000, [1; 32]);
    let mut h = harness(FEE);
    h.connector.set_offer_swaps(false);
    assert!(!h.connector.offers_swaps());
    let outcome = h.prepare_on(NOW, (ONE, USD), 1, &fine);
    assert_eq!(refused(&h, outcome).0, ErrorCode::F02_UNREACHABLE);

    let mut periphery = harness(FEE);
    let uplink = vec![String::from("test.das.done.dnode")];
    periphery
        .connector
        .set_child_addresses(&uplink, true)
        .unwrap();
    assert!(!periphery.connector.offers_swaps());
    let outcome = periphery.prepare_on(NOW, (ONE, USD), 1, &fine);
    assert_eq!(refused(&periphery, outcome).0, ErrorCode::F02_UNREACHABLE);

    let mut child = harness(FEE);
    let outcome = child.prepare(NOW, ALICE, 1, &fine);
    assert_eq!(refused(&child, outcome).0, ErrorCode::F02_UNREACHABLE);
}

// ILDCP is answered as it always was, from a peer as from a child.
#[test]
fn ildcp_stays_at_peer_config() {
    let mut h = harness(FEE);
    let request = dassie_ilp::ildcp::request(Timestamp::from_unix_ms(NOW + 20_000)).to_vec();
    let outcome = h.prepare_on(NOW, (ONE, XRP), 1, &request);
    let Outcome::Respond(length) = outcome else {
        panic!("{outcome:?}");
    };
    let response = dassie_ilp::Response::decode_exact(&h.buffer[..length]).unwrap();
    assert!(matches!(response, dassie_ilp::Response::Fulfill(_)));
}

// The node's own swap goes out from its swap endpoint on the method of
// the ledger it is on, unconverted and uncharged, its address naming
// another method of the peering.
#[test]
fn the_nodes_own_swap_goes_out_unconverted() {
    let mut h = harness(FEE);
    h.add_swap(5, USD);
    let before = owner(&h, USD);
    let packet = swap(TO_XRP, 100_000, NOW + 20_000, [9; 32]);
    let sent = forwarded(h.send_swap(NOW, (SWAP_USD, 1), ONE, &packet));
    assert_eq!((sent.to, sent.ledger), (ONE, USD));
    let outgoing = decode_prepare(&h.buffer[..sent.length]);
    assert_eq!(outgoing.amount, 100_000, "no fee, no conversion");
    assert_eq!(outgoing.destination.as_str(), TO_XRP);
    assert_eq!(outgoing.data, decode_prepare(&packet).data);
    let upstream = h.respond(NOW + 1, ONE, sent.request_id, &fulfill([9; 32]));
    assert_eq!(upstream.unwrap().to, SWAP_USD);
    assert_eq!(owner(&h, USD) - before, -100_000, "the owner's funds paid");
}

// What the node's own swap is refused for.
#[test]
fn the_nodes_own_swap_is_refused_before_it_goes() {
    let mut h = harness(FEE);
    h.add_swap(5, USD);
    let send = |h: &mut Harness, to: EndpointId, destination: &str| {
        let packet = swap(destination, 1_000, NOW + 20_000, [9; 32]);
        let outcome = h.send_swap(NOW, (SWAP_USD, 1), to, &packet);
        refused(h, outcome).0
    };
    // Its own method's scheme, one the peering lacks, or no swap at all.
    for destination in [TO_USD, "peer.swap.hub-xrpl-testnet-xrp", "test.das.dtwo.x"] {
        assert_eq!(send(&mut h, ONE, destination), ErrorCode::F02_UNREACHABLE);
    }
    // Two has no method on USD: none to pay it on.
    assert_eq!(send(&mut h, TWO, TO_XRP), ErrorCode::F02_UNREACHABLE);
    assert_eq!(send(&mut h, ALICE, TO_XRP), ErrorCode::F02_UNREACHABLE);
    h.connector.set_available(ONE, USD, false);
    assert_eq!(
        send(&mut h, ONE, TO_XRP),
        ErrorCode::T04_INSUFFICIENT_LIQUIDITY
    );
    h.connector.set_available(ONE, USD, true);
    // More than the owner has.
    let packet = swap(TO_XRP, 1_000, NOW + 20_000, [9; 32]);
    let rich = Config::DEFAULT.max_packet_amount;
    let mut poor = Harness::new(Config::DEFAULT, rich / 2);
    poor.open_books(USD);
    poor.fund(XRP, TRUST);
    poor.open_books(XRP);
    poor.add_peer(1, "done", TRUST, 0);
    poor.add_slot(XRP, 1, "done", TRUST);
    poor.add_swap(5, USD);
    let large = swap(TO_XRP, rich, NOW + 20_000, [9; 32]);
    let outcome = poor.send_swap(NOW, (SWAP_USD, 1), ONE, &large);
    assert_eq!(
        refused(&poor, outcome).0,
        ErrorCode::T04_INSUFFICIENT_LIQUIDITY
    );
    // Only a swap endpoint sends swaps.
    assert_eq!(
        h.send_swap(NOW, (ALICE, 1), ONE, &packet),
        Outcome::UnknownSource
    );
    assert_eq!(h.connector.pending_count(), 0);
}

// A swap that comes back on the method its address names is the node's
// own: it reaches the swap endpoint on that ledger, unconverted and
// uncharged, and nothing reaches the swap endpoint by its address.
#[test]
fn the_nodes_own_swap_comes_back_to_its_swap_endpoint() {
    let mut h = harness(FEE);
    let packet = swap(TO_XRP, 100_000, NOW + 20_000, [9; 32]);
    // Without a swap endpoint on XRP, it has nowhere to go.
    let outcome = h.prepare_on(NOW, (ONE, XRP), 1, &packet);
    assert_eq!(refused(&h, outcome).0, ErrorCode::F02_UNREACHABLE);
    h.add_swap(6, XRP);
    let before = owner(&h, XRP);
    let sent = forwarded(h.prepare_on(NOW, (ONE, XRP), 2, &packet));
    assert_eq!((sent.to, sent.ledger), (SWAP_XRP, XRP));
    assert_eq!(decode_prepare(&h.buffer[..sent.length]).amount, 100_000);
    let responded = h.respond_full(NOW + 1, SWAP_XRP, sent.request_id, &fulfill([9; 32]));
    assert!(matches!(
        responded,
        Responded::Resolved {
            posted: Some(_),
            ..
        }
    ));
    assert_eq!(owner(&h, XRP) - before, 100_000, "the owner's funds got it");
    // By its address, no packet reaches it.
    let by_address = prepare(
        "test.das.dnode.swap-xrpl-testnet-xrp",
        1,
        NOW + 20_000,
        [1; 32],
    );
    let outcome = h.prepare_on(NOW, (ONE, XRP), 3, &by_address);
    assert_eq!(refused(&h, outcome).0, ErrorCode::F02_UNREACHABLE);
    // On a periphery node, or one that offers no swaps, its own come back
    // all the same.
    h.connector.set_offer_swaps(false);
    let uplink = vec![String::from("test.das.done.dnode")];
    h.connector.set_child_addresses(&uplink, true).unwrap();
    forwarded(h.prepare_on(NOW, (ONE, XRP), 4, &packet));
    // Removed, it takes nothing.
    assert_eq!(h.connector.swap_endpoint(XRP), Some(SWAP_XRP));
    h.connector.remove_endpoint(NOW, SWAP_XRP);
    h.check();
    assert_eq!(h.connector.swap_endpoint(XRP), None);
    let outcome = h.prepare_on(NOW, (ONE, XRP), 5, &packet);
    assert_eq!(refused(&h, outcome).0, ErrorCode::F02_UNREACHABLE);
}

// A whole swap between two nodes: A pays B on USD from its swap endpoint,
// B converts and pays A back on XRP, and A's swap endpoint fulfills. A's
// owner gave dollars for XRP, B's took them, and both sets of books
// balance at every step.
#[test]
fn a_swap_between_two_nodes() {
    let mut a = harness(Charges::NONE);
    a.add_swap(5, USD);
    a.add_swap(6, XRP);
    let mut b = harness(FEE);
    let (a_usd, a_xrp) = (owner(&a, USD), owner(&a, XRP));
    let (b_usd, b_xrp) = (owner(&b, USD), owner(&b, XRP));
    let packet = swap(TO_XRP, 1_000_000, NOW + 20_000, [3; 32]);
    // Out of A: in both harnesses, the other is peer one.
    let out_a = forwarded(a.send_swap(NOW, (SWAP_USD, 40), ONE, &packet));
    let leg = a.written(out_a.length);
    // Through B, back toward A on XRP.
    let through = forwarded(b.prepare_on(NOW, (ONE, out_a.ledger), out_a.request_id, &leg));
    assert_eq!((through.to, through.ledger), (ONE, XRP));
    let back = b.written(through.length);
    let got = decode_prepare(&back).amount;
    // Into A's swap endpoint on XRP.
    let home = forwarded(a.prepare_on(NOW, (ONE, through.ledger), through.request_id, &back));
    assert_eq!(home.to, SWAP_XRP);
    // A fulfills its own swap, and the Fulfill runs back along both legs.
    let up = a
        .respond(NOW + 1, SWAP_XRP, home.request_id, &fulfill([3; 32]))
        .unwrap();
    assert_eq!((up.to, up.ledger), (ONE, XRP));
    let fulfilled = a.written(up.length);
    let up = b
        .respond(NOW + 2, ONE, through.request_id, &fulfilled)
        .unwrap();
    assert_eq!(
        (up.to, up.ledger, up.request_id),
        (ONE, USD, out_a.request_id)
    );
    let fulfilled = b.written(up.length);
    let up = a
        .respond(NOW + 3, ONE, out_a.request_id, &fulfilled)
        .unwrap();
    assert_eq!((up.to, up.request_id), (SWAP_USD, 40));
    assert_eq!(owner(&a, USD) - a_usd, -1_000_000);
    assert_eq!(owner(&a, XRP) - a_xrp, i128::from(got));
    assert_eq!(owner(&b, USD) - b_usd, 1_000_000);
    assert_eq!(owner(&b, XRP) - b_xrp, -i128::from(got));
    assert_eq!(a.connector.pending_count() + b.connector.pending_count(), 0);
}
