//! The peer node never panics on what other nodes send it
//! (spec/decisions/tigerstyle.md): requests and the responses to its own,
//! arbitrary or broken from valid ones, handed to one node of a small
//! network, fresh or formed. The node checks its invariants after each
//! request it takes, and after each round of responses.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

mod support;

use std::sync::Arc;

use dassie_identity::{IdentityKey, NodeId};
use dassie_oer::Decode;
use dassie_peer::sim::{Arriving, Network, forger, key, url};
use dassie_peer::wire::{
    Content, Entry, NodeInfo, SchemeId, SettlementOffer, SignedNodeInfo, TerminationReason,
    sign_node_info,
};
use dassie_peer::{Config, Failure, InboundId, Output, PeerNode, Request, Response};
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
    /// Content of the kind with this tag, broken.
    BrokenContent(usize, Vec<Mutation>),
}

fn body() -> impl Strategy<Value = Body> {
    prop_oneof![
        1 => proptest::collection::vec(any::<u8>(), 0..512).prop_map(Body::Arbitrary),
        4 => (0..KINDS, mutations())
            .prop_map(|(tag, mutations)| Body::BrokenContent(tag, mutations)),
    ]
}

/// A node sending requests to node 0, over a connection that proves its
/// key.
struct Sender {
    key: Arc<IdentityKey>,
    url: String,
}

impl Sender {
    const fn new(key: Arc<IdentityKey>, url: String) -> Self {
        Self { key, url }
    }

    /// Its own link state, as of node 0's clock.
    fn link_state(&self, network: &Network) -> Vec<u8> {
        let info = NodeInfo {
            sequence: network.clock(0),
            url: &self.url,
            alias: "",
            public_key: *self.key.public_key().as_bytes(),
            slh_dsa_public_key: *self.key.identity().slh_dsa().as_bytes(),
            settlement: vec![SettlementOffer {
                scheme: stub(),
                details: &[],
            }],
            entries: vec![Entry::Neighbor(key(0).node_id())],
        };
        sign_node_info(&self.key, &info)
    }

