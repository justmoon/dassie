//! Deterministic simulation of peer networks (spec 7 §7, D16): random
//! sizes and bootstrap lists, a rough network, crashes, restarts and
//! partitions, then a calm in which the network must converge. A failure
//! names its seed; `DASSIE_SIM_SEEDS=from..to` runs other seeds.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

mod support;

use std::panic::{AssertUnwindSafe, catch_unwind};

use dassie_node_core::testing::SeededRng;
use dassie_peer::{Config, routing};
use support::{Faults, Network, START, connected};

/// How long the network is rough.
const CHAOS_MS: u64 = 90_000;

/// How long it then has to settle.
const CALM_MS: u64 = 240_000;

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
    let mut network = Network::new(seed, count, bootstraps, Config::DEV);
    network.faults = Faults::ROUGH;
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
    }
    network.run_until(START + CHAOS_MS + CALM_MS);
    network
}

fn chaos(network: &mut Network, rng: &mut SeededRng) {
    let count = network.nodes.len();
    let pick = usize::try_from(rng.next_u64() % count as u64).unwrap();
    match rng.next_u64() % 4 {
        0 if network.up().count() * 3 > count * 2 => network.crash(pick),
        1 if !network.is_up(pick) => network.start(pick),
        2 => {
            let other = usize::try_from(rng.next_u64() % count as u64).unwrap();
            if other != pick {
                network.cut.insert((pick, other));
            }
        }
        _ => {
            let first = network.cut.iter().next().copied();
            if let Some(cut) = first {
                network.cut.remove(&cut);
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
