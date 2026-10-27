//! Peering over a settlement hub, as the peer protocol carries it
//! (spec/areas/peer-protocol.md#message-set,
//! spec/designs/settlement-hubs.md#4-peering-over-a-hub): Prepares with
//! their commitments, and the payer's claim after the verdict on a Fulfill.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use std::sync::Arc;

use dassie_identity::{NodeId, NodeKey};
use dassie_peer::sim::{Arriving, Network, key, url};
use dassie_peer::wire::{Content, HubMembership};
use dassie_peer::{Config, Event, Inbound, InboundId, OwnInfo, Reply};

/// A Prepare, as far as the peer protocol reads one.
const PREPARE: &[u8] = &[12, 1, 0];

/// A Fulfill, likewise.
const FULFILL: &[u8] = &[13, 1, 0];

/// A commitment, opaque to the peer protocol.
const COMMITMENT: &[u8] = &[0xc0; 96];

/// A claim, likewise.
const CLAIM: &[u8] = &[0xc1; 112];

fn settle(network: &mut Network, ms: u64) {
    let until = network.now + ms;
    network.run_until(until);
}

/// Two nodes, peered on the stub scheme, over a hub if `hub`.
fn peered(seed: u64, hub: bool) -> (Network, NodeId, NodeId) {
    let mut network = Network::new(seed, 2, 1, Config::DEV);
    let (a, b) = (*network.node(0).own(), *network.node(1).own());
    for _ in 0..30 {
        if network.node(0).is_advertised(&b) && network.node(1).is_advertised(&a) {
            break;
        }
        settle(&mut network, 3_000);
    }
    assert!(network.node(0).peering_of(&b).is_some());
    assert!(network.node(1).peering_of(&a).is_some());
    for node in &mut network.nodes {
        node.hooks.commitments = hub;
        node.hooks.claim = CLAIM.to_vec();
    }
    (network, a, b)
}

/// Messages from a node, over a connection that proves its key.
struct Sender(Arc<NodeKey>);

impl Sender {
    fn new(key: &Arc<NodeKey>) -> Self {
        Self(Arc::clone(key))
    }

    fn request(&mut self, _: &Network, _: usize, content: &Content<'_>) -> Arriving {
        Arriving::new(&self.0, content)
    }
}

fn hub_packet(packet: &[u8]) -> Content<'_> {
    Content::HubPacket {
        request_id: 6,
        packet,
        commitment: COMMITMENT,
    }
}

fn plain_packet(packet: &[u8]) -> Content<'_> {
    Content::InterledgerPacket {
        request_id: 6,
        packet,
    }
}

