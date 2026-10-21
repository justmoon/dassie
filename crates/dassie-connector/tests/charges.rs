//! What transit costs (spec/decisions/exchange-rates.md): a fee on what
//! passes from one peer to another, a spread on what it converts, and nothing
//! on the owner's own packets. The original charged neither (ACC-3);
//! registration fees, the gap's other part, are not built
//! (spec/roadmap.md#registration-fees).

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use dassie_connector::sim::*;
use dassie_connector::{
    Charges, Config, EndpointId, Forwarded, Outcome, RATE_PRECISION, Responded, convert,
};
use dassie_ilp::ErrorCode;
use dassie_ledger::{AccountName, LedgerId};
use proptest::prelude::*;

const ALICE: EndpointId = EndpointId::Btp(1);
const ONE: EndpointId = EndpointId::Peer(1);
const TWO: EndpointId = EndpointId::Peer(2);
const TRUST: u64 = 10_000_000_000;
const USD: LedgerId = LedgerId::StubUsd;
const XRP: LedgerId = LedgerId::XrplTestnetXrp;

/// Two peers, `done` on USD and `dtwo` on `two`, routed to at
/// `test.das.done` and `test.das.dtwo`; 1 USD buys 2 XRP.
fn harness(two: LedgerId, charges: Charges) -> Harness {
    let mut h = Harness::new(Config::DEFAULT, TRUST);
    h.open_books(USD);
    if two != USD {
        // What it pays out in XRP is the owner's.
        h.fund(two, 1_000_000_000_000_000);
        h.open_books(two);
    }
    h.add_peer(1, "done", TRUST, 0);
    h.add_peer_on(two, 2, "dtwo", TRUST, 0);
    h.connector
        .set_routes([("test.das.done", &[ONE][..]), ("test.das.dtwo", &[TWO][..])])
        .unwrap();
    h.rates.set("USD", 10u128.pow(RATE_PRECISION));
    h.rates.set("XRP", 2 * 10u128.pow(RATE_PRECISION));
    assert!(h.connector.set_charges(charges));
    h
}

fn forwarded(outcome: Outcome) -> Forwarded {
    match outcome {
        Outcome::Forward(forwarded) => forwarded,
        other => panic!("expected a forward, got {other:?}"),
    }
}

fn amount_sent(h: &Harness, forwarded: Forwarded) -> u64 {
    decode_prepare(&h.buffer[..forwarded.length]).amount
}

const FEE: Charges = Charges {
    fee_ppm: 10_000,
    spread_ppm: 5_000,
};

#[test]
fn transit_pays_our_fee() {
    let mut h = harness(USD, FEE);
    let packet = prepare("test.das.dtwo.x", 100_000, NOW + 20_000, [1; 32]);
    let sent = forwarded(h.prepare(NOW, ONE, 1, &packet));
    // 1% of what came in; no spread without a conversion.
    assert_eq!(amount_sent(&h, sent), 99_000);
    let responded = h.respond_full(NOW + 1, TWO, sent.request_id, &fulfill([1; 32]));
    let Responded::Resolved {
        posted: Some(posted),
        ..
    } = responded
    else {
        panic!("not posted: {responded:?}");
    };
    let [main, conversion, fee] = posted.transfers;
    assert_eq!(main.unwrap().amount, 99_000);
    assert!(conversion.is_none());
    let fee = fee.unwrap();
    assert_eq!((fee.debit, fee.amount), (main.unwrap().debit, 1_000));
    assert_eq!(h.net_at(USD, AccountName::RevenueFees), 1_000);
}

#[test]
fn a_rejected_packet_pays_no_fee() {
    let mut h = harness(USD, FEE);
    let packet = prepare("test.das.dtwo.x", 100_000, NOW + 20_000, [1; 32]);
    let sent = forwarded(h.prepare(NOW, ONE, 1, &packet));
    h.respond(
        NOW + 1,
        TWO,
        sent.request_id,
        &reject(ErrorCode::F99_APPLICATION_ERROR),
    )
    .unwrap();
    assert_eq!(h.net_at(USD, AccountName::RevenueFees), 0);
    assert_eq!(h.ledger.pending_transfers().count(), 0);
    // Nor one that expires.
    forwarded(h.prepare(NOW, ONE, 2, &packet));
    assert!(h.timeout(NOW + 30_000).is_some());
    assert_eq!(h.connector.pending_for(TWO), 0);
    assert_eq!(h.net_at(USD, AccountName::RevenueFees), 0);
    assert_eq!(h.ledger.pending_transfers().count(), 0);
}

#[test]
fn transit_across_ledgers_pays_the_fee_and_the_spread() {
    let mut h = harness(XRP, FEE);
    let packet = prepare("test.das.dtwo.x", 100_000, NOW + 20_000, [1; 32]);
    let sent = forwarded(h.prepare(NOW, ONE, 1, &packet));
    // 99,000 USD after the fee is 198,000 XRP, less 0.5%.
    assert_eq!(amount_sent(&h, sent), 197_010);
    assert_eq!(h.ledger.pending_transfers().count(), 3);
    h.respond(NOW + 1, TWO, sent.request_id, &fulfill([1; 32]))
        .unwrap();
    assert_eq!(h.net_at(USD, AccountName::RevenueFees), 1_000);
    assert_eq!(h.net_at(USD, AccountName::RevenueFx), 99_000);
    assert_eq!(h.net_at(XRP, AccountName::ExpensesFx), -197_010);
}

