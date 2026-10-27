//! Settlement methods in the peer protocol
//! (spec/decisions/settlement-methods.md,
//! spec/areas/network.md#settlement-methods), on two simulated nodes that
//! run two schemes: the methods a peering agrees, adds and drops, the
//! Prepares that travel on each, and what a restart keeps.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use dassie_identity::{NodeId, NodeKey};
use dassie_kit::sim::SeededRng;
use dassie_peer::sim::{Faults, Network, SECOND, START, STUB, StubHooks, key, scheme};
use dassie_peer::wire::{Entry, Kind, NodeInfo, SchemeId, TerminationReason, sign_node_info};
use dassie_peer::{
    Config, Event, MAX_SCHEMES, Output, OwnInfo, PeerNode, SendError, Sources, Stored, StoredNode,
};

fn settle(network: &mut Network, ms: u64) {
    let until = network.now + ms;
    network.run_until(until);
}

/// The schemes of node `index`'s methods with `peer`, sorted: none if it
/// holds no peering with it.
fn methods(network: &Network, index: usize, peer: &NodeId) -> Vec<SchemeId> {
    let mut schemes: Vec<SchemeId> = network
        .node(index)
        .peering_of(peer)
        .map(|peering| peering.schemes().collect())
        .unwrap_or_default();
    schemes.sort_unstable();
    schemes
}

/// Both schemes, sorted.
fn two() -> Vec<SchemeId> {
    vec![scheme(STUB), scheme(SECOND)]
}

/// Two nodes that run both schemes, peered, after a while to agree their
/// methods.
fn both(seed: u64) -> (Network, NodeId, NodeId) {
    let mut network = Network::new(seed, 2, 1, Config::DEV);
    for index in 0..2 {
        network.set_running(index, scheme(SECOND), true);
    }
    let (a, b) = (*network.node(0).own(), *network.node(1).own());
    wait(&mut network, |network| {
        methods(network, 0, &b) == two() && methods(network, 1, &a) == two()
    });
    (network, a, b)
}

/// Runs the network for up to three minutes, until `done`.
fn wait(network: &mut Network, done: impl Fn(&Network) -> bool) {
    for _ in 0..60 {
        if done(network) {
            return;
        }
        settle(network, 3_000);
    }
    assert!(done(network), "never came to pass");
}

/// The node of the two that made the peering: the one that asks for its
/// further methods.
fn requester(network: &Network, b: &NodeId) -> usize {
    usize::from(network.node(0).peering_of(b).unwrap().inbound)
}

/// The events of node `index` that `wanted` picks.
fn events(network: &Network, index: usize, wanted: impl Fn(&Event) -> bool) -> usize {
    network
        .events
        .iter()
        .filter(|(at, event)| *at == index && wanted(event))
        .count()
}

// A pair that runs two schemes peers on the first, and the node that asked
// for the peering asks for the second as a further method; the other takes
// it. The first is the one the peering was made on.
#[test]
fn a_pair_that_runs_two_schemes_settles_on_both() {
    let (network, a, b) = both(71);
    assert_eq!(methods(&network, 0, &b), two());
    assert_eq!(methods(&network, 1, &a), two());
    for (index, peer) in [(0, b), (1, a)] {
        let peering = network.node(index).peering_of(&peer).unwrap();
        assert_eq!(peering.first().scheme, scheme(STUB));
    }
    let asked = requester(&network, &b);
    let added = |index, inbound| {
        events(&network, index, |event| {
            matches!(event, Event::MethodAdded { scheme: s, inbound: i, .. }
                if *s == scheme(SECOND) && *i == inbound)
        })
    };
    assert_eq!((added(asked, false), added(1 - asked, true)), (1, 1));
    assert_eq!(network.node(asked).stats().methods_added, 1);
}

/// Two nodes that run both schemes, where each of `direct` would settle
/// directly: it is neither over a hub nor of unlimited trust.
fn with_direct(seed: u64, direct: &[&str]) -> (Network, NodeId, NodeId) {
    let mut network = Network::new(seed, 2, 1, Config::DEV);
    for index in 0..2 {
        network.set_running(index, scheme(SECOND), true);
        let hooks = &mut network.nodes[index].hooks;
        hooks.direct.extend(direct.iter().map(|id| scheme(id)));
    }
    let (a, b) = (*network.node(0).own(), *network.node(1).own());
    (network, a, b)
}

