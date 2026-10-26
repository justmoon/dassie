//! A packet's next hop is drawn among a route's usable options
//! (spec/decisions/weighted-next-hops.md, spec/areas/connector.md#routing):
//! among those that keep the packet's ledger, weighted by the fees each
//! earned us on packets that kept theirs, with a share of the draws even;
//! only when none keeps it, any usable option, each as likely. The owner's
//! own packets never convert, so they draw only among options on their
//! ledger, and a quote's first hop is not drawn.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use std::collections::BTreeMap;

use dassie_connector::sim::{Harness, NOW, fulfill, prepare, reject};
use dassie_connector::{Charges, Config, EndpointId, Outcome, RATE_PRECISION};
use dassie_ilp::ErrorCode;
use dassie_ledger::LedgerId;

const ALICE: EndpointId = EndpointId::Btp(1);
/// Peers on USD.
const ONE: EndpointId = EndpointId::Peer(1);
const TWO: EndpointId = EndpointId::Peer(2);
const SOURCE: EndpointId = EndpointId::Peer(5);
/// Peers on XRP.
const THREE: EndpointId = EndpointId::Peer(3);
const FOUR: EndpointId = EndpointId::Peer(4);
const TRUST: u64 = 1_000_000_000_000;
const USD: LedgerId = LedgerId::StubUsd;
const XRP: LedgerId = LedgerId::XrplTestnetXrp;
const FAR: &str = "test.das.dfar";
/// A 1% fee on transit, and no spread.
const FEE: Charges = Charges {
    fee_ppm: 10_000,
    spread_ppm: 0,
};

/// Peers one, two and the source on USD, three and four on XRP, each also
/// routed to at its own address; 1 USD buys 2 XRP, and the owner holds
/// both.
fn harness(config: Config) -> Harness {
    let mut h = Harness::new(config, TRUST);
    h.open_books(USD);
    h.fund(XRP, TRUST);
    h.open_books(XRP);
    h.add_peer(1, "done", TRUST, 0);
    h.add_peer(2, "dtwo", TRUST, 0);
    h.add_peer(5, "dsource", TRUST, 0);
    h.add_peer_on(XRP, 3, "dthree", TRUST, 0);
    h.add_peer_on(XRP, 4, "dfour", TRUST, 0);
    h.rates.set("USD", 10u128.pow(RATE_PRECISION));
    h.rates.set("XRP", 2 * 10u128.pow(RATE_PRECISION));
    h.pricing.charges = FEE;
    h
}

/// Routes `test.das.dfar` through `hops`, and each peer at its address
/// through itself.
fn route(h: &mut Harness, hops: &[EndpointId]) {
    let own = [
        ("test.das.done", &[ONE][..]),
        ("test.das.dtwo", &[TWO][..]),
        ("test.das.dthree", &[THREE][..]),
        ("test.das.dfour", &[FOUR][..]),
    ];
    h.connector
        .set_routes(own.into_iter().chain([(FAR, hops)]))
        .unwrap();
}

/// `count` draws, a multiple of ten, on a grid: ten high halves (which
/// explore or not), each with `count / 10` low halves (which pick), each in
/// the middle of an even share of its range.
fn draws(count: u64) -> impl Iterator<Item = u64> {
    let (rows, columns) = (10, count / 10);
    let (high, low) = ((1u64 << 32) / rows, (1u64 << 32) / columns);
    (0..count).map(move |i| {
        let (row, column) = (i / columns, i % columns);
        ((row * high + high / 2) << 32) | (column * low + low / 2)
    })
}

/// Sends a packet of `amount` from `from` to `destination`, drawn with
/// `draw`, and returns where it went, or the code it was refused with.
fn send(
    h: &mut Harness,
    (from, request_id): (EndpointId, u32),
    destination: &str,
    amount: u64,
    draw: u64,
) -> Result<(EndpointId, u32), ErrorCode> {
    h.draw = draw;
    let packet = prepare(destination, amount, NOW + 20_000, [1; 32]);
    match h.prepare(NOW, from, request_id, &packet) {
        Outcome::Forward(sent) => Ok((sent.to, sent.request_id)),
        Outcome::Respond(length) => Err(h.reject_written(length).0),
        other => panic!("{other:?}"),
    }
}

