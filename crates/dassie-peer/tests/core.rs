//! The core, as each node computes it from the signed data
//! (spec/decisions/promotion-to-core.md), on small simulated networks.

#![allow(clippy::unwrap_used, clippy::indexing_slicing)]

use std::collections::BTreeSet;

use dassie_identity::NodeId;
use dassie_peer::Config;
use dassie_peer::sim::{Network, key};

fn settle(network: &mut Network, ms: u64) {
    let until = network.now + ms;
    network.run_until(until);
}

fn ids(network: &Network, indexes: &[usize]) -> BTreeSet<NodeId> {
    indexes.iter().map(|i| *network.node(*i).own()).collect()
}

// Three bootstrap nodes and two more, each registered and peered with
// more than one: every node computes the same core, and each knows since
// when it is in it.
#[test]
fn every_node_computes_the_same_core() {
    let mut network = Network::new(3, 5, 3, Config::DEV);
    settle(&mut network, 120_000);
    let core = network.node(0).core().clone();
    for index in 0..5 {
        let node = network.node(index);
        assert_eq!(node.core(), &core, "node {index} agrees");
        assert_eq!(
            node.own_core_since_ms().is_some(),
            core.contains(node.own()),
            "node {index} knows whether it is core"
        );
    }
    assert!(
        ids(&network, &[0, 1, 2]).is_subset(&core),
        "the bootstrap nodes are core: {core:?}"
    );
    for id in &core {
        assert!(
            network.node(0).majority().contains(id),
            "only registered nodes"
        );
    }
    let view = network.node(0).nodes();
    for entry in view {
        assert_eq!(entry.core, core.contains(&entry.id), "the view shows it");
    }
}

// A node without bootstrap nodes runs alone, as its own bootstrap node: a
// core alone.
#[test]
fn a_node_alone_is_a_core_alone() {
    let mut network = Network::new(5, 1, 0, Config::DEV);
    settle(&mut network, 10_000);
    let node = network.node(0);
    assert_eq!(node.core(), &ids(&network, &[0]));
    assert!(node.own_core_since_ms().is_some());
}

// The dev core is core outright, in every node told of it: even a node
// that never registered, with no edges at all.
#[test]
fn the_dev_core_is_core() {
    let mut network = Network::new(7, 4, 3, Config::DEV);
    let outsider = key(9).node_id();
    for index in 0..4 {
        network.act(index, |node, _| node.set_dev_core(vec![outsider]));
    }
    settle(&mut network, 120_000);
    for index in 0..4 {
        let node = network.node(index);
        assert!(node.core().contains(&outsider), "node {index}");
        assert!(!node.majority().contains(&outsider), "never registered");
    }
}