// spec/decisions/hubs-only.md: a scheme both run that is neither over a hub
// nor of unlimited trust would settle directly, so it never becomes a
// method: the pair peers on the scheme that trusts without limit, and
// never asks for the other.
#[test]
fn a_scheme_that_would_settle_directly_is_never_a_method() {
    let (mut network, a, b) = with_direct(74, &[SECOND]);
    wait(&mut network, |network| {
        methods(network, 0, &b) == [scheme(STUB)] && methods(network, 1, &a) == [scheme(STUB)]
    });
    settle(&mut network, 120_000);
    assert_eq!(methods(&network, 0, &b), [scheme(STUB)]);
    assert_eq!(methods(&network, 1, &a), [scheme(STUB)]);
    for index in 0..2 {
        let added = events(&network, index, |event| {
            matches!(event, Event::MethodAdded { .. })
        });
        assert_eq!(added, 0, "node {index} added a direct method");
    }
}

// spec/decisions/hubs-only.md: a pair whose only common schemes would settle
// directly shares no method, so it never peers.
#[test]
fn a_pair_that_could_only_settle_directly_never_peers() {
    let (mut network, a, b) = with_direct(75, &[STUB, SECOND]);
    settle(&mut network, 180_000);
    for (index, other) in [(0, b), (1, a)] {
        let known = events(&network, index, |event| *event == Event::NodeAdded(other));
        assert!(known > 0, "node {index} never learned of the other");
        assert!(network.node(index).peering_of(&other).is_none());
        let peered = events(&network, index, |event| {
            matches!(event, Event::Peered { .. })
        });
        assert_eq!(peered, 0, "node {index} peered");
    }
}

// spec/decisions/hubs-only.md: a node refuses a peering asked on a scheme
// that would settle directly for it, though the node that asks takes the
// scheme for one of unlimited trust: the asker is refused again and again,
// and the pair never peers.
#[test]
fn a_peering_asked_on_a_scheme_that_would_settle_directly_is_refused() {
    let mut network = Network::new(78, 2, 1, Config::DEV);
    network.nodes[1].hooks.direct.insert(scheme(STUB));
    let (a, b) = (*network.node(0).own(), *network.node(1).own());
    settle(&mut network, 180_000);
    let known = events(&network, 1, |event| *event == Event::NodeAdded(a));
    assert!(known > 0, "node 1 never learned of node 0");
    let refused = events(
        &network,
        0,
        |event| matches!(event, Event::PeeringFailed { node, reason: "refused" } if *node == b),
    );
    assert!(refused > 1, "node 0 was refused {refused} times");
    for (index, other) in [(0, b), (1, a)] {
        assert!(network.node(index).peering_of(&other).is_none());
        let peered = events(&network, index, |event| {
            matches!(event, Event::Peered { .. })
        });
        assert_eq!(peered, 0, "node {index} peered");
    }
}

// spec/decisions/hubs-only.md: a stored peering may have a method on a
// scheme that would settle directly, as one from when the XRP Ledger's
// schemes peered: the node drops it at its next review, without collecting,
// and tells the peer, which drops it at once; the peering goes on, on the
// other. One left with only such methods ends at once.
#[test]
fn a_method_that_would_settle_directly_is_dropped_and_the_peering_kept() {
    let (mut network, a, b) = both(79);
    network.nodes[0].hooks.direct.insert(scheme(SECOND));
    wait(&mut network, |network| {
        methods(network, 0, &b) == [scheme(STUB)] && methods(network, 1, &a) == [scheme(STUB)]
    });
    let dropped = |network: &Network, index, by_us| {
        events(network, index, |event| {
            matches!(event, Event::MethodDropped { scheme: s, by_us: u, .. }
                if *s == scheme(SECOND) && *u == by_us)
        })
    };
    assert_eq!(
        (dropped(&network, 0, true), dropped(&network, 1, false)),
        (1, 1)
    );
    let collected = events(&network, 0, |event| {
        matches!(event, Event::MethodWinding { .. })
    });
    assert_eq!(collected, 0, "collected on a method that settles nothing");
    // The pair does not add it back.
    settle(&mut network, 120_000);
    assert_eq!(methods(&network, 0, &b), [scheme(STUB)]);
    assert_eq!(methods(&network, 1, &a), [scheme(STUB)]);
    network.nodes[0].hooks.direct.insert(scheme(STUB));
    wait(&mut network, |network| {
        network.node(0).peering_of(&b).is_none() && network.node(1).peering_of(&a).is_none()
    });
}

