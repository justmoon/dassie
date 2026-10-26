//! The peer node's rules, one at a time, on small simulated networks
//! (spec/areas/network.md).

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
use dassie_peer::sim::{Arriving, Network, START, forger, key, url};
use dassie_peer::wire::{
    Content, Entry, NodeInfo, PeeringRequest, PeeringResponse, RefusalReason, SchemeId,
    SignedNodeInfo, TerminationReason, sign_node_info,
};
use dassie_peer::{Config, Event, OwnInfo, PeerNode, PeerStatus, RegisterError, Sources, Stored};

fn settle(network: &mut Network, ms: u64) {
    let until = network.now + ms;
    network.run_until(until);
}

/// A request from the node of `from`, whose key its connection proved.
fn request(from: &NodeKey, content: &Content<'_>) -> Arriving {
    Arriving::new(from, content)
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

// A peering request whose link state is not the sender's is refused.
// Mallory's connection proves Mallory's key, so the request is Mallory's,
// whose link state it carries.
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
    });
    let body = request(&mallory, &content);
    assert_eq!(network.inbound(0, &body).status, 400);
}

// A packet goes only between peers.
#[test]
fn packets_need_a_peering() {
    let mut network = Network::new(3, 2, 1, Config::DEV);
    settle(&mut network, 30_000);
    let stranger = Arc::new(NodeKey::from_bytes(&[78; 32]));
    let packet = Content::InterledgerPacket {
        request_id: 1,
        packet: &[1, 2, 3],
    };
    let body = request(&stranger, &packet);
    assert_eq!(network.inbound(0, &body).status, 409);
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
// 409, which says it was not acted upon, as it refuses registrations.
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
    for content in &contents {
        let body = request(&stranger, content);
        assert_eq!(network.inbound(1, &body).status, 409, "{content:?}");
    }
}

// spec/decisions/membership-economics.md: registrations lapse without renewal
// and leave the served list, so the node loses its majority place, and leaves
// the table of every node that is not its peer after the grace period.
// Peers keep it until liveness ends the peering.
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

// Our sequence never goes back across a restart, even when the
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
    let body = request(&subject, &content);
    assert_eq!(network.inbound(0, &body).status, 200);
    let held = network
        .node(0)
        .nodes()
        .into_iter()
        .find(|v| v.id == subject.node_id())
        .unwrap();
    assert!(held.sequence.unwrap() < network.clock(0) + 600_000);
}

