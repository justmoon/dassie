//! Peering over a settlement hub, as the peer protocol carries it
//! (spec/areas/peer-protocol.md#message-set,
//! spec/designs/settlement-hubs.md#4-peering-over-a-hub): Prepares on a
//! method over a hub with their commitments, and the payer's claim after
//! the verdict on a Fulfill.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use std::sync::Arc;

use dassie_identity::{IdentityKey, NodeId};
use dassie_kit::sim::SeededRng;
use dassie_oer::{Decode, Encode};
use dassie_peer::sim::{Arriving, Network, SECOND, START, STUB, StubHooks, key, scheme};
use dassie_peer::wire::{Content, HubDetails, NodeInfo, SchemeId, SettlementOffer, sign_node_info};
use dassie_peer::{
    Config, Event, Inbound, InboundId, Offer, Output, OwnInfo, PeerNode, Reply, Sources, Stored,
    StoredNode,
};

/// A Prepare, as far as the peer protocol reads one.
const PREPARE: &[u8] = &[12, 1, 0];

/// A Fulfill, likewise.
const FULFILL: &[u8] = &[13, 1, 0];

/// A commitment, opaque to the peer protocol.
const COMMITMENT: &[u8] = &[0xc0; 96];

/// A claim, likewise.
const CLAIM: &[u8] = &[0xc1; 112];

fn settle(network: &mut Network, ms: u64) {
    let until = network.now + ms;
    network.run_until(until);
}

/// Two nodes, peered on the stub scheme, over a hub if `hub`.
fn peered(seed: u64, hub: bool) -> (Network, NodeId, NodeId) {
    let mut network = Network::new(seed, 2, 1, Config::DEV);
    let (a, b) = (*network.node(0).own(), *network.node(1).own());
    for _ in 0..30 {
        if network.node(0).is_advertised(&b) && network.node(1).is_advertised(&a) {
            break;
        }
        settle(&mut network, 3_000);
    }
    assert!(network.node(0).peering_of(&b).is_some());
    assert!(network.node(1).peering_of(&a).is_some());
    for node in &mut network.nodes {
        node.hooks.commitments = hub;
        node.hooks.claim = CLAIM.to_vec();
    }
    (network, a, b)
}

/// Messages from a node, over a connection that proves its key.
struct Sender(Arc<IdentityKey>);

impl Sender {
    fn new(key: &Arc<IdentityKey>) -> Self {
        Self(Arc::clone(key))
    }

    fn request(&mut self, _: &Network, _: usize, content: &Content<'_>) -> Arriving {
        Arriving::new(&self.0, content)
    }
}

/// A packet on the stub method, with a commitment.
fn hub_packet(packet: &[u8]) -> Content<'_> {
    Content::MethodPacket {
        request_id: 6,
        scheme: scheme(STUB),
        packet,
        commitment: COMMITMENT,
    }
}

/// A packet on the stub method, without a commitment.
fn bare_packet(packet: &[u8]) -> Content<'_> {
    Content::MethodPacket {
        request_id: 6,
        scheme: scheme(STUB),
        packet,
        commitment: &[],
    }
}

/// A packet outside any method, as Fulfills and Rejects travel.
fn plain_packet(packet: &[u8]) -> Content<'_> {
    Content::InterledgerPacket {
        request_id: 6,
        packet,
    }
}

