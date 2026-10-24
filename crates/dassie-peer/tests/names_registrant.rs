//! A registrant's side of names (spec/areas/network.md#the-registrant),
//! over the peer protocol, on a simulated network that checks the nodes'
//! invariants after every step. The first nodes are the bootstrap nodes,
//! and so the registrars; a node registers a name for itself, as its owner
//! asks it to.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use std::collections::BTreeSet;

use dassie_identity::NodeId;
use dassie_peer::names::registrant::{RegisterError, State};
use dassie_peer::names::{Name, NameStatus, Stage};
use dassie_peer::sim::{Faults, Network, key};
use dassie_peer::{Config, Event, NameEvent};

const CONFIG: Config = Config::DEV;

fn settle(network: &mut Network, ms: u64) {
    let until = network.now + ms;
    network.run_until(until);
}

/// `count` nodes, the first `registrars` of them the registrars, once every
/// node has read every list: their clocks are up to 10 s apart, and a node
/// refuses what is dated before its start.
fn start(seed: u64, count: usize, registrars: usize) -> Network {
    let mut network = Network::new(seed, count, registrars, CONFIG);
    settle(&mut network, 30_000);
    network
}

/// Long enough for the windows of commitments made now to close, and the
/// names granted to reach every node's view.
fn windows(network: &mut Network) {
    settle(network, CONFIG.names.window_ms + 5_000);
}

fn name(text: &str) -> Name {
    Name::parse(text).unwrap()
}

fn id(index: usize) -> NodeId {
    key(index).node_id()
}

/// Node `index` registers `name`, as its owner asks.
fn try_register(network: &mut Network, index: usize, name: &Name) -> Result<(), RegisterError> {
    let (name, mut result) = (*name, Ok(()));
    network.act(index, |node, now| result = node.register_name(now, name));
    result
}

fn register(network: &mut Network, index: usize, name: &Name) {
    try_register(network, index, name).unwrap();
}

/// Who node `index` takes as `name`'s holder.
fn holder(network: &Network, index: usize, name: &Name) -> Option<NodeId> {
    let held = network.node(index).holders().holder(name);
    held.map(|held| held.holder)
}

/// How node `index`'s own `name` stands, as its owner sees it.
fn status(network: &Network, index: usize, name: &Name) -> NameStatus {
    let statuses = network.node(index).name_statuses(network.clock(index));
    statuses
        .into_iter()
        .find(|status| status.name == *name)
        .unwrap()
}

/// The nodes that reported an event `matches` takes.
fn reported(network: &Network, matches: impl Fn(&NameEvent) -> bool) -> Vec<usize> {
    network
        .events
        .iter()
        .filter_map(|(index, event)| match event {
            Event::Name(event) if matches(event) => Some(*index),
            _ => None,
        })
        .collect()
}

/// The registrars that logged a commitment from `committer`, once per
/// commitment logged.
fn logged_from(network: &Network, committer: NodeId) -> Vec<usize> {
    reported(
        network,
        |event| matches!(event, NameEvent::Committed { committer: c, .. } if *c == committer),
    )
}

/// The registrars that took an unsealing of `name`.
fn unsealed(network: &Network, name: &Name) -> BTreeSet<usize> {
    reported(
        network,
        |event| matches!(event, NameEvent::Unsealed { name: n, .. } if n == name),
    )
    .into_iter()
    .collect()
}

// spec/designs/names.md#3-registering: a node commits at every registrar,
// unseals once it holds every receipt, and holds the name, once the
// windows close, in every node's view; its registration is then done, and
// the name is the node's to renew.
#[test]
fn a_node_registers_a_name_at_every_registrar() {
    let mut network = start(31, 6, 4);
    let alice = name("alice");
    register(&mut network, 4, &alice);
    settle(&mut network, 2_000);
    assert_eq!(status(&network, 4, &alice).state, State::Unsealing);
    assert_eq!(unsealed(&network, &alice), BTreeSet::from([0, 1, 2, 3]));
    windows(&mut network);
    for index in 0..6 {
        assert_eq!(holder(&network, index, &alice), Some(id(4)), "{index}");
    }
    let node = network.node(4);
    assert_eq!(node.registrant().registrations().count(), 0);
    assert_eq!(status(&network, 4, &alice).state, State::Held);
    assert_eq!(logged_from(&network, id(4)).len(), 4, "logged once each");
    assert_eq!(node.stats().registrations_started, 1);
    // Registering it again is refused: it is the node's.
    let refused = try_register(&mut network, 4, &alice);
    assert_eq!(refused, Err(RegisterError::Held));
    let refused = try_register(&mut network, 5, &alice);
    assert_eq!(refused, Err(RegisterError::Taken));
}

// A registrar that registers a name commits and unseals at itself as at
// every other registrar.
#[test]
fn a_registrar_registers_a_name_with_itself() {
    let mut network = start(32, 5, 4);
    let bob = name("bob");
    register(&mut network, 0, &bob);
    windows(&mut network);
    for index in 0..5 {
        assert_eq!(holder(&network, index, &bob), Some(id(0)), "{index}");
    }
    let logged: BTreeSet<usize> = logged_from(&network, id(0)).into_iter().collect();
    assert_eq!(logged, BTreeSet::from([0, 1, 2, 3]));
}

