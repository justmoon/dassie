//! What a review of M4a found, each as a test that failed before its fix.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use std::sync::Arc;

use dassie_identity::NodeKey;
use dassie_node_core::testing::SeededRng;
use dassie_oer::{Decode, Encode};
use dassie_peer::auth::{AuthConfig, AuthError, Authenticator};
use dassie_peer::sim::{Network, START, key, url};
use dassie_peer::wire::{
    Authentication, Content, Entry, Envelope, NodeInfo, PeeringRequest, PeeringResponse,
    ResponseEnvelope, SchemeId, SignedNodeInfo, encode_envelope, sign_node_info,
};
use dassie_peer::{Config, OwnInfo, PeerBatch, PeerNode, Stored};

fn settle(network: &mut Network, ms: u64) {
    let until = network.now + ms;
    network.run_until(until);
}

fn stub() -> SchemeId {
    SchemeId::parse("stub").unwrap()
}

/// A sender outside the network, with its session.
struct Sender(Authenticator);

impl Sender {
    fn new(key: &Arc<NodeKey>) -> Self {
        Self(Authenticator::new(
            Arc::clone(key),
            AuthConfig::DEFAULT,
            START - 1,
        ))
    }

    /// Its request to node `to`, authenticated.
    fn request(&mut self, network: &Network, to: usize, content: &Content<'_>) -> Vec<u8> {
        request_with(&mut self.0, network, to, content)
    }
}

/// `from`'s request to node `to`, authenticated, in a session of its own.
fn request(network: &Network, from: &Arc<NodeKey>, to: usize, content: &Content<'_>) -> Vec<u8> {
    let mut auth = Authenticator::new(Arc::clone(from), AuthConfig::DEFAULT, START - 1);
    request_with(&mut auth, network, to, content)
}

fn request_with(
    auth: &mut Authenticator,
    network: &Network,
    to: usize,
    content: &Content<'_>,
) -> Vec<u8> {
    let from = auth.own().to_owned();
    let bytes = content.to_vec();
    let target = key(to);
    let mut rng = SeededRng::new(network.clock(to));
    let (hmac, _) = auth
        .authenticate(
            network.clock(to),
            &mut rng,
            &target.node_id(),
            &target.public_key(),
            &bytes,
        )
        .unwrap();
    encode_envelope(&from, &Authentication::Hmac(hmac), &bytes)
}

fn state(key: &NodeKey, sequence: u64, url: &str) -> Vec<u8> {
    let info = NodeInfo {
        node_id: key.node_id(),
        sequence,
        url,
        alias: "",
        public_key: *key.public_key().as_bytes(),
        entries: vec![Entry::Scheme(stub())],
    };
    sign_node_info(key, &info)
}

fn held(network: &Network, at: usize, of: &NodeKey) -> Option<u64> {
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
    let (victim, mallory) = (key(1), Arc::new(NodeKey::from_bytes(&[90; 32])));
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

// Every node accepted registrations, so any could be filled up.
#[test]
fn only_bootstrap_nodes_take_registrations() {
    let mut network = Network::new(13, 3, 1, Config::DEV);
    settle(&mut network, 30_000);
    let stranger = Arc::new(NodeKey::from_bytes(&[91; 32]));
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
    let stranger = Arc::new(NodeKey::from_bytes(&[92; 32]));
    let own = state(&stranger, network.clock(0), "https://stranger.test");
    let info = SignedNodeInfo::decode_exact(&own).unwrap();
    let mut sender = Sender::new(&stranger);
    let first = sender.request(&network, 0, &Content::Registration(info.clone()));
    assert_eq!(network.inbound(0, &first).status, 0);
    let second = sender.request(&network, 0, &Content::Registration(info));
    assert_eq!(network.inbound(0, &second).status, 429);
}

fn peering_request(network: &Network, from: &Arc<NodeKey>, to: usize, sequence: u64) -> Vec<u8> {
    let own = state(from, sequence, &url(1));
    let info = SignedNodeInfo::decode_exact(&own).unwrap();
    let content = Content::PeeringRequest(PeeringRequest {
        scheme: stub(),
        node_info: info,
        data: &[],
        versions: vec![1],
        credit: 1,
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
    let envelope = ResponseEnvelope::decode(&reply.body).unwrap();
    let answer = PeeringResponse::decode_exact(envelope.body).unwrap();
    assert_eq!(
        answer,
        PeeringResponse::Accepted {
            version: 1,
            credit: 0,
            data: &[],
        }
    );
    let after = network.node(0).peering_of(&node1).unwrap();
    assert_eq!(
        (after.credit_out, after.inbound),
        (before.credit_out, before.inbound)
    );
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
    let content = Content::NodeListRequest.to_vec();
    let bare = encode_envelope(&key(0).node_id(), &Authentication::None, &content);
    assert!(Envelope::decode(&bare).is_ok());
    assert_eq!(network.inbound(0, &bare).status, 400);
}

// A hostname long enough made our URL too long for a link state, and the
// node panicked on every timer.
#[test]
fn an_own_url_too_long_is_no_url() {
    let url = format!("https://{}.example:65535", "a".repeat(240));
    let mut node = PeerNode::new(
        Config::DEV,
        key(0),
        OwnInfo {
            url: Some(url),
            alias: String::new(),
        },
        Vec::new(),
        Stored::default(),
        Box::new(SeededRng::new(1)),
        START,
    );
    node.handle_timeout(START, &mut dassie_peer::sim::StubHooks::default());
    let mut batch = PeerBatch::default();
    node.take_changes(&mut batch);
    assert_eq!(batch.own_sequence, None);
}

// A thousand throwaway identities filled the replay table, and peers got
// 503. Peers and bootstrap nodes keep their place now.
#[test]
fn strangers_cannot_crowd_out_peers() {
    let b = Arc::new(NodeKey::from_bytes(&[3; 32]));
    let config = AuthConfig {
        max_senders: 1,
        ..AuthConfig::DEFAULT
    };
    let mut at_b = Authenticator::new(Arc::clone(&b), config, START - 1);
    let (peer, stranger) = (
        Arc::new(NodeKey::from_bytes(&[1; 32])),
        Arc::new(NodeKey::from_bytes(&[7; 32])),
    );
    at_b.pin(peer.node_id());
    let send = |from: &Arc<NodeKey>, at: u64| {
        let mut auth = Authenticator::new(Arc::clone(from), AuthConfig::DEFAULT, START - 1);
        let content = Content::NodeListRequest.to_vec();
        let (hmac, _) = auth
            .authenticate(
                at,
                &mut SeededRng::new(at),
                &b.node_id(),
                &b.public_key(),
                &content,
            )
            .unwrap();
        encode_envelope(&from.node_id(), &Authentication::Hmac(hmac), &content)
    };
    let accept = |at_b: &mut Authenticator, body: &[u8]| {
        at_b.accept(START, &Envelope::decode(body).unwrap(), true)
            .map(|_| ())
    };
    assert_eq!(accept(&mut at_b, &send(&stranger, START)), Ok(()));
    let other = Arc::new(NodeKey::from_bytes(&[8; 32]));
    assert_eq!(
        accept(&mut at_b, &send(&other, START)),
        Err(AuthError::Busy)
    );
    assert_eq!(accept(&mut at_b, &send(&peer, START)), Ok(()));
    at_b.check_invariants();
}