/// Where packets of `amount` from `from` go over `count` draws, and how
/// often; each is rejected downstream, so nothing is earned.
fn spread(h: &mut Harness, from: EndpointId, amount: u64, count: u64) -> BTreeMap<EndpointId, u64> {
    let mut hits = BTreeMap::new();
    for (request_id, draw) in (0u32..).zip(draws(count)) {
        let (to, sent_id) = send(h, (from, request_id), FAR, amount, draw).unwrap();
        *hits.entry(to).or_default() += 1;
        let back = reject(ErrorCode::T04_INSUFFICIENT_LIQUIDITY);
        h.respond(NOW, to, sent_id, &back).unwrap();
    }
    hits
}

/// Earns `fee` on `hop`, a peer on USD, with a packet from the source
/// routed to it at its own address and fulfilled.
fn earn(h: &mut Harness, hop: EndpointId, address: &str, fee: u64, request_id: u32) {
    let amount = fee * 100;
    let (to, sent_id) = send(h, (SOURCE, request_id), address, amount, 0).unwrap();
    assert_eq!(to, hop);
    h.respond(NOW, to, sent_id, &fulfill([1; 32])).unwrap();
}

// A peer's packet goes to an option on the ledger it came in on, whatever
// the draw, while one does; only when none does, to any usable option, each
// as likely.
#[test]
fn a_packet_keeps_its_ledger_where_an_option_does() {
    let mut h = harness(Config::DEFAULT);
    route(&mut h, &[THREE, TWO, FOUR]);
    let hits = spread(&mut h, SOURCE, 0, 100);
    assert_eq!(hits, BTreeMap::from([(TWO, 100)]));
    // With no option on USD, it converts, and the draw is even.
    route(&mut h, &[THREE, FOUR]);
    let hits = spread(&mut h, SOURCE, 1_000, 100);
    assert_eq!(hits, BTreeMap::from([(THREE, 50), (FOUR, 50)]));
    // A draining option, or the packet's source, is never drawn.
    route(&mut h, &[ONE, TWO, SOURCE]);
    h.connector.set_draining(ONE, true);
    let hits = spread(&mut h, SOURCE, 1_000, 100);
    assert_eq!(hits, BTreeMap::from([(TWO, 100)]));
}

// While no option has earned anything, every draw is even, exploring or
// not.
#[test]
fn options_that_earned_nothing_are_even() {
    let mut h = harness(Config {
        explore_ppm: 0,
        ..Config::DEFAULT
    });
    route(&mut h, &[ONE, TWO]);
    let hits = spread(&mut h, SOURCE, 1_000, 100);
    assert_eq!(hits, BTreeMap::from([(ONE, 50), (TWO, 50)]));
}

// Each option is drawn in proportion to the fees it earned us; what a
// rejected packet or a converting one would have paid counts for nothing.
#[test]
fn options_are_weighted_by_what_they_earned() {
    let mut h = harness(Config {
        explore_ppm: 0,
        ..Config::DEFAULT
    });
    route(&mut h, &[ONE, TWO]);
    earn(&mut h, ONE, "test.das.done", 1_000, 1);
    for request_id in 2..5 {
        earn(&mut h, TWO, "test.das.dtwo", 1_000, request_id);
    }
    assert_eq!(h.connector.earnings(ONE, NOW), Some(1_000));
    assert_eq!(h.connector.earnings(TWO, NOW), Some(3_000));
    let hits = spread(&mut h, SOURCE, 1_000, 1_000);
    assert_eq!(hits, BTreeMap::from([(ONE, 250), (TWO, 750)]));
    // Rejects, the spread's, earned nothing.
    assert_eq!(h.connector.earnings(TWO, NOW), Some(3_000));
    // A converting packet's fee counts toward no hop.
    let (to, sent_id) = send(&mut h, (SOURCE, 10), "test.das.dthree", 100_000, 0).unwrap();
    h.respond(NOW, to, sent_id, &fulfill([1; 32])).unwrap();
    assert_eq!(h.connector.earnings(THREE, NOW), Some(0));
    // Nor does the owner's packet, which pays no fee.
    let (to, sent_id) = send(&mut h, (ALICE, 11), "test.das.done", 100_000, 0).unwrap();
    h.respond(NOW, to, sent_id, &fulfill([1; 32])).unwrap();
    assert_eq!(h.connector.earnings(ONE, NOW), Some(1_000));
}

