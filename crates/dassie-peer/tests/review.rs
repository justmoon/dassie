//! What a review of the peering lifecycle found, each as a test that failed
//! before its fix.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use std::sync::Arc;

use dassie_identity::IdentityKey;
use dassie_kit::sim::SeededRng;
use dassie_oer::Decode;
use dassie_peer::sim::{Arriving, Network, START, key, url};
use dassie_peer::wire::{
    Content, NodeInfo, PeeringRequest, PeeringResponse, SchemeId, SettlementOffer, SignedNodeInfo,
    sign_node_info,
};
use dassie_peer::{Config, OwnInfo, PeerNode, Stored};

fn settle(network: &mut Network, ms: u64) {
    let until = network.now + ms;
    network.run_until(until);
}

fn stub() -> SchemeId {
    SchemeId::parse("stub").unwrap()
}

/// A sender outside the network, over a connection that proves its key.
struct Sender(Arc<IdentityKey>);

impl Sender {
    fn new(key: &Arc<IdentityKey>) -> Self {
        Self(Arc::clone(key))
    }

    /// Its request to node `to`.
    fn request(&mut self, _: &Network, _: usize, content: &Content<'_>) -> Arriving {
        Arriving::new(&self.0, content)
    }
}

/// `from`'s request to node `to`.
fn request(_: &Network, from: &Arc<IdentityKey>, _: usize, content: &Content<'_>) -> Arriving {
    Arriving::new(from, content)
}

fn state(key: &IdentityKey, sequence: u64, url: &str) -> Vec<u8> {
    let info = NodeInfo {
        sequence,
        url,
        alias: "",
        public_key: *key.public_key().as_bytes(),
        slh_dsa_public_key: *key.identity().slh_dsa().as_bytes(),
        settlement: vec![SettlementOffer {
            scheme: stub(),
            details: &[],
        }],
        entries: Vec::new(),
    };
    sign_node_info(key, &info)
}

fn held(network: &Network, at: usize, of: &IdentityKey) -> Option<u64> {
    let views = network.node(at).nodes();
    views
        .into_iter()
        .find(|view| view.id == of.node_id())?
        .sequence
}

// A forged state of X, with a bad signature, used up X's allowance before
// its signature was checked, so X's real states were then dropped.
#[test]
fn forged_link_states_do_not_use_up_the_real_nodes_allowance() {
    let mut network = Network::new(11, 2, 1, Config::DEV);
    settle(&mut network, 30_000);
    let (victim, mallory) = (key(1), Arc::new(IdentityKey::from_bytes(&[90; 32])));
    let base = network.clock(0);
    let mut sender = Sender::new(&mallory);
    for offset in 1..20u64 {
        let mut forged = state(&victim, base + offset, &url(1));
        let last = forged.len() - 1;
        forged[last] ^= 1;
        let info = SignedNodeInfo::decode_exact(&forged).unwrap();
        let body = sender.request(&network, 0, &Content::LinkStateUpdate(info));
        assert_eq!(network.inbound(0, &body).status, 400);
    }
    let real = state(&victim, base + 100, &url(1));
    let info = SignedNodeInfo::decode_exact(&real).unwrap();
    let body = sender.request(&network, 0, &Content::LinkStateUpdate(info));
    assert_eq!(network.inbound(0, &body).status, 200);
    assert_eq!(held(&network, 0, &victim), Some(base + 100));
}

// A Fulfill or Reject a peer did not get was lost for good, and its books
// parted from ours. Now it is sent again until it arrives.
#[test]
fn responses_to_peers_are_retried_until_they_arrive() {
    let mut network = Network::new(12, 2, 1, Config::DEV);
    settle(&mut network, 30_000);
    let peer = *network.node(1).own();
    network.cut.insert((0, 1));
    network.act(0, |node, now| node.send_response(now, &peer, 7, &[13, 1]));
    settle(&mut network, 3_000);
    assert!(network.packets.is_empty());
    network.cut.clear();
    settle(&mut network, 3_000);
    let own = *network.node(0).own();
    assert_eq!(network.packets, [(1, own, 7)]);
}

