//! A peer's endpoint has a slot per settlement method of its peering
//! (spec/decisions/settlement-methods.md, spec/areas/connector.md#routing):
//! a packet keeps the ledger it came in on where a first hop has a method
//! on it, and otherwise takes the drawn hop's method with the best rate;
//! which method it takes depends on the methods available, never on their
//! room; each slot books on its own account, has its own money in flight,
//! its own hub flag and its own weight in the draw.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use std::collections::BTreeMap;

use dassie_connector::sim::{Harness, NOW, decode_prepare, fulfill, prepare, reject};
use dassie_connector::{Config, EndpointId, MAX_SLOTS, Outcome, RATE_PRECISION, SlotError};
use dassie_ilp::{AmountTooLarge, ErrorCode};
use dassie_ledger::{AccountId, LedgerId};
use dassie_oer::Decode;

const ALICE: EndpointId = EndpointId::Btp(1);
const ONE: EndpointId = EndpointId::Peer(1);
const TWO: EndpointId = EndpointId::Peer(2);
const SOURCE: EndpointId = EndpointId::Peer(5);
const TRUST: u64 = 1_000_000_000_000;
const USD: LedgerId = LedgerId::StubUsd;
const XRP: LedgerId = LedgerId::XrplTestnetXrp;
/// XRP in a hub: a ledger of its own, in the same currency as [`XRP`].
const HUB: LedgerId = LedgerId::HubXrplTestnetXrp;
const FAR: &str = "test.das.dfar";

/// Peers one, two and the source on USD and XRP, `test.das.dfar` routed
/// through one and two; 1 USD buys 2 XRP, and the owner holds both.
fn harness(config: Config) -> Harness {
    let mut h = Harness::new(config, TRUST);
    h.open_books(USD);
    h.fund(XRP, TRUST);
    h.open_books(XRP);
    for (id, node) in [(1, "done"), (2, "dtwo"), (5, "dsource")] {
        h.add_peer(id, node, TRUST, 0);
        h.add_slot(XRP, id, node, TRUST);
    }
    h.rates.set_at("USD", 10u128.pow(RATE_PRECISION), NOW);
    h.rates.set_at("XRP", 2 * 10u128.pow(RATE_PRECISION), NOW);
    h.connector.set_routes([(FAR, &[ONE, TWO][..])]).unwrap();
    h
}

