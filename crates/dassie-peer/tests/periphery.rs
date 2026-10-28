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

use dassie_identity::{IdentityKey, NodeId};
use dassie_oer::Decode;
use dassie_peer::sim::{Arriving, Network, START, key};
use dassie_peer::wire::{
    Content, EXTENSION_PERIPHERY, Entry, InvalidNodeInfo, LinkStateResponse, NodeInfo,
    PeeringRequest, PeeringResponse, RefusalReason, SchemeId, SettlementOffer, SignedNodeInfo,
    sign_node_info,
};
use dassie_peer::{Config, Event, Sources};

fn settle(network: &mut Network, ms: u64) {
    let until = network.now + ms;
    network.run_until(until);
}

fn stub() -> SchemeId {
    SchemeId::parse("stub").unwrap()
}

/// A request from `from` to node `to` of `network`, over a connection that
/// proves `from`'s key.
fn request(_: &Network, from: &Arc<IdentityKey>, _: usize, content: &Content<'_>) -> Arriving {
    Arriving::new(from, content)
}

/// A link state of `key` at `url`, with `extensions`.
fn link_state(key: &IdentityKey, sequence: u64, url: &str, extensions: &[u16]) -> Vec<u8> {
    let entries = extensions
        .iter()
        .map(|kind| Entry::Extension {
            kind: *kind,
            value: &[],
        })
        .collect();
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
        entries,
    };
    sign_node_info(key, &info)
}

/// A peering request from `from` to node `to`, with `state` as its link
/// state, and the answer.
fn ask(
    network: &mut Network,
    from: &Arc<IdentityKey>,
    to: usize,
    state: &[u8],
) -> dassie_peer::Reply {
    let content = Content::PeeringRequest(PeeringRequest {
        scheme: stub(),
        node_info: SignedNodeInfo::decode_exact(state).unwrap(),
        data: &[],
    });
    let body = request(network, from, to, &content);
    network.inbound(to, &body)
}

fn answer(reply: &dassie_peer::Reply) -> PeeringResponse {
    PeeringResponse::decode_exact(&reply.body).unwrap()
}

fn id(index: usize) -> NodeId {
    key(index).node_id()
}

/// A key of its own for each `seed`.
fn downlink_key(seed: u8) -> Arc<IdentityKey> {
    Arc::new(dassie_identity::TableKeygen::identity_key(&[seed; 32]))
}

/// Asks node 0 to take `key` as its downlink, now: why it refused, if it
/// did.
fn ask_as_downlink(network: &mut Network, key: &Arc<IdentityKey>) -> Option<RefusalReason> {
    let state = link_state(key, network.clock(0), "", &[EXTENSION_PERIPHERY]);
    let reply = ask(network, key, 0, &state);
    match answer(&reply) {
        PeeringResponse::Accepted => None,
        PeeringResponse::Refused(reason) => Some(reason),
    }
}

/// Node 0's books take a settlement of at least one largest packet from
/// `key`.
fn pays(network: &mut Network, key: &IdentityKey) {
    let node = key.node_id();
    network.act_with_hooks(0, |peer, hooks, now| peer.paid_by(now, hooks, &node));
}

