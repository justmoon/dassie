//! A request to settle the payer could not fund in full when it came is
//! funded as soon as the payer can, within the interval its asker waits
//! before asking again (spec/decisions/settle-when-funds-arrive.md,
//! spec/areas/settlement.md#settling-on-request): a payer over a hub whose
//! account had nothing to spend funds the hold once the owner's deposit
//! lands, without being asked again; it funds nothing below the least
//! settlement; an empty one tries nothing, and forgets the request once
//! its asker would ask again; and nothing is settled twice, a crash
//! included. On one core over the stub with commitments, a downlink
//! asking it to settle.

use std::collections::BTreeMap;
use std::sync::Arc;

use dassie_identity::NodeId;
use dassie_kit::sim::SeededRng;
use dassie_ledger::Name;
use dassie_settlement::policy;

use super::super::{Core, Event, SettlementEvent};
use super::{
    NOW, core_on, hub_books, hub_core_with_downlink, hub_core_with_downlink_on, scheme_id,
};
use crate::engine::Output;
use crate::node::Policies;
use crate::sim::{Settling, Sim, key};
use crate::store::Store;

/// How long a peer waits before it asks again a payer that did not settle.
const ASK_EVERY_MS: u64 = Policies::DEFAULT.settlement_request_every_ms;

/// What one request has a payer over a hub with nothing held fund.
fn one_request(core: &Core) -> u64 {
    policy::requested_amount(
        u128::from(Policies::DEFAULT.peer_credit),
        core.settle.trigger,
        core.settle.minimum,
    )
}

/// [`one_request`] at the default policies.
fn one_request_default() -> u64 {
    let max_packet = Policies::DEFAULT.connector.max_packet_amount;
    policy::requested_amount(
        u128::from(Policies::DEFAULT.peer_credit),
        policy::trigger(max_packet),
        policy::minimum(max_packet),
    )
}

/// Spends what the stub holds elsewhere, behind the books, but `leave`:
/// as a hub account whose other holds took it.
fn drain(core: &mut Core, leave: u64) {
    let stub = dassie_ledger::name!("stub");
    let elsewhere = Name::new("delsewhere").unwrap();
    let scheme = &mut core.schemes.get_mut(&stub).unwrap().scheme;
    let take = scheme.balance() - leave;
    let prepared = scheme
        .prepare_settlement(&mut SeededRng::new(9), None, elsewhere, take, &[])
        .unwrap();
    scheme.execute_settlement(None, prepared.id, &mut Vec::new());
    assert_eq!(scheme.balance(), leave);
}

/// Runs a step of `core` at `at`.
fn step(core: &mut Core, at: u64) -> Vec<Output> {
    let mut outputs = Vec::new();
    core.begin_step(at);
    core.finish_step(&mut outputs);
    outputs
}

/// Whether `outputs` say a request from `peer` found us short.
fn reported_short(outputs: &[Output], peer: NodeId) -> bool {
    outputs.iter().any(|output| {
        matches!(output,
            Output::Event(Event::Settlement(SettlementEvent::RequestShort { node, .. }))
                if *node == peer)
    })
}

// The stall: asked with nothing to spend, a payer answers that nothing is
// on its way, and funds the hold as soon as the owner's deposit lands, a
// few seconds later, rather than when its payee asks again a minute on.
#[test]
fn a_request_the_payer_could_not_fund_is_funded_once_it_can() {
    let (mut core, peer) = hub_core_with_downlink(81);
    let stub = scheme_id("stub");
    drain(&mut core, 0);
    core.begin_step(NOW + 2_000);
    assert_eq!(core.settlement_requested(peer, stub), 0);
    let mut outputs = Vec::new();
    core.finish_step(&mut outputs);
    assert!(reported_short(&outputs, peer));
    assert_eq!(hub_books(&mut core, peer), (0, 0, 0));
    for at in (3_000..10_000).step_by(500) {
        step(&mut core, NOW + at);
        assert_eq!(hub_books(&mut core, peer), (0, 0, 0), "funded with nothing");
    }
    let expected = one_request(&core);
    core.begin_step(NOW + 10_000);
    core.stub_deposit(expected * 10).unwrap();
    outputs.clear();
    core.finish_step(&mut outputs);
    assert_eq!(hub_books(&mut core, peer), (expected, expected, 0));
    assert!(core.settle.unanswered.is_empty());
    assert!(!reported_short(&outputs, peer));
}

