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
use dassie_node_core::testing::SeededRng;
use dassie_oer::Encode;
use dassie_peer::auth::{AuthConfig, Authenticator};
use dassie_peer::sim::{Network, START, key};
use dassie_peer::wire::{Authentication, Content, ResponseEnvelope, encode_envelope};
use dassie_peer::{Config, Event, Inbound, InboundId, Reply, VERDICT_ACCEPTED};

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

/// Messages from a node, in one session of its own.
struct Sender(Authenticator);

impl Sender {
    fn new(key: &Arc<NodeKey>) -> Self {
        Self(Authenticator::new(
            Arc::clone(key),
            AuthConfig::DEFAULT,
            START - 1,
        ))
    }

    fn request(&mut self, network: &Network, to: usize, content: &Content<'_>) -> Vec<u8> {
        let bytes = content.to_vec();
        let target = key(to);
        let (hmac, _) = self
            .0
            .authenticate(
                network.clock(to),
                &mut SeededRng::new(network.clock(to)),
                &target.node_id(),
                &target.public_key(),
                &bytes,
            )
            .unwrap();
        encode_envelope(
            &self.0.own().to_owned(),
            &Authentication::Hmac(hmac),
            &bytes,
        )
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
    body: &[u8],
    check: impl FnOnce(Inbound<'_>) -> Reply,
) -> Reply {
    let now = network.clock(index);
    let sim = &mut network.nodes[index];
    let node = sim.node.as_mut().unwrap();
    check(node.handle_inbound(now, &mut sim.hooks, InboundId(900), body, None))
}

/// The body of a 200 reply.
fn body(reply: &Reply) -> Vec<u8> {
    assert_eq!(
        reply.status,
        200,
        "{}",
        String::from_utf8_lossy(&reply.body)
    );
    ResponseEnvelope::decode(&reply.body).unwrap().body.to_vec()
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
            assert!(!answer.takes_extra());
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

// The reply to a Fulfill over a hub carries the payer's claim after the
// verdict; on any other peering, the verdict alone, and nothing may follow
// it.
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
            assert!(answer.takes_extra());
            answer.reply(CLAIM)
        }
        other => panic!("not a packet: {other:?}"),
    });
    let mut expected = vec![VERDICT_ACCEPTED];
    expected.extend_from_slice(CLAIM);
    assert_eq!(body(&reply), expected);

    let (mut network, _, _) = peered(64, false);
    let request = sender.request(&network, 0, &plain_packet(FULFILL));
    let reply = handle(&mut network, 0, &request, |inbound| match inbound {
        Inbound::Packet { answer, .. } => {
            assert!(!answer.takes_extra());
            answer.reply(&[])
        }
        other => panic!("not a packet: {other:?}"),
    });
    assert_eq!(body(&reply), [VERDICT_ACCEPTED]);
}

#[test]
#[should_panic(expected = "bytes after the verdict only on a Fulfill over a hub")]
fn nothing_follows_the_verdict_off_a_hub() {
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
                extra,
            } => Some((*peer, *request_id, extra.clone())),
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

// A node not over a hub, or one from before hubs, replies with the verdict
// alone: the Fulfill arrived, and there is no claim.
#[test]
fn a_reply_of_the_verdict_alone_gives_no_claim() {
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
