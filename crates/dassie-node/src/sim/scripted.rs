//! The node simulation's scripted runs: the network forms, a node crashes
//! and comes back, payments cross it while another node crashes, an owner
//! ends a peering, and the books are checked. Over the stub, over the XRP
//! Ledger, and across currencies (M7c).

use std::collections::BTreeMap;

use dassie_connector::SpendingCap;
use dassie_node_core::testing::{Outcome, Run, SeededRng};

use super::checks::{check, check_across, check_events, check_settled, check_withdrawn};
use super::{START, Settling, Sim, key};
use crate::store::OutgoingState;

/// How long a network may take to be ready, at production timings: nodes
/// find each other through lists polled once a minute, and a peering that
/// failed is tried again after a backoff.
const READY_WITHIN_MS: u64 = 10 * 60_000;

/// How long the payments of a run may take after the crash in it, at
/// production timings: a busy link waits for its settlements to reach the
/// ledger.
const PAID_WITHIN_MS: u64 = 10 * 60_000;

/// Whether the network is ready for payments: every node has a peer and a
/// route to every other node, and none waits for a peer to settle, so a
/// junior has paid its senior in advance.
fn ready(sim: &Sim) -> bool {
    let count = sim.nodes.len();
    let routed = (0..count).all(|index| {
        let network = sim.core(index).network();
        !network.peers.is_empty()
            && (0..count).filter(|other| *other != index).all(|other| {
                let id = key(other).node_id();
                network
                    .nodes
                    .iter()
                    .any(|node| node.id == id && !node.first_hops.is_empty())
            })
    });
    routed && check_settled(sim).is_ok()
}

/// Whether every payment of the run has finished, one way or the other.
fn paid(sim: &Sim) -> bool {
    sim.payments.iter().all(|payment| payment.done)
        && sim.outgoing.iter().all(|&(from, _, _, outgoing)| {
            sim.nodes[from]
                .store
                .outgoing_payment(outgoing)
                .unwrap()
                .is_some_and(|row| row.state != OutgoingState::Sending)
        })
}

/// What a run over one ledger draws first: its generator, how many nodes,
/// and how many of them are bootstrap nodes.
fn size(seed: u64) -> (SeededRng, usize, usize) {
    let mut rng = SeededRng::new(seed ^ 0xfeed);
    let count = 2 + usize::try_from(rng.next_u64() % 4).unwrap();
    let bootstraps = 1 + usize::try_from(rng.next_u64() % 2).unwrap().min(count - 1);
    (rng, count, bootstraps)
}

/// What a run across currencies draws first: its generator and how many
/// nodes. Two of them are bootstrap nodes.
fn size_across(seed: u64) -> (SeededRng, usize) {
    let mut rng = SeededRng::new(seed ^ 0xc055);
    let count = 3 + usize::try_from(rng.next_u64() % 2).unwrap();
    (rng, count)
}

/// What a seed draws for a run over one ledger, before it starts.
#[must_use]
pub fn describe(seed: u64) -> String {
    let (_, count, bootstraps) = size(seed);
    format!("{count} nodes, {bootstraps} bootstrap")
}

/// What a seed draws for a run across currencies, before it starts.
#[must_use]
pub fn describe_across(seed: u64) -> String {
    let (_, count) = size_across(seed);
    format!("{count} nodes, 2 bootstrap, dollars and XRP")
}

/// A run over the stub.
#[must_use]
pub fn run(seed: u64) -> Run {
    run_on(seed, Settling::Stub)
}

/// A run over the XRP Ledger testnet scheme, against the fake ledger.
#[must_use]
pub fn run_xrpl(seed: u64) -> Run {
    run_on(seed, Settling::Xrpl)
}

/// A run settling over `settling`, which holds one ledger.
#[must_use]
pub fn run_on(seed: u64, settling: Settling) -> Run {
    let (mut rng, count, bootstraps) = size(seed);
    let mut sim = Sim::with(seed, count, bootstraps, BTreeMap::new(), settling);
    let result = script(&mut sim, &mut rng)
        .and_then(|()| check(&sim))
        .and_then(|()| check_withdrawn(&sim).map_err(Outcome::Liveness));
    ended(sim, result)
}

