//! Who holds a name (spec/areas/network.md#holders) and releasing a split
//! (spec/areas/network.md#releasing-a-split), over the peer protocol, on a
//! simulated network that checks the nodes' invariants after every step.
//! The first nodes are the bootstrap nodes, and so the registrars; the
//! nodes after them only read the lists. Registrants are keys of their own
//! that send requests to the registrars, as a registrant node would.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

#[path = "support/registrant.rs"]
mod registrant;

use std::collections::BTreeSet;

use dassie_identity::NodeId;
use dassie_oer::Encode;
use dassie_peer::names::Name;
use dassie_peer::sim::{Faults, Forge, Network};
use dassie_peer::wire::{Kind, NameHolding, NameListResponse};
use dassie_peer::{Config, Event, NameEvent};
use registrant::{Registrant, body, name, receipt};
use sha2::{Digest, Sha256};

const CONFIG: Config = Config::DEV;

/// How often the runs that watch every node's view look.
const LOOK_MS: u64 = 500;

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

/// Who node `index` takes as `name`'s holder.
fn holder(network: &Network, index: usize, name: &Name) -> Option<NodeId> {
    let held = network.node(index).holders().holder(name);
    held.map(|held| held.holder)
}

/// Commits `registrant` to `name` at each of `registrars`, in order.
fn commit(network: &mut Network, registrant: &mut Registrant, at: &[usize], name: &Name) {
    for registrar in at {
        receipt(&registrant.commit_at(network, *registrar, name));
    }
}

/// Unseals `registrant`'s commitment to `name` at each of `registrars`.
fn unseal(network: &mut Network, registrant: &mut Registrant, at: &[usize], name: &Name) {
    for registrar in at {
        body(&registrant.unseal_at(network, *registrar, name));
    }
}

/// Registers `name` for `registrant` at every registrar of `count`, all at
/// once.
fn register(network: &mut Network, registrant: &mut Registrant, count: usize, name: &Name) {
    let all: Vec<usize> = (0..count).collect();
    commit(network, registrant, &all, name);
    unseal(network, registrant, &all, name);
}

/// The registrars that released `name`.
fn released(network: &Network, name: &Name) -> BTreeSet<usize> {
    network
        .events
        .iter()
        .filter(|(_, event)| {
            matches!(event, Event::Name(NameEvent::Released { name: n, .. }) if n == name)
        })
        .map(|(index, _)| *index)
        .collect()
}

/// A list that names `holder` for `name`, or nobody.
fn listing(name: &Name, holder: Option<NodeId>) -> NameListResponse {
    let mut list = NameListResponse::default();
    if let Some(holder) = holder {
        let holding = NameHolding {
            holder,
            expires_at_ms: u64::MAX,
        };
        list.0.insert(*name, holding);
    }
    list
}

/// The nodes `faulty` answer each node's name-list requests, and its hash
/// polls, with the list `show` gives that node, if it gives one: registrars
/// showing nodes different lists, each authenticated to the node it
/// answers.
fn forge(
    faulty: &'static [usize],
    show: impl Fn(usize) -> Option<NameListResponse> + 'static,
) -> Forge {
    Box::new(move |responder, requester, kind, body| {
        if !faulty.contains(&responder) {
            return;
        }
        let Some(list) = show(requester) else {
            return;
        };
        let list = list.to_vec();
        match kind {
            Kind::NameListRequest => *body = list,
            Kind::NameListHashRequest => *body = Sha256::digest(&list).to_vec(),
            _ => {}
        }
    })
}

/// Runs the network for `ms`, checking every [`LOOK_MS`] that no two of
/// the `honest` nodes take different holders for `name`. Returns every
/// node's every view seen.
fn run_watched(
    network: &mut Network,
    ms: u64,
    honest: &[usize],
    name: &Name,
) -> BTreeSet<(usize, Option<NodeId>)> {
    let mut seen = BTreeSet::new();
    let until = network.now + ms;
    while network.now < until {
        settle(network, LOOK_MS);
        let views: Vec<Option<NodeId>> = honest
            .iter()
            .map(|index| holder(network, *index, name))
            .collect();
        let holders: BTreeSet<NodeId> = views.iter().flatten().copied().collect();
        assert!(
            holders.len() <= 1,
            "two holders at {}: {views:?}",
            network.now
        );
        seen.extend(honest.iter().copied().zip(views));
    }
    seen
}

