//! Closing settlement channels (spec/decisions/closing-channels.md): a
//! channel with a node we no longer peer with closes once idle for as long
//! as a silent peer is kept, and what stays of it on disk delivers a late
//! settlement once and opens it again where it was.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use std::sync::Arc;

use dassie_identity::{NodeId, NodeKey};
use dassie_node_core::testing::SeededRng;
use dassie_oer::Encode;
use dassie_peer::auth::{AuthConfig, Authenticator};
use dassie_peer::sim::{Network, START, key};
use dassie_peer::wire::{Authentication, Content, SchemeId, TerminationReason, encode_envelope};
use dassie_peer::{Closed, Config, Event};

const CONFIG: Config = Config::DEV;

fn settle(network: &mut Network, ms: u64) {
    let until = network.now + ms;
    network.run_until(until);
}

fn stub() -> SchemeId {
    SchemeId::parse("stub").unwrap()
}

/// Two nodes, peered.
fn peered(seed: u64) -> (Network, NodeId, NodeId) {
    let mut network = Network::new(seed, 2, 1, CONFIG);
    let (a, b) = (*network.node(0).own(), *network.node(1).own());
    for _ in 0..30 {
        if network.node(0).is_advertised(&b) && network.node(1).is_advertised(&a) {
            break;
        }
        settle(&mut network, 3_000);
    }
    assert!(network.node(0).peering_of(&b).is_some());
    assert!(network.node(1).peering_of(&a).is_some());
    (network, a, b)
}

/// Node 0 ends its peering with `b` for `reason`, and both sides are done.
fn end(network: &mut Network, b: &NodeId, reason: TerminationReason) {
    let a = *network.node(0).own();
    network.act(0, |node, now| {
        assert!(node.terminate(now, b, reason));
    });
    for _ in 0..30 {
        if network.node(0).peering_of(b).is_none() && network.node(1).peering_of(&a).is_none() {
            return;
        }
        settle(network, 2_000);
    }
    panic!("the peering did not end");
}

fn closed(network: &Network, index: usize, peer: &NodeId) -> usize {
    network
        .events
        .iter()
        .filter(|(at, event)| {
            *at == index && matches!(event, Event::ChannelClosed { peer: p } if p == peer)
        })
        .count()
}

/// A settlement message from `auth`'s node to node `to`, sent by hand.
fn settlement(
    auth: &mut Authenticator,
    network: &Network,
    to: usize,
    amount: u64,
    sequence: u64,
) -> Vec<u8> {
    let content = Content::Settlement {
        scheme: stub(),
        amount,
        data: &[],
        sequence,
    };
    let bytes = content.to_vec();
    let target: Arc<NodeKey> = key(to);
    let (hmac, _) = auth
        .authenticate(
            network.clock(to),
            &mut SeededRng::new(network.clock(to) + sequence),
            &target.node_id(),
            &target.public_key(),
            &bytes,
        )
        .unwrap();
    encode_envelope(&auth.own().to_owned(), &Authentication::Hmac(hmac), &bytes)
}

// A channel stays open for as long as a silent peer is kept after the
// peering ended, then closes on both sides: it leaves memory and the
// stored open channels, and what stays of it keeps its sequences.
#[test]
fn an_idle_channel_closes() {
    let (mut network, a, b) = peered(201);
    assert!(network.settle(0, b, 4));
    settle(&mut network, 5_000);
    assert_eq!(network.applied.len(), 1);
    end(&mut network, &b, TerminationReason::Owner);
    assert!(network.node(0).channel(&b).is_some(), "kept after the end");
    settle(&mut network, CONFIG.peer_dead_ms - 30_000);
    assert!(
        network.node(0).channel(&b).is_some(),
        "not idle long enough"
    );
    settle(&mut network, 40_000);
    assert!(network.node(0).channel(&b).is_none());
    assert!(network.node(1).channel(&a).is_none());
    assert_eq!(closed(&network, 0, &b), 1);
    assert_eq!(closed(&network, 1, &a), 1);
    assert!(!network.holds_channel(&a, 1), "no longer stored open");
    assert_eq!(
        network.nodes[0].hooks.closed.get(&b),
        Some(&Closed {
            settled_in: 0,
            settled_out: 1,
            ended_for_room: false,
        })
    );
    assert_eq!(
        network.nodes[1].hooks.closed.get(&a).map(|c| c.settled_in),
        Some(1)
    );
    // A restart loads only the channels still open.
    network.crash(0);
    network.start(0);
    assert!(network.node(0).channel(&b).is_none());
}