/// A run across currencies (M7c): node 0 has dollars, node 2 XRP, nodes 1
/// and 3 both, node 1 charging for what passes. Clients pay receivers in
/// the other currency, an outgoing payment crosses too, and a node
/// crashes mid-payment.
#[must_use]
pub fn run_across(seed: u64) -> Run {
    let (mut rng, count) = size_across(seed);
    let mut sim = Sim::with(seed, count, 2, BTreeMap::new(), Settling::Cross);
    script_across(&mut sim, &mut rng);
    let result = check_across(&sim).and_then(|()| check_withdrawn(&sim).map_err(Outcome::Liveness));
    ended(sim, result)
}

/// How a run ended: an error a node reported fails it before anything
/// else, since the run went on past it.
pub(super) fn ended(mut sim: Sim, result: Result<(), Outcome>) -> Run {
    let outcome = match check_events(&sim) {
        Err(problem) => Outcome::Safety(problem),
        Ok(()) => result.err().unwrap_or(Outcome::Pass),
    };
    // Judged: the harness's own check, when it drops, has nothing to add.
    sim.errors.clear();
    let mut run = Run::new(outcome, std::mem::take(&mut sim.trace));
    run.counts = std::mem::take(&mut sim.counts);
    run
}

fn script(sim: &mut Sim, rng: &mut SeededRng) -> Result<(), Outcome> {
    let count = sim.nodes.len();
    if !sim.run_until_or(START + READY_WITHIN_MS, ready) {
        return Err(Outcome::Liveness(
            "the network was not ready in time".into(),
        ));
    }
    // A node crashes; what it persisted brings it back.
    let victim = usize::try_from(rng.next_u64() % count as u64).unwrap();
    let before = sim.peers(victim);
    let balances = sim.balances(victim);
    sim.crash(victim);
    sim.run_until(sim.now + 5_000);
    sim.start(victim);
    if !before.is_subset(&sim.peers(victim)) || sim.balances(victim) != balances {
        return Err(Outcome::Safety(format!(
            "node {victim} lost its peerings or balances in a restart"
        )));
    }
    sim.loss_per_mille = 0;
    // A node takes nothing dated before its start, and the clocks are up
    // to 20 s apart: its peers reach it again only after that.
    sim.run_until(sim.now + 60_000);
    let deadline = sim.now + READY_WITHIN_MS;
    if !sim.run_until_or(deadline, ready) {
        for index in 0..count {
            if sim.peers(index).is_empty() {
                return Err(Outcome::Liveness(format!("node {index} has no peer")));
            }
        }
        return Err(Outcome::Liveness(
            "the network was not ready again in time".into(),
        ));
    }
    pay(sim, rng);
    // A node's connection to its ledger drops while it settles.
    if sim.settling == Settling::Xrpl {
        sim.run_until(sim.now + rng.next_u64() % 4_000);
        let dropped = usize::try_from(rng.next_u64() % count as u64).unwrap();
        sim.drop_ledger_link(dropped);
    }
    crash_mid_payment(sim, rng, 120_000);
    terminate(sim, rng)
}

/// Payments of up to three times the credit, one with a capped token, and
/// outgoing payments, to another node and to the node's own receiver.
fn pay(sim: &mut Sim, rng: &mut SeededRng) {
    let count = sim.nodes.len();
    for round in 0..3 {
        let from = usize::try_from(rng.next_u64() % count as u64).unwrap();
        let to = (from + 1 + usize::try_from(rng.next_u64() % (count as u64 - 1)).unwrap()) % count;
        // Up to three times the credit a peer extends: only settling lets
        // it through.
        let amount = 100_000_000 + rng.next_u64() % 3_300_000_000;
        // One incoming payment asks for half of what its sender has: it
        // takes that, and no more.
        let limit = (round == 1).then_some(amount / 2);
        sim.pay(from, to, amount, None, limit);
    }
    // A token capped at 0.1 USD per 10 s pays 0.4 USD: it must wait for
    // its cap to refill.
    let from = usize::try_from(rng.next_u64() % count as u64).unwrap();
    let cap = SpendingCap {
        amount: 100_000_000,
        window_ms: 10_000,
    };
    sim.pay(from, (from + 1) % count, 400_000_000, Some(cap), None);
    let from = usize::try_from(rng.next_u64() % count as u64).unwrap();
    // Two minutes in, every node's owner has funds to pay with.
    sim.pay_outgoing(
        from,
        (from + 1) % count,
        50_000_000 + rng.next_u64() % 2_000_000_000,
    )
    .unwrap();
    sim.pay_outgoing(from, from, 10_000_000 + rng.next_u64() % 100_000_000)
        .unwrap();
}

