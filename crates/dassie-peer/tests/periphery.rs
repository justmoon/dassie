//! Periphery nodes and their uplinks (spec/areas/network.md#periphery-nodes):
//! link states without a URL, peering without a probe, downlinks under a
//! bound of their own, and a periphery node that floods nothing and takes
//! no place in anyone's routes.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use std::sync::Arc;

use dassie_identity::{NodeId, NodeKey};
use dassie_node_core::testing::SeededRng;
use dassie_oer::{Decode, Encode};
use dassie_peer::auth::{AuthConfig, Authenticator};
use dassie_peer::sim::{Network, START, key};
use dassie_peer::wire::{
    Authentication, Content, EXTENSION_PERIPHERY, EXTENSION_QUIC, Entry, InvalidNodeInfo,
    LinkStateResponse, NodeInfo, PeeringRequest, PeeringResponse, RefusalReason, ResponseEnvelope,
    SchemeId, SignedNodeInfo, encode_envelope, sign_node_info,
};
use dassie_peer::{Config, Event, Sources};

fn settle(network: &mut Network, ms: u64) {
    let until = network.now + ms;
    network.run_until(until);
}

fn stub() -> SchemeId {
    SchemeId::parse("stub").unwrap()
}

/// A request from `from` to node `to` of `network`, at `to`'s clock.
fn request(network: &Network, from: &Arc<NodeKey>, to: usize, content: &Content<'_>) -> Vec<u8> {
    let mut auth = Authenticator::new(Arc::clone(from), AuthConfig::DEFAULT, START - 1);
    let bytes = content.to_vec();
    let target = key(to);
    let (hmac, _) = auth
        .authenticate(
            network.clock(to),
            &mut SeededRng::new(9),
            &target.node_id(),
            &target.public_key(),
            &bytes,
        )
        .unwrap();
    encode_envelope(&from.node_id(), &Authentication::Hmac(hmac), &bytes)
}

/// A link state of `key` at `url`, with `extensions`.
fn link_state(key: &NodeKey, sequence: u64, url: &str, extensions: &[u16]) -> Vec<u8> {
    let entries = core::iter::once(Entry::Scheme(stub()))
        .chain(extensions.iter().map(|kind| Entry::Extension {
            kind: *kind,
            value: &[],
        }))
        .collect();
    let info = NodeInfo {
        node_id: key.node_id(),
        sequence,
        url,
        alias: "",
        public_key: *key.public_key().as_bytes(),
        entries,
    };
    sign_node_info(key, &info)
}

/// A peering request from `from` to node `to`, with `state` as its link
/// state, and the answer.
fn ask(network: &mut Network, from: &Arc<NodeKey>, to: usize, state: &[u8]) -> dassie_peer::Reply {
    let content = Content::PeeringRequest(PeeringRequest {
        scheme: stub(),
        node_info: SignedNodeInfo::decode_exact(state).unwrap(),
        data: &[],
        versions: vec![1],
        credit: 0,
    });
    let body = request(network, from, to, &content);
    network.inbound(to, &body)
}

fn answer(reply: &dassie_peer::Reply) -> PeeringResponse<'_> {
    let envelope = ResponseEnvelope::decode(&reply.body).unwrap();
    PeeringResponse::decode_exact(envelope.body).unwrap()
}

fn id(index: usize) -> NodeId {
    key(index).node_id()
}

// spec/areas/peer-protocol.md#verification: an empty URL verifies only with
// the periphery extension; with it, the state is a periphery node's.
#[test]
fn an_empty_url_verifies_only_in_a_periphery_link_state() {
    let key = NodeKey::from_bytes(&[90; 32]);
    let periphery = link_state(&key, START, "", &[EXTENSION_PERIPHERY]);
    let signed = SignedNodeInfo::decode_exact(&periphery).unwrap();
    assert!(signed.verify().is_ok());
    assert!(signed.info.is_periphery() && !signed.info.speaks_quic());
    let bare = link_state(&key, START, "", &[EXTENSION_QUIC]);
    assert_eq!(
        SignedNodeInfo::decode_exact(&bare).unwrap().verify(),
        Err(InvalidNodeInfo::Url)
    );
    let core = link_state(&key, START, "https://a.example", &[EXTENSION_QUIC]);
    let signed = SignedNodeInfo::decode_exact(&core).unwrap();
    assert!(signed.verify().is_ok() && !signed.info.is_periphery());
    // Found in review: one with a URL too would be taken for a downlink's,
    // accepted without a probe, and dialled at a URL nobody checked.
    let both = link_state(&key, START, "https://a.example", &[EXTENSION_PERIPHERY]);
    assert_eq!(
        SignedNodeInfo::decode_exact(&both).unwrap().verify(),
        Err(InvalidNodeInfo::Url)
    );
}

