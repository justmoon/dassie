//! The node's own swaps with a peer (spec/decisions/peer-swaps.md,
//! spec/areas/connector.md#swaps): node 0 pays node 1 on one method of
//! their peering from its swap endpoint, node 1 converts it as transit and
//! pays it back on the method its address names, and node 0's swap
//! endpoint fulfills it from its data, with nothing kept per swap. Two
//! nodes run the stub with commitments, in dollars, and
//! [`StubTwo`](super::StubTwo), in XRP in a hub; the books balance after
//! every step, as the simulation checks.

use std::collections::BTreeMap;

use dassie_ilp::ErrorCode;
use dassie_ledger::LedgerId;

use super::STUB_TWO;
use crate::engine::core::{NewSwap, SwapOutcome};
use crate::sim::{START, Settling, Sim, key};

const USD: LedgerId = LedgerId::StubUsd;
/// [`StubTwo`](super::StubTwo)'s ledger.
const HUB: LedgerId = LedgerId::HubXrplTestnetXrp;

/// Two nodes peered over the stub and [`StubTwo`](super::StubTwo), each
/// hold funded, with the rates both learn, so a swap converts.
pub(super) fn pair(seed: u64) -> Sim {
    pair_with(seed, 4)
}

/// [`pair`], each stub taking up to `commitments` of a peer's packets.
pub(super) fn pair_with(seed: u64, commitments: usize) -> Sim {
    let mut sim = Sim::with(seed, 2, 1, BTreeMap::new(), Settling::StubHub(commitments));
    sim.loss_per_mille = 0;
    for index in 0..2 {
        let now = sim.now;
        sim.step(index, |core| {
            let config = format!(r#"{{"commitments":{commitments}}}"#);
            core.add_scheme(STUB_TWO, config).unwrap();
            let rates = crate::sim::RATES.map(|(code, rate)| (code, rate, now));
            core.set_rates(&rates);
        });
    }
    let ready = |sim: &Sim| {
        [(0, 1), (1, 0)].into_iter().all(|(a, b)| {
            let other = key(b).node_id();
            let methods = sim
                .core(a)
                .peer
                .peering_of(&other)
                .map_or(0, |peering| peering.methods.len());
            let held = ["stub", STUB_TWO].iter().all(|scheme| {
                let name = dassie_ledger::Name::new(scheme).unwrap();
                sim.core(a)
                    .hub_books(name, other)
                    .is_some_and(|(credit, _, _)| credit > 0)
            });
            methods == 2 && held
        })
    };
    assert!(
        sim.run_until_or(START + 1_200_000, ready),
        "the pair did not peer on both hubs"
    );
    sim
}

/// Node 0 swaps `amount` on USD with node 1, to come back in the hub,
/// at least `minimum`, and the outcome, once there is one.
fn swap(sim: &mut Sim, amount: u64, minimum: u64) -> SwapOutcome {
    let one = key(1).node_id();
    let mut sent = None;
    sim.step(0, |core| {
        let swap = NewSwap {
            peer: one,
            pay_on: USD,
            back_on: HUB,
            amount,
            minimum,
            timeout_ms: 20_000,
        };
        sent = Some(core.send_swap(swap).unwrap());
    });
    let sent = sent.unwrap();
    let deadline = sim.now + 60_000;
    loop {
        let mut outcome = None;
        sim.step(0, |core| outcome = core.poll_swap_outcome());
        if let Some(outcome) = outcome {
            assert_eq!(outcome.request_id, sent.request_id);
            assert_eq!(outcome.ledger, USD);
            return outcome;
        }
        assert!(sim.now < deadline, "the swap did not end");
        sim.run_until(sim.now + 100);
    }
}

/// What the owner of node `index` holds on each ledger.
pub(super) fn holds(sim: &Sim, index: usize) -> (u64, u64) {
    let core = sim.core(index);
    (core.owner_available(USD), core.owner_available(HUB))
}

// A swap goes out in dollars and comes back in XRP: node 0's owner gave
// dollars for XRP, node 1's took them, each at node 1's curve.
#[test]
fn a_swap_pays_one_asset_for_another() {
    let mut sim = pair(81);
    let (usd_0, hub_0) = holds(&sim, 0);
    let (usd_1, hub_1) = holds(&sim, 1);
    let amount = 1_000_000;
    let outcome = swap(&mut sim, amount, 1);
    assert_eq!(outcome.result, Ok(()));
    let (usd_0_after, hub_0_after) = holds(&sim, 0);
    let (usd_1_after, hub_1_after) = holds(&sim, 1);
    assert_eq!(usd_0 - usd_0_after, amount, "node 0 paid in dollars");
    let got = hub_0_after - hub_0;
    assert!(got > 0, "node 0 got no XRP");
    assert_eq!(usd_1_after - usd_1, amount, "node 1 took the dollars");
    assert_eq!(hub_1 - hub_1_after, got, "node 1 paid the XRP");
    // Its fee counts toward no hop's weight on node 1.
    let core = sim.core(1);
    let zero = key(0).node_id();
    let endpoint = dassie_connector::EndpointId::Peer(core.peer_endpoints[&zero]);
    for ledger in [USD, HUB] {
        let earned = core.connector.earnings(endpoint, ledger, core.now_ms());
        assert_eq!(earned, Some(0), "weight on {ledger:?}");
    }
    crate::sim::checks::check_hub_packets(&sim).unwrap();
}

// A swap that brings back less than its minimum is node 0's to reject
// (F04), on the way back, and both legs leave the books as they were.
#[test]
fn a_swap_below_its_minimum_is_rejected() {
    // A seed whose run has both holds funded when the swap goes: in others
    // a leg finds its method short of funds (T04), which this test is not
    // about.
    let mut sim = pair(85);
    let before = (holds(&sim, 0), holds(&sim, 1));
    let outcome = swap(&mut sim, 1_000_000, u64::MAX);
    assert_eq!(
        outcome.result,
        Err(ErrorCode::F04_INSUFFICIENT_DESTINATION_AMOUNT)
    );
    assert_eq!((holds(&sim, 0), holds(&sim, 1)), before);
}

// A node that offers no swaps refuses them (F02), and so does every node
// for a swap that names the method it is paid on.
#[test]
fn a_node_that_offers_no_swaps_refuses_them() {
    let mut sim = pair(83);
    sim.step(1, |core| core.connector.set_offer_swaps(false));
    let before = holds(&sim, 0);
    let outcome = swap(&mut sim, 1_000_000, 1);
    assert_eq!(outcome.result, Err(ErrorCode::F02_UNREACHABLE));
    assert_eq!(holds(&sim, 0), before);
    // One back on its own method never leaves node 0.
    let one = key(1).node_id();
    let mut outcome = None;
    sim.step(0, |core| {
        let swap = NewSwap {
            peer: one,
            pay_on: USD,
            back_on: USD,
            amount: 1_000,
            minimum: 1,
            timeout_ms: 20_000,
        };
        core.send_swap(swap).unwrap();
        outcome = core.poll_swap_outcome();
    });
    let outcome = outcome.unwrap();
    assert_eq!(outcome.result, Err(ErrorCode::F02_UNREACHABLE));
    assert!(sim.core(0).peer.peering_of(&one).is_some());
}