// Each Prepare travels on the method it is sent on, and arrives naming it;
// none goes on a scheme the peering has no method on.
#[test]
fn prepares_travel_on_each_method() {
    let (mut network, _, b) = both(72);
    assert!(network.prepare(0, b, scheme(STUB)));
    assert!(network.prepare(0, b, scheme(SECOND)));
    settle(&mut network, 2_000);
    assert_eq!(network.prepares_taken.get(&scheme(STUB)), Some(&1));
    assert_eq!(network.prepares_taken.get(&scheme(SECOND)), Some(&1));
    assert!(network.misplaced.is_empty(), "{:?}", network.misplaced);
    let other = SchemeId::parse("other").unwrap();
    let now = network.clock(0);
    let sent = network
        .node_mut(0)
        .send_method_packet(now, &b, other, 9, &[12, 0], &[], 10_000);
    assert_eq!(sent, Err(SendError::NoMethod));
}

// A node that stops running a scheme drops its method on it and tells the
// peer, which drops it at once; the peering goes on, on the other, both
// still advertising it. Once the node runs the scheme again, the method
// comes back.
#[test]
fn a_method_dropped_leaves_the_peering() {
    let (mut network, a, b) = both(73);
    network.set_running(1, scheme(SECOND), false);
    settle(&mut network, 5_000);
    assert_eq!(methods(&network, 0, &b), [scheme(STUB)]);
    assert_eq!(methods(&network, 1, &a), [scheme(STUB)]);
    let dropped = |index, by_us| {
        events(&network, index, |event| {
            matches!(event, Event::MethodDropped { scheme: s, by_us: u, .. }
                if *s == scheme(SECOND) && *u == by_us)
        })
    };
    assert_eq!((dropped(1, true), dropped(0, false)), (1, 1));
    assert!(network.node(0).is_advertised(&b) && network.node(1).is_advertised(&a));
    network.set_running(1, scheme(SECOND), true);
    wait(&mut network, |network| {
        methods(network, 0, &b) == two() && methods(network, 1, &a) == two()
    });
}

// A peering left with no method ends: the node that drops its last tells
// the peer the peering is over, at once, without collecting.
#[test]
fn the_last_method_dropped_ends_the_peering() {
    let (mut network, a, b) = both(74);
    network.set_running(1, scheme(SECOND), false);
    settle(&mut network, 5_000);
    network.set_running(1, scheme(STUB), false);
    wait(&mut network, |network| {
        network.node(0).peering_of(&b).is_none() && network.node(1).peering_of(&a).is_none()
    });
    assert!(
        events(&network, 1, |event| matches!(
            event,
            Event::Terminating { by_us: true, .. }
        )) >= 1
    );
}

// A node that takes no further method, as the node does while it keeps one
// per peering, is not asked for one, and a request for one from a peer that
// asks anyway is refused: the peering keeps the one it was made on, across
// a restart too.
#[test]
fn a_node_that_takes_no_further_method_keeps_one() {
    let mut network = Network::new(75, 2, 1, Config::DEV);
    for index in 0..2 {
        network.set_running(index, scheme(SECOND), true);
        network.nodes[index].hooks.refuse_methods = true;
    }
    let (a, b) = (*network.node(0).own(), *network.node(1).own());
    wait(&mut network, |network| {
        network.node(0).is_advertised(&b) && network.node(1).is_advertised(&a)
    });
    // The responder alone refuses: the requester asks, and backs off.
    let asked = requester(&network, &b);
    network.nodes[asked].hooks.refuse_methods = false;
    settle(&mut network, 60_000);
    assert_eq!(methods(&network, 0, &b), [scheme(STUB)]);
    assert_eq!(methods(&network, 1, &a), [scheme(STUB)]);
    assert_eq!(
        events(&network, asked, |event| matches!(
            event,
            Event::MethodAdded { .. }
        )),
        0
    );
    let asks = network
        .trace
        .iter()
        .filter(|line| line.contains(&format!(" {asked} -> ")) && line.ends_with("MethodAdd"))
        .count();
    // Backing off: a few times, not at every heartbeat.
    assert!((1..=6).contains(&asks), "asked {asks} times in a minute");
    for index in 0..2 {
        network.crash(index);
        network.start(index);
        network.node(index).check_invariants();
    }
    assert_eq!(methods(&network, 0, &b), [scheme(STUB)]);
    assert_eq!(methods(&network, 1, &a), [scheme(STUB)]);
}