// spec/designs/names.md#5-holding-a-name: every node takes the holder a
// quorum of the registrars list. While fewer than a quorum of the lists are
// fresh, its view freezes, across a restart too; once a quorum is fresh
// again, the view follows the lists.
#[test]
fn a_view_without_a_quorum_of_fresh_lists_freezes() {
    let mut network = start(21, 5, 4);
    let mut alice = Registrant::new(71);
    let alice_name = name("alice");
    register(&mut network, &mut alice, 4, &alice_name);
    windows(&mut network);
    for index in 0..5 {
        assert_eq!(
            holder(&network, index, &alice_name),
            Some(alice.id()),
            "{index}"
        );
    }
    // Node 4 is cut off from two registrars, whose lists go stale; then
    // the other two show it no names: too few fresh lists to take that as
    // the truth.
    network.cut.extend([(4, 0), (4, 1)]);
    settle(&mut network, 2 * CONFIG.list_fresh_ms);
    assert!(network.node(4).holders().is_frozen());
    let none = |requester| (requester == 4).then(NameListResponse::default);
    network.forge = Some(forge(&[2, 3], none));
    settle(&mut network, 3 * CONFIG.list_fresh_ms);
    assert!(network.node(4).holders().is_frozen());
    assert_eq!(holder(&network, 4, &alice_name), Some(alice.id()));
    network.crash(4);
    network.start(4);
    settle(&mut network, 3 * CONFIG.list_fresh_ms);
    assert!(network.node(4).holders().is_frozen());
    assert_eq!(holder(&network, 4, &alice_name), Some(alice.id()));
    // A quorum fresh again: two lists of four name Alice, too few.
    network.cut.clear();
    settle(&mut network, 3 * CONFIG.list_poll_interval_ms);
    assert!(!network.node(4).holders().is_frozen());
    assert_eq!(holder(&network, 4, &alice_name), None);
    assert!(network.events.contains(&(
        4,
        Event::Name(NameEvent::Unheld {
            name: alice_name,
            holder: alice.id()
        })
    )));
}

// spec/designs/names.md#6-why-it-holds: four registrars, one faulty, after
// an honest split of two to one. The faulty one shows one node Alice as the
// holder and every other node Bob: that node holds Alice, and no node ever
// holds Bob.
#[test]
fn a_faulty_registrar_of_four_cannot_give_a_name_two_holders() {
    let mut network = start(22, 6, 4);
    let (mut alice, mut bob) = (Registrant::new(71), Registrant::new(72));
    let alice_name = name("alice");
    commit(&mut network, &mut alice, &[0, 1], &alice_name);
    commit(&mut network, &mut bob, &[0, 1, 2], &alice_name);
    commit(&mut network, &mut alice, &[2], &alice_name);
    unseal(&mut network, &mut alice, &[0, 1, 2], &alice_name);
    unseal(&mut network, &mut bob, &[0, 1, 2], &alice_name);
    let (alice_id, bob_id) = (alice.id(), bob.id());
    network.forge = Some(forge(&[3], move |requester| {
        let shown = if requester == 4 { alice_id } else { bob_id };
        Some(listing(&alice_name, Some(shown)))
    }));
    let honest = [0, 1, 2, 4, 5];
    let ms = CONFIG.names.window_ms + 2 * CONFIG.names.release_after_ms;
    let seen = run_watched(&mut network, ms, &honest, &alice_name);
    assert!(seen.contains(&(4, Some(alice.id()))), "node 4 holds Alice");
    assert!(seen.iter().all(|(_, held)| *held != Some(bob.id())));
    // Bob's side of the split is released, and so is Alice's, by the
    // registrars the faulty one shows Bob.
    assert_eq!(released(&network, &alice_name), BTreeSet::from([0, 1, 2]));
}

// The same with five registrars, one faulty, after an honest split of two
// to two: each side has a majority with the faulty one, three of five, but
// no quorum of four, so neither node it lies to takes a holder.
#[test]
fn a_faulty_registrar_of_five_cannot_give_a_name_two_holders() {
    let mut network = start(23, 7, 5);
    let (mut alice, mut bob) = (Registrant::new(71), Registrant::new(72));
    let alice_name = name("alice");
    commit(&mut network, &mut alice, &[0, 1], &alice_name);
    commit(&mut network, &mut bob, &[0, 1, 2, 3], &alice_name);
    commit(&mut network, &mut alice, &[2, 3], &alice_name);
    unseal(&mut network, &mut alice, &[0, 1, 2, 3], &alice_name);
    unseal(&mut network, &mut bob, &[0, 1, 2, 3], &alice_name);
    let (alice_id, bob_id) = (alice.id(), bob.id());
    network.forge = Some(forge(&[4], move |requester| {
        let shown = if requester == 5 { alice_id } else { bob_id };
        Some(listing(&alice_name, Some(shown)))
    }));
    windows(&mut network);
    let now = network.clock(5);
    let votes = network
        .node(5)
        .holders()
        .votes(&alice_name, &alice.id(), now);
    assert_eq!(votes, 3, "a majority of the lists name Alice to node 5");
    let now = network.clock(6);
    let votes = network.node(6).holders().votes(&alice_name, &bob.id(), now);
    assert_eq!(votes, 3, "a majority of the lists name Bob to node 6");
    let honest = [0, 1, 2, 3, 5, 6];
    let ms = 2 * CONFIG.names.release_after_ms;
    let seen = run_watched(&mut network, ms, &honest, &alice_name);
    assert!(seen.iter().all(|(_, held)| held.is_none()), "{seen:?}");
    assert_eq!(
        released(&network, &alice_name),
        BTreeSet::from([0, 1, 2, 3])
    );
}

