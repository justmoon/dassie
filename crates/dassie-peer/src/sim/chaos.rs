//! The peer network's random run (spec/areas/testing.md#the-simulations,
//! spec/decisions/random-seed-simulations.md): random sizes and bootstrap
//! lists, a rough network, a forger sending copies of requests as its own,
//! crashes, restarts and partitions, owners ending peerings and banning
//! nodes, settlements, nodes that run a second scheme and stop and start
//! either, so that peerings gain and drop settlement methods and end with
//! their last, Prepares on each method, then a calm in which the network
//! must converge, every termination finish, every settlement arrive
//! exactly once and both ends of a peering hold the same methods: one on
//! each scheme both run.

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use dassie_identity::NodeId;
use dassie_kit::sim::{Outcome, Run, SeededRng};

use super::{Faults, Network, SECOND, START, STUB, connected, scheme};
use crate::io::Event;
use crate::wire::{SchemeId, TerminationReason};
use crate::{Config, routing};

/// The production settings: on the simulated clock a day costs seconds of
/// wall time per seed, so nothing is scaled down.
const CONFIG: Config = Config::DEFAULT;

/// How long the network is rough: long enough for a termination toward a
/// peer we cannot reach to time out, a bridge to be sought, and failed
/// peerings to be retried, among the faults.
const CHAOS_MS: u64 = 5 * 60_000;
const _: () = assert!(
    CHAOS_MS > CONFIG.termination_timeout_ms
        && CHAOS_MS > CONFIG.bridge_check_ms + CONFIG.unreachable_grace_ms
        && CHAOS_MS > CONFIG.list_poll_interval_ms + CONFIG.min_backoff_ms
);

/// How long it then has to settle: past an owner's cooldown, the longest
/// backoff and the longest hold-down, so every pair may meet again. In a
/// small network, a pair an owner parted may be a node's only way to its
/// target.
const CALM_MS: u64 = CONFIG.owner_termination_cooldown_ms
    + CONFIG.max_backoff_ms
    + CONFIG.advertise_hold_max_ms
    + 10 * 60_000;
// Long enough, too, for a node ending a peering to wait out both its
// steps and tell the peer (spec/decisions/ending-a-peering.md).
const _: () = assert!(CALM_MS > 2 * CONFIG.depeer_wait_ms + CONFIG.termination_timeout_ms);

/// The seed's generator for the run, and what it draws first: how many
/// nodes, and how many of them are bootstrap nodes.
fn size(seed: u64) -> (SeededRng, usize, usize) {
    let mut rng = SeededRng::new(seed ^ 0x5eed);
    let count = 2 + usize::try_from(rng.next_u64() % 11).unwrap();
    let bootstraps = 1 + usize::try_from(rng.next_u64() % 3).unwrap().min(count - 1);
    (rng, count, bootstraps)
}

/// What the seed draws, before the run starts.
#[must_use]
pub fn describe(seed: u64) -> String {
    let (_, count, bootstraps) = size(seed);
    format!("{count} nodes, {bootstraps} bootstrap")
}

/// Runs the seed and checks the calm network it leaves, counting the
/// forger's copies, the methods added and dropped, the Prepares taken on
/// the second scheme's and those refused for a method only their sender
/// held, so that a generator that stops reaching them shows, and so do
/// methods that come apart more often than restarts and lost drops make
/// them.
#[must_use]
pub fn run(seed: u64) -> Run {
    let network = network(seed);
    let count = |n: usize| u64::try_from(n).unwrap_or(u64::MAX);
    let forgeries = count(network.forgeries);
    let added = count(events(&network, |event| {
        matches!(event, Event::MethodAdded { .. })
    }));
    let dropped = count(events(&network, |event| {
        matches!(event, Event::MethodDropped { .. })
    }));
    let second = network
        .prepares_taken
        .get(&scheme(SECOND))
        .copied()
        .unwrap_or(0);
    let one_sided = network.one_sided;
    let mut run = Run::new(check(&network), network.trace);
    run.counts.insert("forgeries", forgeries);
    run.counts.insert("methods added", added);
    run.counts.insert("methods dropped", dropped);
    run.counts.insert("prepares on a second method", second);
    run.counts
        .insert("prepares on a one-sided method", one_sided);
    run
}