/// Sends a packet of `amount` from `from` on its method on `on`, drawn
/// with `draw`, and returns where it went and on which method, with the
/// request ID it went with, or the code it was refused with.
fn send(
    h: &mut Harness,
    (from, on): (EndpointId, LedgerId),
    request_id: u32,
    amount: u64,
    draw: u64,
) -> Result<(EndpointId, LedgerId, u32), ErrorCode> {
    h.draw = draw;
    let packet = prepare(FAR, amount, NOW + 20_000, [1; 32]);
    match h.prepare_on(NOW, (from, on), request_id, &packet) {
        Outcome::Forward(sent) => Ok((sent.to, sent.ledger, sent.request_id)),
        Outcome::Respond(length) => Err(h.reject_written(length).0),
        other => panic!("{other:?}"),
    }
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

/// Where `count` packets from the source on `on` go, drawn on the grid of
/// [`draws`] and rejected downstream, with request IDs of their own.
fn spread(h: &mut Harness, on: LedgerId, count: u64) -> BTreeMap<(EndpointId, LedgerId), u32> {
    let mut hits = BTreeMap::new();
    for (request_id, draw) in (10_000u32..).zip(draws(count)) {
        let (to, ledger, sent) = send(h, (SOURCE, on), request_id, 1_000, draw).unwrap();

        *hits.entry((to, ledger)).or_default() += 1;
        let back = reject(ErrorCode::T04_INSUFFICIENT_LIQUIDITY);
        let upstream = h.respond(NOW, to, sent, &back).unwrap();
        assert_eq!((upstream.to, upstream.ledger), (SOURCE, on));
    }
    hits
}

/// The account of `id`'s slot on `ledger`.
fn account(h: &Harness, id: EndpointId, ledger: LedgerId) -> AccountId {
    h.connector.account_on(id, ledger).unwrap()
}

// A packet goes on, and comes back, on the method of the ledger it came in
// on, whichever of the hops' methods are listed first, and books on that
// method's accounts.
#[test]
fn a_packet_keeps_its_ledger_among_several_methods() {
    let mut h = harness(Config::DEFAULT);
    let hits = spread(&mut h, XRP, 100);
    assert_eq!(hits, BTreeMap::from([((ONE, XRP), 50), ((TWO, XRP), 50)]));
    let hits = spread(&mut h, USD, 100);
    assert_eq!(hits, BTreeMap::from([((ONE, USD), 50), ((TWO, USD), 50)]));
    // Fulfilled, the packet on XRP moved money on XRP alone.
    let nets = |h: &Harness| {
        [(ONE, XRP), (SOURCE, XRP), (ONE, USD), (SOURCE, USD)]
            .map(|(id, ledger)| h.net(account(h, id, ledger)))
    };
    let before = nets(&h);
    let (to, ledger, sent) = send(&mut h, (SOURCE, XRP), 900, 1_000, 0).unwrap();
    assert_eq!((to, ledger), (ONE, XRP));
    h.respond(NOW, to, sent, &fulfill([1; 32])).unwrap();
    let after = nets(&h);
    let moved: Vec<i128> = after.iter().zip(before).map(|(a, b)| a - b).collect();
    assert_eq!(moved, [1_000, -1_000, 0, 0]);
}

// The method a packet takes depends on its incoming ledger and the methods
// available, never on room: a method short of room answers T04, or T05
// for its money in flight, and no packet moves to another method of the
// same peer.
#[test]
fn a_method_short_of_room_answers_t04_and_never_moves_the_packet() {
    let mut h = harness(Config::DEFAULT);
    // Little credit to a fourth peer on USD, much on XRP.
    h.add_peer(4, "dfour", 1_000, 0);
    h.add_slot(XRP, 4, "dfour", TRUST);
    let four = EndpointId::Peer(4);
    assert_eq!(
        send(&mut h, (four, USD), 1, 1_000_000, 0),
        Err(ErrorCode::T04_INSUFFICIENT_LIQUIDITY)
    );
    assert_eq!(h.connector.pending_count(), 0, "nothing moved to XRP");
    // The money in flight is per method: the USD method of the hop full,
    // its XRP method empty, a packet on USD gets T05.
    let largest = Config::DEFAULT.max_packet_amount;
    h.connector.set_routes([(FAR, &[ONE][..])]).unwrap();
    for request_id in 10..12 {
        send(&mut h, (SOURCE, USD), request_id, largest, 0).unwrap();
    }
    assert_eq!(
        send(&mut h, (SOURCE, USD), 12, largest, 0),
        Err(ErrorCode::T05_RATE_LIMITED)
    );
    let usd = h.connector.slots(ONE).find(|s| s.ledger == USD).unwrap();
    let xrp = h.connector.slots(ONE).find(|s| s.ledger == XRP).unwrap();
    assert_eq!((usd.money_to, xrp.money_to), (2 * largest, 0));
    // XRP still has room for its own packets.
    assert_eq!(
        send(&mut h, (SOURCE, XRP), 13, largest, 0).map(|(to, l, _)| (to, l)),
        Ok((ONE, XRP))
    );
}

// A packet whose ledger no hop has a method on converts: its hop is drawn
// evenly, and it takes that hop's method with the best rate, compared at
// spot, which pays out what the node holds above its target.
#[test]
fn a_converting_packet_takes_the_drawn_hops_best_rate_method() {
    let mut h = Harness::new(Config::DEFAULT, TRUST);
    h.open_books(USD);
    h.fund(XRP, TRUST);
    h.open_books(XRP);
    h.fund(HUB, TRUST / 10);
    h.open_books(HUB);
    h.add_peer(5, "dsource", TRUST, 0);
    for (id, node) in [(1, "done"), (2, "dtwo")] {
        h.add_peer_on(HUB, id, node, TRUST, 0);
        h.add_slot(XRP, id, node, TRUST);
    }
    h.rates.set_at("USD", 10u128.pow(RATE_PRECISION), NOW);
    h.rates.set_at("XRP", 2 * 10u128.pow(RATE_PRECISION), NOW);
    h.connector.set_routes([(FAR, &[ONE, TWO][..])]).unwrap();
    // The node holds ten times more on XRP than in the hub: XRP pays most.
    let hits = spread(&mut h, USD, 100);
    assert_eq!(hits, BTreeMap::from([((ONE, XRP), 50), ((TWO, XRP), 50)]));
    // Without XRP's method on one, it goes on its hub method.
    h.connector.set_available(ONE, XRP, false);
    let hits = spread(&mut h, USD, 100);
    assert_eq!(hits, BTreeMap::from([((ONE, HUB), 50), ((TWO, XRP), 50)]));
}

// A hop's weight is per method: fees earned on one ledger weigh in the
// draw among hops on that ledger alone.
#[test]
fn weights_are_per_peer_and_ledger() {
    let mut h = harness(Config {
        explore_ppm: 0,
        ..Config::DEFAULT
    });
    h.pricing.charges.fee_ppm = 10_000;
    h.connector
        .set_routes([(FAR, &[ONE, TWO][..]), ("test.das.done", &[ONE][..])])
        .unwrap();
    h.draw = 0;
    let packet = prepare("test.das.done", 100_000, NOW + 20_000, [1; 32]);
    let Outcome::Forward(sent) = h.prepare_on(NOW, (SOURCE, USD), 1, &packet) else {
        panic!("not forwarded");
    };
    assert_eq!((sent.to, sent.ledger), (ONE, USD));
    h.respond(NOW, ONE, sent.request_id, &fulfill([1; 32]))
        .unwrap();
    assert_eq!(h.connector.earnings(ONE, USD, NOW), Some(1_000));
    assert_eq!(h.connector.earnings(ONE, XRP, NOW), Some(0));
    // On USD, only the hop that earned is drawn; on XRP, neither earned,
    // so each is as likely.
    let hits = spread(&mut h, USD, 100);
    assert_eq!(hits, BTreeMap::from([((ONE, USD), 100)]));
    let hits = spread(&mut h, XRP, 100);
    assert_eq!(hits, BTreeMap::from([((ONE, XRP), 50), ((TWO, XRP), 50)]));
}

// Only available methods carry packets: a hop whose method on the
// incoming ledger is not available is passed over for one whose is; with
// none on it, the packet converts; with no method available at all, it is
// refused for want of liquidity, which passes, rather than as unreachable.
#[test]
fn only_available_methods_carry_packets() {
    let mut h = harness(Config::DEFAULT);
    h.connector.set_available(ONE, XRP, false);
    let hits = spread(&mut h, XRP, 100);
    assert_eq!(hits, BTreeMap::from([((TWO, XRP), 100)]));
    h.connector.set_available(TWO, XRP, false);
    let hits = spread(&mut h, XRP, 100);
    assert_eq!(hits, BTreeMap::from([((ONE, USD), 50), ((TWO, USD), 50)]));
    for (id, ledger) in [(ONE, USD), (TWO, USD)] {
        h.connector.set_available(id, ledger, false);
    }
    assert_eq!(
        send(&mut h, (SOURCE, XRP), 500, 1_000, 0),
        Err(ErrorCode::T04_INSUFFICIENT_LIQUIDITY)
    );
    // A Prepare from a peer on a method not available to us is taken.
    h.connector.set_available(SOURCE, XRP, false);
    h.connector.set_available(ONE, XRP, true);
    assert_eq!(
        send(&mut h, (SOURCE, XRP), 501, 1_000, 0).map(|(to, l, _)| (to, l)),
        Ok((ONE, XRP))
    );
}

// The owner's own packets never convert: they take only methods on the
// ledger they pay from, and a quote's first hop names the first usable hop
// with a method on the ledger asked for, or else the first available
// method of the route's first usable hop.
#[test]
fn the_owners_packets_take_only_methods_on_their_ledger() {
    let mut h = harness(Config::DEFAULT);
    let first = |h: &Harness, ledger| h.connector.first_hop("test.das.dfar.x", ledger);
    assert_eq!(first(&h, None), Some((ONE, USD)));
    assert_eq!(first(&h, Some(XRP)), Some((ONE, XRP)));
    assert_eq!(first(&h, Some(HUB)), Some((ONE, USD)), "no hop on it");
    h.connector.set_available(ONE, XRP, false);
    assert_eq!(first(&h, Some(XRP)), Some((TWO, XRP)));
    h.connector.set_available(ONE, XRP, true);
    h.connector.set_available(ONE, USD, false);
    assert_eq!(first(&h, None), Some((ONE, XRP)));
    assert_eq!(first(&h, Some(USD)), Some((TWO, USD)));
    for request_id in 0..10 {
        h.draw = u64::from(request_id) << 60;
        let packet = prepare(FAR, 1_000, NOW + 20_000, [1; 32]);
        let Outcome::Forward(sent) = h.prepare(NOW, ALICE, request_id, &packet) else {
            panic!("not forwarded");
        };
        assert_eq!((sent.to, sent.ledger), (TWO, USD));
        h.respond(
            NOW,
            TWO,
            sent.request_id,
            &reject(ErrorCode::T04_INSUFFICIENT_LIQUIDITY),
        )
        .unwrap();
    }
}

// A slot removed with packets in flight waits for them: they resolve on
// it as on any other, a Prepare from the peer on it is refused, and it goes
// with the last of them. Added again meanwhile, it is taken up with them.
#[test]
fn a_slot_removed_waits_for_its_packets() {
    let mut h = harness(Config::DEFAULT);
    let (to, ledger, sent) = send(&mut h, (SOURCE, XRP), 1, 1_000, 0).unwrap();
    assert_eq!((to, ledger), (ONE, XRP));
    assert!(h.connector.remove_slot(ONE, XRP));
    assert!(h.connector.remove_slot(SOURCE, XRP));
    assert!(!h.connector.remove_slot(ONE, XRP), "removed once");
    h.check();
    // No new packet takes them.
    assert_eq!(
        send(&mut h, (SOURCE, XRP), 2, 1_000, 0),
        Err(ErrorCode::T04_INSUFFICIENT_LIQUIDITY)
    );
    let hits = spread(&mut h, USD, 100);
    assert_eq!(hits, BTreeMap::from([((ONE, USD), 50), ((TWO, USD), 50)]));
    // The packet in flight resolves on them, and they go with it.
    let upstream = h.respond(NOW, to, sent, &fulfill([1; 32])).unwrap();
    assert_eq!((upstream.to, upstream.ledger), (SOURCE, XRP));
    assert!(h.connector.slots(ONE).all(|slot| slot.ledger == USD));

    // Added again, while a packet is in flight on the removed slot, the
    // slot is taken up with its packet.
    let xrp = h.add_slot(XRP, 1, "done", TRUST);
    h.add_slot(XRP, 5, "dsource", TRUST);
    let (_, _, sent) = send(&mut h, (SOURCE, XRP), 3, 1_000, 0).unwrap();
    assert!(h.connector.remove_slot(ONE, XRP));
    assert_eq!(
        h.connector.add_slot(ONE, XRP, Some(xrp)),
        Ok(()),
        "taken up again"
    );
    assert_eq!(
        h.connector.add_slot(ONE, XRP, Some(xrp)),
        Err(SlotError::Duplicate)
    );
    h.check();
    h.respond(NOW, ONE, sent, &fulfill([1; 32])).unwrap();
    assert!(h.connector.slots(ONE).any(|slot| slot.ledger == XRP));
}

// A slot taken up again comes last, as its method does when the peering
// adds it again, and its packet in flight resolves on it.
#[test]
fn a_slot_taken_up_again_comes_last() {
    let mut h = harness(Config::DEFAULT);
    let ledgers = |h: &Harness| -> Vec<(LedgerId, bool)> {
        h.connector
            .slots(ONE)
            .map(|slot| (slot.ledger, slot.removed))
            .collect()
    };
    assert_eq!(ledgers(&h), [(USD, false), (XRP, false)]);
    let (to, ledger, sent) = send(&mut h, (SOURCE, USD), 1, 1_000, 0).unwrap();
    assert_eq!((to, ledger), (ONE, USD));
    let usd = account(&h, ONE, USD);
    assert!(h.connector.remove_slot(ONE, USD));
    assert_eq!(ledgers(&h), [(USD, true), (XRP, false)]);
    assert_eq!(h.connector.add_slot(ONE, USD, Some(usd)), Ok(()));
    assert_eq!(ledgers(&h), [(XRP, false), (USD, false)]);
    h.check();
    let upstream = h.respond(NOW, ONE, sent, &fulfill([1; 32])).unwrap();
    assert_eq!((upstream.to, upstream.ledger), (SOURCE, USD));
    h.check();
}

// A packet in flight on a slot removed for good expires at its own
// expiry, its slot with it.
#[test]
fn a_removed_slots_packet_expires() {
    let mut h = harness(Config::DEFAULT);
    let (to, ledger, _) = send(&mut h, (SOURCE, XRP), 1, 1_000, 0).unwrap();
    assert!(h.connector.remove_slot(to, ledger));
    let upstream = h.timeout(NOW + 60_000).unwrap();
    assert_eq!(
        h.reject_written(upstream.length).0,
        ErrorCode::R00_TRANSFER_TIMED_OUT
    );
    assert!(h.connector.slots(ONE).all(|slot| slot.ledger == USD));
}

// Only a peer has slots added, at most one per ledger and [`MAX_SLOTS`].
#[test]
fn slots_are_bounded() {
    let mut h = harness(Config::DEFAULT);
    assert_eq!(
        h.connector.add_slot(ALICE, XRP, None),
        Err(SlotError::Unknown)
    );
    assert_eq!(
        h.connector.add_slot(ONE, USD, None),
        Err(SlotError::Duplicate)
    );
    assert!(MAX_SLOTS >= LedgerId::ALL.len(), "a slot for every ledger");
    for ledger in LedgerId::ALL {
        let added = h.connector.add_slot(ONE, ledger, None);
        assert!(added.is_ok() || added == Err(SlotError::Duplicate));
    }
    assert_eq!(h.connector.slots(ONE).count(), LedgerId::ALL.len());
    h.check();
}

// A method over a hub waits for the hub; another method of the same peer
// does not.
#[test]
fn the_hub_flag_is_per_method() {
    let mut h = harness(Config::DEFAULT);
    h.connector.set_routes([(FAR, &[ONE][..])]).unwrap();
    assert!(h.connector.set_hub(ONE, XRP, true));
    let usd = send(&mut h, (SOURCE, USD), 1, 1_000, 0).unwrap();
    let xrp = send(&mut h, (SOURCE, XRP), 2, 1_000, 0).unwrap();
    assert_eq!((usd.1, xrp.1), (USD, XRP));
    let upstream = h.timeout(NOW + 19_000).unwrap();
    assert_eq!((upstream.to, upstream.ledger), (SOURCE, USD));
    assert!(h.timeout(NOW + 19_000).is_none(), "the hub's waits");
    let wait = h.connector.poll_hub_wait().unwrap();
    assert_eq!((wait.destination, wait.ledger), (ONE, XRP));
}

// spec/areas/connector.md#the-largest-packet: a packet that a conversion
// makes larger than the largest packet on the way out gets F08, with the
// most that would pass in its own units, and is never forwarded. Every
// threshold of the next link is counted in the largest packet: a payer
// over a hub can hold too little toward the hop for such a packet while
// the hop has too much room to ask it to settle, so the packet, refused
// for liquidity (T04), would be sent again as it is, for good.
#[test]
fn a_conversion_past_the_largest_packet_gets_f08() {
    let largest = Config::DEFAULT.max_packet_amount;
    let mut h = Harness::new(Config::DEFAULT, TRUST);
    h.open_books(USD);
    h.fund(XRP, TRUST);
    h.open_books(XRP);
    h.add_peer(5, "dsource", TRUST, 0);
    h.add_peer_on(XRP, 1, "done", TRUST, 0);
    h.rates.set_at("USD", 10u128.pow(RATE_PRECISION), NOW);
    h.rates.set_at("XRP", 2 * 10u128.pow(RATE_PRECISION), NOW);
    h.connector.set_routes([(FAR, &[ONE][..])]).unwrap();
    // A dollar buys about two XRP: the largest packet in dollars is about
    // two of them in XRP.
    let packet = prepare(FAR, largest, NOW + 20_000, [1; 32]);
    let Outcome::Respond(length) = h.prepare_on(NOW, (SOURCE, USD), 1, &packet) else {
        panic!("a packet past the largest was forwarded");
    };
    let (code, data) = h.reject_written(length);
    assert_eq!(code, ErrorCode::F08_AMOUNT_TOO_LARGE);
    let too_large = AmountTooLarge::decode_exact(&data).unwrap();
    assert_eq!(too_large.received_amount, largest);
    let mut most = too_large.maximum_amount;
    assert!(most > largest / 3 && most < largest * 3 / 5, "{most}");
    // A sender that sizes its packets by the F08s, as STREAM does, soon
    // gets one through, at most the largest packet on the way out. The
    // most an F08 names is at the larger packet's rate, and a smaller
    // packet's own impact moves its rate less, so it may take a step more.
    for request_id in 2..5 {
        let packet = prepare(FAR, most, NOW + 20_000, [1; 32]);
        match h.prepare_on(NOW, (SOURCE, USD), request_id, &packet) {
            Outcome::Forward(sent) => {
                assert_eq!((sent.to, sent.ledger), (ONE, XRP));
                let out = decode_prepare(&h.buffer[..sent.length]).amount;
                assert!(out <= largest && out > largest * 9 / 10, "{out}");
                return;
            }
            Outcome::Respond(length) => {
                let (code, data) = h.reject_written(length);
                assert_eq!(code, ErrorCode::F08_AMOUNT_TOO_LARGE);
                let too_large = AmountTooLarge::decode_exact(&data).unwrap();
                assert!(too_large.maximum_amount < most);
                most = too_large.maximum_amount;
            }
            other => panic!("{other:?}"),
        }
    }
    panic!("no packet sized by the F08s got through");
}

// spec/decisions/settlement-methods.md, spec/decisions/token-hubs.md: a
// ledger whose conversions are halted, as a dollar token's hub off its peg,
// still carries the packets that keep it: one in on its method goes on, on
// that ledger, needing no rate. Only a packet that would convert to or from
// it is refused.
#[test]
fn a_halted_ledger_still_carries_the_packets_that_keep_it() {
    let mut h = harness(Config::DEFAULT);
    // A fourth peer, on USD alone: a packet in on XRP to it converts.
    h.add_peer(4, "dfour", TRUST, 0);
    h.connector
        .set_routes([
            (FAR, &[ONE, TWO][..]),
            ("test.das.dfour", &[EndpointId::Peer(4)][..]),
        ])
        .unwrap();
    // Built without XRP, as the node builds it while XRP's guard pauses it.
    let inventories: Vec<_> = LedgerId::ALL
        .into_iter()
        .filter(|ledger| *ledger != XRP)
        .filter_map(|ledger| h.inventory(ledger))
        .collect();
    let max_age_ms = h.connector.config().max_rate_age_ms;
    let mut prices =
        dassie_connector::PriceTable::build(&h.rates, &inventories, h.pricing, NOW, max_age_ms);
    prices.halt(XRP);
    let send_halted = |h: &mut Harness, request_id: u32, destination: &str| {
        let packet = prepare(destination, 1_000, NOW + 20_000, [1; 32]);
        let outcome = h.connector.handle_prepare_with(
            NOW,
            (SOURCE, request_id),
            Some(XRP),
            &packet,
            &mut h.ledger,
            &prices,
            &mut dassie_connector::OpensNone,
            0,
            &mut h.buffer,
        );
        h.check();
        outcome
    };
    match send_halted(&mut h, 1, FAR) {
        Outcome::Forward(sent) => {
            let amount = decode_prepare(&h.buffer[..sent.length]).amount;
            assert_eq!((sent.ledger, amount), (XRP, 1_000));
        }
        other => panic!("refused on the ledger it came in on: {other:?}"),
    }
    match send_halted(&mut h, 2, "test.das.dfour") {
        Outcome::Respond(length) => {
            let (code, _) = h.reject_written(length);
            assert_ne!(code, ErrorCode::T04_INSUFFICIENT_LIQUIDITY, "{code:?}");
        }
        other => panic!("converted off a halted ledger: {other:?}"),
    }
}