// What the scheme can spend is less than the least settlement: the payer
// funds nothing now, rather than pay the hub's fee for a few drops, and
// all the request asks once the deposit lands.
#[test]
fn a_payer_funds_nothing_below_the_least_settlement() {
    let (mut core, peer) = hub_core_with_downlink(82);
    let stub = scheme_id("stub");
    let least = core.settle.minimum;
    drain(&mut core, least / 2);
    core.begin_step(NOW + 2_000);
    let before = core.ledger.next_transfer_id();
    assert_eq!(core.settlement_requested(peer, stub), 0);
    assert_eq!(core.ledger.next_transfer_id(), before, "booked nothing");
    let mut outputs = Vec::new();
    core.finish_step(&mut outputs);
    assert_eq!(hub_books(&mut core, peer), (0, 0, 0));
    let expected = one_request(&core);
    core.begin_step(NOW + 3_000);
    core.stub_deposit(expected).unwrap();
    outputs.clear();
    core.finish_step(&mut outputs);
    assert_eq!(hub_books(&mut core, peer), (expected, expected, 0));
}

// A payer that can fund only part funds that part at once, and nothing
// else on the method while that part is on its way, or while its message
// is, which the downlink here never acknowledges: the request is kept for
// the rest.
#[test]
fn nothing_more_goes_while_the_funded_part_is_on_its_way() {
    let (mut core, peer) = hub_core_with_downlink(83);
    let stub = scheme_id("stub");
    let expected = one_request(&core);
    let part = expected / 2;
    assert!(part >= core.settle.minimum);
    drain(&mut core, part);
    core.begin_step(NOW + 2_000);
    assert_eq!(core.settlement_requested(peer, stub), part);
    core.stub_deposit(expected * 10).unwrap();
    core.answer_late();
    assert_eq!(hub_books(&mut core, peer), (part, 0, part));
    let mut outputs = Vec::new();
    core.finish_step(&mut outputs);
    for at in 3..10 {
        step(&mut core, NOW + at * 1_000);
    }
    assert_eq!(hub_books(&mut core, peer), (part, part, 0));
    assert_eq!(core.settle.unanswered.len(), 1);
}

/// Node 0's hold toward node 1 over the stub, with its settlements to it
/// on their way, and how many requests to settle node 1 sent.
fn funded_and_asked(sim: &Sim) -> (u64, u64) {
    let stub = dassie_ledger::name!("stub");
    let one = key(1).node_id();
    let hold = sim
        .core(0)
        .hub_books(stub, one)
        .map_or(0, |(credit, _, _)| credit);
    (hold, sim.core(1).peer.stats().settlements_requested)
}

/// Two nodes over the stub with commitments, node 0 holding `leave` when
/// node 1 first asks it to settle, which it cannot fund in full: then the
/// owner's deposit lands at node 0. Node 1 holds nothing either, so that
/// what it settles does not fund node 0, as the stub's settlements would.
/// Returns how long node 0 took after the deposit to fund its hold toward
/// node 1 further, what it funded in all, and how many more requests node
/// 1 sent meanwhile.
fn deposit_after_a_short_request(seed: u64, leave: u64) -> (u64, u64, u64) {
    let mut sim = Sim::with(seed, 2, 1, BTreeMap::new(), Settling::StubHub(4));
    sim.loss_per_mille = 0;
    sim.step(0, |core| drain(core, leave));
    sim.step(1, |core| drain(core, 0));
    let short = |sim: &Sim| !sim.core(0).settle.unanswered.is_empty();
    assert!(sim.run_until_or(sim.now + 600_000, short), "never asked");
    // What it funded lands, and its message arrives.
    sim.run_until(sim.now + 5_000);
    let (part, asked) = funded_and_asked(&sim);
    assert_eq!(part, leave);
    let deposited_at = sim.now;
    sim.step(0, |core| {
        let deposit = one_request(core) * 10;
        core.stub_deposit(deposit).unwrap();
    });
    let more = |sim: &Sim| funded_and_asked(sim).0 > part;
    assert!(
        sim.run_until_or(deposited_at + ASK_EVERY_MS, more),
        "funded only when asked again"
    );
    let took = sim.now - deposited_at;
    sim.run_until(sim.now + 10_000);
    let (funded, asked_since) = funded_and_asked(&sim);
    (took, funded, asked_since - asked)
}

// The stall, between two nodes: node 1 asks node 0, whose hub account
// has nothing to spend, to settle, and is told nothing is on its way. Node
// 0 funds its hold toward node 1 as soon as the owner's deposit lands, not
// when node 1 asks again a minute on: node 1 sends no other request.
#[test]
fn an_empty_payer_funds_its_payee_once_its_deposit_lands() {
    let (took, funded, asked) = deposit_after_a_short_request(91, 0);
    assert!(took < 1_000, "took {took} ms");
    assert_eq!(funded, one_request_default());
    assert_eq!(asked, 0, "asked again");
}

// A payer that could fund only part funds that part at once, and the rest
// once that part landed and the deposit did, as the rule then stands with
// the part paid: no second request.
#[test]
fn a_part_funded_request_is_topped_up_once_funds_arrive() {
    let part = one_request_default() / 2;
    let (took, funded, asked) = deposit_after_a_short_request(92, part);
    assert!(took < 1_000, "took {took} ms");
    let max_packet = Policies::DEFAULT.connector.max_packet_amount;
    let rest = policy::requested_amount(
        u128::from(Policies::DEFAULT.peer_credit - part),
        policy::trigger(max_packet),
        policy::minimum(max_packet),
    );
    assert!(rest > 0);
    assert_eq!(funded, part + rest);
    assert_eq!(asked, 0, "asked again");
}