// A peering's methods are persisted with it: a node that restarts holds
// them as before, and settles on each.
#[test]
fn methods_survive_a_restart() {
    let (mut network, a, b) = both(76);
    network.crash(0);
    settle(&mut network, 5_000);
    network.start(0);
    assert_eq!(methods(&network, 0, &b), two());
    settle(&mut network, 30_000);
    assert_eq!(methods(&network, 0, &b), two());
    assert_eq!(methods(&network, 1, &a), two());
    assert!(network.prepare(0, b, scheme(SECOND)));
    settle(&mut network, 2_000);
    assert_eq!(network.prepares_taken.get(&scheme(SECOND)), Some(&1));
}

// A method only one side holds, as when the other's drop was lost with a
// restart, is repaired by the first Prepare on it: refused, and its sender
// told, as a node not peered is (spec/areas/network.md#one-sided-peerings).
#[test]
fn a_method_only_one_side_holds_is_repaired() {
    let (mut network, _, b) = both(77);
    let asked = requester(&network, &b);
    let other = 1 - asked;
    let ids = [*network.node(0).own(), *network.node(1).own()];
    network.cut.insert((other, asked));
    network.set_running(other, scheme(SECOND), false);
    settle(&mut network, 500);
    network.crash(other);
    network.cut.clear();
    network.start(other);
    assert_eq!(methods(&network, asked, &ids[other]), two());
    assert_eq!(methods(&network, other, &ids[asked]), [scheme(STUB)]);
    assert!(network.prepare(asked, ids[other], scheme(SECOND)));
    settle(&mut network, 5_000);
    assert_eq!(methods(&network, asked, &ids[other]), [scheme(STUB)]);
    assert!(network.node(asked).peering_of(&ids[other]).is_some());
}

// The same for a request to settle on a method only one side holds: it is
// refused, and its sender told, so it drops its side too
// (spec/areas/network.md#one-sided-peerings).
#[test]
fn a_request_to_settle_on_a_method_only_one_side_holds_repairs_it() {
    let (mut network, _, b) = both(87);
    let asked = requester(&network, &b);
    let other = 1 - asked;
    let ids = [*network.node(0).own(), *network.node(1).own()];
    network.cut.insert((other, asked));
    network.set_running(other, scheme(SECOND), false);
    settle(&mut network, 500);
    network.crash(other);
    network.cut.clear();
    network.start(other);
    settle(&mut network, 5_000);
    assert_eq!(methods(&network, asked, &ids[other]), two());
    assert_eq!(methods(&network, other, &ids[asked]), [scheme(STUB)]);
    network.act(asked, |node, now| {
        node.request_settlement(now, &ids[other], scheme(SECOND))
            .unwrap();
    });
    settle(&mut network, 5_000);
    assert_eq!(methods(&network, asked, &ids[other]), [scheme(STUB)]);
    assert_eq!(methods(&network, other, &ids[asked]), [scheme(STUB)]);
    let dropped = events(&network, asked, |event| {
        matches!(event, Event::MethodDropped { scheme: s, by_us: false, .. }
            if *s == scheme(SECOND))
    });
    assert_eq!(dropped, 1);
}