// Found in review: a peering request whose link state has a URL and the
// periphery extension took a core channel without a probe, outside the
// peering caps; channels are never closed, so a few hundred fresh keys
// would have kept the node from ever peering with a new core node. It is
// refused, and nothing is opened for it.
#[test]
fn a_downlink_with_a_url_is_refused_without_a_channel() {
    let mut network = Network::with_periphery(17, 2, 1, 0, Config::DEV);
    settle(&mut network, 30_000);
    let channels = network.node(0).channels().count();
    let stranger = Arc::new(NodeKey::from_bytes(&[97; 32]));
    let state = link_state(
        &stranger,
        network.clock(0),
        "https://stranger.test",
        &[EXTENSION_PERIPHERY, EXTENSION_QUIC],
    );
    let reply = ask(&mut network, &stranger, 0, &state);
    assert_eq!(reply.status, 400);
    let node = network.node(0);
    assert_eq!(node.channels().count(), channels);
    assert!(node.peering_of(&stranger.node_id()).is_none());
    assert!(!node.nodes().iter().any(|v| v.id == stranger.node_id()));
}

// Found in review: a downlink the scheme refused kept its place in the
// table, under the downlinks' bound, until a restart. It leaves at once.
#[test]
fn a_refused_downlink_takes_no_place() {
    let config = Config {
        max_downlinks: 1,
        ..Config::DEV
    };
    let mut network = Network::with_periphery(18, 2, 1, 0, config);
    settle(&mut network, 30_000);
    let at = network.clock(0);
    let first = Arc::new(NodeKey::from_bytes(&[98; 32]));
    network.nodes[0].hooks.refuse = Some(RefusalReason::Capacity);
    let state = link_state(&first, at, "", &[EXTENSION_PERIPHERY]);
    let reply = ask(&mut network, &first, 0, &state);
    assert_eq!(
        answer(&reply),
        PeeringResponse::Refused(RefusalReason::Capacity)
    );
    assert!(!network.node(0).is_downlink(&first.node_id()));
    network.nodes[0].hooks.refuse = None;
    let second = Arc::new(NodeKey::from_bytes(&[99; 32]));
    let state = link_state(&second, at, "", &[EXTENSION_PERIPHERY]);
    let reply = ask(&mut network, &second, 0, &state);
    assert!(matches!(answer(&reply), PeeringResponse::Accepted { .. }));
}

/// Whether node `at` is ending its peering with `node`, or has none.
fn ended_or_ending(network: &Network, at: usize, node: &NodeId) -> bool {
    let views = network.node(at).nodes();
    views
        .iter()
        .find(|view| view.id == *node)
        .is_none_or(|view| {
            view.termination.is_some()
                || view.peering.as_ref().is_none_or(|p| p.depeering.is_some())
        })
}