// Earnings halve once per half-life, so a hop that stopped earning falls
// back.
#[test]
fn earnings_decay() {
    let half = Config::DEFAULT.earnings_half_life_ms;
    let mut h = harness(Config::DEFAULT);
    route(&mut h, &[ONE, TWO]);
    earn(&mut h, TWO, "test.das.dtwo", 1_000, 1);
    assert_eq!(h.connector.earnings(TWO, NOW + half - 1), Some(1_000));
    assert_eq!(h.connector.earnings(TWO, NOW + half), Some(500));
    assert_eq!(h.connector.earnings(TWO, NOW + 3 * half), Some(125));
    assert_eq!(h.connector.earnings(TWO, NOW + 64 * half), Some(0));
}

// A share of the draws ignores the weights, so a hop that earned nothing
// keeps a chance to earn.
#[test]
fn a_share_of_the_draws_explores() {
    let mut h = harness(Config {
        explore_ppm: 200_000,
        ..Config::DEFAULT
    });
    route(&mut h, &[ONE, TWO]);
    earn(&mut h, TWO, "test.das.dtwo", 1_000, 1);
    // A fifth of the draws are even: half of those reach the hop that
    // earned nothing.
    let hits = spread(&mut h, SOURCE, 1_000, 1_000);
    assert_eq!(hits, BTreeMap::from([(ONE, 100), (TWO, 900)]));
}

// A Reject, T04 included, leaves a hop's weight as it was: room is never a
// weight, so the hop short of room still draws its packets, and answers
// them T04 until it is settled.
#[test]
fn a_hop_short_of_room_keeps_its_weight() {
    let mut h = harness(Config {
        explore_ppm: 0,
        ..Config::DEFAULT
    });
    route(&mut h, &[ONE, TWO]);
    earn(&mut h, TWO, "test.das.dtwo", 1_000, 1);
    let hits = spread(&mut h, SOURCE, 1_000, 100);
    assert_eq!(hits, BTreeMap::from([(TWO, 100)]));
    assert_eq!(h.connector.earnings(TWO, NOW), Some(1_000));
}

// The same draws send packets the same way.
#[test]
fn a_draw_decides_alone() {
    let run = || {
        let mut h = harness(Config::DEFAULT);
        route(&mut h, &[ONE, TWO]);
        earn(&mut h, TWO, "test.das.dtwo", 1_000, 1);
        (0u32..)
            .zip(draws(50))
            .map(|(request_id, draw)| {
                let (to, sent_id) = send(&mut h, (SOURCE, request_id), FAR, 10, draw).unwrap();
                h.respond(NOW, to, sent_id, &fulfill([1; 32])).unwrap();
                to
            })
            .collect::<Vec<_>>()
    };
    let first = run();
    assert_eq!(first, run());
    assert!(first.contains(&ONE) && first.contains(&TWO));
}

// The owner's own packets never convert: they draw only among options on
// their own ledger, and with none, a packet with money is refused. A quote's
// first hop is the route's first usable option, not drawn.
#[test]
fn the_owners_packets_keep_their_ledger() {
    let mut h = harness(Config::DEFAULT);
    route(&mut h, &[THREE, ONE, FOUR, TWO]);
    assert_eq!(h.connector.first_hop("test.das.dfar.x"), Some((THREE, XRP)));
    let hits = spread(&mut h, ALICE, 1_000, 100);
    assert_eq!(hits, BTreeMap::from([(ONE, 50), (TWO, 50)]));
    route(&mut h, &[THREE, FOUR]);
    for (request_id, draw) in (100u32..).zip(draws(10)) {
        assert_eq!(
            send(&mut h, (ALICE, request_id), FAR, 1_000, draw),
            Err(ErrorCode::F02_UNREACHABLE)
        );
    }
}
