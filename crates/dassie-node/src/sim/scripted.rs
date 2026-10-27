//! The node simulation's scripted runs: the network forms, a node crashes
//! and comes back, payments cross it while another node crashes, an owner
//! ends a peering, and the books are checked. Over the stub with unlimited
//! trust, where nothing settles, since what `node-stub` runs is crashes,
//! restarts and payments; and over the stub with commitments, as a hub's
//! members settle. The runs of owner books on the fake XRP Ledger
//! (`owner_books.rs`) follow the same script, with the owner's payments
//! out of its books and the issuer's acts while payments are under way.

use std::collections::BTreeMap;

use dassie_connector::SpendingCap;
use dassie_kit::sim::{Outcome, Run, SeededRng};

use super::checks::{check, check_events, check_settled, check_withdrawn};
use super::{START, Settling, Sim, key};
use crate::node::Policies;
use crate::store::OutgoingState;

/// How long a network may take to be ready, at production timings: nodes
/// find each other through lists polled once a minute, and a peering that
/// failed is tried again after a backoff.
pub(super) const READY_WITHIN_MS: u64 = 10 * 60_000;

/// How long the payments of a run may take after the crash in it, at
/// production timings: a busy link waits for its settlements to reach the
/// ledger.
pub(super) const PAID_WITHIN_MS: u64 = 10 * 60_000;

