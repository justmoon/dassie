//! The peer node's rules, one at a time, on small simulated networks. Most
//! tests are a gap of the original
//! (spec/original/03-peer-network.md#19-open-questions--known-gaps) that
//! would fail against its behaviour.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic,
    clippy::unreachable
)]

use std::sync::Arc;

use dassie_identity::NodeKey;
use dassie_node_core::testing::SeededRng;
use dassie_oer::{Decode, Encode};
use dassie_peer::auth::{AuthConfig, Authenticator};
use dassie_peer::sim::{Network, START, key, url};
use dassie_peer::wire::{
    Authentication, Content, Entry, NodeInfo, PeeringRequest, PeeringResponse, RefusalReason,
    ResponseEnvelope, SchemeId, SignedNodeInfo, encode_envelope, sign_node_info,
};
use dassie_peer::{Config, Event, OwnInfo, PeerNode, PeerStatus, RegisterError, Sources, Stored};

fn settle(network: &mut Network, ms: u64) {
    let until = network.now + ms;
    network.run_until(until);
}

/// A request from `from` (by key) to node `to` of `network`.
fn request(network: &Network, from: &Arc<NodeKey>, to: usize, content: &Content<'_>) -> Vec<u8> {
    request_at(network.clock(to), from, to, content)
}

/// The same, stamped `timestamp_ms` by the sender's clock.
fn request_at(timestamp_ms: u64, from: &Arc<NodeKey>, to: usize, content: &Content<'_>) -> Vec<u8> {
    let mut auth = Authenticator::new(Arc::clone(from), AuthConfig::DEFAULT, START - 1);
    let bytes = content.to_vec();
    let target = key(to);
    let (hmac, _) = auth
        .authenticate(
            timestamp_ms,
            &mut SeededRng::new(9),
            &target.node_id(),
            &target.public_key(),
            &bytes,
        )
        .unwrap();
    encode_envelope(&from.node_id(), &Authentication::Hmac(hmac), &bytes)
}

fn link_state(key: &NodeKey, sequence: u64, url: &str) -> Vec<u8> {
    let info = NodeInfo {
        node_id: key.node_id(),
        sequence,
        url,
        alias: "",
        public_key: *key.public_key().as_bytes(),
        entries: vec![Entry::Scheme(SchemeId::parse("stub").unwrap())],
    };
    sign_node_info(key, &info)
}

#[test]
fn two_nodes_register_peer_and_route() {
    let mut network = Network::new(1, 2, 1, Config::DEV);
    settle(&mut network, 30_000);
    let (a, b) = (network.node(0), network.node(1));
    assert!(a.peering_of(b.own()).is_some() && b.peering_of(a.own()).is_some());
    assert_eq!(a.routes()[b.own()].first_hops, vec![*b.own()]);
    assert!(a.majority().contains(b.own()), "registered, so on the list");
    assert!(b.registration_status()[0].listed);
    let peering = a.peering_of(b.own()).unwrap();
    assert_eq!(peering.credit_out, dassie_peer::sim::CREDIT);
    let views = a.nodes();
    let view = |id: &dassie_identity::NodeId| views.iter().find(|view| view.id == *id).unwrap();
    let (own, peer) = (view(a.own()), view(b.own()));
    assert_eq!((own.distance_hops, peer.distance_hops), (Some(0), Some(1)));
    assert_eq!(peer.peer_status, PeerStatus::Peered);
    assert_eq!(peer.neighbors, vec![*a.own()]);
    assert_eq!(peer.public_key, Some(key(1).public_key()));
    assert_eq!(peer.schemes, vec![SchemeId::parse("stub").unwrap()]);
}

