//! Draining before the node stops (spec/areas/node.md#shutdown): the
//! connector takes no new packet, the node's own payments send none and
//! wait, and those in flight are answered, so that their Fulfills are
//! booked rather than lost as in a crash.

use std::collections::BTreeMap;

use dassie_connector::EndpointId;

use crate::engine::{EngineError, OwnerSwap};
use crate::sim::{START, Settling, Sim, USD, XRP, key};
use crate::store::OutgoingState;

/// Ten units: a hundred of the largest packets, so a payment still sends
/// when the drain begins.
const AMOUNT: u64 = 10_000_000_000;

/// Two peered nodes, node 0 paying node 1 with packets in flight.
fn paying() -> Sim {
    let mut sim = Sim::with(17, 2, 1, BTreeMap::new(), Settling::Trusting);
    sim.loss_per_mille = 0;
    let peered = |sim: &Sim| !sim.peers(0).is_empty() && !sim.peers(1).is_empty();
    assert!(
        sim.run_until_or(START + 60_000, peered),
        "the nodes did not peer"
    );
    sim.run_until(sim.now + 5_000);
    sim.pay_outgoing(0, 1, AMOUNT).unwrap();
    assert!(
        sim.run_until_or(sim.now + 10_000, |sim| sim.core(0).in_flight() > 0),
        "nothing went out"
    );
    sim
}

/// The state of node 0's payment.
fn state(sim: &Sim) -> OutgoingState {
    let (from, _, _, outgoing) = sim.outgoing[0];
    sim.nodes[from]
        .store
        .outgoing_payment(outgoing)
        .unwrap()
        .unwrap()
        .state
}

/// Begins node 0's drain, and returns when it began and its bound.
fn drain(sim: &mut Sim) -> (u64, u64) {
    let mut until = None;
    assert!(sim.step(0, |core| until = Some(core.begin_drain())));
    let began = sim.core(0).now_ms();
    (began, until.unwrap())
}

/// Packets node 0's own payer sent since `since`.
fn payer_packets_since(sim: &Sim, since: u64) -> usize {
    sim.core(0)
        .packets(None, 100_000)
        .into_iter()
        .filter(|record| matches!(record.source, EndpointId::Local(_)))
        .filter(|record| record.received_at_ms > since)
        .count()
}

// Draining, the node sends no new packet of its own and makes no payment;
// those in flight are answered within the drain's bound, and the payment
// waits, sending.
#[test]
fn a_drain_sends_nothing_new_and_waits_for_what_is_in_flight() {
    let mut sim = paying();
    let (began, until) = drain(&mut sim);
    let config = crate::node::Policies::DEFAULT.connector;
    assert!(until <= began + config.max_hold_ms + config.message_window_ms);
    assert!(sim.core(0).is_draining());
    let refused = sim.pay_outgoing(0, 1, 1_000).unwrap_err();
    assert!(refused.contains("the node is stopping"), "{refused}");
    assert!(
        sim.run_until_or(until, |sim| sim.core(0).drained()),
        "packets in flight past the drain's bound"
    );
    assert_eq!(payer_packets_since(&sim, began), 0);
    assert_eq!(state(&sim), OutgoingState::Sending);
    // Nothing more goes out while it holds.
    sim.run_until(sim.now + 10_000);
    assert_eq!(payer_packets_since(&sim, began), 0);
    assert!(sim.core(0).drained());
}

// A drain taken back, as by the desktop app's Cancel: the payment goes on.
#[test]
fn a_drain_taken_back_lets_the_payment_go_on() {
    let mut sim = paying();
    let (_, until) = drain(&mut sim);
    assert!(sim.run_until_or(until, |sim| sim.core(0).drained()));
    assert!(sim.step(0, |core| core.end_drain()));
    assert!(!sim.core(0).is_draining());
    assert!(
        sim.run_until_or(sim.now + 600_000, |sim| state(sim)
            != OutgoingState::Sending),
        "the payment did not end"
    );
    assert_eq!(state(&sim), OutgoingState::Completed);
}

// Drained, the node stops; started again, it sends the rest of the
// payment, from what was posted (spec/areas/payments.md#outgoing-payments).
#[test]
fn after_a_drain_and_a_restart_the_payment_goes_on() {
    let mut sim = paying();
    let (_, until) = drain(&mut sim);
    assert!(sim.run_until_or(until, |sim| sim.core(0).drained()));
    sim.crash(0);
    sim.start(0);
    assert!(!sim.core(0).is_draining(), "a node starts taking packets");
    assert!(
        sim.run_until_or(sim.now + 600_000, |sim| state(sim)
            != OutgoingState::Sending),
        "the payment did not end"
    );
    assert_eq!(state(&sim), OutgoingState::Completed);
}

// The owner's swap, which would send, is refused while the node drains.
#[test]
fn a_draining_node_starts_no_swap() {
    let mut sim = paying();
    drain(&mut sim);
    let swap = OwnerSwap {
        peer: key(1).node_id(),
        pay_on: USD,
        back_on: XRP,
        amount: 1_000,
        rate: (1, 1),
    };
    let mut refused = None;
    assert!(sim.step(0, |core| refused = Some(core.start_owner_swap(1, swap))));
    assert_eq!(
        refused.unwrap(),
        Err(EngineError::Precondition("the node is stopping".into()))
    );
}
