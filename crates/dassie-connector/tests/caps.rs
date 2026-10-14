//! Spending caps (spec 7 §4.5): a BTP token spends at most its cap per
//! window out of the owner's funds, refilling continuously.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

mod support;

use dassie_connector::{
    CapError, CapUsage, Config, EndpointId, Forwarded, Outcome, SpendingCap, replay,
};
use dassie_ilp::{AmountTooLarge, ErrorCode};
use dassie_ledger::AccountId;
use dassie_oer::Decode;
use proptest::prelude::*;
use support::*;

const ALICE: EndpointId = EndpointId::Btp(1);
const BOB: EndpointId = EndpointId::Btp(2);
const PEER: EndpointId = EndpointId::Peer(1);
const CAP: SpendingCap = SpendingCap {
    amount: 1_000,
    window_ms: 10_000,
};

/// Alice's token with `CAP`, funds for many windows, and a peer to pay.
fn harness() -> (Harness, AccountId) {
    let mut h = Harness::new(Config::DEFAULT, 1_000_000_000);
    h.add_peer(1, "dpeer", 1_000_000_000, 0);
    h.connector
        .set_routes([("test.das.dpeer", &[PEER][..])])
        .unwrap();
    let alice = alice(&h);
    h.connector.set_cap(NOW, alice, Some(CAP)).unwrap();
    (h, alice)
}

fn alice(h: &Harness) -> AccountId {
    let path = dassie_ledger::AccountPath::new(
        dassie_ledger::LedgerId::StubUsd,
        dassie_ledger::AccountName::EquityOwnerSub(dassie_ledger::OwnerSub::Btp(1)),
    );
    h.ledger.find(&path).unwrap()
}

/// What became of a Prepare of `amount` from `from` to `destination`.
fn send(
    h: &mut Harness,
    now: u64,
    from: EndpointId,
    request_id: u32,
    destination: &str,
    amount: u64,
) -> Result<Forwarded, ErrorCode> {
    let packet = prepare(destination, amount, now + 20_000, [7; 32]);
    match h.prepare(now, from, request_id, &packet) {
        Outcome::Forward(forwarded) => Ok(forwarded),
        Outcome::Respond(length) => Err(h.reject_written(length).0),
        other => panic!("unexpected {other:?}"),
    }
}

fn to_peer(
    h: &mut Harness,
    now: u64,
    request_id: u32,
    amount: u64,
) -> Result<Forwarded, ErrorCode> {
    send(h, now, ALICE, request_id, "test.das.dpeer.shop", amount)
}

fn fulfilled(h: &mut Harness, now: u64, sent: Forwarded) {
    let upstream = h.respond(now, sent.to, sent.request_id, &fulfill([7; 32]));
    assert!(upstream.is_some());
}

#[test]
fn a_full_window_then_a_unit_per_window_share() {
    let (mut h, alice) = harness();
    let sent = to_peer(&mut h, NOW, 1, 1_000).unwrap();
    fulfilled(&mut h, NOW, sent);
    assert_eq!(
        to_peer(&mut h, NOW, 2, 1),
        Err(ErrorCode::T04_INSUFFICIENT_LIQUIDITY)
    );
    // 1 000 per 10 s: one unit every 10 ms.
    assert_eq!(
        to_peer(&mut h, NOW + 9, 3, 1),
        Err(ErrorCode::T04_INSUFFICIENT_LIQUIDITY)
    );
    assert!(to_peer(&mut h, NOW + 10, 4, 1).is_ok());
    let view = h.connector.cap(alice, NOW + 10).unwrap();
    assert_eq!((view.available, view.reserved), (0, 1));
    assert_eq!(view.full_at_ms, Some(NOW + 10_000));
}

#[test]
fn a_packet_larger_than_the_cap_is_told_the_cap() {
    let (mut h, _) = harness();
    let packet = prepare("test.das.dpeer.shop", 1_001, NOW + 20_000, [7; 32]);
    let Outcome::Respond(length) = h.prepare(NOW, ALICE, 1, &packet) else {
        panic!("forwarded");
    };
    let (code, data) = h.reject_written(length);
    assert_eq!(code, ErrorCode::F08_AMOUNT_TOO_LARGE);
    let too_large = AmountTooLarge::decode_exact(&data).unwrap();
    assert_eq!(
        (too_large.received_amount, too_large.maximum_amount),
        (1_001, 1_000)
    );
}