/// How many of the network's events `wanted` picks.
fn events(network: &Network, wanted: impl Fn(&Event) -> bool) -> usize {
    network
        .events
        .iter()
        .filter(|(_, event)| wanted(event))
        .count()
}

/// Runs the seed: its size, its chaos, its calm.
#[must_use]
pub fn network(seed: u64) -> Network {
    let (mut rng, count, bootstraps) = size(seed);
    let mut network = Network::new(seed, count, bootstraps, CONFIG);
    // Most nodes run a second scheme too, so most pairs settle on two
    // methods; the others settle on one.
    let second: Vec<bool> = (0..count).map(|_| rng.next_u64() % 4 != 0).collect();
    for (index, runs) in second.iter().enumerate() {
        if *runs {
            network.set_running(index, scheme(SECOND), true);
        }
    }
    network.faults = Faults::ROUGH;
    let mut at = START;
    while at < START + CHAOS_MS {
        at += 2_000 + rng.next_u64() % 3_000;
        network.run_until(at);
        chaos(&mut network, &mut rng);
    }
    network.faults = Faults::NONE;
    network.cut.clear();
    for (index, runs_second) in second.iter().enumerate() {
        if !network.is_up(index) {
            network.start(index);
        }
        network.nodes[index].hooks.busy.clear();
        network.set_running(index, scheme(STUB), true);
        network.set_running(index, scheme(SECOND), *runs_second);
        let banned: Vec<NodeId> = network.node(index).bans().copied().collect();
        network.act(index, |node, now| {
            for id in &banned {
                node.unban(now, id);
            }
        });
    }
    // Halfway through the calm, a Prepare on every method: one that only
    // its sender holds, its drop lost with a restart, is told so, and asked
    // for again if both run its scheme.
    network.run_until(START + CHAOS_MS + CALM_MS / 2);
    for index in 0..count {
        prepare_on_each(&mut network, index);
    }
    network.run_until(START + CHAOS_MS + CALM_MS);
    network
}

/// Node `index` sends a Prepare on each method of each of its peerings.
fn prepare_on_each(network: &mut Network, index: usize) {
    let methods: Vec<(NodeId, SchemeId)> = network
        .node(index)
        .active_peers()
        .flat_map(|(peer, peering)| peering.schemes().map(move |scheme| (*peer, scheme)))
        .collect();
    for (peer, method) in methods {
        network.prepare(index, peer, method);
    }
}

fn chaos(network: &mut Network, rng: &mut SeededRng) {
    let count = network.nodes.len();
    let pick = usize::try_from(rng.next_u64() % count as u64).unwrap();
    match rng.next_u64() % 6 {
        0 if network.up().count() * 3 > count * 2 => network.crash(pick),
        1 if !network.is_up(pick) => network.start(pick),
        2 => {
            let other = usize::try_from(rng.next_u64() % count as u64).unwrap();
            if other != pick {
                network.cut.insert((pick, other));
            }
        }
        3 => {
            let first = network.cut.iter().next().copied();
            if let Some(cut) = first {
                network.cut.remove(&cut);
            }
        }
        _ if network.is_up(pick) => lifecycle(network, rng, pick),
        _ => {}
    }
}

