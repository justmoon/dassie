//! Method offers (spec/decisions/method-offers.md,
//! spec/areas/network.md#settlement-methods), on simulated nodes whose
//! messages a test lays out: crossed requests both taken, the schedules
//! that left both ends of a peering holding it as the other's and neither
//! asking for a method, offers copied, late or from an earlier peering,
//! the gate on our packets, the room for accounts an offer keeps, and a
//! peer of the release before.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use dassie_identity::NodeId;
use dassie_oer::{Decode, Encode};
use dassie_peer::sim::{Arriving, Network, SECOND, START, STUB, Scripted, key, scheme};
use dassie_peer::wire::{
    Content, Kind, MethodOffer, MethodResponse, OfferedMethod, PeeringResponse, SchemeId,
    TerminationReason,
};
use dassie_peer::{Config, Event};

fn settle(network: &mut Network, ms: u64) {
    let until = network.now + ms;
    network.run_until(until);
}

/// Runs the network for up to five minutes, until `done`.
fn wait(network: &mut Network, done: impl Fn(&Network) -> bool) {
    for _ in 0..300 {
        if done(network) {
            return;
        }
        settle(network, 1_000);
    }
    assert!(done(network), "never came to pass");
}

/// The schemes of node `index`'s methods with `peer`, sorted.
fn methods(network: &Network, index: usize, peer: &NodeId) -> Vec<SchemeId> {
    let mut schemes: Vec<SchemeId> = network
        .node(index)
        .peering_of(peer)
        .map(|peering| peering.schemes().collect())
        .unwrap_or_default();
    schemes.sort_unstable();
    schemes
}

fn two() -> Vec<SchemeId> {
    vec![scheme(STUB), scheme(SECOND)]
}

fn id(index: usize) -> NodeId {
    key(index).node_id()
}

/// Two nodes that run both schemes from the start, not yet peered.
fn pair(seed: u64) -> Network {
    let mut network = Network::new(seed, 2, 1, Config::DEV);
    for index in 0..2 {
        network.set_running(index, scheme(SECOND), true);
    }
    network
}

/// Whether both nodes hold their peering with both methods, each
/// confirmed.
fn agreed(network: &Network) -> bool {
    (0..2).all(|index| {
        let peer = id(1 - index);
        methods(network, index, &peer) == two()
            && network
                .node(index)
                .peering_of(&peer)
                .is_some_and(|peering| two().into_iter().all(|s| peering.confirmed(s)))
    })
}

/// How many times node `index` sent a request of `kind`.
fn sent(network: &Network, index: usize, kind: &str) -> usize {
    let prefix = format!(" {index} -> ");
    network
        .trace
        .iter()
        .filter(|line| line.contains(&prefix) && line.ends_with(kind))
        .count()
}

/// The index of the two whose node ID is lower.
fn lower() -> usize {
    usize::from(id(1) < id(0))
}

// spec/decisions/method-offers.md: two nodes that ask each other at once
// both take the other's request, rather than refuse the higher ID's. Each
// counts the peering as one it asked for, so neither takes an inbound
// place for it, and both settle on both schemes, nobody asking for a
// method.
#[test]
fn crossed_requests_are_both_taken() {
    let mut network = pair(201);
    // Each request is on its way long enough for the other's to start.
    for (from, to) in [(0, 1), (1, 0)] {
        network
            .script
            .push(Scripted::on(from, to, Kind::PeeringRequest).delayed(4_000));
    }
    wait(&mut network, agreed);
    assert_eq!(
        (
            sent(&network, 0, "PeeringRequest"),
            sent(&network, 1, "PeeringRequest")
        ),
        (1, 1)
    );
    for index in 0..2 {
        let peering = network.node(index).peering_of(&id(1 - index)).unwrap();
        assert!(!peering.inbound, "node {index} counts it as asked of it");
    }
    let refused = network
        .events
        .iter()
        .filter(|(_, event)| matches!(event, Event::PeeringFailed { .. }))
        .count();
    assert_eq!(refused, 0, "a crossed request was refused");
    assert_eq!(
        sent(&network, 0, "MethodAdd") + sent(&network, 1, "MethodAdd"),
        0
    );
    for index in 0..2 {
        network.node(index).check_invariants();
    }
}