// A peer that takes our request for a method and drops the method before
// its acceptance reaches us: its drop ends our request, and the acceptance
// that arrives after it adds nothing, so neither side holds the method.
#[test]
fn a_method_dropped_before_its_acceptance_arrives_is_not_added() {
    let mut network = Network::new(88, 2, 1, Config::DEV);
    let (a, b) = (*network.node(0).own(), *network.node(1).own());
    wait(&mut network, |network| {
        methods(network, 0, &b) == [scheme(STUB)] && methods(network, 1, &a) == [scheme(STUB)]
    });
    let asked = requester(&network, &b);
    let other = 1 - asked;
    let ids = [a, b];
    network.set_running(other, scheme(SECOND), true);
    wait(&mut network, |network| {
        network
            .node(asked)
            .nodes()
            .iter()
            .any(|view| view.id == ids[other] && view.schemes.contains(&scheme(SECOND)))
    });
    // Every message takes a second from here on, so the peer's acceptance
    // is on its way for that long.
    network.faults = Faults {
        min_latency_ms: 1_000,
        max_latency_ms: 1_000,
        ..Faults::NONE
    };
    network.set_running(asked, scheme(SECOND), true);
    let accepted = |network: &Network| {
        events(network, other, |event| {
            matches!(event, Event::MethodAdded { scheme: s, inbound: true, .. }
                if *s == scheme(SECOND))
        }) == 1
    };
    for _ in 0..1_000 {
        if accepted(&network) {
            break;
        }
        settle(&mut network, 10);
    }
    assert!(accepted(&network), "the peer never took the method");
    // The peer drops it at once, and its drop overtakes its acceptance.
    network.faults = Faults::NONE;
    network.set_running(other, scheme(SECOND), false);
    settle(&mut network, 5_000);
    assert_eq!(methods(&network, asked, &ids[other]), [scheme(STUB)]);
    assert_eq!(methods(&network, other, &ids[asked]), [scheme(STUB)]);
    let added = events(&network, asked, |event| {
        matches!(event, Event::MethodAdded { .. })
    });
    assert_eq!(added, 0, "the late acceptance added the method");
}

// A link state that lists a scheme many times names one method on it: the
// methods with a node come from our own schemes, each once, however often
// its list repeats one, and a node seeking peers picks such a node and asks
// it how it peers.
#[test]
fn a_scheme_listed_again_is_one_method() {
    let mallory = NodeKey::from_bytes(&[91; 32]);
    let entries = core::iter::repeat_n(Entry::Scheme(scheme(STUB)), MAX_SCHEMES + 1)
        .chain(core::iter::repeat_n(
            Entry::Scheme(scheme(SECOND)),
            MAX_SCHEMES + 1,
        ))
        .collect();
    let info = NodeInfo {
        sequence: START,
        url: "https://mallory.test",
        alias: "",
        public_key: *mallory.public_key().as_bytes(),
        entries,
    };
    let stored = Stored {
        nodes: vec![StoredNode {
            id: mallory.node_id(),
            link_state: Some(sign_node_info(&mallory, &info)),
            sources: Sources::MAJORITY,
            first_seen_ms: START,
            registration: None,
            peering: None,
            termination: None,
        }],
        ..Stored::default()
    };
    let mut node = PeerNode::new(
        Config::DEV,
        key(0),
        OwnInfo {
            url: Some("https://n0.test".into()),
            alias: String::new(),
            feed_port: None,
            hub: None,
        },
        Vec::new(),
        stored,
        Box::new(SeededRng::new(78)),
        START,
    );
    node.set_schemes(START, vec![scheme(STUB), scheme(SECOND)]);
    let view = node
        .nodes()
        .into_iter()
        .find(|view| view.id == mallory.node_id())
        .unwrap();
    assert_eq!(view.schemes, [scheme(STUB), scheme(SECOND)]);
    let mut hooks = StubHooks::default();
    let mut asked = false;
    let mut now = START;
    for _ in 0..1_000 {
        node.handle_timeout(now, &mut hooks);
        while let Some(output) = node.poll_output() {
            asked |= matches!(output, Output::Request(request)
                if request.to == mallory.node_id() && request.kind == Kind::PeeringInfoRequest);
        }
        if asked {
            break;
        }
        now = node.poll_timeout().unwrap_or(now + 1_000).max(now + 1);
    }
    assert!(asked, "never asked the node how it peers");
    node.check_invariants();
}

// A peering being ended collects on its first method, in whose units the
// peer says what it has on its way: one that loses that method, its scheme
// no longer run, ends at once rather than collect on another.
#[test]
fn a_peering_being_ended_that_loses_its_first_method_ends() {
    let (mut network, _, b) = both(79);
    network.cut.insert((0, 1));
    network.cut.insert((1, 0));
    network.act(0, |node, now| {
        assert!(node.terminate(now, &b, TerminationReason::Owner));
    });
    network.set_running(0, scheme(STUB), false);
    settle(&mut network, 5_000);
    let dropped = events(
        &network,
        0,
        |event| matches!(event, Event::MethodDropped { scheme: s, .. } if *s == scheme(STUB)),
    );
    let told = events(&network, 0, |event| {
        matches!(
            event,
            Event::Terminating {
                by_us: true,
                reason: TerminationReason::Owner,
                ..
            }
        )
    });
    assert_eq!((dropped, told), (0, 1));
}