/// Whether the network is ready for payments: every node has a peer and a
/// route to every other node, and none waits for a peer to settle.
pub(super) fn ready(sim: &Sim) -> bool {
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
pub(super) fn paid(sim: &Sim) -> bool {
    sim.payments.iter().all(|payment| payment.done)
        && sim.outgoing.iter().all(|&(from, _, _, outgoing)| {
            sim.nodes[from]
                .store
                .outgoing_payment(outgoing)
                .unwrap()
                .is_some_and(|row| row.state != OutgoingState::Sending)
        })
}

/// What a run of the script draws first: its generator, how many nodes,
/// and how many of them are bootstrap nodes.
pub(super) fn size(seed: u64) -> (SeededRng, usize, usize) {
    let mut rng = SeededRng::new(seed ^ 0xfeed);
    let count = 2 + usize::try_from(rng.next_u64() % 4).unwrap();
    let bootstraps = 1 + usize::try_from(rng.next_u64() % 2).unwrap().min(count - 1);
    (rng, count, bootstraps)
}

/// What a seed draws for a run of the script, before it starts.
#[must_use]
pub fn describe(seed: u64) -> String {
    let (_, count, bootstraps) = size(seed);
    format!("{count} nodes, {bootstraps} bootstrap")
}

/// A run over the stub with unlimited trust: its subject is crashes,
/// restarts and payments, so no request to settle or settlement shapes it
/// (spec/areas/testing.md#levels).
#[must_use]
pub fn run(seed: u64) -> Run {
    run_on(seed, Settling::Trusting)
}

/// The room a run over the stub with commitments draws: 1, 2, 4 or 8
/// packets in flight per peer each way. Its own generator, so that it
/// draws nothing else's.
fn hub_limit(seed: u64) -> usize {
    1 << SeededRng::new(seed ^ 0x4b0b).below(4)
}

/// What a seed draws for a run over the stub with commitments, before it
/// starts.
#[must_use]
pub fn describe_stub_hub(seed: u64) -> String {
    format!(
        "{}, room for {} packets per peer each way",
        describe(seed),
        hub_limit(seed)
    )
}

/// A run over the stub with commitments: its packets take the path over a
/// settlement hub (spec/areas/hubs.md#in-the-node).
#[must_use]
pub fn run_stub_hub(seed: u64) -> Run {
    run_on(seed, Settling::StubHub(hub_limit(seed)))
}

/// A run settling over `settling`, which holds one ledger.
#[must_use]
pub fn run_on(seed: u64, settling: Settling) -> Run {
    let (mut rng, count, bootstraps) = size(seed);
    let mut sim = Sim::with(seed, count, bootstraps, BTreeMap::new(), settling);
    let result = script(&mut sim, &mut rng, |_, _| {})
        .and_then(|()| check(&sim))
        .and_then(|()| {
            check_withdrawn(&sim).map_err(|problem| Outcome::liveness("withdrawn", problem))
        });
    ended(sim, result)
}

/// How a run ended: an error a node reported fails it before anything
/// else, since the run went on past it.
pub(super) fn ended(mut sim: Sim, result: Result<(), Outcome>) -> Run {
    let outcome = match check_events(&sim) {
        Err(problem) => Outcome::safety("events", problem),
        Ok(()) => result.err().unwrap_or(Outcome::Pass),
    };
    // Judged: the harness's own check, when it drops, has nothing to add.
    sim.errors.clear();
    sim.count_xrpl();
    let mut run = Run::new(outcome, std::mem::take(&mut sim.trace));
    if !sim.withdrawn.is_empty() {
        let withdrawn = u64::try_from(sim.withdrawn.len()).unwrap_or(u64::MAX);
        sim.counts
            .insert("peers withdrawn short of room", withdrawn);
    }
    run.counts = std::mem::take(&mut sim.counts);
    run
}

/// The script: the network forms, a node crashes and comes back, the
/// network forms again, payments start, and `while_paying` acts while they
/// are under way; then a node crashes mid-payment, and an owner ends a
/// peering.
pub(super) fn script(
    sim: &mut Sim,
    rng: &mut SeededRng,
    while_paying: impl FnOnce(&mut Sim, &mut SeededRng),
) -> Result<(), Outcome> {
    let count = sim.nodes.len();
    if !sim.run_until_or(START + READY_WITHIN_MS, ready) {
        return Err(Outcome::liveness(
            "ready",
            "the network was not ready in time",
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
        return Err(Outcome::safety(
            "restart",
            format!("node {victim} lost its peerings or balances in a restart"),
        ));
    }
    sim.loss_per_mille = 0;
    // A node takes nothing dated before its start, and the clocks are up
    // to 20 s apart: its peers reach it again only after that.
    sim.run_until(sim.now + 60_000);
    let deadline = sim.now + READY_WITHIN_MS;
    if !sim.run_until_or(deadline, ready) {
        for index in 0..count {
            if sim.peers(index).is_empty() {
                return Err(Outcome::liveness(
                    "ready-again",
                    format!("node {index} has no peer"),
                ));
            }
        }
        return Err(Outcome::liveness(
            "ready-again",
            "the network was not ready again in time",
        ));
    }
    pay(sim, rng);
    // A node's connection to its ledger drops while it pays.
    if sim.settling.on_xrpl() {
        sim.run_until(sim.now + rng.next_u64() % 4_000);
        let dropped = usize::try_from(rng.next_u64() % count as u64).unwrap();
        sim.drop_ledger_link(dropped);
    }
    while_paying(sim, rng);
    crash_mid_payment(sim, rng, 120_000);
    terminate(sim, rng)
}

/// Payments of up to three times the default credit, one with a capped
/// token, and outgoing payments, to another node and to the node's own
/// receiver.
fn pay(sim: &mut Sim, rng: &mut SeededRng) {
    let count = sim.nodes.len();
    for round in 0..3 {
        let from = usize::try_from(rng.next_u64() % count as u64).unwrap();
        let to = (from + 1 + usize::try_from(rng.next_u64() % (count as u64 - 1)).unwrap()) % count;
        // Up to three times the default credit: where peers settle, only
        // settling lets it through.
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

/// How long ending a peering may take (spec/decisions/ending-a-peering.md):
/// waiting for our own settlements, then for what the peer has on its way,
/// each at most the wait, then telling it, then the packets in flight.
const ENDED_WITHIN_MS: u64 = 2 * Policies::DEFAULT.peer.depeer_wait_ms
    + Policies::DEFAULT.peer.termination_timeout_ms
    + Policies::DEFAULT.connector.max_hold_ms;

/// An owner ends a peering: its node collects first, then tells the peer,
/// and it ends on both sides. Neither side settles anything once it knows
/// (spec/decisions/ending-a-peering.md): what the books show stays owed, as
/// the check of each node's books then sees.
fn terminate(sim: &mut Sim, rng: &mut SeededRng) -> Result<(), Outcome> {
    let count = sim.nodes.len();
    let ender = usize::try_from(rng.next_u64() % count as u64).unwrap();
    let Some(peer) = sim.peers(ender).into_iter().next() else {
        return Ok(());
    };
    sim.trace
        .push(format!("{} {ender} terminates {peer}", sim.now));
    let mark = sim.trace.len();
    sim.step(ender, |core| core.terminate_peering(peer).unwrap());
    let other = (0..count).find(|i| key(*i).node_id() == peer).unwrap();
    let ender_id = key(ender).node_id();
    let ended =
        |sim: &Sim| !sim.peers(ender).contains(&peer) && !sim.peers(other).contains(&ender_id);
    if !sim.run_until_or(sim.now + ENDED_WITHIN_MS, ended) {
        return Err(Outcome::liveness(
            "peering-ended",
            format!(
                "node {ender}'s peering with node {other} did not end within {} s",
                ENDED_WITHIN_MS / 1_000
            ),
        ));
    }
    // Then a minute's calm, so that the checks find the rest of the network
    // settled.
    sim.run_until(sim.now + 60_000);
    // The node ending it settles nothing from its decision on, the other
    // from when it is told.
    settled_nothing_once_ending(sim, mark, ender, other, "Depeering")?;
    settled_nothing_once_ending(sim, mark, other, ender, "Terminating")?;
    collected_before_telling(sim, mark, sim.trace.len(), ender, other)
}

/// What node `with` said it had on its way, when node `index`, ending their
/// peering, asked it a last time, was booked before `index` told it, unless
/// `index` waited the whole time for it first
/// (spec/decisions/ending-a-peering.md): between honest nodes, no settlement
/// crosses the end. Booked, not announced: on the XRP Ledger the ledger may
/// show it before its message arrives, or after, and even before `index`
/// decided to end the peering. Only if `index` told it before `until`, and
/// `with` was not ending the peering too: both ending it at once, each
/// answers the other as one ending does, and either may tell the other
/// without waiting for what was on its way, which is then credited once after
/// the end. One whose owner bans the peer from `until` on also tells it at
/// once.
pub(super) fn collected_before_telling(
    sim: &Sim,
    mark: usize,
    until: usize,
    index: usize,
    with: usize,
) -> Result<(), Outcome> {
    let peer = format!("{:?}", key(with).node_id());
    let trace = &sim.trace;
    let answered =
        format!(" {index} PeerNode(SettlementRequestAnswered {{ peer: {peer}, settling: ");
    let telling = format!(" {index} PeerNode(Terminating {{ peer: {peer}");
    let Some(told) = trace[mark..until_peered(&trace[..until], mark, index, with)]
        .iter()
        .position(|line| line.contains(&telling) && line.contains("by_us: true"))
        .map(|told| mark + told)
    else {
        return Ok(());
    };
    let own = format!("{:?}", key(index).node_id());
    let peered = |node: usize, other: &str| format!(" {node} PeerNode(Peered {{ node: {other}");
    let since = [peered(index, &peer), peered(with, &own)]
        .iter()
        .filter_map(|peered| trace[..mark].iter().rposition(|line| line.contains(peered)))
        .max()
        .unwrap_or(0);
    let crossing = [
        format!(" {with} PeerNode(Depeering {{ peer: {own}"),
        format!(" {with} PeerNode(Terminating {{ peer: {own}"),
    ];
    if trace[since..told]
        .iter()
        .any(|line| crossing.iter().any(|ending| line.contains(ending)))
    {
        return Ok(());
    }
    let Some((at, promised)) = trace[mark..told]
        .iter()
        .enumerate()
        .rev()
        .find_map(|(at, line)| {
            let rest = &line[line.find(&answered)? + answered.len()..];
            Some((mark + at, rest[..rest.find(' ')?].parse::<u64>().ok()?))
        })
    else {
        return Ok(());
    };
    // Telling once the wait from the ask has passed is the rule too: the
    // settlement may still be on its way, or booked before the ask, which
    // the node does not count.
    let asking = format!(" {index} asks {with} to settle");
    let asked = trace[mark..at]
        .iter()
        .rev()
        .find(|line| line.contains(&asking))
        .and_then(|line| trace_ms(line));
    if let (Some(asked), Some(told_ms)) = (asked, trace_ms(&trace[told]))
        && told_ms >= asked.saturating_add(sim.peer.depeer_wait_ms)
    {
        return Ok(());
    }
    let booking = format!(" {index} Settlement(SettlementBooked {{ from: {peer}, amount: ");
    // Counted from the decision, or from when `with` started the settlement
    // it named, if earlier: the settlement may be booked before the answer
    // that names it, and even before the decision.
    let decided = format!(" {index} PeerNode(Depeering {{ peer: {peer}");
    let decision = trace[mark..at]
        .iter()
        .rposition(|line| line.contains(&decided))
        .map_or(at, |decision| mark + decision);
    let started = format!(
        " {with} Settlement(Settling {{ node: {:?}, amount: {promised}, ",
        key(index).node_id()
    );
    let from = trace[..at]
        .iter()
        .rposition(|line| line.contains(&started))
        .map_or(decision, |started| started.min(decision));
    let booked: u64 = trace[from..told]
        .iter()
        .filter_map(|line| {
            let rest = &line[line.find(&booking)? + booking.len()..];
            rest[..rest.find(' ')?].parse::<u64>().ok()
        })
        .sum();
    if booked >= promised {
        return Ok(());
    }
    Err(Outcome::liveness(
        "collected-before-telling",
        format!(
            "node {index} told node {with} the peering ends before booking the {promised} it had on its way"
        ),
    ))
}

/// Where, in `lines` from `start` on, node `index` peers with node `with`
/// again, or the end of `lines`: a later peering settles what the books
/// show as usual (spec/decisions/ending-a-peering.md).
pub(super) fn until_peered(lines: &[String], start: usize, index: usize, with: usize) -> usize {
    let peered = format!(
        " {index} PeerNode(Peered {{ node: {:?}",
        key(with).node_id()
    );
    lines[start..]
        .iter()
        .position(|line| line.contains(&peered))
        .map_or(lines.len(), |at| start + at)
}

/// When a line of the trace happened.
pub(super) fn trace_ms(line: &str) -> Option<u64> {
    line.split(' ').next()?.parse().ok()
}

/// Node `index` started no settlement to node `with` from its `event` on:
/// from when it decided, or learned, that their peering ends
/// (spec/decisions/ending-a-peering.md), until they peer again, if they do.
/// An honest node paying then is money the rule says it keeps.
pub(super) fn settled_nothing_once_ending(
    sim: &Sim,
    mark: usize,
    index: usize,
    with: usize,
    event: &str,
) -> Result<(), Outcome> {
    let peer = format!("{:?}", key(with).node_id());
    let ending = format!(" {index} PeerNode({event} {{ peer: {peer}");
    let settling = format!(" {index} Settlement(Settling {{ node: {peer}");
    let lines = &sim.trace[mark..];
    let Some(start) = lines.iter().position(|line| line.contains(&ending)) else {
        return Ok(());
    };
    let ending = &lines[start..until_peered(lines, start, index, with)];
    match ending.iter().find(|line| line.contains(&settling)) {
        Some(line) => Err(Outcome::safety(
            "settled-once-ending",
            format!("node {index} settled with node {with} while their peering ended: {line}"),
        )),
        None => Ok(()),
    }
}