// The first schedule of the campaign's `peer/liveness/finished` failure
// (seed 4111319787455610), on two nodes: both ask at once; the higher ID's
// request arrives twice; the lower ID's answer is lost, so its attempt
// fails while the other recorded the peering. Before, each end then held
// the peering as the other's, and neither asked for the second scheme.
#[test]
fn a_lost_answer_and_a_request_that_arrives_twice_still_agree_both_methods() {
    let mut network = pair(202);
    let (low, high) = (lower(), 1 - lower());
    network.script.push(
        Scripted::on(low, high, Kind::PeeringRequest)
            .delayed(3_000)
            .answer_lost(),
    );
    network.script.push(
        Scripted::on(high, low, Kind::PeeringRequest)
            .delayed(2_000)
            .copied(6_000),
    );
    wait(&mut network, agreed);
    let failed = network
        .events
        .iter()
        .filter(|(at, event)| *at == low && matches!(event, Event::PeeringFailed { .. }))
        .count();
    assert!(failed <= 1);
    for index in 0..2 {
        network.node(index).check_invariants();
    }
}

// The second schedule (seed 640815090597371): the lower ID's attempt fails
// with no answer, which ends its asking; the higher ID, still asking, takes
// the lower's request; the higher's request, still on its way, then reaches
// a node no longer asking, which takes it too. The lower holds the peering
// as asked of it, the higher as its own, and both settle on both schemes.
#[test]
fn a_request_that_reaches_a_node_no_longer_asking_still_agrees_both_methods() {
    let mut network = pair(203);
    let (low, high) = (lower(), 1 - lower());
    network.script.push(
        Scripted::on(low, high, Kind::PeeringRequest)
            .delayed(5_000)
            .answer_lost(),
    );
    // The higher's request is on its way until the lower's attempt has
    // failed: it reaches the lower then, as a node that no longer asks,
    // before the higher's first offer does. (An offer that comes first
    // finds no peering, and the lower tells the higher so; they peer
    // again, as any one-sided peering does.)
    network
        .script
        .push(Scripted::on(high, low, Kind::PeeringRequest).delayed(600_000));
    network
        .script
        .push(Scripted::on(high, low, Kind::MethodOffer).delayed(20_000));
    wait(&mut network, |network| {
        network
            .events
            .iter()
            .any(|(at, event)| *at == low && matches!(event, Event::PeeringFailed { .. }))
    });
    assert_eq!(
        sent(&network, high, "PeeringRequest"),
        1,
        "the higher asked too"
    );
    let info = stranger_info(&network, high, &two());
    let content = Content::PeeringRequest(dassie_peer::wire::PeeringRequest {
        scheme: scheme(STUB),
        node_info: dassie_peer::wire::SignedNodeInfo::decode_exact(&info).unwrap(),
        data: &[],
    });
    network.inbound(low, &Arriving::new(&key(high), &content));
    wait(&mut network, agreed);
    let (at_low, at_high) = (
        network.node(low).peering_of(&id(high)).unwrap().inbound,
        network.node(high).peering_of(&id(low)).unwrap().inbound,
    );
    assert!(!at_high, "the higher ID asked, and counts it as its own");
    assert!(
        at_low,
        "the lower ID no longer asked when the request reached it"
    );
}

// The state both schedules left before: each end holds the peering as the
// other's, on one scheme. Nobody needs to ask: each offers the other the
// second scheme, and both adopt it.
#[test]
fn a_peering_both_ends_hold_as_asked_of_them_gains_its_methods() {
    let mut network = Network::new(204, 2, 1, Config::DEV);
    wait(&mut network, |network| {
        (0..2).all(|index| methods(network, index, &id(1 - index)) == [scheme(STUB)])
    });
    for index in 0..2 {
        network.crash(index);
        network.edit_stored_peering(index, &id(1 - index), |peering| peering.inbound = true);
    }
    for index in 0..2 {
        network.start(index);
        network.set_running(index, scheme(SECOND), true);
    }
    wait(&mut network, agreed);
    for index in 0..2 {
        assert!(
            network
                .node(index)
                .peering_of(&id(1 - index))
                .unwrap()
                .inbound
        );
    }
}