// The books hold a channel open while anything of it is pending, or a
// scheme it has accounts with does not see its ledger; and a settlement of
// ours under way holds it too.
#[test]
fn the_books_hold_a_channel_open() {
    let (mut network, _, b) = peered(202);
    end(&mut network, &b, TerminationReason::Owner);
    network.nodes[0].hooks.holding.insert(b);
    settle(&mut network, CONFIG.peer_dead_ms + 30_000);
    assert!(network.node(0).channel(&b).is_some(), "held by the books");
    network.nodes[0].hooks.holding.clear();
    network.nodes[0].hooks.settling.insert(b);
    settle(&mut network, 10_000);
    assert!(
        network.node(0).channel(&b).is_some(),
        "a settlement under way"
    );
    network.nodes[0].hooks.settling.clear();
    settle(&mut network, 10_000);
    assert!(network.node(0).channel(&b).is_none(), "closed once let go");
}

// A settlement that comes over a closed channel is applied once: the
// sequence that stayed tells it from a copy. The channel stays closed.
#[test]
fn a_late_settlement_over_a_closed_channel_is_applied_once() {
    let (mut network, _, b) = peered(203);
    end(&mut network, &b, TerminationReason::Owner);
    settle(&mut network, CONFIG.peer_dead_ms + 30_000);
    assert!(network.node(0).channel(&b).is_none());
    let applied = network.node(0).stats().settlements_applied;
    let mut auth = Authenticator::new(key(1), AuthConfig::DEFAULT, START - 1);
    let body = settlement(&mut auth, &network, 0, 6, 1);
    assert_eq!(network.inbound(0, &body).status, 200);
    let copy = settlement(&mut auth, &network, 0, 6, 1);
    assert_eq!(network.inbound(0, &copy).status, 200);
    let node = network.node(0);
    assert_eq!(node.stats().settlements_applied, applied + 1);
    assert_eq!(node.stats().settlements_duplicate, 1);
    assert!(node.channel(&b).is_none(), "still closed");
    assert_eq!(
        network.nodes[0].hooks.closed.get(&b).map(|c| c.settled_in),
        Some(1)
    );
    // One out of order is refused, as over an open channel.
    let gap = settlement(&mut auth, &network, 0, 6, 3);
    assert_eq!(network.inbound(0, &gap).status, 400);
}

// Peering again opens the channel where it was: our next settlement
// follows the last, so the peer applies it, and the mark of an end for
// room stays (spec/decisions/ended-for-room.md).
#[test]
fn peering_again_opens_the_channel_where_it_was() {
    let (mut network, a, b) = peered(204);
    assert!(network.settle(0, b, 4));
    settle(&mut network, 5_000);
    end(&mut network, &b, TerminationReason::ShortOfRoom);
    settle(&mut network, CONFIG.peer_dead_ms + 30_000);
    assert!(network.node(0).channel(&b).is_none());
    assert!(network.nodes[0].hooks.closed[&b].ended_for_room);
    // The cooldown after an end for room passes, and the pair peers again.
    for _ in 0..60 {
        if network.node(0).peering_of(&b).is_some() && network.node(1).peering_of(&a).is_some() {
            break;
        }
        settle(&mut network, 10_000);
    }
    assert!(network.node(0).peering_of(&b).is_some(), "peered again");
    let channel = network.node(0).channel(&b).unwrap();
    assert_eq!(channel.settled_out, 1);
    assert!(channel.ended_for_room);
    assert!(!network.nodes[0].hooks.closed.contains_key(&b));
    assert_eq!(network.node(1).channel(&a).unwrap().settled_in, 1);
    assert!(network.settle(0, b, 5));
    settle(&mut network, 5_000);
    assert_eq!(network.applied.len(), 2);
    assert_eq!(network.applied[1].sequence, 2);
}