// A peering request from a node not in the table is refused as unknown.
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
    });
    let body = request(&stranger, &content);
    let reply = network.inbound(0, &body);
    assert_eq!(reply.status, 200);
    assert_eq!(
        PeeringResponse::decode_exact(&reply.body).unwrap(),
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
// say; only with a bootstrap node, and once at a time.
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

/// A registration of `from` at `url` with bootstrap node 0.
fn registration(network: &Network, from: &NodeKey, url: &str) -> Arriving {
    let state = link_state(from, network.clock(0), url);
    let info = SignedNodeInfo::decode_exact(&state).unwrap();
    request(from, &Content::Registration(info))
}

// spec/areas/peer-protocol.md#authentication: a request is the request of
// the node whose key its connection proved, and of no other. A node's own
// key is refused; another node's link state in a registration is refused;
// and a copy of a peer's requests, sent by someone who saw them, is the
// copier's, which holds nothing with the node: its packets, settlements and
// requests to settle are refused as a stranger's, and its registration as
// not its own.
#[test]
fn a_request_is_the_proved_senders_alone() {
    let mut network = Network::new(9, 2, 1, Config::DEV);
    settle(&mut network, 30_000);
    let (own, peer) = (key(0), key(1));
    assert!(network.node(0).peering_of(&peer.node_id()).is_some());
    let read = request(&own, &Content::NodeListHashRequest);
    assert_eq!(network.inbound(0, &read).status, 400, "our own key");
    let renewal = registration(&network, &peer, &url(1));
    let copier = forger();
    let stub = SchemeId::parse("stub").unwrap();
    let copies = [
        Content::InterledgerPacket {
            request_id: 3,
            packet: &[12, 1, 0],
        },
        Content::InterledgerPacket {
            request_id: 3,
            packet: &[13, 1, 0],
        },
        Content::Settlement {
            scheme: stub,
            amount: 10,
            data: &[],
            sequence: 1,
        },
        Content::SettlementRequest { scheme: stub },
        Content::PeeringTermination(TerminationReason::Owner),
    ];
    for content in &copies {
        let copy = request(&copier, content);
        assert_eq!(network.inbound(0, &copy).status, 409, "{content:?}");
    }
    let copy = Arriving {
        from: copier.public_key(),
        body: renewal.body.clone(),
    };
    assert_eq!(network.inbound(0, &copy).status, 400, "not its own state");
    assert_eq!(network.inbound(0, &renewal).status, 200, "the peer's own");
    assert!(network.node(0).peering_of(&peer.node_id()).is_some());
    network.node(0).check_invariants();
}

/// A core node, with `boot` as its bootstrap node.
fn core_node(own: &Arc<NodeKey>, boot: &NodeKey) -> PeerNode {
    PeerNode::new(
        Config::DEV,
        Arc::clone(own),
        OwnInfo {
            url: Some(url(0)),
            alias: String::new(),
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

// A core node sends its link state to a bootstrap node known only from the
// configuration at once, at its configured URL: QUIC is the only transport,
// and needs no link state to dial a node whose ID it knows. Every node takes
// QUIC, so the link state says nothing of it.
#[test]
fn bootstrap_nodes_are_asked_at_once() {
    let (own, boot) = (key(0), key(1));
    let mut hooks = dassie_peer::sim::StubHooks::default();
    let mut node = core_node(&own, &boot);
    node.handle_timeout(START, &mut hooks);
    let before = sent(&mut node);
    // Our link state goes to the bootstrap node at once.
    let ours =
        before
            .iter()
            .find_map(|request| {
                let envelope = dassie_peer::wire::Envelope::decode(&request.body).ok()?;
                match Content::decode_exact(envelope.content).ok()? {
                    Content::LinkStateUpdate(state) => {
                        Some(state.info.entries.iter().any(|entry| {
                            matches!(entry, dassie_peer::wire::Entry::Extension { .. })
                        }))
                    }
                    _ => None,
                }
            })
            .unwrap();
    assert!(!ours, "no extension");
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

// A core node with a rate feed names its port in its link state; one
// without names none (spec/areas/rate-feed.md#finding-a-feed).
#[test]
fn a_core_node_names_its_feed_port_in_its_link_state() {
    let (own, boot) = (key(0), key(1));
    let bytes = own_link_state(with_feed(&own, &boot, Some(7768)));
    let state = SignedNodeInfo::decode_exact(&bytes).unwrap();
    assert!(state.verify().is_ok());
    assert_eq!(state.info.feed_port(), Some(7768));
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
            feed_port: Some(7768),
            hub: Some(membership),
        },
    );
    let bytes = own_link_state(node);
    let state = SignedNodeInfo::decode_exact(&bytes).unwrap();
    assert!(state.verify().is_ok());
    assert_eq!(state.info.hub(), Some(membership));
    assert_eq!(state.info.feed_port(), Some(7768));
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
// know their tags does: not acted upon.
#[test]
fn method_messages_are_refused_until_methods_are_built() {
    let mut network = Network::new(2, 2, 1, Config::DEV);
    settle(&mut network, 30_000);
    let stub = SchemeId::parse("stub").unwrap();
    let sender = key(0);
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
        },
        Content::MethodDrop { scheme: stub },
    ];
    for content in &contents {
        let body = request(&sender, content);
        assert_eq!(network.inbound(1, &body).status, 400, "{content:?}");
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