// spec/decisions/method-offers.md: two nodes with room for one core peer's
// accounts each, that prefer different schemes, cross their requests. Each
// asks on the lowest scheme ID, so both peer on the same one; neither opens
// accounts on the other, and neither offers it, so no pair is left unused.
#[test]
fn a_crossing_at_the_last_pair_agrees_on_the_lowest_scheme() {
    let mut network = pair(205);
    for index in 0..2 {
        network.nodes[index].hooks.max_pairs = Some(1);
    }
    // Node 1 prefers the second scheme.
    network
        .schemes
        .insert(1, vec![scheme(SECOND), scheme(STUB)]);
    let offers = network.offers(1);
    network.act(1, |node, now| node.set_offers(now, offers));
    for (from, to) in [(0, 1), (1, 0)] {
        network
            .script
            .push(Scripted::on(from, to, Kind::PeeringRequest).delayed(4_000));
    }
    wait(&mut network, |network| {
        (0..2).all(|index| network.node(index).peering_of(&id(1 - index)).is_some())
    });
    settle(&mut network, 60_000);
    for index in 0..2 {
        let peer = id(1 - index);
        assert_eq!(methods(&network, index, &peer), [scheme(STUB)]);
        let pairs: Vec<_> = network.nodes[index].hooks.pairs.iter().copied().collect();
        assert_eq!(
            pairs,
            [(peer, scheme(STUB))],
            "node {index} left a pair unused"
        );
        network.node(index).check_invariants();
    }
}

// spec/decisions/method-offers.md: an offer is a commitment, so a scheme
// that would open a core peer's accounts is offered only while the core
// peers' share keeps a pair for it, and the pair stays kept until the peer
// takes the method or turns it down. A third node asking meanwhile for a
// peering that would open accounts is refused for room; once the peer has
// turned the scheme down, the pair is free, and it peers.
#[test]
fn an_offer_keeps_its_pair_until_the_peer_turns_it_down() {
    let mut network = Network::new(206, 3, 1, Config::DEV);
    let b = 1;
    let (a, c) = (0, 2);
    // Only a and b to start: c joins later.
    network.crash(c);
    wait(&mut network, |network| {
        methods(network, b, &id(a)) == [scheme(STUB)]
            && methods(network, a, &id(b)) == [scheme(STUB)]
    });
    // b has room for one more pair, and runs the second scheme; a, which
    // runs it too, takes no newcomer on it, so it never offers it.
    network.nodes[b].hooks.max_pairs = Some(network.nodes[b].hooks.pairs.len() + 1);
    network.nodes[a].hooks.shut.insert((scheme(SECOND), false));
    network.set_running(a, scheme(SECOND), true);
    // b's offer with it, and a's answer, take a while: the pair is kept.
    network
        .script
        .push(Scripted::on(b, a, Kind::MethodOffer).delayed(8_000));
    network.set_running(b, scheme(SECOND), true);
    settle(&mut network, 3_000);
    assert!(
        network
            .node(b)
            .peering_of(&id(a))
            .unwrap()
            .offered
            .contains(&scheme(SECOND)),
        "b offers a the second scheme"
    );
    // c, peering with b on the stub, needs a pair: refused for room while
    // b's offer keeps it.
    network.start(c);
    settle(&mut network, 3_000);
    assert!(network.node(b).peering_of(&id(c)).is_none());
    // a turns it down: b leaves it out, and c peers.
    wait(&mut network, |network| {
        network.node(b).peering_of(&id(c)).is_some()
    });
    assert_eq!(methods(&network, b, &id(a)), [scheme(STUB)]);
    assert!(network.nodes[b].hooks.pairs.len() <= network.nodes[b].hooks.max_pairs.unwrap());
    network.node(b).check_invariants();
}

// spec/decisions/method-offers.md: our attempt to peer keeps a pair too,
// for as long as it is on its way, so a core node's request that arrives
// meanwhile and would open accounts is refused for room.
#[test]
fn an_attempt_keeps_its_pair() {
    let mut network = Network::new(207, 3, 1, Config::DEV);
    network.set_dev_core(&[0, 1, 2]);
    let asker = 0;
    network.nodes[asker].hooks.max_pairs = Some(1);
    for to in [1, 2] {
        network
            .script
            .push(Scripted::on(asker, to, Kind::PeeringRequest).delayed(20_000));
    }
    wait(&mut network, |network| {
        sent(network, asker, "PeeringRequest") == 1
    });
    let asked = (1..3)
        .find(|to| {
            let target = format!("{:?}", id(*to));
            network
                .trace
                .iter()
                .any(|line| line.contains(&format!(" {asker} -> {target} PeeringRequest")))
        })
        .unwrap();
    let other = 3 - asked;
    assert!(network.node(asker).peering_of(&id(other)).is_none());
    let info = stranger_info(&network, other, &[scheme(STUB)]);
    let content = Content::PeeringRequest(dassie_peer::wire::PeeringRequest {
        scheme: scheme(STUB),
        node_info: dassie_peer::wire::SignedNodeInfo::decode_exact(&info).unwrap(),
        data: &[],
    });
    let reply = network.inbound(asker, &Arriving::new(&key(other), &content));
    assert_eq!(reply.status, 200);
    assert_eq!(
        PeeringResponse::decode_exact(&reply.body).unwrap(),
        PeeringResponse::Refused(dassie_peer::wire::RefusalReason::Capacity)
    );
    assert!(network.node(asker).peering_of(&id(other)).is_none());
    // Once its attempt made its peering, the pair is the peering's.
    wait(&mut network, |network| {
        network.node(asker).peering_of(&id(asked)).is_some()
    });
    assert_eq!(network.nodes[asker].hooks.pairs.len(), 1);
}