// spec/areas/peer-protocol.md#verification: an empty URL verifies only with
// the periphery extension; with it, the state is a periphery node's.
#[test]
fn an_empty_url_verifies_only_in_a_periphery_link_state() {
    let key = dassie_identity::TableKeygen::identity_key(&[90; 32]);
    let periphery = link_state(&key, START, "", &[EXTENSION_PERIPHERY]);
    let signed = SignedNodeInfo::decode_exact(&periphery).unwrap();
    assert!(signed.verify().is_ok());
    assert!(signed.info.is_periphery());
    let bare = link_state(&key, START, "", &[]);
    assert_eq!(
        SignedNodeInfo::decode_exact(&bare).unwrap().verify(),
        Err(InvalidNodeInfo::Url)
    );
    let core = link_state(&key, START, "https://a.example", &[]);
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
// peering caps; a channel closes only a day after its peering ended, so a
// few hundred fresh keys would have kept the node from peering with a new
// core node. It is refused, and nothing is opened for it.
#[test]
fn a_downlink_with_a_url_is_refused_without_a_channel() {
    let mut network = Network::with_periphery(17, 2, 1, 0, Config::DEV);
    settle(&mut network, 30_000);
    let channels = network.node(0).channels().count();
    let stranger = Arc::new(dassie_identity::TableKeygen::identity_key(&[97; 32]));
    let state = link_state(
        &stranger,
        network.clock(0),
        "https://stranger.test",
        &[EXTENSION_PERIPHERY],
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
        max_downlinks: 2,
        ..Config::DEV
    };
    let mut network = Network::with_periphery(18, 2, 1, 0, config);
    settle(&mut network, 30_000);
    let at = network.clock(0);
    let first = Arc::new(dassie_identity::TableKeygen::identity_key(&[98; 32]));
    network.nodes[0].hooks.refuse = Some(RefusalReason::Capacity);
    let state = link_state(&first, at, "", &[EXTENSION_PERIPHERY]);
    let reply = ask(&mut network, &first, 0, &state);
    assert_eq!(
        answer(&reply),
        PeeringResponse::Refused(RefusalReason::Capacity)
    );
    assert!(!network.node(0).is_downlink(&first.node_id()));
    network.nodes[0].hooks.refuse = None;
    let second = Arc::new(dassie_identity::TableKeygen::identity_key(&[99; 32]));
    let state = link_state(&second, at, "", &[EXTENSION_PERIPHERY]);
    let reply = ask(&mut network, &second, 0, &state);
    assert!(matches!(answer(&reply), PeeringResponse::Accepted));
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
                feed_port: None,
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
// peerings it starts, so it still reaches every node it is pinned to,
// more than the peerings it would start unpinned.
#[test]
fn earlier_uplinks_leave_room_for_the_pinned_ones() {
    let (core, periphery) = (11, 1);
    let mut network = Network::with_periphery(25, core, 1, periphery, Config::DEV);
    settle(&mut network, 90_000);
    let before = peers_of(&network, core);
    assert!(before.len() >= Config::DEV.peer_target, "{before:?}");
    let count = Config::DEV.max_outbound() + 1;
    let mut pinned: Vec<NodeId> = (0..core)
        .map(id)
        .filter(|node| !before.contains(node))
        .take(count)
        .collect();
    assert_eq!(pinned.len(), count);
    network.pin_uplinks(core, pinned.clone());
    settle(&mut network, 120_000);
    let after = peers_of(&network, core);
    pinned.sort_unstable();
    for node in before.iter().chain(&pinned) {
        assert!(after.contains(node), "{node} not in {after:?}");
    }
}

// spec/areas/network.md#pinned-uplinks, spec/decisions/periphery-lists-no-neighbors.md:
// a periphery node pinned to more uplinks than it would start peerings
// with, and than a link state could list, reaches every one of them. Its
// link state lists no neighbors, so it stays what every node decodes, and
// each uplink holds its newest.
#[test]
fn a_periphery_node_reaches_more_uplinks_than_a_link_state_lists() {
    let core = dassie_peer::wire::MAX_ENTRIES + 6;
    // At production timings, the bootstrap node takes and probes every
    // registration at once, and core nodes keep a peer or two among
    // themselves: neither is what this is about, and a network this size
    // costs less to simulate so.
    let config = Config {
        registration_rate: dassie_peer::Rate {
            burst: 256,
            refill_ms: 1_000,
        },
        probe_rate: dassie_peer::Rate {
            burst: 256,
            refill_ms: 1_000,
        },
        peer_target: 1,
        max_peers: 4,
        ..Config::DEFAULT
    };
    let mut network = Network::with_periphery(26, core, 1, 1, config);
    let pinned: Vec<NodeId> = (0..core).map(id).collect();
    assert!(pinned.len() > config.max_outbound() && pinned.len() <= dassie_peer::MAX_UPLINKS);
    network.pin_uplinks(core, pinned.clone());
    // One asked a second, once each is known.
    settle(&mut network, 4 * 60_000);
    let mut sorted = pinned.clone();
    sorted.sort_unstable();
    assert_eq!(peers_of(&network, core), sorted);
    // Its link state as it signed it, which it serves itself: what every
    // node decodes, and verifies.
    let asker = Arc::new(dassie_identity::TableKeygen::identity_key(&[101; 32]));
    let content = Content::LinkStateRequest(vec![id(core)]);
    let body = request(&network, &asker, core, &content);
    let reply = network.inbound(core, &body);
    let states = LinkStateResponse::decode_exact(&reply.body).unwrap();
    let signed = states.0[0].clone().expect("its own link state");
    let node = network.node(core);
    assert!(
        pinned.iter().all(|uplink| node.is_advertised(uplink)),
        "every uplink confirmed"
    );
    let own = node.nodes().into_iter().find(|v| v.id == id(core)).unwrap();
    assert!(own.neighbors.is_empty(), "{:?}", own.neighbors);
    assert!(signed.verify().is_ok() && signed.info.is_periphery());
    assert_eq!(signed.info.neighbors().count(), 0);
    for index in 0..core {
        let uplink = network.node(index);
        assert!(uplink.is_downlink(&id(core)), "a downlink of {index}");
        let held = uplink
            .nodes()
            .into_iter()
            .find(|v| v.id == id(core))
            .unwrap();
        assert_eq!(held.sequence, own.sequence, "node {index} holds its newest");
        assert!(held.neighbors.is_empty());
        // It still reaches no route there.
        assert!(!uplink.routes().contains_key(&id(core)));
    }
    // Its routes go through its uplinks, all of them.
    let route = &node.routes()[&id(0)];
    assert!(route.first_hops.iter().all(|hop| pinned.contains(hop)));
}

// spec/areas/network.md#one-sided-peerings: an uplink that ended its
// peering with a periphery node, whose termination never arrived, tells it
// so when its next heartbeat comes: a periphery node's own link state lists
// nobody, but goes only to its uplinks, so it presumes a peering.
#[test]
fn an_uplink_tells_a_periphery_node_it_is_not_peered() {
    let (core, periphery) = (3, 1);
    let mut network = Network::with_periphery(27, core, 1, periphery, Config::DEV);
    settle(&mut network, 90_000);
    let downlink = id(core);
    let uplink = uplink_among(&network, core, core);
    network.cut.insert((uplink, core));
    network.act(uplink, |node, now| {
        assert!(node.terminate(now, &downlink, dassie_peer::wire::TerminationReason::Owner));
    });
    let config = Config::DEV;
    settle(
        &mut network,
        config.depeer_wait_ms + config.termination_timeout_ms + 5_000,
    );
    assert!(ended(&network, uplink, &downlink), "the uplink ended it");
    assert!(network.node(core).peering_of(&id(uplink)).is_some());
    assert!(
        network
            .node(uplink)
            .nodes()
            .iter()
            .any(|v| v.id == downlink),
        "the uplink still knows it"
    );
    network.cut.clear();
    // A heartbeat comes within its longest interval.
    settle(
        &mut network,
        config.heartbeat_min_ms + config.heartbeat_jitter_ms + 5_000,
    );
    assert!(
        ended(&network, core, &id(uplink)),
        "the periphery node ended it"
    );
    assert!(network.node(core).peering_of(&id(uplink)).is_none());
}

/// The core node among the first `core` that is `periphery`'s uplink.
fn uplink_among(network: &Network, core: usize, periphery: usize) -> usize {
    (0..core)
        .find(|index| network.node(*index).is_downlink(&id(periphery)))
        .expect("an uplink")
}

// spec/areas/network.md#pinned-uplinks: a pinned uplink that is not there
// is waited for; the node asks no other meanwhile.
#[test]
fn a_periphery_node_pinned_to_an_unknown_node_asks_nobody() {
    let (core, periphery) = (3, 1);
    let mut network = Network::with_periphery(24, core, 1, periphery, Config::DEV);
    let nobody = dassie_identity::TableKeygen::identity_key(&[95; 32]).node_id();
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
    let states = LinkStateResponse::decode_exact(&reply.body).unwrap();
    assert!(states.0.iter().all(Option::is_none), "not served");
    // No core node but its uplinks ever held it.
    for index in 0..core {
        let node = network.node(index);
        let held = node.nodes().iter().any(|view| view.id == downlink);
        assert_eq!(held, node.peering_of(&downlink).is_some(), "node {index}");
    }
}

// spec/areas/network.md#asking-and-answering: a core node accepts a
// periphery node at once, without a probe; one in no list with a URL is
// still unknown; a core node with no room for downlinks takes none.
#[test]
fn downlinks_are_accepted_without_a_probe() {
    let mut network = Network::with_periphery(13, 2, 1, 0, Config::DEV);
    settle(&mut network, 30_000);
    let stranger = Arc::new(dassie_identity::TableKeygen::identity_key(&[91; 32]));
    let at = network.clock(0);
    let periphery = link_state(&stranger, at, "", &[EXTENSION_PERIPHERY]);
    let reply = ask(&mut network, &stranger, 0, &periphery);
    assert_eq!(reply.status, 200, "answered at once, not deferred");
    assert!(matches!(answer(&reply), PeeringResponse::Accepted));
    assert!(network.node(0).is_downlink(&stranger.node_id()));
    assert_eq!(
        network.node(0).core_peers().count(),
        1,
        "no core slot taken"
    );
    // Its URL would be probed; it is in no list, so it is refused first.
    let other = Arc::new(dassie_identity::TableKeygen::identity_key(&[92; 32]));
    let core = link_state(&other, at, "https://other.test", &[]);
    let reply = ask(&mut network, &other, 0, &core);
    assert_eq!(
        answer(&reply),
        PeeringResponse::Refused(RefusalReason::UnknownNode)
    );
    let none = Config {
        max_downlinks: 0,
        ..Config::DEV
    };
    let mut plain = Network::new(14, 2, 1, none);
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
// capacity. Those that paid us are bounded by it alone.
#[test]
fn downlinks_are_bounded() {
    let config = Config {
        max_downlinks: 2,
        ..Config::DEV
    };
    let mut network = Network::with_periphery(15, 2, 1, 0, config);
    settle(&mut network, 30_000);
    for seed in [93, 94, 95] {
        let key = downlink_key(seed);
        let refusal = ask_as_downlink(&mut network, &key);
        if seed == 95 {
            assert_eq!(refusal, Some(RefusalReason::Capacity));
        } else {
            assert_eq!(refusal, None, "seed {seed}");
            pays(&mut network, &key);
        }
    }
    let node = network.node(0);
    assert_eq!(node.peers().count() - node.core_peers().count(), 2);
    assert_eq!(node.core_peers().count(), 1, "the core peer kept its slot");
}

// A downlink's channel closes once idle after its peering ended, and frees
// its place: the bound counts the channels open, not every downlink ever
// peered with (spec/decisions/closing-channels.md).
#[test]
fn a_closed_channel_frees_a_downlinks_place() {
    let config = Config {
        max_downlinks: 2,
        ..Config::DEV
    };
    let mut network = Network::with_periphery(17, 2, 1, 0, config);
    settle(&mut network, 30_000);
    let first = Arc::new(dassie_identity::TableKeygen::identity_key(&[97; 32]));
    let second = Arc::new(dassie_identity::TableKeygen::identity_key(&[98; 32]));
    let at = network.clock(0);
    let state = link_state(&first, at, "", &[EXTENSION_PERIPHERY]);
    let reply = ask(&mut network, &first, 0, &state);
    assert!(matches!(answer(&reply), PeeringResponse::Accepted));
    network.act(0, |node, now| {
        assert!(node.terminate(
            now,
            &first.node_id(),
            dassie_peer::wire::TerminationReason::Owner
        ));
    });
    settle(&mut network, 120_000);
    assert!(network.node(0).peering_of(&first.node_id()).is_none());
    let state = link_state(&second, network.clock(0), "", &[EXTENSION_PERIPHERY]);
    let reply = ask(&mut network, &second, 0, &state);
    assert_eq!(
        answer(&reply),
        PeeringResponse::Refused(RefusalReason::Capacity),
        "the first one's channel still holds the free receivers' place"
    );
    settle(&mut network, Config::DEV.peer_dead_ms);
    assert!(network.node(0).channel(&first.node_id()).is_none());
    // Another newcomer.
    let third = Arc::new(dassie_identity::TableKeygen::identity_key(&[99; 32]));
    let state = link_state(&third, network.clock(0), "", &[EXTENSION_PERIPHERY]);
    let reply = ask(&mut network, &third, 0, &state);
    assert!(matches!(answer(&reply), PeeringResponse::Accepted));
}

// spec/decisions/free-receivers-half.md: downlinks that never paid us take
// at most half of the downlinks' places, so keys made for nothing cannot
// fill them all. One that pays takes a paying place from then on, and
// leaves its free one to another.
#[test]
fn free_receivers_take_at_most_half_of_the_downlinks() {
    let config = Config {
        max_downlinks: 4,
        ..Config::DEV
    };
    let mut network = Network::with_periphery(19, 2, 1, 0, config);
    settle(&mut network, 30_000);
    let [a, b, c, d] = [80, 81, 82, 83].map(downlink_key);
    assert_eq!(ask_as_downlink(&mut network, &a), None);
    assert_eq!(ask_as_downlink(&mut network, &b), None);
    assert_eq!(
        ask_as_downlink(&mut network, &c),
        Some(RefusalReason::Capacity),
        "the free receivers' half is full, with room left among the downlinks"
    );
    assert!(!network.node(0).is_downlink(&c.node_id()));
    pays(&mut network, &a);
    assert!(network.node(0).channel(&a.node_id()).unwrap().paid_us);
    settle(&mut network, 1_000);
    assert_eq!(ask_as_downlink(&mut network, &c), None);
    assert_eq!(
        ask_as_downlink(&mut network, &d),
        Some(RefusalReason::Capacity)
    );
    let node = network.node(0);
    assert_eq!(node.peers().count() - node.core_peers().count(), 3);
}

// spec/decisions/free-receivers-half.md: the rule only refuses newcomers. A
// free receiver peered when the free half fills stays, and its repeated
// request, after an answer that got lost, is answered as before.
#[test]
fn a_full_free_half_ends_no_downlink() {
    let config = Config {
        max_downlinks: 2,
        ..Config::DEV
    };
    let mut network = Network::with_periphery(20, 2, 1, 0, config);
    settle(&mut network, 30_000);
    let [a, b] = [84, 85].map(downlink_key);
    assert_eq!(ask_as_downlink(&mut network, &a), None);
    for _ in 0..3 {
        settle(&mut network, 1_000);
        assert_eq!(
            ask_as_downlink(&mut network, &b),
            Some(RefusalReason::Capacity)
        );
        assert_eq!(ask_as_downlink(&mut network, &a), None, "a repeat");
    }
    let peering = network.node(0).peering_of(&a.node_id()).unwrap();
    assert!(peering.depeering.is_none());
    assert!(!ended_or_ending(&network, 0, &a.node_id()));
}

// spec/decisions/free-receivers-half.md: a downlink that paid us once is no
// free receiver for good. Its channel keeps the mark when it closes, and it
// gets in past a full free half when it comes back; one that never paid
// waits for a free place, as a newcomer does.
#[test]
fn a_downlink_that_paid_comes_back_past_a_full_free_half() {
    let config = Config {
        max_downlinks: 4,
        ..Config::DEV
    };
    let mut network = Network::with_periphery(21, 2, 1, 0, config);
    settle(&mut network, 30_000);
    let [paying, free, a, b, c] = [86, 87, 88, 89, 90].map(downlink_key);
    assert_eq!(ask_as_downlink(&mut network, &paying), None);
    assert_eq!(ask_as_downlink(&mut network, &free), None);
    pays(&mut network, &paying);
    for key in [&paying, &free] {
        let node = key.node_id();
        network.act(0, |peer, now| {
            assert!(peer.terminate(
                now,
                &node,
                dassie_peer::wire::TerminationReason::Unresponsive
            ));
        });
    }
    settle(&mut network, 120_000);
    settle(&mut network, Config::DEV.peer_dead_ms);
    for key in [&paying, &free] {
        let node = key.node_id();
        assert!(network.node(0).channel(&node).is_none(), "closed");
        assert_eq!(
            network.nodes[0].hooks.closed.get(&node).map(|c| c.paid_us),
            Some(Arc::ptr_eq(key, &paying))
        );
    }
    assert_eq!(ask_as_downlink(&mut network, &a), None);
    assert_eq!(ask_as_downlink(&mut network, &b), None);
    assert_eq!(
        ask_as_downlink(&mut network, &c),
        Some(RefusalReason::Capacity)
    );
    assert_eq!(
        ask_as_downlink(&mut network, &free),
        Some(RefusalReason::Capacity),
        "it never paid us"
    );
    assert_eq!(ask_as_downlink(&mut network, &paying), None);
    let channel = network.node(0).channel(&paying.node_id()).unwrap();
    assert!(channel.paid_us, "opened again with the mark");
}

// A periphery node takes no peers: it peers only as a downlink.
#[test]
fn a_periphery_node_takes_no_peers() {
    let (core, periphery) = (2, 1);
    let mut network = Network::with_periphery(16, core, 1, periphery, Config::DEV);
    settle(&mut network, 30_000);
    let stranger = Arc::new(dassie_identity::TableKeygen::identity_key(&[96; 32]));
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
    let stranger = Arc::new(dassie_identity::TableKeygen::identity_key(&[100; 32]));
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
                feed_port: None,
            },
        );
    });
    settle(&mut network, 10 * 60_000);
    assert!(ended(&network, turning, &id(4)), "the downlink ended");
    assert!(core_entries(&network, turning) <= 3);
}