// NET-3, NET-4: a peering request whose link state is not the sender's is
// refused; one with a forged sender does not authenticate.
#[test]
fn peering_requests_carry_the_senders_own_link_state() {
    let mut network = Network::new(2, 2, 1, Config::DEV);
    settle(&mut network, 30_000);
    let mallory = Arc::new(NodeKey::from_bytes(&[77; 32]));
    let victim = key(1);
    let state = link_state(&victim, network.clock(0), &url(1));
    let content = Content::PeeringRequest(PeeringRequest {
        scheme: SchemeId::parse("stub").unwrap(),
        node_info: SignedNodeInfo::decode_exact(&state).unwrap(),
        data: &[],
        versions: vec![1],
        credit: 0,
    });
    let body = request(&network, &mallory, 0, &content);
    assert_eq!(network.inbound(0, &body).status, 400);
    // Mallory cannot send it as the victim either: the MAC needs its key.
    let envelope = dassie_peer::wire::Envelope::decode(&body).unwrap();
    let Authentication::Hmac(mut hmac) = envelope.authentication else {
        unreachable!()
    };
    hmac.sender_key = *victim.public_key().as_bytes();
    let forged = encode_envelope(
        &victim.node_id(),
        &Authentication::Hmac(hmac),
        envelope.content,
    );
    assert_eq!(network.inbound(0, &forged).status, 401);
}

// A packet goes only between peers, authenticated.
#[test]
fn packets_need_a_peering() {
    let mut network = Network::new(3, 2, 1, Config::DEV);
    settle(&mut network, 30_000);
    let stranger = Arc::new(NodeKey::from_bytes(&[78; 32]));
    let packet = Content::InterledgerPacket {
        request_id: 1,
        packet: &[1, 2, 3],
    };
    let body = request(&network, &stranger, 0, &packet);
    assert_eq!(network.inbound(0, &body).status, 409);
    let bare = encode_envelope(&stranger.node_id(), &Authentication::None, &packet.to_vec());
    assert_eq!(network.inbound(0, &bare).status, 401);
    let peer = *network.node(1).own();
    let now = network.clock(0);
    network
        .node_mut(0)
        .send_packet(now, &peer, 5, &[9], 10_000)
        .unwrap();
    assert!(
        network
            .node_mut(0)
            .send_packet(now, &stranger.node_id(), 5, &[9], 10_000)
            .is_err()
    );
}

// D4: registrations lapse without renewal and leave the served list, so
// the node loses its majority place, and leaves the table of every node
// that is not its peer after the grace period (NET-18). Peers keep it
// until liveness ends the peering (M4b).
#[test]
fn registrations_expire_and_nodes_leave() {
    let mut network = Network::new(4, 6, 1, Config::DEV);
    settle(&mut network, 30_000);
    let gone = *network.node(5).own();
    assert!(network.node(0).nodes().iter().any(|v| v.id == gone));
    network.crash(5);
    let config = network.config;
    settle(
        &mut network,
        config.registration_ttl_ms + config.removal_grace_ms + 30_000,
    );
    let bootstrap = network.node(0);
    let view = bootstrap.nodes().into_iter().find(|v| v.id == gone);
    assert!(
        view.as_ref()
            .is_none_or(|v| v.registered_until_ms.is_none())
    );
    let mut removed = 0;
    for index in 0..5 {
        let node = network.node(index);
        assert!(!node.majority().contains(&gone), "node {index}");
        let held = node.nodes().into_iter().find(|v| v.id == gone);
        // A peer keeps it, or ended its peering with it for its silence,
        // which takes as long as the window here, and holds it no more
        // than the grace (D28).
        let ended = network
            .events
            .iter()
            .any(|(at, event)| *at == index && *event == Event::PeeringEnded { peer: gone });
        match held {
            Some(view) if view.sources == Sources::PEER => {}
            Some(view) => assert!(view.sources.is_empty() && ended, "node {index}"),
            None => removed += 1,
        }
    }
    assert!(removed > 0, "some node was not its peer");
}