/// A signed link state of node `index`'s, offering `schemes`, as old as
/// the network: one its URL's answer is at least as new as.
fn stranger_info(_: &Network, index: usize, schemes: &[SchemeId]) -> Vec<u8> {
    let from = key(index);
    let url = dassie_peer::sim::url(index);
    let info = dassie_peer::wire::NodeInfo {
        sequence: START,
        url: &url,
        alias: "",
        public_key: *from.public_key().as_bytes(),
        slh_dsa_public_key: *from.identity().slh_dsa().as_bytes(),
        settlement: schemes
            .iter()
            .map(|scheme| dassie_peer::wire::SettlementOffer {
                scheme: *scheme,
                details: &[],
            })
            .collect(),
        entries: Vec::new(),
    };
    dassie_peer::wire::sign_node_info(&from, &info)
}

/// Node `from`'s offer to node `to`, as `methodOffer` would carry it.
fn offer(version: u64, seen: u64, schemes: &[SchemeId]) -> Vec<u8> {
    Content::MethodOffer(MethodOffer {
        version,
        seen,
        methods: schemes
            .iter()
            .map(|scheme| OfferedMethod {
                scheme: *scheme,
                data: &[],
            })
            .collect(),
    })
    .to_vec()
}

// spec/decisions/method-offers.md: an offer older than one taken in, or a
// copy of it, changes nothing, though it would drop a method; one that says
// it has seen a version of ours we never took is refused. Each answer is
// our own offer as it stands.
#[test]
fn old_and_copied_offers_change_nothing() {
    let mut network = pair(208);
    wait(&mut network, agreed);
    let channel = network.node(0).channel(&id(1)).unwrap().clone();
    let from = key(1);
    // Older than the newest taken in, and a copy of it: both without the
    // second scheme.
    for version in [channel.offer_in - 1, channel.offer_in] {
        let reply = network.inbound(
            0,
            &Arriving::of(&from, offer(version, channel.offer_out, &[scheme(STUB)])),
        );
        assert_eq!(reply.status, 200);
        let ours = MethodOffer::decode_exact(&reply.body).unwrap();
        assert_eq!(ours.version, channel.offer_out);
        assert_eq!(methods(&network, 0, &id(1)), two());
    }
    let reply = network.inbound(
        0,
        &Arriving::of(
            &from,
            offer(channel.offer_in + 1, channel.offer_out + 1, &two()),
        ),
    );
    assert_eq!(reply.status, 400);
    network.node(0).check_invariants();
}

// spec/decisions/method-offers.md: versions belong to the pair, on its
// settlement channel, and never start again. A delayed offer of the pair's
// earlier peering, carrying no method, reaches its new peering, and is
// not taken: the new peering keeps its methods.
#[test]
fn an_offer_from_an_earlier_peering_is_not_taken() {
    let mut network = pair(209);
    wait(&mut network, agreed);
    let old = network.node(0).channel(&id(1)).unwrap().offer_in;
    network.act(0, |node, now| {
        assert!(node.terminate(now, &id(1), TerminationReason::Unspecified));
    });
    wait(&mut network, |network| {
        network.node(0).peering_of(&id(1)).is_none()
    });
    // Peered again: a new peering of the pair.
    wait(&mut network, agreed);
    assert!(network.node(0).channel(&id(1)).unwrap().offer_in > old);
    let reply = network.inbound(0, &Arriving::of(&key(1), offer(old, 0, &[])));
    assert_eq!(reply.status, 200);
    assert_eq!(methods(&network, 0, &id(1)), two());
    network.node(0).check_invariants();
}

