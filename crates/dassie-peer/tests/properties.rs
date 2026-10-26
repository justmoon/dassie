//! The peer node never panics on what other nodes send it
//! (spec/decisions/tigerstyle.md): requests and the responses to its own,
//! arbitrary or broken from valid ones, on a small simulated network that
//! checks the nodes' invariants after every step.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

mod support;

use std::sync::Arc;

use dassie_identity::{NodeId, NodeKey};
use dassie_kit::sim::SeededRng;
use dassie_peer::Config;
use dassie_peer::auth::{AuthConfig, Authenticator};
use dassie_peer::sim::{Network, START, key, url};
use dassie_peer::wire::{
    Authentication, Entry, Envelope, NodeInfo, SchemeId, TerminationReason, encode_envelope,
    sign_node_info,
};
use proptest::prelude::*;
use support::inputs::{KINDS, Mutation, content, mutate, mutations};

fn settle(network: &mut Network, ms: u64) {
    let until = network.now + ms;
    network.run_until(until);
}

fn stub() -> SchemeId {
    SchemeId::parse("stub").unwrap()
}

/// A request body.
#[derive(Clone, Debug)]
enum Body {
    /// Any bytes.
    Arbitrary(Vec<u8>),
    /// Content of the kind with this tag, broken, in a valid envelope: it
    /// gets past authentication to what reads the content.
    BrokenContent(usize, Vec<Mutation>),
    /// A valid request of the kind with this tag, its envelope broken.
    BrokenEnvelope(usize, Vec<Mutation>),
}

fn body() -> impl Strategy<Value = Body> {
    prop_oneof![
        1 => proptest::collection::vec(any::<u8>(), 0..512).prop_map(Body::Arbitrary),
        3 => (0..KINDS, mutations())
            .prop_map(|(tag, mutations)| Body::BrokenContent(tag, mutations)),
        2 => (0..KINDS, mutations())
            .prop_map(|(tag, mutations)| Body::BrokenEnvelope(tag, mutations)),
    ]
}

/// A node sending requests to node 0, with its own session, so that its
/// requests are not replays of each other.
struct Sender {
    key: Arc<NodeKey>,
    url: String,
    /// `None` for a sender that does not authenticate.
    auth: Option<Authenticator>,
    rng: SeededRng,
}

impl Sender {
    fn new(key: Arc<NodeKey>, url: String, authenticates: bool) -> Self {
        let auth = authenticates
            .then(|| Authenticator::new(Arc::clone(&key), AuthConfig::DEFAULT, START - 1));
        Self {
            key,
            url,
            auth,
            rng: SeededRng::new(7),
        }
    }

    /// Its own link state, as of node 0's clock.
    fn link_state(&self, network: &Network) -> Vec<u8> {
        let info = NodeInfo {
            node_id: self.key.node_id(),
            sequence: network.clock(0),
            url: &self.url,
            alias: "",
            public_key: *self.key.public_key().as_bytes(),
            entries: vec![Entry::Neighbor(key(0).node_id()), Entry::Scheme(stub())],
        };
        sign_node_info(&self.key, &info)
    }

    /// `content` in an envelope to node 0.
    fn envelope(&mut self, network: &Network, content: &[u8]) -> Vec<u8> {
        let authentication = match &mut self.auth {
            Some(auth) => {
                let target = key(0);
                let (hmac, _) = auth
                    .authenticate(
                        network.clock(0),
                        &mut self.rng,
                        &target.node_id(),
                        &target.public_key(),
                        content,
                    )
                    .unwrap();
                Authentication::Hmac(hmac)
            }
            None => Authentication::None,
        };
        encode_envelope(&self.key.node_id(), &authentication, content)
    }

    fn body(&mut self, network: &Network, body: &Body) -> Vec<u8> {
        match body {
            Body::Arbitrary(bytes) => bytes.clone(),
            Body::BrokenContent(tag, mutations) => {
                let content = mutate(&content(*tag, &self.link_state(network)), mutations);
                self.envelope(network, &content)
            }
            Body::BrokenEnvelope(tag, mutations) => {
                let content = content(*tag, &self.link_state(network));
                mutate(&self.envelope(network, &content), mutations)
            }
        }
    }
}