// Found in review: a core node with downlinks that lost its URL, as one
// whose hostname became too long for it, asserted it had none, at every
// sweep and after every restart. It ends them instead; its core peers end
// their peerings with it, made with a core node, and it asks for uplinks
// as a periphery node, across a restart too.
#[test]
fn a_core_node_that_becomes_periphery_ends_its_downlinks() {
    let (core, periphery) = (4, 2);
    let mut network = Network::with_periphery(19, core, 1, periphery, Config::DEV);
    settle(&mut network, 90_000);
    let turning = (1..core)
        .find(|index| (core..core + periphery).any(|p| network.node(*index).is_downlink(&id(p))))
        .expect("a core node with a downlink");
    let downlinks: Vec<NodeId> = (core..core + periphery)
        .map(id)
        .filter(|node| network.node(turning).is_downlink(node))
        .collect();
    let core_peers: Vec<usize> = (0..core)
        .filter(|index| network.node(turning).peering_of(&id(*index)).is_some())
        .collect();
    network.periphery.insert(turning);
    network.act(turning, |node, now| {
        node.set_own_info(
            now,
            dassie_peer::OwnInfo {
                url: None,
                alias: String::new(),
                quic: true,
                feed_port: None,
                hub: None,
            },
        );
    });
    assert!(network.node(turning).is_periphery());
    for downlink in &downlinks {
        assert!(ended_or_ending(&network, turning, downlink));
    }
    // Restarted as a periphery node, with its downlinks still ending.
    network.crash(turning);
    network.start(turning);
    settle(&mut network, 10 * 60_000);
    let node = network.node(turning);
    for downlink in &downlinks {
        assert!(node.peering_of(downlink).is_none(), "the downlink ended");
    }
    for index in &core_peers {
        let at = network.node(*index);
        assert!(
            at.peering_of(&id(turning)).is_none() || at.is_downlink(&id(turning)),
            "node {index} keeps no core peering with it"
        );
    }
    assert!(
        (0..core).any(|index| network.node(index).is_downlink(&id(turning))),
        "it peers again, as a downlink"
    );
}

// spec/areas/network.md#periphery-nodes: periphery nodes find two uplinks
// among the core nodes, which take them as downlinks; they register
// nowhere, take no place in node lists or routes, and are never asked to
// peer. Their own routes go through their uplinks.
#[test]
fn periphery_nodes_peer_with_uplinks_and_stay_out_of_the_core() {
    let (core, periphery) = (4, 2);
    let mut network = Network::with_periphery(11, core, 1, periphery, Config::DEV);
    settle(&mut network, 90_000);
    for index in core..core + periphery {
        let node = network.node(index);
        assert!(node.is_periphery());
        let uplinks: Vec<NodeId> = node.peers().map(|(id, _)| *id).collect();
        assert_eq!(uplinks.len(), 2, "node {index} has two uplinks");
        for uplink in &uplinks {
            let at = (0..core).find(|i| id(*i) == *uplink).unwrap();
            assert!(
                network.node(at).is_downlink(&id(index)),
                "a downlink of {at}"
            );
            // Its uplinks never list it, nor route to it.
            assert!(!network.node(at).routes().contains_key(&id(index)));
            assert!(!network.node(at).is_advertised(&id(index)));
        }
        // It reaches every core node through them.
        for other in 0..core {
            let route = &node.routes()[&id(other)];
            assert!(route.first_hops.iter().all(|hop| uplinks.contains(hop)));
        }
        assert!(node.registration_status()[0].registered_ms.is_none());
    }
    for index in 0..core {
        let node = network.node(index);
        for downlink in core..core + periphery {
            assert!(!node.majority().contains(&id(downlink)));
            assert!(!node.routes().contains_key(&id(downlink)));
            // Only its uplinks hold it at all.
            let held = node.nodes().into_iter().find(|v| v.id == id(downlink));
            assert!(held.is_none_or(|view| view.sources == Sources::DOWNLINK));
        }
        // Core peers still meet their target among themselves.
        assert!(node.core_peers().count() >= 2, "node {index}");
    }
    // Every peering of a periphery node was its own request.
    for (index, event) in &network.events {
        if let Event::Peered { node, inbound, .. } = event {
            let periphery_side = *index >= core;
            let with_periphery = (core..core + periphery).any(|i| id(i) == *node);
            assert!(
                !(periphery_side && *inbound),
                "a periphery node is never asked"
            );
            assert!(!with_periphery || *inbound, "a core node never asks one");
        }
    }
}

/// The nodes `index` peers with, sorted.
fn peers_of(network: &Network, index: usize) -> Vec<NodeId> {
    let mut peers: Vec<NodeId> = network.node(index).peers().map(|(id, _)| *id).collect();
    peers.sort_unstable();
    peers
}

