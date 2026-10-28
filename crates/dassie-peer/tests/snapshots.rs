//! Snapshot subscriptions in the peer protocol
//! (spec/decisions/peer-swaps.md,
//! spec/areas/network.md#snapshot-subscriptions), on simulated nodes: who
//! subscribes to whom, what a publisher pushes, who refuses, and how a
//! subscription ends and comes back.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use std::sync::Arc;

use dassie_identity::NodeId;
use dassie_oer::Decode;
use dassie_peer::sim::{Arriving, Network, SECOND, STUB, forger, key, scheme};
use dassie_peer::wire::{
    Content, RefusalReason, SchemeId, SubscriptionResponse, TerminationReason,
};
use dassie_peer::{Config, SUBSCRIPTION_CHECK_MS, SUBSCRIPTION_SILENCE_MS};

fn settle(network: &mut Network, ms: u64) {
    let until = network.now + ms;
    network.run_until(until);
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

fn methods(network: &Network, index: usize, peer: &NodeId) -> Vec<SchemeId> {
    let mut schemes: Vec<SchemeId> = network
        .node(index)
        .peering_of(peer)
        .map(|peering| peering.schemes().collect())
        .unwrap_or_default();
    schemes.sort_unstable();
    schemes
}

/// Two nodes, peered, that swap as `swaps` says of each and run the second
/// scheme as `second` says: with it, their peering has two methods.
fn pair(seed: u64, swaps: [bool; 2], second: bool) -> (Network, NodeId, NodeId) {
    let mut network = Network::new(seed, 2, 1, Config::DEV);
    for (index, swaps) in swaps.into_iter().enumerate() {
        network.nodes[index].hooks.swaps = swaps;
        if second {
            network.set_running(index, scheme(SECOND), true);
        }
    }
    let (a, b) = (*network.node(0).own(), *network.node(1).own());
    let wanted = if second {
        vec![scheme(STUB), scheme(SECOND)]
    } else {
        vec![scheme(STUB)]
    };
    wait(&mut network, |network| {
        methods(network, 0, &b) == wanted && methods(network, 1, &a) == wanted
    });
    (network, a, b)
}

/// A snapshot's bytes, opaque to the peer protocol.
fn snapshot(byte: u8) -> Arc<[u8]> {
    Arc::from(vec![byte; 100])
}

fn subscription(reply: &dassie_peer::Reply) -> SubscriptionResponse {
    assert_eq!(reply.status, 200, "{reply:?}");
    SubscriptionResponse::decode_exact(&reply.body).unwrap()
}

// A peering on two methods, between nodes that swap, subscribes both ways;
// a new subscriber is sent the publisher's snapshot at once, and every
// subscriber each one it pushes after.
#[test]
fn a_peering_on_two_methods_subscribes_both_ways_and_gets_every_push() {
    let (mut network, a, b) = pair(81, [true, true], true);
    network.act(1, |node, _| {
        assert!(node.set_snapshot(Some(snapshot(1))));
    });
    wait(&mut network, |network| {
        network.node(0).subscribed_to(&b) && network.node(1).subscribed_to(&a)
    });
    assert_eq!(network.node(1).subscribers().collect::<Vec<_>>(), [&a]);
    assert_eq!(network.node(0).subscribers().collect::<Vec<_>>(), [&b]);
    // Sent at subscription, without a push.
    assert_eq!(network.node(0).snapshot_of(&b), Some(&[1; 100][..]));
    // Node 0 has none to send yet.
    assert_eq!(network.node(1).snapshot_of(&a), None);

    network.act(1, |node, now| {
        assert!(node.set_snapshot(Some(snapshot(2))));
        // The same bytes again are no change.
        assert!(!node.set_snapshot(Some(snapshot(2))));
        node.push_snapshot(now);
    });
    settle(&mut network, 1_000);
    assert_eq!(network.node(0).snapshot_of(&b), Some(&[2; 100][..]));
    assert_eq!(
        network.node(0).snapshots().collect::<Vec<_>>(),
        [(&b, &[2; 100][..])]
    );
    network.node(0).check_invariants();
    network.node(1).check_invariants();
}

// A push waits for none in flight: a second push before the first is
// answered sends nothing to that subscriber, which gets the next.
#[test]
fn at_most_one_push_is_in_flight_per_subscriber() {
    let (mut network, a, b) = pair(82, [true, true], true);
    wait(&mut network, |network| network.node(0).subscribed_to(&b));
    let _ = a;
    let before = network.node(1).stats().requests_sent;
    network.act(1, |node, now| {
        node.set_snapshot(Some(snapshot(3)));
        node.push_snapshot(now);
        node.set_snapshot(Some(snapshot(4)));
        node.push_snapshot(now);
    });
    assert_eq!(network.node(1).stats().requests_sent, before + 1);
    settle(&mut network, 1_000);
    assert_eq!(network.node(0).snapshot_of(&b), Some(&[3; 100][..]));
    network.act(1, |node, now| node.push_snapshot(now));
    settle(&mut network, 1_000);
    assert_eq!(network.node(0).snapshot_of(&b), Some(&[4; 100][..]));
}

// A peer that unsubscribes and subscribes again, over and over, while a
// push to it is unanswered, is sent no second one: the push in flight
// stands for the one a subscription sends at once, so one peer never fills
// the node's requests in flight. Once it is answered, pushes go again.
#[test]
fn subscribing_again_sends_no_second_push_while_one_is_in_flight() {
    let (mut network, a, b) = pair(89, [true, true], true);
    wait(&mut network, |network| {
        network.node(0).subscribed_to(&b) && network.node(1).subscribed_to(&a)
    });
    network.act(1, |node, _| {
        node.set_snapshot(Some(snapshot(5)));
    });
    let before = network.node(1).stats().requests_sent;
    // Nothing is delivered while the requests are handed in directly, so
    // the first push stays unanswered.
    for subscribe in [true, false, true, false, true] {
        let content = Content::Subscription { subscribe };
        let reply = network.inbound(1, &Arriving::new(&key(0), &content));
        assert_eq!(subscription(&reply), SubscriptionResponse::Accepted);
        network.node(1).check_invariants();
    }
    assert_eq!(
        network.node(1).stats().requests_sent,
        before + 1,
        "one push"
    );
    network.act(1, |node, now| node.push_snapshot(now));
    assert_eq!(
        network.node(1).stats().requests_sent,
        before + 1,
        "still one"
    );
    settle(&mut network, 1_000);
    assert_eq!(network.node(0).snapshot_of(&b), Some(&[5; 100][..]));
    let answered = network.node(1).stats().requests_sent;
    network.act(1, |node, now| node.push_snapshot(now));
    assert_eq!(
        network.node(1).stats().requests_sent,
        answered + 1,
        "answered"
    );
}

// A peering on one method has nothing to swap between: neither side
// subscribes, and a node that does not swap subscribes to nobody.
#[test]
fn only_a_peering_on_two_methods_is_subscribed_to() {
    let (mut network, a, b) = pair(83, [true, true], false);
    settle(&mut network, 4 * SUBSCRIPTION_CHECK_MS);
    assert!(!network.node(0).subscribed_to(&b));
    assert!(!network.node(1).subscribed_to(&a));
    assert!(!network.node(0).has_subscribers() && !network.node(1).has_subscribers());
}

// A node that offers no swaps refuses a subscription, saying so; the
// subscriber keeps no subscription, and asks again only after a long
// wait.
#[test]
fn a_node_that_offers_no_swaps_refuses_saying_so() {
    let (mut network, _, b) = pair(84, [true, false], true);
    settle(&mut network, 4 * SUBSCRIPTION_CHECK_MS);
    assert!(!network.node(0).subscribed_to(&b));
    assert!(!network.node(1).has_subscribers());
    let asked = Arriving::new(&key(0), &Content::Subscription { subscribe: true });
    let reply = network.inbound(1, &asked);
    assert_eq!(
        subscription(&reply),
        SubscriptionResponse::Refused(RefusalReason::NoSwaps)
    );
    // Turned on, the owner's peers come back once their wait is over.
    network.nodes[1].hooks.swaps = true;
    settle(
        &mut network,
        dassie_peer::SUBSCRIPTION_REFUSED_MS + 2 * SUBSCRIPTION_CHECK_MS,
    );
    assert!(network.node(0).subscribed_to(&b));
}

// Subscriptions and snapshots are a peer's alone: a stranger's are
// refused as not peered, and a snapshot from a peer we do not subscribe to
// is refused too, which tells its publisher to stop.
#[test]
fn strangers_and_peers_not_subscribed_to_are_refused() {
    let (mut network, _, _) = pair(85, [true, true], false);
    let copier = forger();
    for content in [
        Content::Subscription { subscribe: true },
        Content::Subscription { subscribe: false },
        Content::Snapshot(&[7; 10]),
    ] {
        let reply = network.inbound(1, &Arriving::new(&copier, &content));
        assert_eq!(reply.status, 409, "{content:?}");
    }
    // Peered on one method, node 1 is not subscribed to node 0.
    let pushed = Arriving::new(&key(0), &Content::Snapshot(&[7; 10]));
    assert_eq!(network.inbound(1, &pushed).status, 409);
    // A malformed snapshot is malformed.
    let empty = Arriving::of(&key(0), vec![0x94, 0x00]);
    assert_eq!(network.inbound(1, &empty).status, 400);
}

// A subscription ends with its peering, on both sides, and a node that
// stops swapping unsubscribes, which its peer takes at once.
#[test]
fn subscriptions_end_with_the_peering_and_with_swapping() {
    let (mut network, a, b) = pair(86, [true, true], true);
    wait(&mut network, |network| {
        network.node(0).subscribed_to(&b) && network.node(1).subscribed_to(&a)
    });
    network.nodes[0].hooks.swaps = false;
    settle(&mut network, 2 * SUBSCRIPTION_CHECK_MS);
    assert!(!network.node(0).subscribed_to(&b));
    assert_eq!(network.node(0).snapshot_of(&b), None);
    assert!(!network.node(1).has_subscribers(), "told at once");
    // Node 0 offers no more either: node 1's subscription goes.
    assert!(!network.node(0).has_subscribers());

    network.nodes[0].hooks.swaps = true;
    wait(&mut network, |network| network.node(0).subscribed_to(&b));
    network.act(0, |node, now| {
        assert!(node.terminate(now, &b, TerminationReason::Owner));
    });
    // Ours collects first, and the peering stays active until it tells.
    wait(&mut network, |network| {
        (0..2).all(|index| network.node(index).active_peers().next().is_none())
    });
    for index in 0..2 {
        let node = network.node(index);
        assert!(!node.has_subscribers());
        assert!(node.snapshots().next().is_none());
        node.check_invariants();
    }
    assert!(!network.node(0).subscribed_to(&b));
}

// A publisher that restarted has forgotten its subscribers: a subscriber
// that hears nothing for the silence subscribes again, and is sent the
// snapshot at once.
#[test]
fn a_silent_publisher_is_subscribed_to_again() {
    let (mut network, a, b) = pair(87, [true, true], true);
    wait(&mut network, |network| network.node(0).subscribed_to(&b));
    network.crash(1);
    network.start(1);
    network.act(1, |node, _| {
        node.set_snapshot(Some(snapshot(9)));
    });
    assert!(!network.node(1).subscribers().any(|node| *node == a));
    settle(
        &mut network,
        SUBSCRIPTION_SILENCE_MS + 2 * SUBSCRIPTION_CHECK_MS,
    );
    assert!(network.node(1).subscribers().any(|node| *node == a));
    assert_eq!(network.node(0).snapshot_of(&b), Some(&[9; 100][..]));
}

// A periphery node forwards nothing, so it offers no swaps: it refuses a
// subscription whatever its owner says, while it subscribes to its uplink
// over two methods. Nobody subscribes to a downlink.
#[test]
fn a_periphery_node_refuses_and_subscribes_to_its_uplink() {
    let mut network = Network::with_periphery(88, 1, 1, 1, Config::DEV);
    for index in 0..2 {
        network.nodes[index].hooks.swaps = true;
        network.set_running(index, scheme(SECOND), true);
    }
    let (core, periphery) = (*network.node(0).own(), *network.node(1).own());
    wait(&mut network, |network| {
        methods(network, 1, &core).len() == 2 && network.node(1).subscribed_to(&core)
    });
    assert!(network.node(0).is_downlink(&periphery));
    assert!(!network.node(0).subscribed_to(&periphery));
    assert!(!network.node(1).has_subscribers());
    assert_eq!(
        network.node(0).subscribers().collect::<Vec<_>>(),
        [&periphery]
    );
    let asked = Arriving::new(&key(0), &Content::Subscription { subscribe: true });
    let reply = network.inbound(1, &asked);
    assert_eq!(
        subscription(&reply),
        SubscriptionResponse::Refused(RefusalReason::NoSwaps)
    );
}