/// Gives node `index` `url`, or takes its URL away, and makes it reached
/// as the simulation reaches a core or periphery node.
fn set_url(network: &mut Network, index: usize, url: Option<String>) {
    if url.is_some() {
        network.periphery.remove(&index);
    } else {
        network.periphery.insert(index);
    }
    network.act(index, |node, now| {
        node.set_own_info(
            now,
            dassie_peer::OwnInfo {
                url,
                alias: String::new(),
                feed_port: None,
            },
        );
    });
}

/// Whether `at` is peered with `node` as it was `since`, as a downlink if
/// `downlink`, its channel reaching it as the kind is reached.
fn peered_as(network: &Network, at: usize, node: &NodeId, since: u64, downlink: bool) -> bool {
    let peer = network.node(at);
    peer.peering_of(node).is_some_and(|p| p.since_ms == since)
        && peer.is_downlink(node) == downlink
        && peer
            .channels()
            .any(|(id, channel)| id == node && channel.endpoint.is_none() == downlink)
}

// spec/decisions/promotion-to-core.md: a peering's kind follows the peer's
// status in place. A periphery node that gets a URL, a core node for now,
// stays peered with its uplinks: they take it as a core peer, with the
// same peering and channel, once it is registered, whose URL the bootstrap
// nodes probed, and as a downlink until then. Losing its URL again makes it
// their downlink again, still the same peering.
#[test]
fn a_peers_kind_changes_in_place() {
    let (core, periphery) = (3, 1);
    let mut network = Network::with_periphery(23, core, 1, periphery, Config::DEV);
    settle(&mut network, 90_000);
    let turning = core;
    let node = id(turning);
    let uplinks: Vec<(usize, u64)> = (0..core)
        .filter(|at| network.node(*at).is_downlink(&node))
        .map(|at| (at, network.node(at).peering_of(&node).unwrap().since_ms))
        .collect();
    assert!(!uplinks.is_empty(), "the periphery node has an uplink");
    set_url(&mut network, turning, Some(dassie_peer::sim::url(turning)));
    let start = network.now;
    while !uplinks
        .iter()
        .all(|(at, since)| peered_as(&network, *at, &node, *since, false))
    {
        for (at, since) in &uplinks {
            let as_core = peered_as(&network, *at, &node, *since, false);
            assert!(as_core || peered_as(&network, *at, &node, *since, true));
        }
        settle(&mut network, 5_000);
        assert!(network.now < start + 10 * 60_000, "it turns core");
    }
    // Once the hold a change of kind starts is over, it may change again.
    settle(&mut network, Config::DEV.advertise_hold_max_ms);
    set_url(&mut network, turning, None);
    settle(&mut network, 60_000);
    for (at, since) in &uplinks {
        assert!(peered_as(&network, *at, &node, *since, true), "node {at}");
    }
    // Changing again while held down ends the peering, as a peer that
    // flaps: a peer flipping its URL costs no more than one asking anew.
    set_url(&mut network, turning, Some(dassie_peer::sim::url(turning)));
    let start = network.now;
    while uplinks
        .iter()
        .any(|(at, since)| peered_as(&network, *at, &node, *since, true))
    {
        settle(&mut network, 5_000);
        assert!(network.now < start + 10 * 60_000, "it turns core");
    }
    set_url(&mut network, turning, None);
    settle(&mut network, 60_000);
    for (at, since) in &uplinks {
        let peering = network.node(*at).peering_of(&node);
        assert!(
            peering.is_none_or(|p| p.since_ms != *since || p.depeering.is_some()),
            "node {at} ends the peering that flipped while held down"
        );
    }
}