/// What owners and the books do to node `index`'s peerings.
fn lifecycle(network: &mut Network, rng: &mut SeededRng, index: usize) {
    let peers: Vec<NodeId> = network.node(index).peers().map(|(id, _)| *id).collect();
    let count = network.nodes.len();
    let pick_peer = |rng: &mut SeededRng| {
        let at = usize::try_from(rng.next_u64() % peers.len().max(1) as u64).unwrap();
        peers.get(at).copied()
    };
    match rng.next_u64() % 6 {
        0 => {
            if let Some(peer) = pick_peer(rng) {
                network.act(index, |node, now| {
                    node.terminate(now, &peer, TerminationReason::Owner);
                });
            }
        }
        1 => {
            let target = usize::try_from(rng.next_u64() % count as u64).unwrap();
            if target == index || !network.is_up(target) {
                return;
            }
            let id = *network.node(target).own();
            network.act(index, |node, now| {
                if node.is_banned(&id) {
                    node.unban(now, &id);
                } else {
                    let _ = node.ban(now, id);
                }
            });
        }
        2 => {
            // Only to a node that holds a channel too, as the books only
            // owe nodes that were peered.
            let Some(peer) = pick_peer(rng) else {
                return;
            };
            if network.holds_channel(&peer, index) {
                let amount = 1 + rng.next_u64() % 1_000;
                network.settle(index, peer, amount);
            }
        }
        3 => {
            if let Some(peer) = pick_peer(rng) {
                let busy = &mut network.nodes[index].hooks.busy;
                if !busy.remove(&peer) {
                    busy.insert(peer);
                }
            }
        }
        // The node stops running a scheme, or starts again: its methods on
        // it are dropped, and a peering left with none ends.
        4 => {
            let toggled = scheme(if rng.next_u64().is_multiple_of(3) {
                STUB
            } else {
                SECOND
            });
            let running = network.runs(index, toggled);
            network.set_running(index, toggled, !running);
        }
        _ => prepare_on_each(network, index),
    }
}

/// How the run ended: a settlement applied twice, for another amount or
/// never sent, and anything taken from the forger that only a node holding
/// something with the recipient may send, is a safety failure; a network
/// that did not converge in the calm, a liveness failure.
fn check(network: &Network) -> Outcome {
    let result = forgeries(network)
        .and_then(|()| misplaced(network))
        .and_then(|()| settlements(network))
        .and_then(|()| {
            converged(network).map_err(|problem| Outcome::liveness("converged", problem))
        })
        .and_then(|()| finished(network).map_err(|problem| Outcome::liveness("finished", problem)));
    result.err().unwrap_or(Outcome::Pass)
}

/// Nothing was taken from the forger but what anyone may send.
fn forgeries(network: &Network) -> Result<(), Outcome> {
    match network.forgeries_taken.first() {
        Some(taken) => Err(Outcome::safety("forgeries", taken.clone())),
        None => Ok(()),
    }
}

/// Every Prepare taken on a method was sent on it.
fn misplaced(network: &Network) -> Result<(), Outcome> {
    match network.misplaced.first() {
        Some(misplaced) => Err(Outcome::safety("methods", misplaced.clone())),
        None => Ok(()),
    }
}

/// Every settlement sent arrived exactly once, and none arrived that was
/// not sent.
fn settlements(network: &Network) -> Result<(), Outcome> {
    let ids = ids(network);
    for (from, to, amount, sequence) in &network.settled {
        let to_index = ids.iter().position(|id| id == to).unwrap();
        let applied: Vec<_> = network
            .applied
            .iter()
            .filter(|a| a.to == to_index && a.from == ids[*from] && a.sequence == *sequence)
            .collect();
        match applied.as_slice() {
            [once] if once.amount == *amount => {}
            [] => {
                return Err(Outcome::liveness(
                    "settlements",
                    format!(
                        "node {from}'s settlement #{sequence} to node {to_index} never arrived"
                    ),
                ));
            }
            _ => {
                return Err(Outcome::safety(
                    "settlements",
                    format!(
                        "node {from}'s settlement #{sequence} to node {to_index} was applied {applied:?}"
                    ),
                ));
            }
        }
    }
    if network.applied.len() != network.settled.len() {
        return Err(Outcome::safety(
            "settlements",
            "a settlement was applied that was not sent",
        ));
    }
    Ok(())
}