// D4: with most bootstrap nodes down, their lists go stale, and nobody is
// removed for losing a majority they can no longer have.
#[test]
fn membership_freezes_when_lists_go_stale() {
    let mut network = Network::new(5, 5, 3, Config::DEV);
    settle(&mut network, 60_000);
    let everyone: Vec<_> = (0..5).map(|i| *network.node(i).own()).collect();
    network.crash(0);
    network.crash(1);
    settle(&mut network, 600_000);
    for index in 2..5 {
        let node = network.node(index);
        for id in &everyone {
            assert!(
                node.nodes().iter().any(|v| v.id == *id),
                "node {index} lost {id}"
            );
        }
    }
}

// NET-7: our sequence never goes back across a restart, even when the
// clock does; a sequence far in the future is given up for now.
#[test]
fn our_sequence_survives_restarts_and_clock_rollback() {
    let start = |stored: u64| {
        let mut node = PeerNode::new(
            Config::DEV,
            key(0),
            OwnInfo {
                url: Some(url(0)),
                alias: String::new(),
            },
            Vec::new(),
            Stored {
                own_sequence: stored,
                ..Stored::default()
            },
            Box::new(SeededRng::new(1)),
            START,
        );
        node.handle_timeout(START, &mut dassie_peer::sim::StubHooks::default());
        let mut batch = dassie_peer::PeerBatch::default();
        node.take_changes(&mut batch);
        batch.own_sequence.unwrap()
    };
    assert_eq!(start(START + 60_000), START + 60_001);
    assert_eq!(start(START - 1), START);
    assert_eq!(start(START + 86_400_000), START);
}

// A link state far in the future is refused: it would never go stale.
#[test]
fn link_states_from_the_future_are_refused() {
    let mut network = Network::new(6, 2, 1, Config::DEV);
    settle(&mut network, 30_000);
    let subject = key(1);
    let future = link_state(&subject, network.clock(0) + 3_600_000, &url(1));
    let content = Content::LinkStateUpdate(SignedNodeInfo::decode_exact(&future).unwrap());
    let body = request(&network, &subject, 0, &content);
    assert_eq!(network.inbound(0, &body).status, 200);
    let held = network
        .node(0)
        .nodes()
        .into_iter()
        .find(|v| v.id == subject.node_id())
        .unwrap();
    assert!(held.sequence.unwrap() < network.clock(0) + 600_000);
}

// A peering request from a node not in the table is refused as unknown,
// and the answer is authenticated.
#[test]
fn unknown_nodes_cannot_peer() {
    let mut network = Network::new(7, 2, 1, Config::DEV);
    settle(&mut network, 30_000);
    let stranger = Arc::new(NodeKey::from_bytes(&[79; 32]));
    let state = link_state(&stranger, network.clock(0), "https://stranger.test");
    let content = Content::PeeringRequest(PeeringRequest {
        scheme: SchemeId::parse("stub").unwrap(),
        node_info: SignedNodeInfo::decode_exact(&state).unwrap(),
        data: &[],
        versions: vec![1],
        credit: 0,
    });
    let body = request(&network, &stranger, 0, &content);
    let reply = network.inbound(0, &body);
    assert_eq!(reply.status, 200);
    let envelope = ResponseEnvelope::decode(&reply.body).unwrap();
    assert!(envelope.mac.is_some());
    assert_eq!(
        PeeringResponse::decode_exact(envelope.body).unwrap(),
        PeeringResponse::Refused(RefusalReason::UnknownNode)
    );
}

// The bootstrap node lists itself and its registrations; a node it
// learned of only through its own majority stays out of its list.
#[test]
fn the_served_list_is_registrations_and_self() {
    let mut network = Network::new(8, 4, 2, Config::DEV);
    settle(&mut network, 60_000);
    let bootstrap = network.node(0);
    for view in bootstrap.nodes() {
        let listed = view.registered_until_ms.is_some() || view.id == *bootstrap.own();
        assert!(listed || !view.sources.contains(Sources::REGISTERED));
    }
    assert!(
        network
            .events
            .iter()
            .any(|(index, event)| *index == 0 && matches!(event, Event::Registered(_)))
    );
}