/// What a node answers in place of the real body of a response.
#[derive(Clone, Debug)]
enum Answer {
    /// Any bytes.
    Arbitrary(Vec<u8>),
    /// The real body, broken.
    Broken(Vec<Mutation>),
}

fn answer() -> impl Strategy<Value = Answer> {
    prop_oneof![
        proptest::collection::vec(any::<u8>(), 0..256).prop_map(Answer::Arbitrary),
        mutations().prop_map(Answer::Broken),
    ]
}

proptest! {
    // Node 0, a bootstrap node peered with node 1, gets requests from its
    // peer, from a node outside the network, and from its peer without
    // authentication.
    #[test]
    fn inbound_requests_never_panic(
        requests in proptest::collection::vec((0..3usize, body()), 1..=8),
    ) {
        let mut network = Network::new(1, 2, 1, Config::DEV);
        settle(&mut network, 30_000);
        prop_assert!(network.node(0).peering_of(&key(1).node_id()).is_some());
        let stranger = Arc::new(NodeKey::from_bytes(&[90; 32]));
        let mut senders = [
            Sender::new(key(1), url(1), true),
            Sender::new(stranger, "https://stranger.test".to_owned(), true),
            Sender::new(key(1), url(1), false),
        ];
        for (from, body) in &requests {
            let bytes = senders[*from].body(&network, body);
            // Checks node 0's invariants after.
            let reply = network.inbound(0, &bytes);
            // 0: deferred, waiting on a probe.
            prop_assert!(
                matches!(reply.status, 0 | 200 | 400 | 401 | 409 | 429 | 503),
                "status {}",
                reply.status
            );
            if Envelope::decode(&bytes).is_err() {
                prop_assert_eq!(reply.status, 400);
            }
        }
        // The probes the requests started are answered.
        settle(&mut network, 10_000);
        network.node(0).check_invariants();
    }
}

proptest! {
    // Each case runs three nodes for two minutes: fewer cases than the
    // default.
    #![proptest_config(ProptestConfig::with_cases(64))]

    // Every response with status 200, or every second or third, carries a
    // body of the responder's choosing, MACed properly. From the start,
    // registrations, list polls, peering and probes get them; after the
    // network formed, gossip, packets, settlements and the end
    // of a peering.
    #[test]
    fn responses_never_panic(
        seed in any::<u64>(),
        from_start in any::<bool>(),
        every in 1..=3usize,
        answers in proptest::collection::vec(answer(), 1..=8),
    ) {
        let mut network = Network::new(seed, 3, 1, Config::DEV);
        if !from_start {
            settle(&mut network, 30_000);
        }
        let mut count = 0usize;
        network.tamper = Some(Box::new(move |body: &mut Vec<u8>| {
            count += 1;
            if !count.is_multiple_of(every) {
                return;
            }
            *body = match &answers[(count / every) % answers.len()] {
                Answer::Arbitrary(bytes) => bytes.clone(),
                Answer::Broken(mutations) => mutate(body, mutations),
            };
        }));
        settle(&mut network, 30_000);
        for index in 0..3 {
            network.act(index, |node, now| {
                let peers: Vec<NodeId> = node.peers().map(|(peer, _)| *peer).collect();
                for peer in &peers {
                    let _ = node.send_packet(now, peer, 1, &[12, 0], 10_000);
                    node.send_response(now, peer, 2, &[13, 0]);
                    node.send_response(now, peer, 4, &[14, 0]);
                    let _ = node.send_settlement(now, peer, stub(), 1, Vec::new());
                }
                if let Some(peer) = peers.first() {
                    node.terminate(now, peer, TerminationReason::Owner);
                }
            });
        }
        settle(&mut network, 60_000);
        for index in 0..3 {
            network.node(index).check_invariants();
        }
    }
}
