//! Deterministic simulation of peer networks (spec 7 §7, D16): random
//! sizes and bootstrap lists, a rough network, crashes, restarts and
//! partitions, owners ending peerings and banning nodes, settlements, then
//! a calm in which the network must converge, every termination finish and
//! every settlement arrive exactly once. A failure names its seed;
//! `DASSIE_SIM_SEEDS=from..to` runs other seeds.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

mod support;

use std::panic::{AssertUnwindSafe, catch_unwind};

use dassie_identity::NodeId;
use dassie_node_core::testing::SeededRng;
use dassie_peer::wire::TerminationReason;
use dassie_peer::{Config, routing};
use support::{Faults, Network, START, connected};

/// How long the network is rough.
const CHAOS_MS: u64 = 90_000;

/// How long it then has to settle.
const CALM_MS: u64 = 240_000;

// Which seeds to run is the one input read from the environment; each run
// is still fixed by its seed.
#[allow(clippy::disallowed_methods)]
fn seeds() -> std::ops::Range<u64> {
    std::env::var("DASSIE_SIM_SEEDS")
        .ok()
        .and_then(|range| {
            let (from, to) = range.split_once("..")?;
            Some(from.parse().ok()?..to.parse().ok()?)
        })
        .unwrap_or(0..12)
}

/// Runs a seed: its size, its chaos, its calm.
fn run(seed: u64) -> Network {
    let mut rng = SeededRng::new(seed ^ 0x5eed);
    let count = 2 + usize::try_from(rng.next_u64() % 11).unwrap();
    let bootstraps = 1 + usize::try_from(rng.next_u64() % 3).unwrap().min(count - 1);
    // An owner's termination keeps the pair apart for less than the calm,
    // so the network can converge again.
    let config = Config {
        owner_termination_cooldown_ms: 30_000,
        ..Config::DEV
    };
    let mut network = Network::new(seed, count, bootstraps, config);
    network.faults = Faults::ROUGH;
    network.settle_on_end = true;
    let mut at = START;
    while at < START + CHAOS_MS {
        at += 2_000 + rng.next_u64() % 3_000;
        network.run_until(at);
        chaos(&mut network, &mut rng);
    }
    network.faults = Faults::NONE;
    network.cut.clear();
    for index in 0..count {
        if !network.is_up(index) {
            network.start(index);
        }
        network.nodes[index].hooks.busy.clear();
        let banned: Vec<NodeId> = network.node(index).bans().copied().collect();
        network.act(index, |node, now| {
            for id in &banned {
                node.unban(now, id);
            }
        });
    }
    network.run_until(START + CHAOS_MS + CALM_MS);
    network
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
    match rng.next_u64() % 4 {
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
        _ => {
            if let Some(peer) = pick_peer(rng) {
                let busy = &mut network.nodes[index].hooks.busy;
                if !busy.remove(&peer) {
                    busy.insert(peer);
                }
            }
        }
    }
}

/// What a calm network must look like.
fn check(network: &Network) -> Result<(), String> {
    let count = network.nodes.len();
    let ids: Vec<_> = (0..count).map(|index| *network.node(index).own()).collect();
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
    check_lifecycle(network)?;
    let graph = network.peer_graph();
    if !connected(&graph) {
        return Err(format!("the peer graph is split: {graph:?}"));
    }
    for index in 0..count {
        let node = network.node(index);
        let expected = routing::compute(node.own(), |id| graph.get(id).map(Vec::as_slice));
        if *node.routes() != expected {
            return Err(format!(
                "node {index}'s routes are {:?}, not {expected:?}",
                node.routes()
            ));
        }
    }
    Ok(())
}

/// Every termination finished, both sides hold and advertise every
/// peering, and every settlement arrived exactly once.
fn check_lifecycle(network: &Network) -> Result<(), String> {
    let count = network.nodes.len();
    let ids: Vec<NodeId> = (0..count).map(|index| *network.node(index).own()).collect();
    for index in 0..count {
        let node = network.node(index);
        if let Some(view) = node.nodes().iter().find(|view| view.termination.is_some()) {
            return Err(format!(
                "node {index} is still ending its peering with {}",
                view.id
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
        }
    }
    for (from, to, amount, sequence) in &network.settled {
        let to_index = ids.iter().position(|id| id == to).unwrap();
        let applied: Vec<_> = network
            .applied
            .iter()
            .filter(|a| a.to == to_index && a.from == ids[*from] && a.sequence == *sequence)
            .collect();
        match applied.as_slice() {
            [once] if once.amount == *amount => {}
            _ => {
                return Err(format!(
                    "node {from}'s settlement #{sequence} to node {to_index} was applied {applied:?}"
                ));
            }
        }
    }
    if network.applied.len() != network.settled.len() {
        return Err("a settlement was applied that was not sent".into());
    }
    Ok(())
}

#[test]
fn networks_converge_after_chaos() {
    for seed in seeds() {
        let outcome = catch_unwind(AssertUnwindSafe(|| check(&run(seed))));
        match outcome {
            Ok(Ok(())) => {}
            Ok(Err(problem)) => panic!("seed {seed}: {problem}"),
            Err(_) => panic!("seed {seed} panicked"),
        }
    }
}

#[test]
fn a_seed_replays_the_same_run() {
    let first = run(3);
    let second = run(3);
    assert_eq!(first.trace, second.trace);
}

/// Explains a failing seed: `SEED=n GREP=text cargo test -p dassie-peer
/// --test simulation explain_seed -- --ignored --nocapture` runs it, and
/// prints every trace line containing `text` and the check's verdict.
#[test]
#[ignore = "a tool for failing seeds"]
#[allow(clippy::disallowed_methods)]
fn explain_seed() {
    let seed: u64 = std::env::var("SEED").unwrap().parse().unwrap();
    let pattern = std::env::var("GREP").unwrap_or_default();
    let network = run(seed);
    println!(
        "{} nodes, {} bootstrap",
        network.nodes.len(),
        network.bootstrap.len()
    );
    for line in network.trace.iter().filter(|line| line.contains(&pattern)) {
        println!("{line}");
    }
    println!("check: {:?}", check(&network));
}
