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
use dassie_kit::sim::SeededRng;
use dassie_oer::{Decode, Encode};
use dassie_peer::auth::{AuthConfig, Authenticator};
use dassie_peer::sim::{Network, START, key, url};
use dassie_peer::wire::{
    Authentication, Content, Entry, NodeInfo, PeeringRequest, PeeringResponse, RefusalReason,
    ResponseEnvelope, SchemeId, SignedNodeInfo, TerminationReason, encode_envelope, sign_node_info,
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
    request_by(&mut auth, timestamp_ms, to, content)
}

/// The same, in the session of `auth`, so that a sender's requests follow
/// one another as a real sender's do.
fn request_by(
    auth: &mut Authenticator,
    timestamp_ms: u64,
    to: usize,
    content: &Content<'_>,
) -> Vec<u8> {
    let from = auth.node_key().node_id();
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
    encode_envelope(&from, &Authentication::Hmac(hmac), &bytes)
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

// spec/areas/network.md#how-link-states-spread: a newcomer that registered
// with the only bootstrap node before its scheme synced, and whose new link
// state then got lost on its way there, still peers with it. The bootstrap
// node holds the newcomer's state without the scheme, and asks it for
// nothing; the newcomer's sweeps ask the bootstrap node for its own state,
// which nothing else brings, and the newcomer asks to peer.
#[test]
fn a_newcomer_learns_the_only_bootstrap_nodes_link_state() {
    let stub = SchemeId::parse("stub").unwrap();
    let mut network = Network::new(1, 2, 1, Config::DEFAULT);
    network.act(1, |node, now| node.set_schemes(now, Vec::new()));
    while network.node(1).registration_status()[0]
        .registered_ms
        .is_none()
    {
        settle(&mut network, 1_000);
        assert!(network.now < START + 120_000, "node 1 registers");
    }
    let newcomer = *network.node(1).own();
    let held = |network: &Network| {
        let view = network.node(0).nodes();
        view.into_iter().find(|v| v.id == newcomer).unwrap().schemes
    };
    assert!(held(&network).is_empty(), "registered before its scheme");

    // Its scheme syncs, and its new link state is lost on the way to the
    // bootstrap node.
    network.cut_one_way.insert((1, 0));
    network.act(1, |node, now| node.set_schemes(now, vec![stub]));
    settle(&mut network, Config::DEFAULT.issue_debounce_ms + 1_000);
    network.cut_one_way.clear();
    assert!(held(&network).is_empty(), "the push was lost");

    // Well before a renewal or a re-issue would bring it.
    settle(&mut network, 60_000);
    let (bootstrap, newcomer) = (network.node(0), network.node(1));
    assert!(newcomer.peering_of(bootstrap.own()).is_some());
    assert!(bootstrap.peering_of(newcomer.own()).is_some());
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

// Only a registrar, a node that is one of its own bootstrap nodes, takes
// name messages (spec/areas/network.md#names): any other refuses each with
// 409, which says it was not acted upon, as it refuses registrations; one
// that must be authenticated and is not gets 401 first.
#[test]
fn only_a_registrar_takes_name_messages() {
    use dassie_peer::names::{Commitment, Name, Salt};
    let mut network = Network::new(2, 2, 1, Config::DEV);
    assert!(network.node(0).registrar().is_some() && network.node(1).registrar().is_none());
    settle(&mut network, 30_000);
    let stranger = Arc::new(NodeKey::from_bytes(&[79; 32]));
    let alice = Name::parse("alice").unwrap();
    let contents = [
        Content::NameCommit(Commitment([1; 32])),
        Content::NameUnseal {
            name: alice,
            salt: Salt([2; 32]),
        },
        Content::NameRenew(alice),
        Content::NameListHashRequest,
        Content::NameListRequest,
    ];
    let mut auth = Authenticator::new(Arc::clone(&stranger), AuthConfig::DEFAULT, START - 1);
    for content in &contents {
        let body = request_by(&mut auth, network.clock(1), 1, content);
        assert_eq!(network.inbound(1, &body).status, 409, "{content:?}");
        let bare = encode_envelope(
            &stranger.node_id(),
            &Authentication::None,
            &content.to_vec(),
        );
        let status = if content.kind().requires_authentication() {
            401
        } else {
            409
        };
        assert_eq!(network.inbound(1, &bare).status, status, "{content:?}");
    }
}

// spec/decisions/membership-economics.md: registrations lapse without renewal
// and leave the served list, so the node loses its majority place, and leaves
// the table of every node that is not its peer after the grace period
// (NET-18). Peers keep it until liveness ends the peering (M4b).
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
        // than the grace (spec/decisions/ending-a-peering.md).
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

// spec/decisions/membership-economics.md: with most bootstrap nodes down,
// their lists go stale, and nobody is removed for losing a majority they can
// no longer have.
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
                quic: false,
                feed_port: None,
                hub: None,
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
    settlement_request_at(network.clock(0), from)
}

/// The same, stamped `timestamp_ms` by the sender's clock.
fn settlement_request_at(timestamp_ms: u64, from: &Arc<NodeKey>) -> Vec<u8> {
    let content = Content::SettlementRequest {
        scheme: SchemeId::parse("stub").unwrap(),
    };
    request_at(timestamp_ms, from, 0, &content)
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

// A peering that ended kept no replay floor, so after a restart a request
// its former peer sent shortly before, stamped ahead of our clock, was
// taken again: the timestamp was within the window and above the start. A
// node we hold a settlement channel with keeps its floor, peered or not.
#[test]
fn a_restart_does_not_reopen_a_former_peers_replays() {
    let count = 6;
    let mut network = Network::new(7, count, 1, Config::DEV);
    settle(&mut network, 60_000);
    // Two peers neither of which is the other's bootstrap node, nor
    // registered with it, so that only the peering and then its channel
    // keep the sender's replay state.
    let (at, from) = (1..count)
        .flat_map(|at| (1..count).map(move |from| (at, from)))
        .find(|(at, from)| {
            at != from
                && network
                    .node(*at)
                    .peering_of(&key(*from).node_id())
                    .is_some()
        })
        .expect("two peers apart from the bootstrap node");
    let former = key(from).node_id();
    let mut sender = Authenticator::new(key(from), AuthConfig::DEFAULT, START - 1);
    let content = Content::SettlementRequest {
        scheme: SchemeId::parse("stub").unwrap(),
    };
    // Its clock runs ahead of ours by less than the window.
    let ahead = AuthConfig::DEFAULT.skew_ms / 2;
    let while_peered = request_by(&mut sender, network.clock(at) + ahead, at, &content);
    assert_ne!(network.inbound(at, &while_peered).status, 401, "taken");
    network.act(at, |node, now| {
        assert!(node.terminate(now, &former, TerminationReason::Owner));
    });
    settle(&mut network, 10_000);
    assert!(network.node(at).peering_of(&former).is_none(), "ended");
    let own = *network.node(at).own();
    assert!(network.holds_channel(&own, from), "the channel is kept");
    let after_the_end = request_by(&mut sender, network.clock(at) + ahead, at, &content);
    assert_ne!(network.inbound(at, &after_the_end).status, 401, "taken");
    network.crash(at);
    network.start(at);
    for (body, sent) in [
        (&while_peered, "while peered"),
        (&after_the_end, "after the end"),
    ] {
        let replayed = network.inbound(at, body);
        assert_eq!(replayed.status, 401, "sent {sent}, refused as a replay");
    }
    let fresh = request_by(&mut sender, network.clock(at) + ahead + 1, at, &content);
    assert_ne!(
        network.inbound(at, &fresh).status,
        401,
        "a fresh one is taken"
    );
    network.node(at).check_invariants();
}

// Raising every sender's floor after a restart by the timestamp window
// would refuse honest requests for that long. Only what was taken before
// the restart is refused: a registered node's next renewal, and a
// stranger's first request, are taken right after it.
#[test]
fn honest_requests_are_taken_right_after_a_restart() {
    let count = 6;
    let mut network = Network::new(5, count, 1, Config::DEFAULT);
    settle(&mut network, 600_000);
    let registered = registered_stranger(&network, count);
    let mut sender = Authenticator::new(key(registered), AuthConfig::DEFAULT, START - 1);
    let renewal = |network: &Network, sender: &mut Authenticator, ahead: u64| {
        let state = link_state(&key(registered), network.clock(0), &url(registered));
        let info = SignedNodeInfo::decode_exact(&state).unwrap();
        request_by(
            sender,
            network.clock(0) + ahead,
            0,
            &Content::Registration(info),
        )
    };
    let first = renewal(&network, &mut sender, 1_000);
    assert_eq!(network.inbound(0, &first).status, 200, "renewed");
    network.crash(0);
    network.start(0);
    assert_eq!(
        network.inbound(0, &first).status,
        401,
        "refused as a replay"
    );
    let next = renewal(&network, &mut sender, 2_000);
    assert_eq!(network.inbound(0, &next).status, 200, "renewed again");
    let stranger = settlement_request_at(network.clock(0) + 1, &made_up(0));
    assert_eq!(
        network.inbound(0, &stranger).status,
        409,
        "taken, and refused"
    );
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

/// A node that takes QUIC, with `boot` as its bootstrap node.
fn taking_quic(own: &Arc<NodeKey>, boot: &NodeKey) -> PeerNode {
    PeerNode::new(
        Config::DEV,
        Arc::clone(own),
        OwnInfo {
            url: Some(url(0)),
            alias: String::new(),
            quic: true,
            feed_port: None,
            hub: None,
        },
        vec![dassie_peer::Bootstrap {
            id: boot.node_id(),
            url: url(1),
            key: boot.public_key(),
        }],
        Stored::default(),
        Box::new(SeededRng::new(1)),
        START,
    )
}

/// The requests the node wants sent now.
fn sent(node: &mut PeerNode) -> Vec<dassie_peer::Request> {
    let mut requests = Vec::new();
    while let Some(output) = node.poll_output() {
        if let dassie_peer::Output::Request(request) = output {
            requests.push(request);
        }
    }
    requests
}

// A node that takes QUIC says so in its link state, and asks a bootstrap
// node known only from the configuration at once, at its configured URL:
// QUIC is the only transport, and needs no link state to dial a node whose
// ID it knows.
#[test]
fn link_states_say_who_takes_quic_and_bootstrap_nodes_are_asked_at_once() {
    let (own, boot) = (key(0), key(1));
    let mut hooks = dassie_peer::sim::StubHooks::default();
    let mut node = taking_quic(&own, &boot);
    node.handle_timeout(START, &mut hooks);
    let before = sent(&mut node);
    // Our link state goes to the bootstrap node at once.
    let ours = before
        .iter()
        .find_map(|request| {
            let envelope = dassie_peer::wire::Envelope::decode(&request.body).ok()?;
            match Content::decode_exact(envelope.content).ok()? {
                Content::LinkStateUpdate(state) => Some(state.info.speaks_quic()),
                _ => None,
            }
        })
        .unwrap();
    assert!(ours);
    let to_boot: Vec<_> = before.iter().filter(|r| r.to == boot.node_id()).collect();
    assert!(!to_boot.is_empty());
    assert!(to_boot.iter().all(|request| request.url == url(1)));
}

/// The link state `node` issues first, as it goes to its bootstrap node.
fn own_link_state(mut node: PeerNode) -> Vec<u8> {
    node.handle_timeout(START, &mut dassie_peer::sim::StubHooks::default());
    sent(&mut node)
        .iter()
        .find_map(|request| {
            let envelope = dassie_peer::wire::Envelope::decode(&request.body).ok()?;
            match Content::decode_exact(envelope.content).ok()? {
                Content::LinkStateUpdate(state) => Some(state.bytes.to_vec()),
                _ => None,
            }
        })
        .unwrap()
}

/// A core node at `url(0)` whose rate feed is at `feed_port`, if any.
fn with_feed(own: &Arc<NodeKey>, boot: &NodeKey, feed_port: Option<u16>) -> PeerNode {
    PeerNode::new(
        Config::DEV,
        Arc::clone(own),
        OwnInfo {
            url: Some(url(0)),
            alias: String::new(),
            quic: true,
            feed_port,
            hub: None,
        },
        vec![dassie_peer::Bootstrap {
            id: boot.node_id(),
            url: url(1),
            key: boot.public_key(),
        }],
        Stored::default(),
        Box::new(SeededRng::new(1)),
        START,
    )
}

// A core node with a rate feed names its port in its link state, beside
// QUIC's; one without names none (spec/areas/rate-feed.md#finding-a-feed).
#[test]
fn a_core_node_names_its_feed_port_in_its_link_state() {
    let (own, boot) = (key(0), key(1));
    let bytes = own_link_state(with_feed(&own, &boot, Some(7768)));
    let state = SignedNodeInfo::decode_exact(&bytes).unwrap();
    assert!(state.verify().is_ok());
    assert_eq!(state.info.feed_port(), Some(7768));
    assert!(state.info.speaks_quic());
    let bytes = own_link_state(with_feed(&own, &boot, None));
    let state = SignedNodeInfo::decode_exact(&bytes).unwrap();
    assert_eq!(state.info.feed_port(), None);
}

// A member of a settlement hub names it in its link state, its account and
// whether it takes new peers over it with it; a node in no hub names none
// (spec/designs/settlement-hubs.md#11-choosing-a-hub).
#[test]
fn a_hub_member_names_its_hub_in_its_link_state() {
    let (own, boot) = (key(0), key(1));
    let membership = dassie_peer::wire::HubMembership {
        hub_id: [0x42; 32],
        account: 7,
        takes_peers: true,
    };
    let mut node = with_feed(&own, &boot, Some(7768));
    node.set_own_info(
        START,
        OwnInfo {
            url: Some(url(0)),
            alias: String::new(),
            quic: true,
            feed_port: Some(7768),
            hub: Some(membership),
        },
    );
    let bytes = own_link_state(node);
    let state = SignedNodeInfo::decode_exact(&bytes).unwrap();
    assert!(state.verify().is_ok());
    assert_eq!(state.info.hub(), Some(membership));
    assert_eq!(state.info.feed_port(), Some(7768));
    assert!(state.info.speaks_quic());
    let bytes = own_link_state(with_feed(&own, &boot, None));
    let state = SignedNodeInfo::decode_exact(&bytes).unwrap();
    assert_eq!(state.info.hub(), None);
}

// spec/areas/peer-protocol.md#link-states: the first hub, set through the
// own info with another membership, is changed in the list, which stays.
#[test]
fn the_first_hub_changes_in_the_list_through_the_own_info() {
    use dassie_peer::wire::{HubListing, HubMembership};
    let (own, boot) = (key(0), key(1));
    let listing = |scheme: &str, hub: u8| HubListing {
        scheme: SchemeId::parse(scheme).unwrap(),
        membership: HubMembership {
            hub_id: [hub; 32],
            account: u32::from(hub),
            takes_peers: true,
        },
    };
    let hubs = vec![listing("hub-xrp", 1), listing("hub-usd", 2)];
    let mut node = with_feed(&own, &boot, None);
    node.set_own_hubs(START, hubs.clone());
    let closed = HubMembership {
        takes_peers: false,
        ..hubs[0].membership
    };
    let info = node.own_info().clone();
    node.set_own_info(
        START,
        OwnInfo {
            hub: Some(closed),
            ..info
        },
    );
    let mut changed = hubs;
    changed[0].membership = closed;
    assert_eq!(node.own_hubs(), changed);
    let state = own_link_state(node);
    let state = SignedNodeInfo::decode_exact(&state).unwrap();
    assert_eq!(state.info.hubs(), changed);
    assert_eq!(state.info.hub(), Some(closed));
}

// A member of several hubs lists each with its scheme, and names the first
// alone too, for nodes that read no list; a hub set in its own info other than
// the first of them replaces them, named alone, as before lists
// (spec/areas/peer-protocol.md#link-states).
#[test]
fn a_member_of_several_hubs_lists_them_in_its_link_state() {
    use dassie_peer::wire::{HubListing, HubMembership};
    let (own, boot) = (key(0), key(1));
    let listing = |scheme: &str, hub: u8| HubListing {
        scheme: SchemeId::parse(scheme).unwrap(),
        membership: HubMembership {
            hub_id: [hub; 32],
            account: u32::from(hub),
            takes_peers: true,
        },
    };
    let hubs = vec![listing("hub-xrp", 1), listing("hub-usd", 2)];
    let mut node = with_feed(&own, &boot, None);
    node.set_own_hubs(START, hubs.clone());
    assert_eq!(node.own_hubs(), hubs);
    assert_eq!(node.own_info().hub, Some(hubs[0].membership));
    // Setting the first again through the own info keeps the list.
    let info = node.own_info().clone();
    node.set_own_info(
        START,
        OwnInfo {
            alias: "a".into(),
            ..info
        },
    );
    assert_eq!(node.own_hubs(), hubs);
    let bytes = own_link_state(node);
    let state = SignedNodeInfo::decode_exact(&bytes).unwrap();
    assert!(state.verify().is_ok());
    assert_eq!(state.info.hubs(), hubs);
    assert_eq!(state.info.hub(), Some(hubs[0].membership));
    assert!(!state.info.has_methods());

    // Another hub through the own info: named alone, without a list.
    let mut node = with_feed(&own, &boot, None);
    node.set_own_hubs(START, hubs.clone());
    let other = listing("hub-xrp", 3).membership;
    let info = node.own_info().clone();
    node.set_own_info(
        START,
        OwnInfo {
            hub: Some(other),
            ..info
        },
    );
    assert_eq!(node.own_hubs(), []);
    let state = own_link_state(node);
    let state = SignedNodeInfo::decode_exact(&state).unwrap();
    assert_eq!(state.info.hubs(), []);
    assert_eq!(state.info.hub(), Some(other));

    // No hubs: neither extension.
    let mut node = with_feed(&own, &boot, None);
    node.set_own_hubs(START, hubs);
    node.set_own_hubs(START, Vec::new());
    assert_eq!(node.own_info().hub, None);
    let state = own_link_state(node);
    let state = SignedNodeInfo::decode_exact(&state).unwrap();
    assert_eq!((state.info.hubs(), state.info.hub()), (Vec::new(), None));
}

// spec/areas/peer-protocol.md#versions: settlement methods are not built
// yet, so a node refuses their messages with 400, as a node that does not
// know their tags does: not acted upon. Unauthenticated, they get 401.
#[test]
fn method_messages_are_refused_until_methods_are_built() {
    let mut network = Network::new(2, 2, 1, Config::DEV);
    settle(&mut network, 30_000);
    let stub = SchemeId::parse("stub").unwrap();
    let sender = key(0);
    let mut auth = Authenticator::new(Arc::clone(&sender), AuthConfig::DEFAULT, START - 1);
    let contents = [
        Content::MethodPacket {
            request_id: 1,
            scheme: stub,
            packet: &[12, 0x10, 0, 0, 0, 0, 0, 0, 0, 1],
            commitment: &[],
        },
        Content::MethodAdd {
            scheme: stub,
            data: &[],
            versions: vec![1],
        },
        Content::MethodDrop { scheme: stub },
    ];
    for content in &contents {
        let body = request_by(&mut auth, network.clock(1), 1, content);
        assert_eq!(network.inbound(1, &body).status, 400, "{content:?}");
        let bare = encode_envelope(&sender.node_id(), &Authentication::None, &content.to_vec());
        assert_eq!(network.inbound(1, &bare).status, 401, "{content:?}");
    }
}

// A feed is at the URL's host: a node without a URL, a periphery node,
// names no port, and port 0 is none.
#[test]
fn only_a_node_with_a_url_names_a_feed_port() {
    let info = |url: Option<String>, feed_port| {
        OwnInfo {
            url,
            alias: String::new(),
            quic: true,
            feed_port,
            hub: None,
        }
        .checked()
        .feed_port
    };
    assert_eq!(info(Some(url(0)), Some(7768)), Some(7768));
    assert_eq!(info(None, Some(7768)), None);
    assert_eq!(info(Some(url(0)), Some(0)), None);
}

/// A signed link state of `key` at `url(0)` with a feed extension of
/// `value`.
fn feed_link_state(key: &NodeKey, value: &[u8]) -> Vec<u8> {
    let url = url(0);
    let info = NodeInfo {
        node_id: key.node_id(),
        sequence: START,
        url: &url,
        alias: "",
        public_key: *key.public_key().as_bytes(),
        entries: vec![Entry::Extension {
            kind: dassie_peer::wire::EXTENSION_FEED,
            value,
        }],
    };
    sign_node_info(key, &info)
}

// The extension is a port, two octets, never 0. One that is not names no
// port, but still verifies and re-encodes to its bytes, so a node that
// reads it keeps and forwards it as it came, as one that does not know the
// kind does.
#[test]
fn the_feed_extension_round_trips_and_a_bad_one_names_no_port() {
    let key = key(3);
    for (value, port) in [
        (&[0x1e, 0x58][..], Some(7768)),
        (&[0x00, 0x00][..], None),
        (&[0x1e][..], None),
        (&[0x1e, 0x58, 0x00][..], None),
        (&[][..], None),
    ] {
        let bytes = feed_link_state(&key, value);
        let state = SignedNodeInfo::decode_exact(&bytes).unwrap();
        assert!(state.verify().is_ok(), "{value:?}");
        assert_eq!(state.info.feed_port(), port, "{value:?}");
        assert_eq!(state.bytes, &bytes[..]);
        assert_eq!(state.info.to_vec(), state.signed);
    }
}
