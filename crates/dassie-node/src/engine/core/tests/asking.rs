//! Asking a peer to settle at the trigger
//! (spec/decisions/settle-at-the-trigger.md,
//! spec/areas/settlement.md#asking-a-peer-to-settle): a Prepare that takes
//! a peer's room to the trigger has it asked at once, never two requests
//! per method are in flight, a peer that does not settle is asked again
//! only after the interval, and the settlement check stays the backstop
//! (`over_a_hub_a_payee_asks_by_the_room_of_the_hold_toward_it`).

use std::collections::BTreeMap;

use super::{NOW, hub_core_with_downlink, scheme_id};
use crate::node::Policies;
use crate::sim::{Settling, Sim, key, received};

/// How often a peer that did not settle since it was last asked is asked
/// again.
const ASK_EVERY_MS: u64 = Policies::DEFAULT.settlement_request_every_ms;

/// How many requests to settle `core` sent.
fn requested(core: &super::Core) -> u64 {
    core.peer.stats().settlements_requested
}

// However many Prepares find a peer short while our request is on its way,
// one request per method is in flight: here even with our books showing a
// settlement from it after each, which lets it be asked again.
#[test]
fn a_burst_of_prepares_sends_one_request() {
    let (mut core, peer) = hub_core_with_downlink(71);
    let stub = scheme_id("stub");
    // No settlement check runs: what asks is the Prepare.
    core.settle.at_ms = u64::MAX;
    core.begin_step(NOW + 2_000);
    // A downlink nothing moved with owes us nothing and is extended
    // nothing: its room is at our trigger.
    let before = requested(&core);
    core.room_fell(peer, stub);
    assert!(core.peer.asking(&peer, stub));
    assert_eq!(requested(&core), before + 1);
    for _ in 0..100 {
        // As a settlement from the peer booked does.
        core.settle.asked.clear();
        core.room_fell(peer, stub);
    }
    assert_eq!(requested(&core), before + 1, "a second request in flight");
    core.check_invariants();
}

// A peer asked at the trigger that does not settle is asked again only
// after the interval, however many of its Prepares find it short, even
// once our request was given up.
#[test]
fn a_peer_that_does_not_settle_is_asked_again_after_the_interval() {
    let (mut core, peer) = hub_core_with_downlink(72);
    let stub = scheme_id("stub");
    core.settle.at_ms = u64::MAX;
    let asked_at = NOW + 2_000;
    core.begin_step(asked_at);
    let before = requested(&core);
    core.room_fell(peer, stub);
    assert_eq!(requested(&core), before + 1);
    let mut outputs = Vec::new();
    core.finish_step(&mut outputs);
    // The downlink never answers: the request is given up.
    let given_up = asked_at + Policies::DEFAULT.peer.request_timeout_ms + 1_000;
    assert!(given_up < asked_at + ASK_EVERY_MS);
    core.begin_step(given_up);
    outputs.clear();
    core.finish_step(&mut outputs);
    assert!(!core.peer.asking(&peer, stub));
    core.room_fell(peer, stub);
    assert_eq!(
        requested(&core),
        before + 1,
        "asked again within the interval"
    );
    core.begin_step(asked_at + ASK_EVERY_MS);
    core.room_fell(peer, stub);
    assert_eq!(requested(&core), before + 2, "not asked again after it");
}

// A Prepare that leaves the peer's room above the trigger asks nothing.
#[test]
fn room_above_the_trigger_asks_nothing() {
    let (mut core, peer) = hub_core_with_downlink(73);
    let stub = scheme_id("stub");
    core.settle.at_ms = u64::MAX;
    // The peer's hold toward us: well above our trigger.
    let funded = core.settle.trigger * 4;
    core.begin_step(NOW + 2_000);
    core.incoming_settlement(peer, stub, funded, &[]).unwrap();
    let mut outputs = Vec::new();
    core.finish_step(&mut outputs);
    let before = requested(&core);
    core.begin_step(NOW + 3_000);
    core.room_fell(peer, stub);
    assert_eq!(requested(&core), before);
    assert!(!core.peer.asking(&peer, stub));
}

// spec/decisions/settle-at-the-trigger.md: a payee whose settlement check
// never runs still has its payer fund its hold as the payer's packets
// drain it, so a payment of many holds' worth arrives: each request goes
// out on the Prepare that took the room to the trigger.
#[test]
fn a_payee_without_its_check_still_asks_at_the_trigger() {
    let mut sim = Sim::with(74, 2, 1, BTreeMap::new(), Settling::StubHub(4));
    sim.loss_per_mille = 0;
    let stub = dassie_ledger::name!("stub");
    let (zero, one) = (key(0).node_id(), key(1).node_id());
    // Each funds its hold toward the other once it is asked at its first
    // check.
    let funded = |sim: &Sim| {
        sim.core(0)
            .hub_books(stub, one)
            .is_some_and(|(c, _, _)| c > 0)
            && sim
                .core(1)
                .hub_books(stub, zero)
                .is_some_and(|(c, _, _)| c > 0)
    };
    assert!(
        sim.run_until_or(sim.now + 600_000, funded),
        "the holds were not funded"
    );
    sim.run_until(sim.now + 10_000);
    let hold = sim.core(0).hub_books(stub, one).unwrap().0;
    sim.step(1, |core| core.settle.at_ms = u64::MAX);
    let before = requested(sim.core(1));
    let amount = hold * 6;
    assert!(sim.pay(0, 1, amount, None, None));
    let paid = |sim: &Sim| received(sim, &sim.payments[0]) == amount;
    assert!(
        sim.run_until_or(sim.now + 600_000, paid),
        "paid {} of {amount}",
        received(&sim, &sim.payments[0])
    );
    assert!(
        requested(sim.core(1)) >= before + 4,
        "asked {} times",
        requested(sim.core(1)) - before
    );
}