/// Hands `body` to node `index` and gives what it made of it to `check`,
/// which answers a packet as the node would.
fn handle(
    network: &mut Network,
    index: usize,
    request: &Arriving,
    check: impl FnOnce(Inbound<'_>) -> Reply,
) -> Reply {
    let now = network.clock(index);
    let sim = &mut network.nodes[index];
    let node = sim.node.as_mut().unwrap();
    let inbound = InboundId(900);
    check(node.handle_inbound(
        now,
        &mut sim.hooks,
        inbound,
        request.from,
        &request.body,
        None,
    ))
}

/// The body of a 200 reply.
fn body(reply: &Reply) -> Vec<u8> {
    assert_eq!(
        reply.status,
        200,
        "{}",
        String::from_utf8_lossy(&reply.body)
    );
    reply.body.clone()
}

// A Prepare with its commitment reaches the node on a peering over a hub,
// commitment and all; it answers with an empty body, as for any Prepare.
#[test]
fn a_prepare_over_a_hub_comes_with_its_commitment() {
    let (mut network, _, _) = peered(61, true);
    let mut sender = Sender::new(&key(1));
    let request = sender.request(&network, 0, &hub_packet(PREPARE));
    let reply = handle(&mut network, 0, &request, |inbound| match inbound {
        Inbound::Packet {
            from,
            request_id,
            packet,
            scheme: method,
            commitment,
            release,
            answer,
        } => {
            assert_eq!((from, request_id, packet), (key(1).node_id(), 6, PREPARE));
            assert_eq!(method, Some(scheme(STUB)));
            assert_eq!(commitment, Some(COMMITMENT));
            assert_eq!(release, None);
            assert!(!answer.takes_claim());
            answer.reply(&[])
        }
        other => panic!("not a packet: {other:?}"),
    });
    assert!(body(&reply).is_empty());
}

// spec/areas/peer-protocol.md#message-set: a Prepare with a commitment is
// refused as an unpeered Prepare is (409), from a node not peered and on a
// method not over a hub. Only a Prepare travels on a method, and a Prepare
// only on one.
#[test]
fn a_commitment_comes_only_with_prepares_over_a_hub() {
    let (mut network, _, _) = peered(62, false);
    let mut sender = Sender::new(&key(1));
    let request = sender.request(&network, 0, &hub_packet(PREPARE));
    assert_eq!(network.inbound(0, &request).status, 409);
    let request = sender.request(&network, 0, &bare_packet(PREPARE));
    assert_eq!(network.inbound(0, &request).status, 200);
    let request = sender.request(&network, 0, &plain_packet(PREPARE));
    assert_eq!(network.inbound(0, &request).status, 400);

    for node in &mut network.nodes {
        node.hooks.commitments = true;
    }
    let request = sender.request(&network, 0, &hub_packet(FULFILL));
    assert_eq!(network.inbound(0, &request).status, 400);
    let request = sender.request(&network, 0, &hub_packet(PREPARE));
    assert_eq!(network.inbound(0, &request).status, 200);

    let mut stranger = Sender::new(&key(7));
    let request = stranger.request(&network, 0, &hub_packet(PREPARE));
    assert_eq!(network.inbound(0, &request).status, 409);
}

// spec/designs/settlement-hubs.md#5-joining: on a method over a hub, a
// Prepare without a commitment, as one to or from a peer with no account
// in the hub yet goes, reaches the node with none, for its scheme to judge.
#[test]
fn a_prepare_over_a_hub_may_come_without_a_commitment() {
    let (mut network, _, _) = peered(65, true);
    let mut sender = Sender::new(&key(1));
    let request = sender.request(&network, 0, &bare_packet(PREPARE));
    let reply = handle(&mut network, 0, &request, |inbound| match inbound {
        Inbound::Packet {
            scheme: method,
            commitment,
            answer,
            ..
        } => {
            assert_eq!(method, Some(scheme(STUB)));
            assert_eq!(commitment, None);
            assert!(!answer.takes_claim());
            answer.reply(&[])
        }
        other => panic!("not a packet: {other:?}"),
    });
    assert!(body(&reply).is_empty());
}

// The reply to a Fulfill over a hub carries the payer's claim; on any other
// peering it is empty, and no claim may go in it.
#[test]
fn the_reply_to_a_fulfill_over_a_hub_carries_the_claim() {
    let (mut network, _, _) = peered(63, true);
    let mut sender = Sender::new(&key(1));
    let request = sender.request(&network, 0, &plain_packet(FULFILL));
    let reply = handle(&mut network, 0, &request, |inbound| match inbound {
        Inbound::Packet {
            commitment, answer, ..
        } => {
            assert_eq!(commitment, None);
            assert!(answer.takes_claim());
            answer.reply(CLAIM)
        }
        other => panic!("not a packet: {other:?}"),
    });
    assert_eq!(body(&reply), CLAIM);

    let (mut network, _, _) = peered(64, false);
    let request = sender.request(&network, 0, &plain_packet(FULFILL));
    let reply = handle(&mut network, 0, &request, |inbound| match inbound {
        Inbound::Packet { answer, .. } => {
            assert!(!answer.takes_claim());
            answer.reply(&[])
        }
        other => panic!("not a packet: {other:?}"),
    });
    assert!(body(&reply).is_empty());
}

#[test]
#[should_panic(expected = "a claim only on a Fulfill over a hub")]
fn no_claim_off_a_hub() {
    let (mut network, _, _) = peered(65, false);
    let mut sender = Sender::new(&key(1));
    let request = sender.request(&network, 0, &plain_packet(FULFILL));
    handle(&mut network, 0, &request, |inbound| match inbound {
        Inbound::Packet { answer, .. } => answer.reply(CLAIM),
        other => panic!("not a packet: {other:?}"),
    });
}

/// The claims node `index` was given for its Fulfills.
fn claims(network: &Network, index: usize) -> Vec<(NodeId, u32, Vec<u8>)> {
    network
        .events
        .iter()
        .filter(|(at, _)| *at == index)
        .filter_map(|(_, event)| match event {
            Event::FulfillReply {
                peer,
                request_id,
                claim,
            } => Some((*peer, *request_id, claim.clone())),
            _ => None,
        })
        .collect()
}

// Between two nodes over a hub: the payer's Prepare goes with its
// commitment, and the payee, sending the Fulfill back, is given the payer's
// claim to take to the hub.
#[test]
fn a_payee_over_a_hub_is_given_the_claim() {
    let (mut network, a, b) = peered(66, true);
    network.act(0, |node, now| {
        node.send_hub_packet(now, &b, 9, PREPARE, COMMITMENT, 10_000)
            .unwrap();
    });
    settle(&mut network, 2_000);
    assert!(network.packets.contains(&(1, a, 9)), "the Prepare arrived");
    network.act(1, |node, now| node.send_response(now, &a, 9, FULFILL));
    settle(&mut network, 2_000);
    assert_eq!(claims(&network, 1), [(a, 9, CLAIM.to_vec())]);
    assert_eq!(network.node(1).stats().answers_dropped, 0);
}

// A node not over a hub replies with nothing: the Fulfill arrived, and there
// is no claim.
#[test]
fn an_empty_reply_gives_no_claim() {
    let (mut network, a, b) = peered(67, false);
    network.act(0, |node, now| {
        node.send_packet(now, &b, 9, PREPARE, 10_000).unwrap();
    });
    settle(&mut network, 2_000);
    network.act(1, |node, now| node.send_response(now, &a, 9, FULFILL));
    settle(&mut network, 2_000);
    assert!(claims(&network, 1).is_empty());
    assert_eq!(network.node(1).stats().answers_dropped, 0);
    assert!(network.node(1).reachable(&a));
}

/// Two nodes whose only scheme is over a hub, each a member of `hubs[i]`,
/// after a while to find each other.
fn over_hubs(seed: u64, hubs: [Option<HubDetails>; 2]) -> (Network, NodeId, NodeId) {
    over_hubs_with(seed, hubs, false)
}

/// [`over_hubs`], each node running the second scheme, not over a hub,
/// too if `second`.
fn over_hubs_with(
    seed: u64,
    hubs: [Option<HubDetails>; 2],
    second: bool,
) -> (Network, NodeId, NodeId) {
    let mut network = Network::new(seed, 2, 1, Config::DEV);
    for (index, hub) in hubs.into_iter().enumerate() {
        network.nodes[index].hooks.commitments = true;
        if second {
            network.set_running(index, scheme(SECOND), true);
        }
        network.set_hub(index, hub);
    }
    let (a, b) = (*network.node(0).own(), *network.node(1).own());
    settle(&mut network, 60_000);
    (network, a, b)
}

fn member(hub: u8) -> Option<HubDetails> {
    Some(HubDetails {
        hub_id: [hub; 32],
        account: 1,
    })
}

/// A member of `hub` the hub has no account for yet, to be paid in.
fn newcomer(hub: u8) -> Option<HubDetails> {
    Some(HubDetails {
        hub_id: [hub; 32],
        account: HubDetails::NO_ACCOUNT,
    })
}

/// The schemes of node `index`'s methods with `peer`, none without a
/// peering.
fn methods(network: &Network, index: usize, peer: &NodeId) -> Vec<SchemeId> {
    network
        .node(index)
        .peering_of(peer)
        .map(|peering| peering.schemes().collect())
        .unwrap_or_default()
}

// A member without an account in a hub peers over it with one that has an
// account, which pays it in; two without one could never pay each other in
// (spec/designs/settlement-hubs.md#5-joining), so they do not peer over it,
// and peer on another scheme both run instead.
#[test]
fn two_members_without_an_account_peer_on_another_scheme() {
    let (network, a, b) = over_hubs(7, [member(1), newcomer(1)]);
    assert_eq!(methods(&network, 0, &b), [scheme(STUB)]);
    assert_eq!(methods(&network, 1, &a), [scheme(STUB)]);
    let (network, a, b) = over_hubs(7, [newcomer(1), newcomer(1)]);
    assert!(network.node(0).peering_of(&b).is_none());
    assert!(network.node(1).peering_of(&a).is_none());
    let (network, a, b) = over_hubs_with(7, [newcomer(1), newcomer(1)], true);
    assert_eq!(methods(&network, 0, &b), [scheme(SECOND)]);
    assert_eq!(methods(&network, 1, &a), [scheme(SECOND)]);
}

/// The accounts node `index` heard `peer` name in its offers on a method
/// over a hub, in order.
fn advertised(network: &Network, index: usize, peer: &NodeId) -> Vec<u32> {
    (network.events.iter())
        .filter_map(|(at, event)| match event {
            Event::HubAccountAdvertised {
                peer: from,
                scheme: method,
                details,
            } if *at == index && from == peer && *method == scheme(STUB) => Some(details.account),
            _ => None,
        })
        .collect()
}

// A newcomer that peered before its account was opened names the account
// in its link state once it has one: its peer hears of it as the state
// arrives, once per account it names, so that its scheme looks it up then
// (spec/areas/hubs.md#the-members-side). Its own account, and an account
// it named already, say nothing.
#[test]
fn a_peer_naming_its_new_account_is_heard_at_once() {
    let (mut network, a, b) = over_hubs(7, [member(1), newcomer(1)]);
    assert_eq!(methods(&network, 0, &b), [scheme(STUB)]);
    assert!(advertised(&network, 0, &b).is_empty(), "no account yet");
    network.set_hub(1, member(1));
    settle(&mut network, 10_000);
    assert_eq!(advertised(&network, 0, &b), [1]);
    assert!(advertised(&network, 1, &a).is_empty());
    // A state it issues again, naming the same, says nothing more.
    let accepted = network.node(0).stats().link_states_accepted;
    network.set_hub(1, member(1));
    network.set_running(1, scheme(SECOND), true);
    settle(&mut network, 10_000);
    assert!(network.node(0).stats().link_states_accepted > accepted);
    assert_eq!(advertised(&network, 0, &b), [1]);
}

// A scheme over a hub is asked for only by a node that shares the hub with
// the other, which offers the scheme over it
// (spec/designs/settlement-hubs.md#11-choosing-a-hub): without, neither
// would find the other's account. A node that takes no peers over its hub
// offers no scheme over it.
#[test]
fn a_hub_scheme_is_chosen_only_for_a_shared_hub() {
    let (network, a, b) = over_hubs(7, [member(1), member(1)]);
    assert!(network.node(0).peering_of(&b).is_some());
    assert!(network.node(1).peering_of(&a).is_some());
    for hubs in [[member(1), member(2)], [member(1), None]] {
        let (network, a, b) = over_hubs(7, hubs);
        assert!(network.node(0).peering_of(&b).is_none(), "{hubs:?}");
        assert!(network.node(1).peering_of(&a).is_none(), "{hubs:?}");
    }
}

/// Offers of another node: each scheme with its details.
type Theirs<'a> = &'a [(SchemeId, Vec<u8>)];

/// Details of the hub of bytes `hub`, with `account`.
fn details(hub: u8, account: u32) -> Vec<u8> {
    HubDetails {
        hub_id: [hub; 32],
        account,
    }
    .to_vec()
}

/// The scheme a node offering `own` first asks a node offering `theirs` to
/// peer on, if it asks at all: the first of the methods it reads from the
/// other's offers. The stub's peerings are over a hub, and the second
/// scheme's too if `second_over_hub`.
fn first_asked(own: Vec<Offer>, theirs: Theirs<'_>, second_over_hub: bool) -> Option<SchemeId> {
    let other = dassie_identity::TableKeygen::identity_key(&[92; 32]);
    let info = NodeInfo {
        sequence: START,
        url: "https://other.test",
        alias: "",
        public_key: *other.public_key().as_bytes(),
        slh_dsa_public_key: *other.identity().slh_dsa().as_bytes(),
        settlement: theirs
            .iter()
            .map(|(scheme, details)| SettlementOffer {
                scheme: *scheme,
                details,
            })
            .collect(),
        entries: Vec::new(),
    };
    let stored = Stored {
        nodes: vec![StoredNode {
            id: other.node_id(),
            link_state: Some(sign_node_info(&other, &info)),
            sources: Sources::MAJORITY,
            first_seen_ms: START,
            registration: None,
            peering: None,
            termination: None,
        }],
        ..Stored::default()
    };
    let mut node = PeerNode::new(
        Config::DEV,
        key(0),
        OwnInfo {
            url: Some("https://n0.test".into()),
            alias: String::new(),
            feed_port: None,
        },
        Vec::new(),
        stored,
        Box::new(SeededRng::new(78)),
        START,
    );
    // A core node, which a node asks to peer.
    node.set_dev_core(vec![other.node_id()]);
    node.set_offers(START, own);
    let mut hooks = StubHooks {
        commitments: true,
        second_over_hub,
        ..StubHooks::default()
    };
    let mut now = START;
    while now < START + 600_000 {
        node.handle_timeout(now, &mut hooks);
        while let Some(output) = node.poll_output() {
            if let Output::Request(request) = output
                && request.to == other.node_id()
                && let Ok(Content::PeeringInfoRequest { scheme }) =
                    Content::decode_exact(&request.body)
            {
                return Some(scheme);
            }
        }
        now = node.poll_timeout().unwrap_or(now + 1_000).max(now + 1);
    }
    None
}

// spec/areas/peer-protocol.md#settlement-offers: a node reads another's
// offer on a scheme it offers itself by its own offer's kind: over a hub,
// the hub's details, which must name the hub it is in; on the stub, none.
// An offer whose details are not that is taken as absent, as is one whose
// scheme an earlier offer names, or whose hub an earlier offer it took
// names; an offer on a scheme it does not offer is passed over. It asks on
// the first scheme left, in its own order.
#[test]
fn offers_are_read_by_the_schemes_a_node_offers() {
    let (stub, second) = (scheme(STUB), scheme(SECOND));
    let hub = |hub: u8, account: u32| HubDetails {
        hub_id: [hub; 32],
        account,
    };
    let own = vec![Offer::over_hub(stub, hub(1, 1)), Offer::plain(second)];
    let other_hub = SchemeId::parse("hub-other").unwrap();
    let cases: [(Theirs<'_>, Option<SchemeId>); 10] = [
        (&[(stub, details(1, 2))], Some(stub)),
        (&[(stub, details(1, 0))], Some(stub)),
        (&[(stub, details(2, 2))], None),
        (&[(stub, details(1, 2)[..35].to_vec())], None),
        (&[(stub, Vec::new())], None),
        (&[(stub, Vec::new()), (stub, details(1, 2))], None),
        (
            &[(other_hub, details(1, 2)), (stub, details(1, 2))],
            Some(stub),
        ),
        (&[(second, vec![1])], None),
        (&[(second, Vec::new())], Some(second)),
        (&[(second, Vec::new()), (stub, details(1, 2))], Some(stub)),
    ];
    for (theirs, asked) in cases {
        assert_eq!(first_asked(own.clone(), theirs, false), asked, "{theirs:?}");
    }
    // Two without an account: neither could pay the other in.
    let newcomer = vec![Offer::over_hub(stub, hub(1, HubDetails::NO_ACCOUNT))];
    assert_eq!(first_asked(newcomer, &[(stub, details(1, 0))], false), None);
    // Over two hubs: an offer naming a hub an earlier one named is absent.
    let two_hubs = vec![
        Offer::over_hub(stub, hub(1, 1)),
        Offer::over_hub(second, hub(2, 1)),
    ];
    let theirs = [(second, details(2, 2))];
    assert_eq!(first_asked(two_hubs.clone(), &theirs, true), Some(second));
    let theirs = [(stub, details(2, 2)), (second, details(2, 2))];
    assert_eq!(first_asked(two_hubs, &theirs, true), None);
}

/// A Reject, as far as the peer protocol reads one.
const REJECT: &[u8] = &[14, 1, 0];

/// A payee's release, opaque to the peer protocol.
const RELEASE: &[u8] = &[0x5e; 80];

// spec/decisions/signed-rejects.md: a Reject over a hub comes in a
// `hubReject` with the payee's release, which reaches the node beside it;
// anything but a Reject in one is malformed.
#[test]
fn a_reject_over_a_hub_comes_with_its_release() {
    let (mut network, _, _) = peered(61, true);
    let mut sender = Sender::new(&key(1));
    let released = Content::HubReject {
        request_id: 6,
        packet: REJECT,
        release: RELEASE,
    };
    let request = sender.request(&network, 0, &released);
    let reply = handle(&mut network, 0, &request, |inbound| match inbound {
        Inbound::Packet {
            packet,
            scheme,
            release,
            answer,
            ..
        } => {
            assert_eq!(packet, REJECT);
            assert_eq!(scheme, None);
            assert_eq!(release, Some(RELEASE));
            assert!(!answer.takes_claim());
            answer.reply(&[])
        }
        other => panic!("{other:?}"),
    });
    assert!(body(&reply).is_empty());
    // A plain Reject comes without one.
    let request = sender.request(&network, 0, &plain_packet(REJECT));
    handle(&mut network, 0, &request, |inbound| match inbound {
        Inbound::Packet {
            release, answer, ..
        } => {
            assert_eq!(release, None);
            answer.reply(&[])
        }
        other => panic!("{other:?}"),
    });
    for packet in [FULFILL, PREPARE] {
        let wrong = Content::HubReject {
            request_id: 6,
            packet,
            release: RELEASE,
        };
        let request = sender.request(&network, 0, &wrong);
        let reply = handle(&mut network, 0, &request, |inbound| match inbound {
            Inbound::Reply(reply) => reply,
            other => panic!("{other:?}"),
        });
        assert_eq!(reply.status, 400);
    }
}