// The owner can register with a bootstrap node now, whatever the timers
// say (UI-4); only with a bootstrap node, and once at a time.
#[test]
fn the_owner_registers_now() {
    // Once registered, renewing is not due for another 30 s.
    let mut network = Network::new(1, 2, 1, Config::DEV);
    while network.node(1).registration_status()[0]
        .registered_ms
        .is_none()
    {
        settle(&mut network, 100);
    }
    settle(&mut network, 5_000);
    let bootstrap = *network.node(0).own();
    let before = network.node(1).registration_status()[0]
        .registered_ms
        .unwrap();
    let own = *network.node(1).own();
    let stranger = key(7).node_id();
    let mut results = vec![];
    network.act(1, |node, now| {
        results.push(node.register_now(now, &own));
        results.push(node.register_now(now, &stranger));
        results.push(node.register_now(now, &bootstrap));
        results.push(node.register_now(now, &bootstrap));
    });
    assert_eq!(
        results,
        vec![
            Err(RegisterError::NotBootstrap),
            Err(RegisterError::NotBootstrap),
            Ok(()),
            Err(RegisterError::InFlight)
        ]
    );
    settle(&mut network, 1_000);
    let after = network.node(1).registration_status()[0]
        .registered_ms
        .unwrap();
    assert!(
        after >= before + 5_000,
        "registered again: {before} → {after}"
    );
}

/// An identity of a sender's own making.
fn made_up(index: usize) -> Arc<NodeKey> {
    let mut seed = [0xa5_u8; 32];
    seed[..8].copy_from_slice(&(index as u64).to_be_bytes());
    Arc::new(NodeKey::from_bytes(&seed))
}

/// A registration of `from` at `url` with bootstrap node 0.
fn registration(network: &Network, from: &Arc<NodeKey>, url: &str) -> Vec<u8> {
    let state = link_state(from, network.clock(0), url);
    let info = SignedNodeInfo::decode_exact(&state).unwrap();
    request(network, from, 0, &Content::Registration(info))
}

/// A request that changes state, from a node that is not a peer: refused
/// with 409, after its replay state is recorded.
fn settlement_request(network: &Network, from: &Arc<NodeKey>) -> Vec<u8> {
    let content = Content::SettlementRequest {
        scheme: SchemeId::parse("stub").unwrap(),
    };
    request(network, from, 0, &content)
}

// Anyone may make identities, and requests that only read are answered 200,
// which the HTTP layer never charges. Each such request held a place in the
// replay pool for strangers for two minutes, so one address could fill it
// at a few requests a second, and every newcomer then got 503. A request
// that only reads now leaves no replay state.
#[test]
fn new_identities_reading_leave_room_for_newcomers() {
    let mut network = Network::new(4, 2, 1, Config::DEV);
    settle(&mut network, 30_000);
    let pool = AuthConfig::DEFAULT.max_senders;
    for index in 0..pool + 8 {
        let body = request(
            &network,
            &made_up(index),
            0,
            &Content::LinkStateRequest(vec![]),
        );
        assert_eq!(network.inbound(0, &body).status, 200, "request {index}");
    }
    let newcomer = Arc::new(NodeKey::from_bytes(&[201; 32]));
    let body = registration(&network, &newcomer, "https://newcomer.example");
    assert_eq!(
        network.inbound(0, &body).status,
        0,
        "the registration waits on a probe of its URL"
    );
}