// spec/decisions/method-offers.md: our packets take a method only once the
// peer has seen an offer of ours that carried it. The node that accepted a
// peering waits for the requester's first offer; until then it sends no
// Prepare on the peering's first method, while it takes the peer's.
#[test]
fn the_node_that_accepted_waits_for_the_first_offer() {
    let mut network = pair(210);
    // Every offer takes a while.
    for (from, to) in [(0, 1), (1, 0)] {
        network
            .script
            .push(Scripted::on(from, to, Kind::MethodOffer).delayed(10_000));
    }
    wait(&mut network, |network| {
        (0..2).all(|index| network.node(index).peering_of(&id(1 - index)).is_some())
    });
    let mut waiting = 0;
    for index in 0..2 {
        let peering = network.node(index).peering_of(&id(1 - index)).unwrap();
        let first = peering.first().scheme;
        if peering.confirmed(first) {
            // Its own request's acceptance confirmed it.
            assert!(network.prepare(index, id(1 - index), first));
        } else {
            waiting += 1;
            assert!(
                !network.prepare(index, id(1 - index), first),
                "sent before confirmed"
            );
            assert!(!network.node(index).is_advertised(&id(1 - index)));
        }
    }
    assert!(waiting >= 1, "the node that accepted waits");
    wait(&mut network, agreed);
    for index in 0..2 {
        assert!(network.prepare(index, id(1 - index), scheme(STUB)));
    }
    settle(&mut network, 2_000);
    assert!(network.misplaced.is_empty());
}

// spec/decisions/method-offers.md: a method withdrawn is left out of the
// next offer; a Prepare of the peer's that crosses it is refused, with no
// drop sent after it, since the offer is on its way.
#[test]
fn a_withdrawn_method_needs_no_drop_message() {
    let mut network = pair(211);
    wait(&mut network, agreed);
    network
        .script
        .push(Scripted::on(0, 1, Kind::MethodOffer).delayed(5_000));
    // Node 0 no longer runs the second scheme: it drops the method at once.
    network.set_running(0, scheme(SECOND), false);
    settle(&mut network, 500);
    assert_eq!(methods(&network, 0, &id(1)), [scheme(STUB)]);
    assert!(network.prepare(1, id(0), scheme(SECOND)));
    settle(&mut network, 1_000);
    assert_eq!(network.one_sided, 1, "the crossing Prepare was refused");
    wait(&mut network, |network| {
        methods(network, 1, &id(0)) == [scheme(STUB)]
    });
    assert_eq!(sent(&network, 0, "MethodDrop"), 0);
    for index in 0..2 {
        network.node(index).check_invariants();
    }
}

// spec/decisions/method-offers.md: a peer of the release before answers
// an offer with 400. Every method held with it counts as confirmed, as
// they did, it is offered nothing new, and a method it asks for
// (`methodAdd`) is taken if we would offer it; one we drop it is told of.
#[test]
fn a_peer_of_the_release_before_is_answered_as_before() {
    let mut network = Network::new(212, 2, 1, Config::DEV);
    network.refuse_offers.insert(1);
    wait(&mut network, |network| {
        (0..2).all(|index| network.node(index).peering_of(&id(1 - index)).is_some())
    });
    wait(&mut network, |network| {
        network.node(0).speaks_offers(&id(1)) == Some(false)
    });
    let peering = network.node(0).peering_of(&id(1)).unwrap();
    assert!(peering.confirmed(peering.first().scheme));
    for index in 0..2 {
        network.set_running(index, scheme(SECOND), true);
    }
    settle(&mut network, 5_000);
    assert_eq!(
        methods(&network, 0, &id(1)),
        [scheme(STUB)],
        "nothing new offered"
    );
    let content = Content::MethodAdd {
        scheme: scheme(SECOND),
        data: &[],
    };
    let reply = network.inbound(0, &Arriving::new(&key(1), &content));
    assert_eq!(
        MethodResponse::decode_exact(&reply.body).unwrap(),
        MethodResponse::Accepted
    );
    assert!(
        network
            .node(0)
            .peering_of(&id(1))
            .unwrap()
            .confirmed(scheme(SECOND))
    );
    network.set_running(0, scheme(SECOND), false);
    settle(&mut network, 1_000);
    assert_eq!(methods(&network, 0, &id(1)), [scheme(STUB)]);
    assert!(sent(&network, 0, "MethodDrop") >= 1);
    network.node(0).check_invariants();
}
