//! Inbound peer requests waiting on the engine are bounded in all and per
//! client, whichever transport carries them: over QUIC one connection holds
//! many requests, so one address must not take every place
//! (spec/areas/peer-protocol.md#transport).

#![allow(clippy::unwrap_used, clippy::disallowed_types)]

use std::net::IpAddr;
use std::sync::Arc;

use dassie_node::clock::ManualClock;
use dassie_node::node::{
    MAX_PEER_REQUESTS_PER_CLIENT, Node, NodeOptions, PeerOptions, Policies, Runtime,
};
use dassie_node::store::Store;
use dassie_node_core::testing::SeededRng;

fn node() -> Arc<Node> {
    Node::new(NodeOptions {
        store: Store::open_in_memory().unwrap(),
        clock: Arc::new(ManualClock::new(1_800_000_000_000)),
        rng: Box::new(SeededRng::new(1)),
        runtime: Runtime::Systemd,
        http_port: 80,
        https_port: 443,
        acme_ca_file: None,
        policies: Policies::DEFAULT,
        peer: PeerOptions::default(),
        dev_wallet_dir: None,
    })
    .unwrap()
}

fn ip(text: &str) -> IpAddr {
    text.parse().unwrap()
}

#[test]
fn one_client_holds_only_its_share_of_the_places() {
    let node = node();
    let one = ip("203.0.113.1");
    let mut held: Vec<_> = (0..MAX_PEER_REQUESTS_PER_CLIENT)
        .map(|_| node.peer_permit(one).unwrap())
        .collect();
    assert!(node.peer_permit(one).is_none());
    // Its /56 counts as one client too.
    let near = ip("2001:db8:0:1::1");
    let near_held: Vec<_> = (0..MAX_PEER_REQUESTS_PER_CLIENT)
        .map(|_| node.peer_permit(near).unwrap())
        .collect();
    assert!(node.peer_permit(ip("2001:db8:0:1:ffff::2")).is_none());
    // Another client still finds room.
    assert!(node.peer_permit(ip("203.0.113.2")).is_some());
    // A place given back is the client's again.
    held.pop();
    assert!(node.peer_permit(one).is_some());
    drop(near_held);
    assert!(node.peer_permit(near).is_some());
}