/// How many times node `index` asked `peer` to settle: the requests `peer`
/// took from it.
fn asked(network: &Network, index: usize, peer: &NodeId) -> usize {
    let from = *network.node(index).own();
    let to = (0..network.nodes.len())
        .find(|other| network.node(*other).own() == peer)
        .unwrap();
    network
        .settlement_requests
        .iter()
        .filter(|(at, sender)| *at == to && *sender == from)
        .count()
}

/// Whether node `index` winds down its method on `on` with `peer`.
fn winding(network: &Network, index: usize, peer: &NodeId, on: &str) -> bool {
    network
        .node(index)
        .peering_of(peer)
        .and_then(|peering| peering.method(scheme(on)))
        .is_some_and(dassie_peer::Method::winding_down)
}

// spec/decisions/settlement-methods.md: a method wound down is collected
// first. The node asks the peer to settle on it once, waits for what the
// peer said is on its way, and only once it is booked drops the method and
// tells the peer, which drops it at once; the peering goes on, on the
// other method, both still advertising it.
#[test]
fn a_method_wound_down_is_collected_once_then_dropped() {
    let (mut network, a, b) = both(80);
    network.promise.insert(1, 5);
    network.remove_scheme(0, scheme(SECOND));
    settle(&mut network, 5_000);
    assert!(
        winding(&network, 0, &b, SECOND),
        "dropped before collecting"
    );
    assert_eq!(methods(&network, 1, &a), two(), "the peer was told early");
    assert_eq!(asked(&network, 0, &b), 1);
    // A settlement on the other method counts for nothing here.
    network.act(0, |node, now| {
        node.settlement_booked(now, &b, scheme(STUB), 5);
    });
    settle(&mut network, 5_000);
    assert!(winding(&network, 0, &b, SECOND));
    network.act(0, |node, now| {
        node.settlement_booked(now, &b, scheme(SECOND), 5);
    });
    settle(&mut network, 5_000);
    assert_eq!(methods(&network, 0, &b), [scheme(STUB)]);
    assert_eq!(methods(&network, 1, &a), [scheme(STUB)]);
    assert_eq!(asked(&network, 0, &b), 1, "asked more than once");
    assert!(network.node(0).is_advertised(&b) && network.node(1).is_advertised(&a));
    assert_eq!(
        events(&network, 0, |event| matches!(
            event,
            Event::MethodWinding { .. }
        )),
        1
    );
}

// Each step is bounded: a settlement of ours on the method that never
// finishes is waited for no longer than the wait, and what the peer said is
// on its way, never booked, no longer than the wait again.
#[test]
fn a_method_wound_down_is_dropped_within_its_bound() {
    let (mut network, a, b) = both(81);
    let wait = Config::DEV.depeer_wait_ms;
    network.promise.insert(1, 5);
    network.nodes[0].hooks.settling.insert(b);
    network.remove_scheme(0, scheme(SECOND));
    settle(&mut network, wait - 2_000);
    assert_eq!(asked(&network, 0, &b), 0, "asked before its own settled");
    settle(&mut network, 4_000);
    assert_eq!(asked(&network, 0, &b), 1);
    assert!(winding(&network, 0, &b, SECOND));
    settle(&mut network, wait);
    assert_eq!(methods(&network, 0, &b), [scheme(STUB)]);
    assert_eq!(methods(&network, 1, &a), [scheme(STUB)]);
}