// Review of #669: a downlink whose peering we are ending keeps its kind
// while it ends, whatever its link state says: it is never listed.
#[test]
fn an_ending_peering_keeps_its_kind() {
    let (core, periphery) = (3, 1);
    let mut network = Network::with_periphery(29, core, 1, periphery, Config::DEV);
    settle(&mut network, 90_000);
    let turning = core;
    let uplink = (0..core)
        .find(|index| network.node(*index).is_downlink(&id(turning)))
        .expect("an uplink");
    network.act(uplink, |node, now| {
        assert!(node.terminate(
            now,
            &id(turning),
            dassie_peer::wire::TerminationReason::Owner
        ));
    });
    set_url(&mut network, turning, Some(dassie_peer::sim::url(turning)));
    for _ in 0..60 {
        settle(&mut network, 5_000);
        let node = network.node(uplink);
        if node.peering_of(&id(turning)).is_none() {
            break;
        }
        assert!(node.is_downlink(&id(turning)), "it ends as a downlink");
    }
    assert!(
        network.node(uplink).peering_of(&id(turning)).is_none(),
        "it ended"
    );
}

// Review of #669: a downlink whose link state gained a URL, but which is not
// registered, stays our downlink, across a restart too: the kind restored
// is the one its peering had, which its channel keeps, never one its link
// state alone would give it without the checks a core peer passes.
#[test]
fn an_unregistered_downlink_with_a_url_stays_one_across_a_restart() {
    let (core, periphery) = (3, 1);
    let mut network = Network::with_periphery(31, core, 1, periphery, Config::DEV);
    settle(&mut network, 90_000);
    let turning = core;
    let node = id(turning);
    let Some(uplink) = (1..core).find(|at| network.node(*at).is_downlink(&node)) else {
        panic!("an uplink that is not the bootstrap node");
    };
    let since = network.node(uplink).peering_of(&node).unwrap().since_ms;
    // It can never register: nothing it sends reaches the bootstrap node.
    network.cut_one_way.insert((turning, 0));
    set_url(&mut network, turning, Some(dassie_peer::sim::url(turning)));
    settle(&mut network, 120_000);
    assert!(
        peered_as(&network, uplink, &node, since, true),
        "a downlink still"
    );
    network.crash(uplink);
    network.start(uplink);
    settle(&mut network, 10_000);
    assert!(
        peered_as(&network, uplink, &node, since, true),
        "a downlink after the restart"
    );
    assert!(!network.node(uplink).core_peers().any(|(id, _)| *id == node));
}