// A Fulfill from a peer whose clock is behind, sent as a node starts again,
// arrives: nothing is dated, so nothing is refused for its age
// (spec/areas/peer-protocol.md#authentication). A Fulfill given up though
// the peer had booked it would drift the two books apart.
#[test]
fn a_response_a_node_that_just_started_refused_is_sent_again() {
    let mut network = Network::new(12, 2, 1, Config::DEV);
    network.nodes[0].skew_ms = 5_000;
    network.nodes[1].skew_ms = 0;
    settle(&mut network, 30_000);
    let restarted = *network.node(0).own();
    let peer = *network.node(1).own();
    assert!(network.node(0).peering_of(&peer).is_some(), "not peered");
    network.crash(0);
    network.start(0);
    network.act(1, |node, now| {
        node.send_response(now, &restarted, 7, &[13, 1])
    });
    settle(&mut network, 10_000);
    assert_eq!(network.packets, [(0, peer, 7)]);
}

// spec/decisions/unreachable-peers.md: a peer our ILP packets surely do not
// reach is reported as such, so the node takes no new Prepare from it;
// heartbeats that fail say nothing, since the bounds on requests in flight
// may refuse them here. Once the cut is gone, the next request it answers, a
// heartbeat or the Reject sent again, reports it reached.
#[test]
fn a_peer_our_packets_do_not_reach_is_reported_until_it_answers() {
    let mut network = Network::new(16, 2, 1, Config::DEV);
    settle(&mut network, 30_000);
    let peer = *network.node(1).own();
    assert!(network.node(0).peering_of(&peer).is_some(), "not peered");
    let reports = |network: &Network| -> Vec<bool> {
        network
            .events
            .iter()
            .filter_map(|(index, event)| match event {
                dassie_peer::Event::Reachable { peer: p, reachable }
                    if *index == 0 && *p == peer =>
                {
                    Some(*reachable)
                }
                _ => None,
            })
            .collect()
    };
    network.cut_one_way.insert((0, 1));
    settle(&mut network, 30_000);
    assert!(reports(&network).is_empty(), "failed heartbeats reported");
    assert!(network.node(0).reachable(&peer));
    network.act(0, |node, now| node.send_response(now, &peer, 7, &[13, 1]));
    settle(&mut network, 3_000);
    assert_eq!(reports(&network), [false]);
    assert!(!network.node(0).reachable(&peer));
    network.cut_one_way.clear();
    settle(&mut network, 30_000);
    assert_eq!(reports(&network), [false, true]);
    assert!(network.node(0).reachable(&peer));
}

// Every node accepted registrations, so any could be filled up.
#[test]
fn only_bootstrap_nodes_take_registrations() {
    let mut network = Network::new(13, 3, 1, Config::DEV);
    settle(&mut network, 30_000);
    let stranger = Arc::new(IdentityKey::from_bytes(&[91; 32]));
    let own = state(&stranger, network.clock(2), "https://stranger.test");
    let info = SignedNodeInfo::decode_exact(&own).unwrap();
    let body = request(&network, &stranger, 2, &Content::Registration(info));
    assert_eq!(network.inbound(2, &body).status, 409);
}

// A registered node could move its URL anywhere by flooding a new state
// and renewing: the renewal compared with the state held, not the URL
// that was probed.
#[test]
fn a_registration_from_a_new_url_is_probed_again() {
    let mut network = Network::new(14, 2, 1, Config::DEV);
    settle(&mut network, 30_000);
    let node = key(1);
    let moved = state(&node, network.clock(0) + 1_000, "https://elsewhere.test");
    let info = SignedNodeInfo::decode_exact(&moved).unwrap();
    let body = request(&network, &node, 0, &Content::Registration(info));
    assert_eq!(network.inbound(0, &body).status, 0, "deferred for a probe");
}

// One sender could hold every probe slot with registrations at a URL that
// never answers.
#[test]
fn one_probe_per_sender_at_a_time() {
    let mut network = Network::new(15, 2, 1, Config::DEV);
    settle(&mut network, 30_000);
    let stranger = Arc::new(IdentityKey::from_bytes(&[92; 32]));
    let own = state(&stranger, network.clock(0), "https://stranger.test");
    let info = SignedNodeInfo::decode_exact(&own).unwrap();
    let mut sender = Sender::new(&stranger);
    let first = sender.request(&network, 0, &Content::Registration(info.clone()));
    assert_eq!(network.inbound(0, &first).status, 0);
    let second = sender.request(&network, 0, &Content::Registration(info));
    assert_eq!(network.inbound(0, &second).status, 429);
}