// spec/areas/network.md#releasing-a-split: two registrants that commit at
// nearly the same time split four honest registrars two to two. Nobody
// holds the name, and every registrar releases it once the split has
// lasted longer than the wait, and no sooner; then it is free for anyone.
#[test]
fn a_split_of_two_to_two_is_released_at_all_four() {
    let mut network = start(24, 5, 4);
    let (mut alice, mut bob) = (Registrant::new(71), Registrant::new(72));
    let alice_name = name("alice");
    commit(&mut network, &mut alice, &[0, 1], &alice_name);
    commit(&mut network, &mut bob, &[0, 1, 2, 3], &alice_name);
    commit(&mut network, &mut alice, &[2, 3], &alice_name);
    unseal(&mut network, &mut alice, &[0, 1, 2, 3], &alice_name);
    unseal(&mut network, &mut bob, &[0, 1, 2, 3], &alice_name);
    settle(&mut network, CONFIG.names.window_ms);
    let early = CONFIG.names.release_after_ms - 2_000;
    let seen = run_watched(&mut network, early, &[0, 1, 2, 3, 4], &alice_name);
    assert!(seen.iter().all(|(_, held)| held.is_none()), "{seen:?}");
    assert!(released(&network, &alice_name).is_empty(), "not yet");
    settle(&mut network, 10_000);
    assert_eq!(
        released(&network, &alice_name),
        BTreeSet::from([0, 1, 2, 3])
    );
    let mut carol = Registrant::new(73);
    register(&mut network, &mut carol, 4, &alice_name);
    windows(&mut network);
    for index in 0..5 {
        assert_eq!(
            holder(&network, index, &alice_name),
            Some(carol.id()),
            "{index}"
        );
    }
}

// A holder with a quorum is never released, however late or seldom lists
// arrive: not while one registrar lists someone else for the name and
// another's list is out of reach since before the grant, nor while one
// cannot be reached at all. A list not fresh counts for nothing.
#[test]
fn a_holder_with_a_quorum_is_never_released() {
    let mut network = start(25, 5, 4);
    let (mut alice, bob) = (Registrant::new(71), Registrant::new(72));
    let alice_name = name("alice");
    register(&mut network, &mut alice, 4, &alice_name);
    let bob_id = bob.id();
    network.forge = Some(forge(&[3], move |_| {
        Some(listing(&alice_name, Some(bob_id)))
    }));
    network.cut_one_way.insert((0, 2));
    windows(&mut network);
    let ms = 2 * CONFIG.names.release_after_ms;
    let seen = run_watched(&mut network, ms, &[1, 2, 4], &alice_name);
    assert!(
        seen.iter().all(|(_, held)| *held == Some(alice.id())),
        "{seen:?}"
    );
    network.forge = None;
    network.cut_one_way.clear();
    for index in [0, 1, 2, 4] {
        network.cut.insert((3, index));
    }
    settle(&mut network, 3 * CONFIG.list_poll_interval_ms);
    let seen = run_watched(&mut network, ms, &[0, 1, 2, 4], &alice_name);
    assert!(
        seen.iter().all(|(_, held)| *held == Some(alice.id())),
        "{seen:?}"
    );
    assert!(released(&network, &alice_name).is_empty());
}

// Lists that are merely late release nothing: two registrars log Alice's
// commitments near the end of the others' windows, and so grant the name a
// window later, on a rough network.
#[test]
fn late_lists_release_nothing() {
    let mut network = start(26, 5, 4);
    network.faults = Faults::ROUGH;
    let mut alice = Registrant::new(71);
    let alice_name = name("alice");
    commit(&mut network, &mut alice, &[0, 1], &alice_name);
    settle(&mut network, CONFIG.names.window_ms - 5_000);
    commit(&mut network, &mut alice, &[2, 3], &alice_name);
    unseal(&mut network, &mut alice, &[0, 1, 2, 3], &alice_name);
    let ms = 2 * CONFIG.names.window_ms + CONFIG.names.release_after_ms;
    run_watched(&mut network, ms, &[0, 1, 2, 3, 4], &alice_name);
    assert!(released(&network, &alice_name).is_empty());
    for index in 0..5 {
        assert_eq!(
            holder(&network, index, &alice_name),
            Some(alice.id()),
            "{index}"
        );
    }
}