fn ids(network: &Network) -> Vec<NodeId> {
    (0..network.nodes.len())
        .map(|index| *network.node(index).own())
        .collect()
}

/// Every node knows every other's latest information and has its peers,
/// the peer graph is connected, and the routes follow from it.
fn converged(network: &Network) -> Result<(), String> {
    let count = network.nodes.len();
    let ids = ids(network);
    for index in 0..count {
        let node = network.node(index);
        let views = node.nodes();
        for (other, id) in ids.iter().enumerate() {
            let own_sequence = network
                .node(other)
                .nodes()
                .into_iter()
                .find(|v| v.id == *id)
                .and_then(|v| v.sequence);
            let seen = views
                .iter()
                .find(|view| view.id == *id)
                .and_then(|view| view.sequence);
            if seen != own_sequence {
                return Err(format!(
                    "node {index} holds {seen:?} for node {other}, which is at {own_sequence:?}"
                ));
            }
        }
        let wanted = network.config.peer_target.min(count - 1);
        if node.peers().count() < wanted {
            return Err(format!("node {index} has {} peers", node.peers().count()));
        }
    }
    let graph = network.peer_graph();
    if !connected(&graph) {
        return Err(format!("the peer graph is split: {graph:?}"));
    }
    for index in 0..count {
        let node = network.node(index);
        let expected = routing::compute(node.own(), node.route_key(), |id| {
            graph.get(id).map(Vec::as_slice)
        });
        if *node.routes() != expected {
            return Err(format!(
                "node {index}'s routes are {:?}, not {expected:?}",
                node.routes()
            ));
        }
    }
    Ok(())
}

/// Every termination finished, nothing is left to settle, and both sides
/// hold and advertise every peering.
fn finished(network: &Network) -> Result<(), String> {
    let ids = ids(network);
    for index in 0..ids.len() {
        let node = network.node(index);
        if let Some(view) = node.nodes().iter().find(|view| view.termination.is_some()) {
            return Err(format!(
                "node {index} is still ending its peering with {}",
                view.id
            ));
        }
        if let Some((peer, _)) = node
            .peers()
            .find(|(_, peering)| peering.depeering.is_some())
        {
            return Err(format!(
                "node {index} still collects before ending its peering with {peer}"
            ));
        }
        if let Some((peer, _)) = node
            .channels()
            .find(|(_, channel)| channel.outbox.is_some())
        {
            return Err(format!(
                "node {index}'s settlement to {peer} is still on its way"
            ));
        }
        for (peer, _) in node.peers() {
            let other = ids.iter().position(|id| id == peer).unwrap();
            if network.node(other).peering_of(node.own()).is_none() {
                return Err(format!(
                    "only node {index} holds its peering with node {other}"
                ));
            }
            if !node.is_advertised(peer) {
                return Err(format!("node {index} does not advertise node {other}"));
            }
            methods_agree(network, index, other)?;
        }
    }
    Ok(())
}

/// Both ends of the peering of nodes `index` and `other` hold the same
/// methods: one on each scheme both run
/// (spec/decisions/settlement-methods.md).
fn methods_agree(network: &Network, index: usize, other: usize) -> Result<(), String> {
    let both: Vec<SchemeId> = [scheme(STUB), scheme(SECOND)]
        .into_iter()
        .filter(|scheme| network.runs(index, *scheme) && network.runs(other, *scheme))
        .collect();
    let held = |from: usize, to: usize| -> Vec<SchemeId> {
        let to = *network.node(to).own();
        let mut schemes: Vec<SchemeId> = network
            .node(from)
            .peering_of(&to)
            .map(|peering| peering.schemes().collect())
            .unwrap_or_default();
        schemes.sort_unstable();
        schemes
    };
    let (ours, theirs) = (held(index, other), held(other, index));
    if ours != both || theirs != both {
        return Err(format!(
            "nodes {index} and {other} run {both:?} and hold methods {ours:?} and {theirs:?}"
        ));
    }
    Ok(())
}