// spec/areas/network.md#periphery-nodes: a periphery node, in no node
// list, reaches the registrars over the connections it opens, and
// registers a name like any node.
#[test]
fn a_periphery_node_registers_a_name() {
    let mut network = Network::with_periphery(33, 4, 4, 1, CONFIG);
    settle(&mut network, 30_000);
    let carol = name("carol");
    register(&mut network, 4, &carol);
    windows(&mut network);
    for index in 0..5 {
        assert_eq!(holder(&network, index, &carol), Some(id(4)), "{index}");
    }
}

// A registration is on disk, salt and all, before its first commitment
// leaves: a node that crashes right after commits to the same commitment
// again, which every registrar answers with the receipt it gave before,
// and the registration goes on.
#[test]
fn a_registration_survives_a_crash() {
    let mut network = start(34, 5, 4);
    let alice = name("alice");
    register(&mut network, 4, &alice);
    network.crash(4);
    settle(&mut network, 1_000);
    assert_eq!(logged_from(&network, id(4)).len(), 4);
    network.start(4);
    let stored: Vec<Stage> = network.nodes[4]
        .stored
        .registrations
        .iter()
        .map(|registration| registration.stage)
        .collect();
    assert_eq!(stored, [Stage::Committing]);
    windows(&mut network);
    for index in 0..5 {
        assert_eq!(holder(&network, index, &alice), Some(id(4)), "{index}");
    }
    assert_eq!(logged_from(&network, id(4)).len(), 4, "logged once each");
}

// spec/designs/names.md#6-why-it-holds: a registrar that cannot be reached
// blocks the registration. The registrant never unseals without every
// receipt, so no registrar learns the name; once it is reached again, the
// name is registered.
#[test]
fn a_registration_without_every_receipt_is_abandoned() {
    let mut network = start(35, 5, 4);
    network.cut.insert((4, 3));
    let alice = name("alice");
    register(&mut network, 4, &alice);
    windows(&mut network);
    assert_eq!(status(&network, 4, &alice).state, State::Abandoned);
    assert!(unsealed(&network, &alice).is_empty());
    let abandoned = reported(
        &network,
        |event| matches!(event, NameEvent::Abandoned { name } if *name == alice),
    );
    assert_eq!(abandoned, [4]);
    assert_eq!(holder(&network, 0, &alice), None);
    network.cut.clear();
    register(&mut network, 4, &alice);
    windows(&mut network);
    for index in 0..5 {
        assert_eq!(holder(&network, index, &alice), Some(id(4)), "{index}");
    }
}

// spec/designs/names.md#1-what-it-guarantees: a node that commits to a
// name only after seeing it unsealed never holds it, on a rough network
// too; the registrant that unsealed it does, everywhere.
#[test]
fn a_rival_that_commits_after_the_unsealing_never_holds_the_name() {
    let mut network = start(36, 6, 4);
    network.faults = Faults::ROUGH;
    let alice = name("alice");
    register(&mut network, 4, &alice);
    while unsealed(&network, &alice).is_empty() {
        settle(&mut network, 10);
    }
    register(&mut network, 5, &alice);
    settle(&mut network, CONFIG.names.window_ms);
    network.faults = Faults::NONE;
    windows(&mut network);
    for index in 0..6 {
        assert_eq!(holder(&network, index, &alice), Some(id(4)), "{index}");
    }
    assert_eq!(status(&network, 5, &alice).state, State::Lost);
}

// spec/designs/names.md#5-holding-a-name: the holder renews its name at
// every registrar before it expires, by itself; a renewal that comes late
// to a registrar, cut off past the name's expiry there, is taken in its
// grace period, and the name stays the holder's.
#[test]
fn a_name_is_renewed_before_it_expires() {
    let mut network = start(37, 3, 2);
    let alice = name("alice");
    register(&mut network, 2, &alice);
    windows(&mut network);
    let first = network.node(2).holders().holder(&alice).unwrap().expires_ms;
    // Registrar 1 is out of reach from before the renewal until after the
    // name expired there.
    let renew_at = first - CONFIG.names.renew_before_ms;
    let wait = renew_at - network.clock(0) - 60_000;
    settle(&mut network, wait);
    network.cut.insert((2, 1));
    settle(&mut network, CONFIG.names.renew_before_ms + 120_000);
    let renewed = reported(
        &network,
        |event| matches!(event, NameEvent::Renewed { name, .. } if *name == alice),
    );
    assert_eq!(renewed, [0], "only registrar 0 took the renewal in time");
    network.cut.clear();
    settle(&mut network, 120_000);
    let renewed = reported(
        &network,
        |event| matches!(event, NameEvent::Renewed { name, .. } if *name == alice),
    );
    assert_eq!(renewed, [0, 1], "registrar 1 took it in its grace period");
    for index in 0..3 {
        let held = network.node(index).holders().holder(&alice).unwrap();
        assert_eq!(held.holder, id(2), "{index}");
        assert!(held.expires_ms > first + CONFIG.names.grace_ms, "{index}");
    }
    assert_eq!(status(&network, 2, &alice).state, State::Held);
}