#[test]
fn packets_in_flight_hold_the_cap_and_a_reject_returns_it() {
    let (mut h, _) = harness();
    let first = to_peer(&mut h, NOW, 1, 600).unwrap();
    assert_eq!(
        to_peer(&mut h, NOW, 2, 600),
        Err(ErrorCode::T04_INSUFFICIENT_LIQUIDITY)
    );
    h.respond(
        NOW,
        first.to,
        first.request_id,
        &reject(ErrorCode::T04_INSUFFICIENT_LIQUIDITY),
    );
    assert!(to_peer(&mut h, NOW, 3, 600).is_ok());
}

// Money that stays in the owner's funds is not spending: paying the node's
// own receiver, or another token's client.
#[test]
fn only_money_leaving_the_owners_funds_counts() {
    let (mut h, alice) = harness();
    let sent = to_peer(&mut h, NOW, 1, 1_000).unwrap();
    fulfilled(&mut h, NOW, sent);
    let own = format!("{OWN}.stream.x");
    let sent = send(&mut h, NOW, ALICE, 2, &own, 5_000).unwrap();
    fulfilled(&mut h, NOW, sent);
    let bob = format!("{OWN}.bob");
    let sent = send(&mut h, NOW, ALICE, 3, &bob, 5_000).unwrap();
    fulfilled(&mut h, NOW, sent);
    assert_eq!(h.connector.cap(alice, NOW).unwrap().available, 0);
    // And Bob has no cap.
    assert!(send(&mut h, NOW, BOB, 1, "test.das.dpeer.x", 5_000).is_ok());
}

#[test]
fn a_restored_cap_decides_the_same() {
    let (mut h, alice) = harness();
    let sent = to_peer(&mut h, NOW, 1, 700).unwrap();
    fulfilled(&mut h, NOW + 100, sent);
    let (account, cap, usage) = h.connector.poll_cap_change().unwrap();
    assert_eq!((account, cap), (alice, CAP));
    assert!(h.connector.poll_cap_change().is_none());
    let mut restored = harness().0;
    restored.connector.restore_cap(alice, cap, usage).unwrap();
    let at = NOW + 2_000;
    assert_eq!(
        restored.connector.cap(alice, at),
        h.connector.cap(alice, at)
    );
    assert_eq!(restored.connector.cap(alice, at).unwrap().available, 490);
}

#[test]
fn changing_a_cap() {
    let (mut h, alice) = harness();
    let sent = to_peer(&mut h, NOW, 1, 500).unwrap();
    fulfilled(&mut h, NOW, sent);
    let in_flight = to_peer(&mut h, NOW, 2, 400).unwrap();
    // Halved: what was used stays used, so the packet in flight is over.
    let half = SpendingCap {
        amount: 500,
        window_ms: 10_000,
    };
    h.connector.set_cap(NOW, alice, Some(half)).unwrap();
    h.check();
    assert_eq!(h.connector.cap(alice, NOW).unwrap().reserved, 400);
    assert_eq!(
        to_peer(&mut h, NOW + 5_000, 3, 1),
        Err(ErrorCode::T04_INSUFFICIENT_LIQUIDITY)
    );
    fulfilled(&mut h, NOW + 5_000, in_flight);
    // Lifted, and set again: it starts from nothing.
    h.connector.set_cap(NOW, alice, None).unwrap();
    assert!(h.connector.cap(alice, NOW).is_none());
    h.connector.set_cap(NOW + 5_000, alice, Some(CAP)).unwrap();
    assert_eq!(
        h.connector.cap(alice, NOW + 5_000).unwrap().available,
        1_000
    );
    let invalid = SpendingCap {
        amount: 0,
        window_ms: 10_000,
    };
    assert_eq!(
        h.connector.set_cap(NOW, alice, Some(invalid)),
        Err(CapError::Invalid)
    );
}

/// One step of a random run.
#[derive(Clone, Debug)]
enum Step {
    /// Alice pays the peer this much.
    Pay(u64),
    /// Alice pays the node's own receiver.
    PayOwn(u64),
    /// The oldest packet in flight is fulfilled, or rejected.
    Resolve(bool),
    /// Time passes.
    Wait(u64),
}

fn step() -> impl Strategy<Value = Step> {
    prop_oneof![
        4 => (1..=1_200u64).prop_map(Step::Pay),
        1 => (1..=1_200u64).prop_map(Step::PayOwn),
        4 => any::<bool>().prop_map(Step::Resolve),
        2 => (0..=4_000u64).prop_map(Step::Wait),
    ]
}