/// A node crashes mid-payment, with packets in flight: a sender, a hop or
/// a receiver. It comes back, and the network has `calm_ms`, and then up
/// to [`PAID_WITHIN_MS`] while payments are still going, to finish.
/// Whether they all did is checked at the end.
fn crash_mid_payment(sim: &mut Sim, rng: &mut SeededRng, calm_ms: u64) {
    let count = sim.nodes.len();
    sim.run_until(sim.now + 500 + rng.next_u64() % 4_000);
    let victim = usize::try_from(rng.next_u64() % count as u64).unwrap();
    sim.crash(victim);
    sim.run_until(sim.now + 1_000 + rng.next_u64() % 4_000);
    sim.start(victim);
    sim.run_until(sim.now + calm_ms);
    sim.run_until_or(sim.now + PAID_WITHIN_MS, paid);
}

/// An owner ends a peering: it drains, ends on both sides, and what is
/// owed is settled.
fn terminate(sim: &mut Sim, rng: &mut SeededRng) -> Result<(), Outcome> {
    let count = sim.nodes.len();
    let ender = usize::try_from(rng.next_u64() % count as u64).unwrap();
    let Some(peer) = sim.peers(ender).into_iter().next() else {
        return Ok(());
    };
    sim.trace
        .push(format!("{} {ender} terminates {peer}", sim.now));
    sim.step(ender, |core| core.terminate_peering(peer).unwrap());
    sim.run_until(sim.now + 60_000);
    if sim.peers(ender).contains(&peer) {
        return Err(Outcome::Liveness(format!(
            "node {ender}'s peering with {peer} did not end"
        )));
    }
    // Each side settled everything its own books say it owed the other.
    // What a side is still owed is a Fulfill the other never counted: each
    // books its own (D23).
    let other = (0..count).find(|i| key(*i).node_id() == peer).unwrap();
    let mine = sim.owes(ender, &peer);
    let theirs = sim.owes(other, &key(ender).node_id());
    // Less than the ledger can move stays owed: a drop on the XRP Ledger.
    let unit = if sim.settling == Settling::Xrpl {
        1_000
    } else {
        1
    };
    if mine >= unit || theirs >= unit {
        return Err(Outcome::Liveness(format!(
            "nodes {ender} and {other} parted owing: {mine}, {theirs}"
        )));
    }
    Ok(())
}

fn script_across(sim: &mut Sim, rng: &mut SeededRng) {
    sim.run_until(START + 60_000);
    sim.loss_per_mille = 0;
    sim.run_until(sim.now + 60_000);
    // Dollars to XRP, and back; each up to twice the credit, so the hops
    // settle on both ledgers.
    let amount = |rng: &mut SeededRng| 100_000_000 + rng.next_u64() % 2_200_000_000;
    let there = amount(rng);
    sim.pay(0, 2, there, None, None);
    let back = amount(rng);
    sim.pay(2, 0, back, None, None);
    // Into a node's own receiver in the other currency: it converts, and
    // charges nothing.
    let own = amount(rng);
    sim.pay(0, 1, own, None, None);
    let debit = 50_000_000 + rng.next_u64() % 1_000_000_000;
    sim.pay_outgoing_across(2, 0, debit);
    crash_mid_payment(sim, rng, 180_000);
}