#[test]
fn the_owners_packets_are_never_charged() {
    let mut h = harness(USD, FEE);
    let packet = prepare("test.das.dtwo.x", 100_000, NOW + 20_000, [1; 32]);
    let sent = forwarded(h.prepare(NOW, ALICE, 1, &packet));
    assert_eq!(amount_sent(&h, sent), 100_000);
    h.respond(NOW + 1, TWO, sent.request_id, &fulfill([1; 32]))
        .unwrap();
    let into_ours = prepare(&format!("{OWN}.stream.x"), 100_000, NOW + 20_000, [2; 32]);
    let sent = forwarded(h.prepare(NOW, ONE, 2, &into_ours));
    assert_eq!((sent.to, amount_sent(&h, sent)), (RECEIVER, 100_000));
    h.respond(NOW + 1, RECEIVER, sent.request_id, &fulfill([2; 32]))
        .unwrap();
    assert_eq!(h.net_at(USD, AccountName::RevenueFees), 0);
}

// A packet too small to carry a fee passes free: rounded up, the fee
// would take a payment's last packets whole.
#[test]
fn a_packet_too_small_for_a_fee_passes_free() {
    let mut h = harness(USD, FEE);
    let packet = prepare("test.das.dtwo.x", 99, NOW + 20_000, [1; 32]);
    let sent = forwarded(h.prepare(NOW, ONE, 1, &packet));
    assert_eq!(amount_sent(&h, sent), 99);
    let packet = prepare("test.das.dtwo.x", 100, NOW + 20_000, [2; 32]);
    let sent = forwarded(h.prepare(NOW, ONE, 2, &packet));
    assert_eq!(amount_sent(&h, sent), 99);
    // Nothing that moves no money is charged.
    let packet = prepare("test.das.dtwo.x", 0, NOW + 20_000, [3; 32]);
    let sent = forwarded(h.prepare(NOW, ONE, 3, &packet));
    assert_eq!(amount_sent(&h, sent), 0);
}

// What converts to nothing is refused: half an XRP unit is no dollar unit.
#[test]
fn what_converts_to_nothing_is_refused() {
    let mut h = harness(XRP, FEE);
    h.fund(USD, TRUST);
    let packet = prepare("test.das.done.x", 1, NOW + 20_000, [1; 32]);
    let Outcome::Respond(length) = h.prepare(NOW, TWO, 1, &packet) else {
        panic!("forwarded");
    };
    assert_eq!(h.reject_written(length).0, ErrorCode::F02_UNREACHABLE);
}

#[test]
fn charges_beyond_the_maximum_are_refused() {
    let mut h = harness(USD, Charges::NONE);
    let too_much = Charges {
        fee_ppm: Charges::MAX_PPM + 1,
        spread_ppm: 0,
    };
    assert!(!h.connector.set_charges(too_much));
    assert_eq!(h.connector.charges(), Charges::NONE);
}

proptest! {
    // What the next hop gets is never more than what came in, at the mid
    // rate, less the fee; the source pays exactly what came in.
    #[test]
    fn charges_never_give_away_money(
        amount in 1u64..=Config::DEFAULT.max_packet_amount,
        fee_ppm in 0..=Charges::MAX_PPM,
        spread_ppm in 0..=Charges::MAX_PPM,
        xrp_rate in 1u128..=1_000 * 10u128.pow(RATE_PRECISION),
    ) {
        let charges = Charges { fee_ppm, spread_ppm };
        let mut h = harness(XRP, charges);
        h.rates.set("XRP", xrp_rate);
        let source = h.ledger.find(&dassie_ledger::AccountPath::new(
            USD,
            AccountName::AssetsInterledger(dassie_ledger::Name::new("done").unwrap()),
        )).unwrap();
        let before = h.net(source);
        let packet = prepare("test.das.dtwo.x", amount, NOW + 20_000, [1; 32]);
        let sent = match h.prepare(NOW, ONE, 1, &packet) {
            Outcome::Forward(sent) => sent,
            Outcome::Respond(length) => {
                // Only an amount the charges take whole.
                prop_assert_eq!(h.reject_written(length).0, ErrorCode::F02_UNREACHABLE);
                return Ok(());
            }
            other => panic!("{other:?}"),
        };
        let out = amount_sent(&h, sent);
        let fee = charges.fee(amount);
        let mid = convert(&h.rates, USD.asset(), XRP.asset(), amount - fee).unwrap();
        prop_assert!(out <= mid);
        prop_assert!(out > 0);
        h.respond(NOW + 1, TWO, sent.request_id, &fulfill([1; 32])).unwrap();
        prop_assert_eq!(before - h.net(source), i128::from(amount));
        prop_assert_eq!(h.net_at(USD, AccountName::RevenueFees), i128::from(fee));
        prop_assert_eq!(h.net_at(XRP, AccountName::ExpensesFx), -i128::from(out));
    }
}