// A registered node renews every few hours, so between renewals its replay
// state went idle, and strangers could take its place: its renewal got 503.
// A node registered with us keeps its replay state for good now.
#[test]
fn a_registered_node_renews_while_strangers_fill_the_pool() {
    let config = Config {
        auth: AuthConfig {
            max_senders: 8,
            ..AuthConfig::DEFAULT
        },
        ..Config::DEFAULT
    };
    let count = 6;
    let mut network = Network::new(5, count, 1, config);
    settle(&mut network, 600_000);
    // A node registered with the bootstrap node, and neither its peer nor
    // a node it holds a channel with, which keep their state anyway.
    let registered = registered_stranger(&network, count);
    let mut busy = false;
    for index in 0..2 * config.auth.max_senders {
        match network
            .inbound(0, &settlement_request(&network, &made_up(index)))
            .status
        {
            409 => {}
            503 => {
                busy = true;
                break;
            }
            other => panic!("unexpected status {other}"),
        }
    }
    assert!(busy, "strangers hold every place");
    let body = registration(&network, &key(registered), &url(registered));
    assert_eq!(network.inbound(0, &body).status, 200, "renewed");
    network.node(0).check_invariants();
}

/// A node registered with bootstrap node 0 that is neither its peer nor a
/// node it holds a channel with, whose replay state was kept apart from
/// theirs.
fn registered_stranger(network: &Network, count: usize) -> usize {
    let views = network.node(0).nodes();
    (1..count)
        .find(|index| {
            let id = key(*index).node_id();
            views
                .iter()
                .any(|view| view.id == id && view.registered_until_ms.is_some())
                && network.node(0).peering_of(&id).is_none()
                && !network.holds_channel(&key(0).node_id(), *index)
        })
        .expect("a registered node that is not a peer")
}

// Only a peer's replay floor was persisted. After a restart, every other
// sender's floor was the start time, so a registration renewed just before
// it, by a node whose clock runs ahead, was taken again when replayed: the
// timestamp was within the window and above the start. The floor of every
// sender whose replay state is kept for good is persisted now.
#[test]
fn a_restart_does_not_reopen_a_registered_nodes_replays() {
    let count = 6;
    let mut network = Network::new(5, count, 1, Config::DEFAULT);
    settle(&mut network, 600_000);
    let registered = registered_stranger(&network, count);
    // Its clock runs ahead of node 0's by less than the window.
    let ahead = network.clock(0) + AuthConfig::DEFAULT.skew_ms / 2;
    let state = link_state(&key(registered), network.clock(0), &url(registered));
    let info = SignedNodeInfo::decode_exact(&state).unwrap();
    let renewal = request_at(ahead, &key(registered), 0, &Content::Registration(info));
    assert_eq!(network.inbound(0, &renewal).status, 200, "renewed");
    network.crash(0);
    network.start(0);
    let replayed = network.inbound(0, &renewal);
    assert_eq!(replayed.status, 401, "refused as a replay");
    network.node(0).check_invariants();
}

// One network (a /24 or /48) holds only its share of the replay pool for
// strangers, so one address, or its neighbours, cannot take every place.
#[test]
fn one_network_holds_only_its_share_of_strangers() {
    let mut network = Network::new(6, 2, 1, Config::DEV);
    settle(&mut network, 30_000);
    let share = AuthConfig::DEFAULT.max_senders_per_network;
    let address = |octets: [u8; 4]| Some(std::net::IpAddr::from(octets));
    for index in 0..share {
        let body = settlement_request(&network, &made_up(index));
        let last = u8::try_from(index).unwrap();
        let reply = network.inbound_from(0, &body, address([198, 51, 100, last]));
        assert_eq!(reply.status, 409, "sender {index}");
    }
    let body = settlement_request(&network, &made_up(share));
    let reply = network.inbound_from(0, &body, address([198, 51, 100, 250]));
    assert_eq!(reply.status, 503, "the network has its share");
    let body = settlement_request(&network, &made_up(share + 1));
    let reply = network.inbound_from(0, &body, address([203, 0, 113, 1]));
    assert_eq!(reply.status, 409, "another network has room");
    let newcomer = Arc::new(NodeKey::from_bytes(&[202; 32]));
    let body = registration(&network, &newcomer, "https://newcomer.example");
    let reply = network.inbound_from(0, &body, address([192, 0, 2, 1]));
    assert_eq!(reply.status, 0, "the registration waits on a probe");
    network.node(0).check_invariants();
}