    fn body(&self, network: &Network, body: &Body) -> Arriving {
        let bytes = match body {
            Body::Arbitrary(bytes) => bytes.clone(),
            Body::BrokenContent(tag, mutations) => {
                mutate(&content(*tag, &self.link_state(network)), mutations)
            }
        };
        Arriving::of(&self.key, bytes)
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
    // peer, from a node outside the network, and from the forger, which
    // sends what it saw as its own.
    #[test]
    fn inbound_requests_never_panic(
        requests in proptest::collection::vec((0..3usize, body()), 1..=8),
    ) {
        let mut network = Network::new(1, 2, 1, Config::DEV);
        settle(&mut network, 30_000);
        prop_assert!(network.node(0).peering_of(&key(1).node_id()).is_some());
        let stranger = Arc::new(dassie_identity::TableKeygen::identity_key(&[90; 32]));
        let senders = [
            Sender::new(key(1), url(1)),
            Sender::new(stranger, "https://stranger.test".to_owned()),
            Sender::new(forger(), url(1)),
        ];
        for (from, body) in &requests {
            let request = senders[*from].body(&network, body);
            // Checks node 0's invariants after.
            let reply = network.inbound(0, &request);
            // 0: deferred, waiting on a probe.
            prop_assert!(
                matches!(reply.status, 0 | 200 | 400 | 409 | 429 | 503),
                "status {}",
                reply.status
            );
            if Content::decode_exact(&request.body).is_err() {
                prop_assert_eq!(reply.status, 400);
            }
        }
        // The probes the requests started are answered.
        settle(&mut network, 10_000);
        network.node(0).check_invariants();
    }
}

/// The nodes of the network.
const COUNT: usize = 3;

/// The rounds a node's requests are answered in, each a second of its own
/// clock.
const ROUNDS: u64 = 8;

/// The most timers a round fires: a bound, never reached.
const MAX_TIMERS: usize = 1_000;

/// Takes the requests `node` put out before the network carries them; its
/// deferred replies are dropped.
fn take_requests(node: &mut PeerNode, out: &mut Vec<Request>) {
    while let Some(output) = node.poll_output() {
        if let Output::Request(request) = output {
            out.push(request);
        }
    }
}

/// Node `from`'s `request`, answered at once by the node it went to, as
/// that node answers it: a deferred answer, or one to a node outside the
/// network, as a request that may have arrived.
fn answer_of(network: &mut Network, from: usize, request: &Request) -> Result<Response, Failure> {
    let Some(to) = (0..COUNT).find(|index| key(*index).node_id() == request.to) else {
        return Err(Failure::Unknown);
    };
    let reply = network.inbound(to, &Arriving::of(&key(from), request.body.clone()));
    if reply.status == 0 {
        return Err(Failure::Unknown);
    }
    Ok(Response {
        status: reply.status,
        body: reply.body,
    })
}

proptest! {
    // Each case runs the network's forming for up to 30 s first: fewer
    // cases than the default.
    #![proptest_config(ProptestConfig::with_cases(64))]

    // The response handler alone, from a node of the network as it forms
    // on one seed: at its start, part way, or formed. Node `index` asks
    // what its timers ask over a few seconds of its own clock; the
    // bootstrap node also takes the others' registrations, and a node with
    // peers sends them packets, responses and settlements and ends a
    // peering. Each request is answered at once by the node it went to, as
    // that node answers it, and the body of every response with status
    // 200, or of every second or third, is one of the responder's choosing,
    // which the connection takes as the responder's: registrations, list
    // polls, peering, probes, gossip, packets, settlements and the end of a
    // peering get them. The node checks its invariants once a round, after
    // it took the round's responses and fired its timers.
    #[test]
    fn responses_never_panic(
        index in 0..COUNT,
        settled in prop_oneof![Just(0u64), 0..30_000u64, Just(30_000u64)],
        every in 1..=3usize,
        answers in proptest::collection::vec(answer(), 1..=8),
    ) {
        let mut network = Network::new(1, COUNT, 1, Config::DEV);
        settle(&mut network, settled);
        let start = network.clock(index);
        // Each registration new to the bootstrap node starts a probe.
        let registrations: Vec<Arriving> = (1..COUNT)
            .filter(|_| index == 0)
            .map(|from| {
                let signed = Sender::new(key(from), url(from)).link_state(&network);
                let info = SignedNodeInfo::decode_exact(&signed).unwrap();
                Arriving::new(&key(from), &Content::Registration(info))
            })
            .collect();
        let mut pending = Vec::new();
        network.act_with_hooks(index, |node, hooks, now| {
            for (id, registration) in (u64::MAX - 10..).zip(&registrations) {
                let from = registration.from;
                node.handle_inbound(now, hooks, InboundId(id), from, &registration.body, None);
            }
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
            take_requests(node, &mut pending);
        });
        let mut count = 0usize;
        for round in 1..=ROUNDS {
            let mut answered = Vec::new();
            for request in pending.drain(..) {
                let mut result = answer_of(&mut network, index, &request);
                if let Ok(response) = &mut result
                    && response.status == 200
                {
                    count += 1;
                    if count.is_multiple_of(every) {
                        response.body = match &answers[(count / every) % answers.len()] {
                            Answer::Arbitrary(bytes) => bytes.clone(),
                            Answer::Broken(mutations) => mutate(&response.body, mutations),
                        };
                    }
                }
                answered.push((request.id, result));
            }
            let until = start + round * 1_000;
            network.act_with_hooks(index, |node, hooks, now| {
                for (id, result) in answered {
                    node.handle_response(now, hooks, id, result);
                }
                for _ in 0..MAX_TIMERS {
                    match node.poll_timeout() {
                        Some(at) if at <= until => node.handle_timeout(at, hooks),
                        _ => break,
                    }
                }
                take_requests(node, &mut pending);
            });
        }
        network.node(index).check_invariants();
    }
}