proptest! {
    // Whatever happens, what Alice spends in any period of t ms is at most
    // A + A·t/W, and the cap's usage is the replay of what she spent.
    #[test]
    fn a_cap_bounds_spending_over_every_period(steps in prop::collection::vec(step(), 1..300)) {
        let (mut h, alice) = harness();
        let mut now = NOW;
        // Each packet in flight: where it went, its amount, whether it
        // counts, and when it expires (the incoming expiry less 1 s).
        let mut in_flight: Vec<(Forwarded, u64, bool, u64)> = Vec::new();
        let mut spent: Vec<(u64, u64)> = Vec::new();
        let mut request_id = 0;
        for step in steps {
            request_id += 1;
            match step {
                Step::Pay(amount) => {
                    if let Ok(sent) = to_peer(&mut h, now, request_id, amount) {
                        in_flight.push((sent, amount, true, now + 19_000));
                    }
                }
                Step::PayOwn(amount) => {
                    let own = format!("{OWN}.stream.x");
                    if let Ok(sent) = send(&mut h, now, ALICE, request_id, &own, amount) {
                        in_flight.push((sent, amount, false, now + 19_000));
                    }
                }
                Step::Resolve(fulfill_it) => {
                    if in_flight.is_empty() {
                        continue;
                    }
                    let (sent, amount, counted, expires_at) = in_flight.remove(0);
                    if fulfill_it {
                        // A fulfillment after the expiry counts as the
                        // timeout it is.
                        fulfilled(&mut h, now, sent);
                        if counted && now < expires_at {
                            spent.push((now, amount));
                        }
                    } else {
                        h.respond(now, sent.to, sent.request_id, &reject(ErrorCode::T00_INTERNAL_ERROR));
                    }
                }
                Step::Wait(ms) => now += ms,
            }
        }
        for (i, &(start, _)) in spent.iter().enumerate() {
            let mut total = 0u128;
            for &(at, amount) in &spent[i..] {
                total += u128::from(amount);
                let allowed = u128::from(CAP.amount)
                    * (u128::from(CAP.window_ms) + u128::from(at - start));
                prop_assert!(total * u128::from(CAP.window_ms) <= allowed);
            }
        }
        let expected = replay(CAP, CapUsage::new(NOW), spent.iter().copied());
        let view = h.connector.cap(alice, now).unwrap();
        prop_assert_eq!(view.usage, expected.refilled(CAP, now));
    }
}

/// A step of a run in which the owner also changes the cap.
#[derive(Clone, Debug)]
enum Change {
    Pay(u64),
    Resolve(bool),
    Wait(u64),
    /// The cap becomes this much per 10 s.
    Set(u64),
    /// The cap is lifted.
    Lift,
}

fn change() -> impl Strategy<Value = Change> {
    prop_oneof![
        4 => (1..=1_200u64).prop_map(Change::Pay),
        4 => any::<bool>().prop_map(Change::Resolve),
        2 => (0..=4_000u64).prop_map(Change::Wait),
        1 => (1..=2_000u64).prop_map(Change::Set),
        1 => Just(Change::Lift),
    ]
}

proptest! {
    // Changing, lifting and setting the cap again while packets are in
    // flight keeps the connector's invariants: every cap holds exactly
    // what its packets in flight charge, and stays within itself unless
    // it was lowered under them.
    #[test]
    fn caps_change_safely_with_packets_in_flight(steps in prop::collection::vec(change(), 1..200)) {
        let (mut h, alice) = harness();
        let mut now = NOW;
        let mut in_flight: Vec<Forwarded> = Vec::new();
        let mut request_id = 0;
        for step in steps {
            request_id += 1;
            match step {
                Change::Pay(amount) => {
                    if let Ok(sent) = to_peer(&mut h, now, request_id, amount) {
                        in_flight.push(sent);
                    }
                }
                Change::Resolve(fulfill_it) => {
                    if in_flight.is_empty() {
                        continue;
                    }
                    let sent = in_flight.remove(0);
                    let response = if fulfill_it {
                        fulfill([7; 32])
                    } else {
                        reject(ErrorCode::T00_INTERNAL_ERROR)
                    };
                    h.respond(now, sent.to, sent.request_id, &response);
                }
                Change::Wait(ms) => now += ms,
                Change::Set(amount) => {
                    let cap = SpendingCap { amount, window_ms: 10_000 };
                    h.connector.set_cap(now, alice, Some(cap)).unwrap();
                    h.check();
                }
                Change::Lift => {
                    h.connector.set_cap(now, alice, None).unwrap();
                    h.check();
                }
            }
        }
    }
}