// spec/areas/network.md#pinned-uplinks: a periphery node pinned to its
// uplinks asks only them, as many as it names, across a restart too; it
// never asks the others, which it would have asked otherwise. A core node
// does not use the list.
#[test]
fn a_pinned_periphery_node_asks_only_its_uplinks() {
    let (core, periphery) = (4, 2);
    let mut network = Network::with_periphery(23, core, 1, periphery, Config::DEV);
    network.pin_uplinks(core, vec![id(2)]);
    network.pin_uplinks(core + 1, vec![id(3), id(1)]);
    network.pin_uplinks(1, vec![id(0)]);
    assert_eq!(network.node(core).pinned_uplinks(), Some(&[id(2)][..]));
    assert_eq!(network.node(1).pinned_uplinks(), None);
    settle(&mut network, 90_000);
    assert_eq!(peers_of(&network, core), vec![id(2)]);
    let mut both = vec![id(1), id(3)];
    both.sort_unstable();
    assert_eq!(peers_of(&network, core + 1), both);
    // The core node pinned to one node still meets the usual target.
    assert!(network.node(1).core_peers().count() >= 2);
    network.crash(core);
    network.start(core);
    settle(&mut network, 90_000);
    assert_eq!(peers_of(&network, core), vec![id(2)]);
    // No other node was ever asked by them: only their pinned uplinks
    // took them as downlinks.
    for index in 0..core {
        let node = network.node(index);
        assert_eq!(node.is_downlink(&id(core)), index == 2, "node {index}");
        assert_eq!(
            node.is_downlink(&id(core + 1)),
            index == 1 || index == 3,
            "node {index}"
        );
    }
    for (index, event) in &network.events {
        if let Event::Peered { node, .. } = event
            && *index >= core
        {
            assert!(
                network.pinned[index].contains(node),
                "{index} peered with {node}"
            );
        }
    }
}

// spec/areas/network.md#pinned-uplinks: a node pinned after it found
// uplinks of its own keeps them, but they take none of the places for
// peerings it starts, so it still reaches every node it is pinned to.
#[test]
fn earlier_uplinks_leave_room_for_the_pinned_ones() {
    let (core, periphery) = (8, 1);
    let mut network = Network::with_periphery(25, core, 1, periphery, Config::DEV);
    settle(&mut network, 90_000);
    let before = peers_of(&network, core);
    assert!(before.len() >= Config::DEV.peer_target, "{before:?}");
    let mut pinned: Vec<NodeId> = (0..core)
        .map(id)
        .filter(|node| !before.contains(node))
        .take(dassie_peer::MAX_PINNED_UPLINKS)
        .collect();
    assert_eq!(pinned.len(), dassie_peer::MAX_PINNED_UPLINKS);
    network.pin_uplinks(core, pinned.clone());
    settle(&mut network, 120_000);
    let after = peers_of(&network, core);
    pinned.sort_unstable();
    for node in before.iter().chain(&pinned) {
        assert!(after.contains(node), "{node} not in {after:?}");
    }
}

// spec/areas/network.md#pinned-uplinks: a pinned uplink that is not there
// is waited for; the node asks no other meanwhile.
#[test]
fn a_periphery_node_pinned_to_an_unknown_node_asks_nobody() {
    let (core, periphery) = (3, 1);
    let mut network = Network::with_periphery(24, core, 1, periphery, Config::DEV);
    let nobody = NodeKey::from_bytes(&[95; 32]).node_id();
    network.pin_uplinks(core, vec![nobody]);
    settle(&mut network, 90_000);
    assert!(peers_of(&network, core).is_empty());
    assert!((0..core).all(|index| !network.node(index).is_downlink(&id(core))));
}

// spec/areas/network.md#periphery-nodes: an uplink serves its downlink's
// link state to nobody, and floods it nowhere.
#[test]
fn a_downlinks_link_state_goes_nowhere() {
    let (core, periphery) = (3, 1);
    let mut network = Network::with_periphery(12, core, 1, periphery, Config::DEV);
    settle(&mut network, 90_000);
    let downlink = id(core);
    let uplink = (0..core)
        .find(|index| network.node(*index).is_downlink(&downlink))
        .unwrap();
    let asker = key(core + 1);
    let content = Content::LinkStateRequest(vec![downlink]);
    let body = request(&network, &asker, uplink, &content);
    let reply = network.inbound(uplink, &body);
    let envelope = ResponseEnvelope::decode(&reply.body).unwrap();
    let states = LinkStateResponse::decode_exact(envelope.body).unwrap();
    assert!(states.0.iter().all(Option::is_none), "not served");
    // No core node but its uplinks ever held it.
    for index in 0..core {
        let node = network.node(index);
        let held = node.nodes().iter().any(|view| view.id == downlink);
        assert_eq!(held, node.peering_of(&downlink).is_some(), "node {index}");
    }
}