/// Hands `body` to node `index` and gives what it made of it to `check`,
/// which answers a packet as the node would.
fn handle(
    network: &mut Network,
    index: usize,
    request: &Arriving,
    check: impl FnOnce(Inbound<'_>) -> Reply,
) -> Reply {
    let now = network.clock(index);
    let sim = &mut network.nodes[index];
    let node = sim.node.as_mut().unwrap();
    let inbound = InboundId(900);
    check(node.handle_inbound(
        now,
        &mut sim.hooks,
        inbound,
        request.from,
        &request.body,
        None,
    ))
}

/// The body of a 200 reply.
fn body(reply: &Reply) -> Vec<u8> {
    assert_eq!(
        reply.status,
        200,
        "{}",
        String::from_utf8_lossy(&reply.body)
    );
    reply.body.clone()
}

// A Prepare with its commitment reaches the node on a peering over a hub,
// commitment and all; it answers with an empty body, as for any Prepare.
#[test]
fn a_prepare_over_a_hub_comes_with_its_commitment() {
    let (mut network, _, _) = peered(61, true);
    let mut sender = Sender::new(&key(1));
    let request = sender.request(&network, 0, &hub_packet(PREPARE));
    let reply = handle(&mut network, 0, &request, |inbound| match inbound {
        Inbound::Packet {
            from,
            request_id,
            packet,
            commitment,
            answer,
        } => {
            assert_eq!((from, request_id, packet), (key(1).node_id(), 6, PREPARE));
            assert_eq!(commitment, Some(COMMITMENT));
            assert!(!answer.takes_claim());
            answer.reply(&[])
        }
        other => panic!("not a packet: {other:?}"),
    });
    assert!(body(&reply).is_empty());
}

// spec/areas/peer-protocol.md#message-set: a Prepare with a commitment is
// refused as an unpeered Prepare is (409), from a node not peered and on a
// peering not over a hub; and a Prepare without one where the peering is
// over a hub. Only a Prepare carries a commitment.
#[test]
fn a_commitment_comes_exactly_with_prepares_over_a_hub() {
    let (mut network, _, _) = peered(62, false);
    let mut sender = Sender::new(&key(1));
    let request = sender.request(&network, 0, &hub_packet(PREPARE));
    assert_eq!(network.inbound(0, &request).status, 409);
    let request = sender.request(&network, 0, &plain_packet(PREPARE));
    assert_eq!(network.inbound(0, &request).status, 200);

    for node in &mut network.nodes {
        node.hooks.commitments = true;
    }
    let request = sender.request(&network, 0, &plain_packet(PREPARE));
    assert_eq!(network.inbound(0, &request).status, 409);
    let request = sender.request(&network, 0, &hub_packet(FULFILL));
    assert_eq!(network.inbound(0, &request).status, 400);
    let request = sender.request(&network, 0, &hub_packet(PREPARE));
    assert_eq!(network.inbound(0, &request).status, 200);

    let mut stranger = Sender::new(&key(7));
    let request = stranger.request(&network, 0, &hub_packet(PREPARE));
    assert_eq!(network.inbound(0, &request).status, 409);
}

// The reply to a Fulfill over a hub carries the payer's claim; on any other
// peering it is empty, and no claim may go in it.
#[test]
fn the_reply_to_a_fulfill_over_a_hub_carries_the_claim() {
    let (mut network, _, _) = peered(63, true);
    let mut sender = Sender::new(&key(1));
    let request = sender.request(&network, 0, &plain_packet(FULFILL));
    let reply = handle(&mut network, 0, &request, |inbound| match inbound {
        Inbound::Packet {
            commitment, answer, ..
        } => {
            assert_eq!(commitment, None);
            assert!(answer.takes_claim());
            answer.reply(CLAIM)
        }
        other => panic!("not a packet: {other:?}"),
    });
    assert_eq!(body(&reply), CLAIM);

    let (mut network, _, _) = peered(64, false);
    let request = sender.request(&network, 0, &plain_packet(FULFILL));
    let reply = handle(&mut network, 0, &request, |inbound| match inbound {
        Inbound::Packet { answer, .. } => {
            assert!(!answer.takes_claim());
            answer.reply(&[])
        }
        other => panic!("not a packet: {other:?}"),
    });
    assert!(body(&reply).is_empty());
}

#[test]
#[should_panic(expected = "a claim only on a Fulfill over a hub")]
fn no_claim_off_a_hub() {
    let (mut network, _, _) = peered(65, false);
    let mut sender = Sender::new(&key(1));
    let request = sender.request(&network, 0, &plain_packet(FULFILL));
    handle(&mut network, 0, &request, |inbound| match inbound {
        Inbound::Packet { answer, .. } => answer.reply(CLAIM),
        other => panic!("not a packet: {other:?}"),
    });
}

/// The claims node `index` was given for its Fulfills.
fn claims(network: &Network, index: usize) -> Vec<(NodeId, u32, Vec<u8>)> {
    network
        .events
        .iter()
        .filter(|(at, _)| *at == index)
        .filter_map(|(_, event)| match event {
            Event::FulfillReply {
                peer,
                request_id,
                claim,
            } => Some((*peer, *request_id, claim.clone())),
            _ => None,
        })
        .collect()
}

// Between two nodes over a hub: the payer's Prepare goes with its
// commitment, and the payee, sending the Fulfill back, is given the payer's
// claim to take to the hub.
#[test]
fn a_payee_over_a_hub_is_given_the_claim() {
    let (mut network, a, b) = peered(66, true);
    network.act(0, |node, now| {
        node.send_hub_packet(now, &b, 9, PREPARE, COMMITMENT, 10_000)
            .unwrap();
    });
    settle(&mut network, 2_000);
    assert!(network.packets.contains(&(1, a, 9)), "the Prepare arrived");
    network.act(1, |node, now| node.send_response(now, &a, 9, FULFILL));
    settle(&mut network, 2_000);
    assert_eq!(claims(&network, 1), [(a, 9, CLAIM.to_vec())]);
    assert_eq!(network.node(1).stats().answers_dropped, 0);
}

// A node not over a hub replies with nothing: the Fulfill arrived, and there
// is no claim.
#[test]
fn an_empty_reply_gives_no_claim() {
    let (mut network, a, b) = peered(67, false);
    network.act(0, |node, now| {
        node.send_packet(now, &b, 9, PREPARE, 10_000).unwrap();
    });
    settle(&mut network, 2_000);
    network.act(1, |node, now| node.send_response(now, &a, 9, FULFILL));
    settle(&mut network, 2_000);
    assert!(claims(&network, 1).is_empty());
    assert_eq!(network.node(1).stats().answers_dropped, 0);
    assert!(network.node(1).reachable(&a));
}

/// Two nodes whose only scheme is over a hub, each a member of `hubs[i]`,
/// after a while to find each other.
fn over_hubs(seed: u64, hubs: [Option<HubMembership>; 2]) -> (Network, NodeId, NodeId) {
    let mut network = Network::new(seed, 2, 1, Config::DEV);
    for (index, hub) in hubs.into_iter().enumerate() {
        network.nodes[index].hooks.commitments = true;
        let now = network.clock(index);
        let info = OwnInfo {
            url: Some(url(index)),
            alias: format!("n{index}"),
            feed_port: None,
            hub,
        };
        network.node_mut(index).set_own_info(now, info);
    }
    let (a, b) = (*network.node(0).own(), *network.node(1).own());
    settle(&mut network, 60_000);
    (network, a, b)
}

fn member(hub: u8, takes_peers: bool) -> Option<HubMembership> {
    Some(HubMembership {
        hub_id: [hub; 32],
        account: 1,
        takes_peers,
    })
}

// A scheme over a hub is asked for only by a node that shares the hub with
// the other, which takes peers there
// (spec/designs/settlement-hubs.md#11-choosing-a-hub): without, neither
// would find the other's account.
#[test]
fn a_hub_scheme_is_chosen_only_for_a_shared_hub() {
    let (network, a, b) = over_hubs(7, [member(1, true), member(1, true)]);
    assert!(network.node(0).peering_of(&b).is_some());
    assert!(network.node(1).peering_of(&a).is_some());
    for hubs in [
        [member(1, true), member(2, true)],
        [member(1, true), None],
        [member(1, true), member(1, false)],
    ] {
        let (network, a, b) = over_hubs(7, hubs);
        assert!(network.node(0).peering_of(&b).is_none(), "{hubs:?}");
        assert!(network.node(1).peering_of(&a).is_none(), "{hubs:?}");
    }
}