// A peering that winds down its last method ends once it is collected,
// told then; while it collects, the peer is no longer advertised.
#[test]
fn a_peering_that_winds_down_its_last_method_ends() {
    let mut network = Network::new(82, 2, 1, Config::DEV);
    let (a, b) = (*network.node(0).own(), *network.node(1).own());
    wait(&mut network, |network| {
        network.node(0).is_advertised(&b) && network.node(1).is_advertised(&a)
    });
    network.promise.insert(1, 5);
    network.act(0, |node, now| {
        assert!(node.wind_down(now, &b, scheme(STUB)));
    });
    settle(&mut network, 3_000);
    assert!(!network.node(0).is_advertised(&b), "still advertised");
    assert!(network.node(0).peering_of(&b).is_some());
    assert!(network.node(1).peering_of(&a).is_some());
    network.act(0, |node, now| {
        node.settlement_booked(now, &b, scheme(STUB), 5);
    });
    wait(&mut network, |network| {
        network.node(0).peering_of(&b).is_none() && network.node(1).peering_of(&a).is_none()
    });
    assert_eq!(asked(&network, 0, &b), 1);
}

// A method being wound down keeps its decision across a restart, and takes
// its steps again: the method is dropped within the bound counted from the
// start.
#[test]
fn a_method_wound_down_carries_on_after_a_restart() {
    let (mut network, a, b) = both(83);
    let wait = Config::DEV.depeer_wait_ms;
    network.nodes[0].hooks.settling.insert(b);
    network.remove_scheme(0, scheme(SECOND));
    settle(&mut network, 1_000);
    network.crash(0);
    settle(&mut network, 1_000);
    network.start(0);
    network.node(0).check_invariants();
    assert!(winding(&network, 0, &b, SECOND), "forgotten in the restart");
    settle(&mut network, 2 * wait + 5_000);
    assert_eq!(methods(&network, 0, &b), [scheme(STUB)]);
    assert_eq!(methods(&network, 1, &a), [scheme(STUB)]);
}

// spec/decisions/settlement-methods.md: a method on a scheme that cannot
// receive is dropped at once, collecting nothing, once it has not been able
// to for as long as a silent peer is kept; before then it stays.
#[test]
fn a_method_that_cannot_receive_is_dropped_after_a_silent_peers_time() {
    let (mut network, a, b) = both(84);
    let dead = Config::DEV.peer_dead_ms;
    // The node lists it no more while it cannot receive.
    network.schemes.insert(0, vec![scheme(STUB)]);
    network.act(0, |node, now| node.set_schemes(now, vec![scheme(STUB)]));
    network.nodes[0].hooks.unpayable.insert(scheme(SECOND));
    settle(&mut network, dead - 10_000);
    assert_eq!(methods(&network, 0, &b), two());
    settle(&mut network, 20_000);
    assert_eq!(methods(&network, 0, &b), [scheme(STUB)]);
    assert_eq!(methods(&network, 1, &a), [scheme(STUB)]);
    assert_eq!(asked(&network, 0, &b), 0, "collected on it");
    assert_eq!(
        events(&network, 0, |event| matches!(
            event,
            Event::MethodWinding { .. }
        )),
        0
    );
}

// A scheme that can receive again before then keeps its method, and its
// time starts again.
#[test]
fn a_scheme_that_can_receive_again_keeps_its_method() {
    let (mut network, _, b) = both(85);
    let dead = Config::DEV.peer_dead_ms;
    network.nodes[0].hooks.unpayable.insert(scheme(SECOND));
    settle(&mut network, dead - 10_000);
    network.nodes[0].hooks.unpayable.clear();
    settle(&mut network, 5_000);
    network.nodes[0].hooks.unpayable.insert(scheme(SECOND));
    settle(&mut network, dead - 10_000);
    assert_eq!(methods(&network, 0, &b), two());
}

// The owner's removal of a scheme, as the simulation's nodes do it: the
// node lists it no more, winds down its methods on it, and runs it until
// each is dropped. The peering is kept on the other method, and the
// method is not added back.
#[test]
fn a_scheme_removed_winds_down_its_methods() {
    let (mut network, a, b) = both(86);
    network.remove_scheme(0, scheme(SECOND));
    assert!(winding(&network, 0, &b, SECOND));
    assert!(network.removing.contains(&(0, scheme(SECOND))));
    settle(&mut network, 2 * Config::DEV.depeer_wait_ms + 5_000);
    network.finish_removals();
    assert!(network.removing.is_empty(), "the scheme still runs");
    settle(&mut network, 60_000);
    assert_eq!(methods(&network, 0, &b), [scheme(STUB)]);
    assert_eq!(methods(&network, 1, &a), [scheme(STUB)]);
    assert!(network.node(0).is_advertised(&b) && network.node(1).is_advertised(&a));
}