// spec/areas/network.md#asking-and-answering: a core node that takes QUIC
// accepts a periphery node at once, without a probe; one in no list with
// a URL is still unknown; one that does not take QUIC takes no downlinks.
#[test]
fn downlinks_are_accepted_without_a_probe() {
    let mut network = Network::with_periphery(13, 2, 1, 0, Config::DEV);
    settle(&mut network, 30_000);
    let stranger = Arc::new(NodeKey::from_bytes(&[91; 32]));
    let at = network.clock(0);
    let periphery = link_state(&stranger, at, "", &[EXTENSION_PERIPHERY]);
    let reply = ask(&mut network, &stranger, 0, &periphery);
    assert_eq!(reply.status, 200, "answered at once, not deferred");
    assert!(matches!(answer(&reply), PeeringResponse::Accepted { .. }));
    assert!(network.node(0).is_downlink(&stranger.node_id()));
    assert_eq!(
        network.node(0).core_peers().count(),
        1,
        "no core slot taken"
    );
    // Its URL would be probed; it is in no list, so it is refused first.
    let other = Arc::new(NodeKey::from_bytes(&[92; 32]));
    let core = link_state(&other, at, "https://other.test", &[EXTENSION_QUIC]);
    let reply = ask(&mut network, &other, 0, &core);
    assert_eq!(
        answer(&reply),
        PeeringResponse::Refused(RefusalReason::UnknownNode)
    );
    // Without QUIC, a core node cannot reach a downlink, and takes none.
    let mut plain = Network::new(14, 2, 1, Config::DEV);
    settle(&mut plain, 30_000);
    let reply = ask(&mut plain, &stranger, 0, &periphery);
    assert_eq!(
        answer(&reply),
        PeeringResponse::Refused(RefusalReason::Unspecified)
    );
    assert!(!plain.node(0).is_downlink(&stranger.node_id()));
}

// spec/areas/network.md#downlinks: downlinks have a bound of their own,
// apart from the core peering slots; past it, a new one is refused for
// capacity.
#[test]
fn downlinks_are_bounded() {
    let config = Config {
        max_downlinks: 2,
        ..Config::DEV
    };
    let mut network = Network::with_periphery(15, 2, 1, 0, config);
    settle(&mut network, 30_000);
    let at = network.clock(0);
    for seed in [93, 94, 95] {
        let key = Arc::new(NodeKey::from_bytes(&[seed; 32]));
        let state = link_state(&key, at, "", &[EXTENSION_PERIPHERY]);
        let reply = ask(&mut network, &key, 0, &state);
        let accepted = matches!(answer(&reply), PeeringResponse::Accepted { .. });
        assert_eq!(accepted, seed != 95, "seed {seed}");
        if seed == 95 {
            assert_eq!(
                answer(&reply),
                PeeringResponse::Refused(RefusalReason::Capacity)
            );
        }
    }
    let node = network.node(0);
    assert_eq!(node.peers().count() - node.core_peers().count(), 2);
    assert_eq!(node.core_peers().count(), 1, "the core peer kept its slot");
}

// A periphery node takes no peers: it peers only as a downlink.
#[test]
fn a_periphery_node_takes_no_peers() {
    let (core, periphery) = (2, 1);
    let mut network = Network::with_periphery(16, core, 1, periphery, Config::DEV);
    settle(&mut network, 30_000);
    let stranger = Arc::new(NodeKey::from_bytes(&[96; 32]));
    let state = link_state(&stranger, network.clock(core), "", &[EXTENSION_PERIPHERY]);
    let reply = ask(&mut network, &stranger, core, &state);
    assert_eq!(
        answer(&reply),
        PeeringResponse::Refused(RefusalReason::Unspecified)
    );
    // Nor does a bootstrap node take its registration.
    let own = network.node(core).nodes();
    let bytes = own.iter().find(|view| view.id == id(core)).unwrap();
    assert!(
        bytes.url.as_deref() == Some(""),
        "its link state has no URL"
    );
    let reply = {
        let state = link_state(&key(core), network.clock(0), "", &[EXTENSION_PERIPHERY]);
        let content = Content::Registration(SignedNodeInfo::decode_exact(&state).unwrap());
        let body = request(&network, &key(core), 0, &content);
        network.inbound(0, &body)
    };
    assert_eq!(reply.status, 409);
}