fn peering_request(
    network: &Network,
    from: &Arc<IdentityKey>,
    to: usize,
    sequence: u64,
) -> Arriving {
    let own = state(from, sequence, &url(1));
    let info = SignedNodeInfo::decode_exact(&own).unwrap();
    let content = Content::PeeringRequest(PeeringRequest {
        scheme: stub(),
        node_info: info,
        data: &[],
    });
    request(network, from, to, &content)
}

// A repeated request replaced the stored peering: its credit, its state
// and which side asked.
#[test]
fn a_repeated_peering_request_keeps_the_peering() {
    let mut network = Network::new(16, 2, 1, Config::DEV);
    settle(&mut network, 30_000);
    let node1 = *network.node(1).own();
    let before = network.node(0).peering_of(&node1).unwrap().clone();
    let body = peering_request(&network, &key(1), 0, network.clock(0));
    let reply = network.inbound(0, &body);
    assert_eq!(reply.status, 200);
    let answer = PeeringResponse::decode_exact(&reply.body).unwrap();
    assert_eq!(answer, PeeringResponse::Accepted);
    let after = network.node(0).peering_of(&node1).unwrap();
    assert_eq!(
        (after.first().credit_out, after.inbound),
        (before.first().credit_out, before.inbound)
    );
}

// A scheme the node no longer peers on anew, such as one that stopped,
// keeps the peerings made on it: a repeat request, after an acceptance
// that got lost, is still answered with the stored peering
// (spec/areas/network.md#asking-and-answering).
#[test]
fn a_repeat_is_answered_on_a_scheme_no_longer_peered_on() {
    let mut network = Network::new(16, 2, 1, Config::DEV);
    settle(&mut network, 30_000);
    let now = network.clock(0);
    network.node_mut(0).set_offers(now, Vec::new());
    let body = peering_request(&network, &key(1), 0, network.clock(0));
    let reply = network.inbound(0, &body);
    assert_eq!(reply.status, 200);
    let answer = PeeringResponse::decode_exact(&reply.body).unwrap();
    assert!(matches!(answer, PeeringResponse::Accepted), "{answer:?}");
    let node1 = *network.node(1).own();
    assert!(network.node(0).peering_of(&node1).is_some());
}

// A peering request whose state was far ahead stored a peer without a
// key, which broke an invariant.
#[test]
fn peering_requests_from_the_future_are_refused() {
    let mut network = Network::new(17, 3, 1, Config::DEV);
    settle(&mut network, 30_000);
    let body = peering_request(&network, &key(1), 2, network.clock(2) + 3_600_000);
    assert_eq!(network.inbound(2, &body).status, 400);
    network.node(2).check_invariants();
}

// The majority set started empty after a restart, so nodes that lost it
// meanwhile kept it for good.
#[test]
fn the_majority_survives_a_restart() {
    let mut network = Network::new(18, 4, 2, Config::DEV);
    settle(&mut network, 60_000);
    let own = *network.node(3).own();
    let mut before = network.node(3).majority().clone();
    before.remove(&own);
    assert!(!before.is_empty());
    network.crash(3);
    network.start(3);
    let mut after = network.node(3).majority().clone();
    after.remove(&own);
    assert_eq!(after, before);
}

#[test]
fn a_message_from_ourselves_is_malformed() {
    let mut network = Network::new(19, 2, 1, Config::DEV);
    settle(&mut network, 10_000);
    let own = Arriving::new(&key(0), &Content::NodeListRequest);
    assert_eq!(network.inbound(0, &own).status, 400);
}

// A hostname long enough made our URL too long for a link state, and the
// node panicked on every timer. Without a URL it runs as a periphery node.
#[test]
fn an_own_url_too_long_is_no_url() {
    let url = format!("https://{}.example:65535", "a".repeat(240));
    let mut node = PeerNode::new(
        Config::DEV,
        key(0),
        OwnInfo {
            url: Some(url),
            alias: String::new(),
            feed_port: None,
        },
        Vec::new(),
        Stored::default(),
        Box::new(SeededRng::new(1)),
        START,
    );
    node.handle_timeout(START, &mut dassie_peer::sim::StubHooks::default());
    assert_eq!(node.own_info().url, None);
    assert!(node.is_periphery());
}