// spec/decisions/promotion-to-core.md: an uplink watches its downlink with
// a URL, which asks to be promoted by having one: its good standing, the
// heartbeats it answers, its room in our books, its settlement and its
// clock. Registered it is not, cut off from the bootstrap node, so that is
// all that is left once the time has passed and it has paid.
#[test]
fn an_uplink_watches_its_downlink_for_promotion() {
    use dassie_peer::{PROMOTION_ROOM_PACKETS, PromotionCheck};

    let (core, periphery) = (3, 1);
    let mut network = Network::with_periphery(37, core, 1, periphery, Config::DEV);
    settle(&mut network, 90_000);
    let turning = core;
    let node = id(turning);
    let Some(uplink) = (1..core).find(|at| network.node(*at).is_downlink(&node)) else {
        panic!("an uplink that is not the bootstrap node");
    };
    let room = std::cell::Cell::new(0);
    let promotion = |network: &Network| network.node(uplink).promotion(&node, |_| room.get());
    assert_eq!(promotion(&network), None, "no URL, no candidate");
    network.cut_one_way.insert((turning, 0));
    set_url(&mut network, turning, Some(dassie_peer::sim::url(turning)));
    settle(&mut network, 30_000);
    let early = promotion(&network).expect("a candidate");
    let pending = early.pending(&Config::DEV);
    for check in [
        PromotionCheck::Registered,
        PromotionCheck::Standing,
        PromotionCheck::Room,
        PromotionCheck::Settled,
    ] {
        assert!(pending.contains(&check), "{check:?} in {pending:?}");
    }
    assert!(early.skew_ms.is_some(), "its clock, from its link state");
    assert_eq!(
        early.url_answered, None,
        "a URL is checked only once it paid us"
    );

    room.set(PROMOTION_ROOM_PACKETS);
    network.act_with_hooks(uplink, |peer, hooks, now| peer.paid_by(now, hooks, &node));
    settle(&mut network, Config::DEV.promotion_standing_ms);
    // The node checks its URL over HTTPS, as the uplink asked: what it
    // found comes back for that URL only.
    let url = dassie_peer::sim::url(turning);
    let asked = promotion(&network).expect("a candidate still");
    assert_eq!(asked.https_answered, None, "not yet answered");
    network.act(uplink, |peer, now| {
        peer.https_checked(now, &node, "https://elsewhere.test", true);
    });
    assert_eq!(promotion(&network).unwrap().https_answered, None);
    network.act(uplink, |peer, now| {
        peer.https_checked(now, &node, &url, true)
    });
    let later = promotion(&network).expect("a candidate still");
    assert_eq!(
        later.pending(&Config::DEV),
        vec![PromotionCheck::Registered],
        "{later:?}"
    );
    assert!(later.heartbeats.sent > 0 && later.heartbeats.answered > 0);
    assert_eq!(later.url_answered, Some(true), "it answers at its URL");
    assert!(network.node(uplink).is_downlink(&node), "not acted on");
    // Its URL no longer answers us: the next check finds it so.
    network.cut_one_way.insert((uplink, turning));
    settle(&mut network, Config::DEV.promotion_standing_ms / 3);
    let cut = promotion(&network).expect("a candidate still");
    assert_eq!(cut.url_answered, Some(false));
    assert!(cut.pending(&Config::DEV).contains(&PromotionCheck::Url));
}