/// A network whose core nodes' tables are full: four core nodes, with room
/// for three, and `periphery` periphery nodes after them.
fn full_tables(seed: u64, periphery: usize) -> Network {
    let config = Config {
        max_nodes: 3,
        ..Config::DEV
    };
    let mut network = Network::with_periphery(seed, 4, 1, periphery, config);
    settle(&mut network, 90_000);
    network
}

/// The entries of node `at` that take a core place: all but downlinks.
fn core_entries(network: &Network, at: usize) -> usize {
    let nodes = network.node(at).nodes();
    nodes
        .iter()
        .filter(|view| !view.sources.contains(Sources::DOWNLINK))
        .count()
}

// Found in review: a downlink takes no core place, but once its downlink
// source was dropped it stayed as an orphan, taking one. With the core
// table full, that broke the table's bound, and the node panicked. A
// downlink refused after it entered the table leaves it at once.
#[test]
fn a_refused_downlink_leaves_a_full_table() {
    let mut network = full_tables(20, 0);
    assert_eq!(core_entries(&network, 0), 3, "the core table is full");
    network.nodes[0].hooks.refuse = Some(RefusalReason::Capacity);
    let stranger = Arc::new(NodeKey::from_bytes(&[100; 32]));
    let state = link_state(&stranger, network.clock(0), "", &[EXTENSION_PERIPHERY]);
    let reply = ask(&mut network, &stranger, 0, &state);
    assert_eq!(
        answer(&reply),
        PeeringResponse::Refused(RefusalReason::Capacity)
    );
    assert_eq!(core_entries(&network, 0), 3);
    let nodes = network.node(0).nodes();
    assert!(!nodes.iter().any(|view| view.id == stranger.node_id()));
    settle(&mut network, 60_000);
}

/// The core node among the first four that is `periphery`'s uplink.
fn uplink_of(network: &Network, periphery: usize) -> usize {
    (0..4)
        .find(|index| network.node(*index).is_downlink(&id(periphery)))
        .expect("an uplink")
}

/// Whether node `at` ended its peering with `node`.
fn ended(network: &Network, at: usize, node: &NodeId) -> bool {
    network.events.iter().any(|(index, event)| {
        *index == at && matches!(event, Event::PeeringEnded { peer } if peer == node)
    })
}

// The same for a downlink whose peering ends: its owner ended it here.
#[test]
fn an_ended_downlink_leaves_a_full_table() {
    let mut network = full_tables(21, 1);
    let uplink = uplink_of(&network, 4);
    assert_eq!(core_entries(&network, uplink), 3, "the core table is full");
    network.act(uplink, |node, now| {
        node.terminate(now, &id(4), dassie_peer::wire::TerminationReason::Owner);
    });
    settle(&mut network, 10 * 60_000);
    assert!(ended(&network, uplink, &id(4)), "the peering ended");
    assert!(core_entries(&network, uplink) <= 3);
}

// The same for the downlinks a core node ends when it becomes periphery.
#[test]
fn downlinks_ended_by_a_node_turning_periphery_leave_a_full_table() {
    let mut network = full_tables(22, 1);
    let turning = uplink_of(&network, 4);
    assert_eq!(core_entries(&network, turning), 3, "the core table is full");
    network.periphery.insert(turning);
    network.act(turning, |node, now| {
        node.set_own_info(
            now,
            dassie_peer::OwnInfo {
                url: None,
                alias: String::new(),
                quic: true,
                feed_port: None,
                hub: None,
            },
        );
    });
    settle(&mut network, 10 * 60_000);
    assert!(ended(&network, turning, &id(4)), "the downlink ended");
    assert!(core_entries(&network, turning) <= 3);
}