// A payer that stays empty tries nothing, books nothing and asks its
// scheme for nothing at each step, and forgets the request once its asker
// would ask again: funds that arrive after that settle nothing, since a
// node never settles on its own.
#[test]
fn an_empty_payer_tries_nothing_and_forgets_the_request() {
    let (mut core, peer) = hub_core_with_downlink(84);
    let stub = scheme_id("stub");
    drain(&mut core, 0);
    let asked_at = NOW + 2_000;
    core.begin_step(asked_at);
    assert_eq!(core.settlement_requested(peer, stub), 0);
    let mut outputs = Vec::new();
    core.finish_step(&mut outputs);
    let booked = core.ledger.next_transfer_id();
    let mut at = asked_at;
    while at + 1_000 < asked_at + ASK_EVERY_MS {
        at += 1_000;
        step(&mut core, at);
        assert_eq!(core.ledger.next_transfer_id(), booked, "booked at {at}");
        assert_eq!(core.settle.unanswered.len(), 1);
    }
    step(&mut core, asked_at + ASK_EVERY_MS);
    assert!(core.settle.unanswered.is_empty(), "kept past the interval");
    core.begin_step(asked_at + ASK_EVERY_MS + 1_000);
    core.stub_deposit(one_request(&core) * 10).unwrap();
    outputs.clear();
    core.finish_step(&mut outputs);
    for at in 2..20 {
        step(&mut core, asked_at + ASK_EVERY_MS + at * 1_000);
    }
    assert_eq!(hub_books(&mut core, peer), (0, 0, 0), "settled on its own");
}

// Asked again while its late settlement is on its way, a payer answers
// with it and makes no other; once it landed, the request is answered,
// and nothing more is settled however long the node runs.
#[test]
fn a_late_answer_settles_once() {
    let (mut core, peer) = hub_core_with_downlink(85);
    let stub = scheme_id("stub");
    drain(&mut core, 0);
    core.begin_step(NOW + 2_000);
    assert_eq!(core.settlement_requested(peer, stub), 0);
    let mut outputs = Vec::new();
    core.finish_step(&mut outputs);
    let expected = one_request(&core);
    core.begin_step(NOW + 3_000);
    core.stub_deposit(expected * 10).unwrap();
    core.answer_late();
    assert_eq!(hub_books(&mut core, peer), (expected, 0, expected));
    let pending = core.ledger.pending_transfers().count();
    assert_eq!(core.settlement_requested(peer, stub), expected);
    core.answer_late();
    assert_eq!(core.ledger.pending_transfers().count(), pending);
    outputs.clear();
    core.finish_step(&mut outputs);
    for at in 4..30 {
        step(&mut core, NOW + at * 1_000);
    }
    assert_eq!(hub_books(&mut core, peer), (expected, expected, 0));
    assert!(core.settle.unanswered.is_empty());
}

// A crash after the late settlement was booked, before it executed: the
// restart cancels it, as any settlement cut short, and forgets the
// request, so nothing goes unless the peer asks again, and then once.
#[test]
fn a_late_answer_cut_short_by_a_crash_settles_once() {
    let store = Arc::new(Store::open_in_memory().unwrap());
    let (mut core, peer) = hub_core_with_downlink_on(Arc::clone(&store), 86);
    let stub = scheme_id("stub");
    drain(&mut core, 0);
    core.begin_step(NOW + 2_000);
    assert_eq!(core.settlement_requested(peer, stub), 0);
    let mut outputs = Vec::new();
    core.finish_step(&mut outputs);
    let expected = one_request(&core);
    core.begin_step(NOW + 3_000);
    core.stub_deposit(expected * 10).unwrap();
    core.answer_late();
    assert_eq!(hub_books(&mut core, peer), (expected, 0, expected));
    // On disk, and then the crash: never executed.
    core.flush();
    drop(core);
    assert_eq!(store.load_ledger().unwrap().pending.len(), 1);
    let mut core = core_on(Arc::clone(&store), 87);
    assert!(core.settle.unanswered.is_empty());
    for at in 4..20 {
        step(&mut core, NOW + at * 1_000);
    }
    assert_eq!(hub_books(&mut core, peer), (0, 0, 0));
    core.begin_step(NOW + 20_000);
    assert_eq!(core.settlement_requested(peer, stub), expected);
    outputs.clear();
    core.finish_step(&mut outputs);
    for at in 21..40 {
        step(&mut core, NOW + at * 1_000);
    }
    assert_eq!(hub_books(&mut core, peer), (expected, expected, 0));
}
