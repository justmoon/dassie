//! The peering lifecycle and settlement delivery (D12, spec 7 §4.7), one
//! rule at a time, on two simulated nodes.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

mod support;

use std::sync::Arc;

use dassie_identity::{NodeId, NodeKey};
use dassie_node_core::testing::SeededRng;
use dassie_oer::{Decode, Encode};
use dassie_peer::auth::{AuthConfig, Authenticator};
use dassie_peer::wire::{
    Authentication, Content, Entry, NodeInfo, PeeringRequest, PeeringResponse, RefusalReason,
    ResponseEnvelope, SchemeId, SignedNodeInfo, TerminationReason, encode_envelope, sign_node_info,
};
use dassie_peer::{Config, Event, PeerStatus, SendError};
use support::{Network, START, key, url};

fn settle(network: &mut Network, ms: u64) {
    let until = network.now + ms;
    network.run_until(until);
}

fn stub() -> SchemeId {
    SchemeId::parse("stub").unwrap()
}

/// Two nodes, peered. Their clocks are apart by up to 20 s, and a node
/// takes nothing dated before its start, so it may take a while.
fn peered(seed: u64) -> (Network, NodeId, NodeId) {
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
    (network, a, b)
}

fn ended(network: &Network, index: usize, peer: &NodeId) -> Vec<bool> {
    network
        .events
        .iter()
        .filter_map(|(at, event)| match event {
            Event::PeeringEnded {
                peer: p, settle, ..
            } if *at == index && p == peer => Some(*settle),
            _ => None,
        })
        .collect()
}

/// Messages from a node, in one session of its own.
struct Sender(Authenticator);

impl Sender {
    fn new(key: &Arc<NodeKey>) -> Self {
        Self(Authenticator::new(
            Arc::clone(key),
            AuthConfig::DEFAULT,
            START - 1,
        ))
    }

    fn request(&mut self, network: &Network, to: usize, content: &Content<'_>) -> Vec<u8> {
        let bytes = content.to_vec();
        let target = key(to);
        let (hmac, _) = self
            .0
            .authenticate(
                network.clock(to),
                &mut SeededRng::new(network.clock(to)),
                &target.node_id(),
                &target.public_key(),
                &bytes,
            )
            .unwrap();
        encode_envelope(
            &self.0.own().to_owned(),
            &Authentication::Hmac(hmac),
            &bytes,
        )
    }
}

fn peering_request(network: &Network, from: &Arc<NodeKey>, index: usize, to: usize) -> Vec<u8> {
    let info = NodeInfo {
        node_id: from.node_id(),
        sequence: network.clock(to),
        url: &url(index),
        alias: "",
        public_key: *from.public_key().as_bytes(),
        entries: vec![Entry::Scheme(stub())],
    };
    let signed = sign_node_info(from, &info);
    let content = Content::PeeringRequest(PeeringRequest {
        scheme: stub(),
        node_info: SignedNodeInfo::decode_exact(&signed).unwrap(),
        data: &[],
        versions: vec![1],
        credit: 1,
    });
    Sender::new(from).request(network, to, &content)
}

fn refusal(reply: &dassie_peer::Reply) -> RefusalReason {
    assert_eq!(reply.status, 200);
    let envelope = ResponseEnvelope::decode(&reply.body).unwrap();
    match PeeringResponse::decode_exact(envelope.body).unwrap() {
        PeeringResponse::Refused(reason) => reason,
        accepted => panic!("accepted: {accepted:?}"),
    }
}

// A settlement that arrives again, as a retry after a lost answer would,
// is acknowledged and not applied twice: the stub credits from the message
// itself (ACC-23).
#[test]
fn a_settlement_is_applied_once() {
    let (mut network, a, _) = peered(31);
    let mut sender = Sender::new(&key(1));
    let content = Content::Settlement {
        scheme: stub(),
        amount: 5,
        data: &[],
        sequence: 1,
    };
    let first = sender.request(&network, 0, &content);
    let again = sender.request(&network, 0, &content);
    assert_eq!(network.inbound(0, &first).status, 200);
    assert_eq!(network.inbound(0, &again).status, 200);
    let node = network.node(0);
    assert_eq!(node.channel(&key(1).node_id()).unwrap().settled_in, 1);
    assert_eq!(node.stats().settlements_applied, 1);
    assert_eq!(node.stats().settlements_duplicate, 1);
    let _ = a;
}

// ACC-17: one settlement on its way per channel.
#[test]
fn one_settlement_at_a_time() {
    let (mut network, _, b) = peered(32);
    network.cut.insert((0, 1));
    assert!(network.settle(0, b, 3));
    let now = network.clock(0);
    assert_eq!(
        network
            .node_mut(0)
            .send_settlement(now, &b, stub(), 4, Vec::new()),
        Err(SendError::Busy)
    );
}

// A settlement whose node is unreachable, and whose sender restarts, still
// arrives, once.
#[test]
fn a_settlement_survives_a_restart() {
    let (mut network, a, b) = peered(33);
    network.cut.insert((0, 1));
    assert!(network.settle(0, b, 7));
    settle(&mut network, 3_000);
    network.crash(0);
    network.start(0);
    assert!(network.node(0).settling(&b), "the outbox was persisted");
    network.cut.clear();
    settle(&mut network, 30_000);
    assert!(!network.node(0).settling(&b));
    assert_eq!(network.applied.len(), 1);
    assert_eq!((network.applied[0].from, network.applied[0].amount), (a, 7));
}

// An owner's termination ends the peering on both sides, settled, and the
// pair does not peer again during the cooldown.
#[test]
fn an_owner_ends_a_peering() {
    let (mut network, a, b) = peered(34);
    network.act(0, |node, now| {
        assert!(node.terminate(now, &b, TerminationReason::Owner));
    });
    assert!(!network.node(0).is_advertised(&b));
    settle(&mut network, 10_000);
    assert!(network.node(0).peering_of(&b).is_none());
    assert!(network.node(1).peering_of(&a).is_none());
    assert_eq!(ended(&network, 0, &b), [true]);
    assert_eq!(ended(&network, 1, &a), [true]);
    let by_them = network.events.iter().any(|(at, event)| {
        *at == 1
            && matches!(
                event,
                Event::Terminating {
                    by_us: false,
                    reason: TerminationReason::Owner,
                    ..
                }
            )
    });
    assert!(by_them, "the other side learns why");
    settle(&mut network, 120_000);
    assert!(network.node(0).peering_of(&b).is_none(), "a cooldown");
    // A settlement after the end is still credited.
    assert!(network.settle(0, b, 9));
    settle(&mut network, 5_000);
    assert_eq!(network.applied.len(), 1);
}

#[test]
fn crossed_terminations_end_both_sides() {
    let (mut network, a, b) = peered(35);
    network.act(0, |node, now| {
        node.terminate(now, &b, TerminationReason::Owner);
    });
    network.act(1, |node, now| {
        node.terminate(now, &a, TerminationReason::Owner);
    });
    settle(&mut network, 10_000);
    assert_eq!(ended(&network, 0, &b), [true]);
    assert_eq!(ended(&network, 1, &a), [true]);
}

// A termination outlives a restart of the side that began it.
#[test]
fn a_termination_survives_a_restart() {
    let (mut network, a, b) = peered(36);
    network.cut.insert((0, 1));
    network.act(0, |node, now| {
        node.terminate(now, &b, TerminationReason::Owner);
    });
    settle(&mut network, 2_000);
    network.crash(0);
    network.start(0);
    assert!(
        network
            .node(0)
            .nodes()
            .iter()
            .any(|view| view.termination.is_some())
    );
    network.cut.clear();
    settle(&mut network, 15_000);
    assert_eq!(ended(&network, 0, &b), [true]);
    assert_eq!(ended(&network, 1, &a), [true]);
}

// A peering ends only once nothing is in flight with the peer.
#[test]
fn a_termination_waits_for_packets_in_flight() {
    let (mut network, _, b) = peered(37);
    network.nodes[0].hooks.busy.insert(b);
    network.act(0, |node, now| {
        node.terminate(now, &b, TerminationReason::Owner);
    });
    settle(&mut network, 10_000);
    assert!(network.node(0).peering_of(&b).is_some(), "still draining");
    // Meanwhile it refuses to peer again: reason 9.
    let body = peering_request(&network, &key(1), 1, 0);
    let reply = network.inbound(0, &body);
    assert_eq!(refusal(&reply), RefusalReason::Terminating);
    network.nodes[0].hooks.busy.clear();
    settle(&mut network, 3_000);
    assert!(network.node(0).peering_of(&b).is_none());
}

// A peer silent for too long is terminated; it is not told, so it is not
// settled with. When it comes back, the two hold the same view again.
#[test]
fn an_unresponsive_peer_is_terminated() {
    let (mut network, a, b) = peered(38);
    network.crash(1);
    settle(&mut network, 150_000);
    assert_eq!(ended(&network, 0, &b), [false]);
    network.start(1);
    assert!(
        network.node(1).peering_of(&a).is_some(),
        "it still holds it"
    );
    settle(&mut network, 60_000);
    assert_eq!(
        network.node(0).peering_of(&b).is_some(),
        network.node(1).peering_of(&a).is_some(),
        "peered again, or ended on both sides"
    );
}

// A peering only one side still holds is ended there: the other side
// answers its heartbeat with a termination.
#[test]
fn a_one_sided_peering_is_repaired() {
    let (mut network, a, b) = peered(43);
    network.cut.insert((0, 1));
    network.act(0, |node, now| {
        node.terminate(now, &b, TerminationReason::Owner);
    });
    settle(&mut network, 25_000);
    assert_eq!(ended(&network, 0, &b), [false], "not told, so not settled");
    assert!(network.node(1).peering_of(&a).is_some());
    network.cut.clear();
    settle(&mut network, 30_000);
    assert_eq!(ended(&network, 1, &a), [true]);
    assert!(
        network.node(0).peering_of(&b).is_none(),
        "the cooldown holds"
    );
}

// After a long downtime of both, no one is terminated for the time it was
// down: silence counts from the start.
#[test]
fn downtime_is_not_silence() {
    let (mut network, a, b) = peered(39);
    network.crash(0);
    network.crash(1);
    settle(&mut network, 600_000);
    network.start(0);
    network.start(1);
    settle(&mut network, 60_000);
    let terminated = network.events.iter().any(|(_, event)| {
        matches!(
            event,
            Event::Terminating { .. } | Event::PeeringEnded { .. }
        )
    });
    assert!(!terminated);
    assert!(network.node(0).is_advertised(&b));
    assert!(network.node(1).is_advertised(&a));
}

// A ban ends the peering, and the node is refused from then on.
#[test]
fn a_banned_node_is_refused() {
    let (mut network, a, b) = peered(40);
    network.act(0, |node, now| node.ban(now, b).unwrap());
    settle(&mut network, 10_000);
    assert!(network.node(0).peering_of(&b).is_none());
    assert!(network.node(0).is_banned(&b));
    let body = peering_request(&network, &key(1), 1, 0);
    let reply = network.inbound(0, &body);
    assert_eq!(refusal(&reply), RefusalReason::Banned);
    // It survives a restart.
    network.crash(0);
    network.start(0);
    assert!(network.node(0).is_banned(&b));
    network.act(0, |node, now| assert!(node.unban(now, &b)));
    assert!(!network.node(0).is_banned(&b));
    let _ = a;
}

// Found in review: a sequence far ahead was stored as applied, and could
// not be persisted. Only the next one is applied.
#[test]
fn a_settlement_out_of_order_is_refused() {
    let (mut network, _, _) = peered(44);
    let mut sender = Sender::new(&key(1));
    for sequence in [2, u64::MAX] {
        let content = Content::Settlement {
            scheme: stub(),
            amount: 5,
            data: &[],
            sequence,
        };
        let body = sender.request(&network, 0, &content);
        assert_eq!(network.inbound(0, &body).status, 400, "{sequence}");
    }
    let channel = network.node(0).channel(&key(1).node_id()).unwrap();
    assert_eq!(channel.settled_in, 0);
}

// Found in review: the final settlement after a peering ended was kept
// only in memory. It is on the channel, persisted, until it is made.
#[test]
fn a_final_settlement_due_survives_a_restart() {
    let (mut network, _, b) = peered(45);
    network.act(0, |node, now| {
        node.terminate(now, &b, TerminationReason::Owner);
    });
    settle(&mut network, 10_000);
    assert!(network.node(0).channel(&b).unwrap().final_due.is_some());
    network.crash(0);
    network.start(0);
    assert!(network.node(0).channel(&b).unwrap().final_due.is_some());
    network.act(0, |node, _| node.final_settled(&b));
    assert!(network.node(0).channel(&b).unwrap().final_due.is_none());
}

// Found in review: peerings persisted before channels existed (M4a) had
// none, so settling with them could not be announced.
#[test]
fn peerings_from_before_channels_get_one() {
    let (mut network, _, b) = peered(46);
    network.crash(0);
    network.nodes[0].stored_channels.clear();
    network.start(0);
    assert!(network.node(0).channel(&b).is_some());
}

// Found in review: a peer that kept sending Prepares while its peering
// was ending kept it from ever ending. They are refused at the protocol.
#[test]
fn a_peering_being_ended_takes_no_new_prepares() {
    let (mut network, _, b) = peered(47);
    network.nodes[0].hooks.busy.insert(b);
    network.act(0, |node, now| {
        node.terminate(now, &b, TerminationReason::Owner);
    });
    let mut sender = Sender::new(&key(1));
    let prepare = Content::InterledgerPacket {
        request_id: 1,
        packet: &[12, 1, 0],
    };
    let body = sender.request(&network, 0, &prepare);
    assert_eq!(network.inbound(0, &body).status, 409);
    // Responses still come through.
    let fulfill = Content::InterledgerPacket {
        request_id: 2,
        packet: &[13, 1, 0],
    };
    let body = sender.request(&network, 0, &fulfill);
    assert_eq!(network.inbound(0, &body).status, 200);
}

// The advertise flag follows liveness, and each withdrawal holds the peer
// down longer (damping, D12).
#[test]
fn the_advertise_flag_is_damped() {
    let (mut network, _, b) = peered(41);
    let view = |network: &Network| {
        network
            .node(0)
            .nodes()
            .into_iter()
            .find(|view| view.id == b)
            .unwrap()
    };
    assert!(view(&network).online);
    for flap in 1..=2u32 {
        network.cut.insert((0, 1));
        settle(&mut network, 70_000);
        let withdrawn = view(&network);
        assert!(!withdrawn.advertised);
        assert!(!withdrawn.online, "offline, and still peered");
        assert_eq!(withdrawn.peer_status, PeerStatus::Peered);
        assert_eq!(withdrawn.advertise.flaps, flap);
        network.cut.clear();
        settle(&mut network, 40_000);
        assert!(view(&network).online, "online again");
        assert!(view(&network).advertised, "advertised again");
    }
    let hold = view(&network).advertise.hold_until_ms;
    assert!(hold > 0);
}

// A restart does not drop the peers we advertise: the flag starts from
// when they were last heard from.
#[test]
fn a_restart_keeps_the_neighbors() {
    let (mut network, _, b) = peered(42);
    network.crash(0);
    network.start(0);
    assert!(network.node(0).is_advertised(&b));
}

fn verdicts(network: &Network, index: usize) -> Vec<(u64, bool)> {
    network
        .events
        .iter()
        .filter_map(|(at, event)| match event {
            Event::Verdict { tag, accepted, .. } if *at == index => Some((*tag, *accepted)),
            _ => None,
        })
        .collect()
}

// D17: a Fulfill gets the peer's verdict in its reply, and still does
// after the peering ended, from a node we hold a channel with; a Prepare
// then does not.
#[test]
fn a_fulfill_gets_a_verdict_before_and_after_the_end() {
    let (mut network, a, b) = peered(48);
    network.refused.insert(5);
    let fulfill = [13, 1, 0];
    network.act(1, |node, now| {
        node.send_fulfill(now, &a, 4, &fulfill, 40).unwrap();
        node.send_fulfill(now, &a, 5, &fulfill, 50).unwrap();
    });
    settle(&mut network, 1_000);
    assert_eq!(verdicts(&network, 1), [(40, true), (50, false)]);
    network.act(0, |node, now| {
        assert!(node.terminate(now, &b, TerminationReason::Owner));
    });
    settle(&mut network, 10_000);
    assert!(network.node(1).peering_of(&a).is_none());
    network.act(1, |node, now| {
        node.send_fulfill(now, &a, 6, &fulfill, 60).unwrap();
    });
    settle(&mut network, 1_000);
    assert_eq!(verdicts(&network, 1).last(), Some(&(60, true)));
    let mut sender = Sender::new(&key(1));
    let prepare = Content::InterledgerPacket {
        request_id: 7,
        packet: &[12, 1, 0],
    };
    let body = sender.request(&network, 0, &prepare);
    assert_eq!(network.inbound(0, &body).status, 409);
}

// Without a verdict in the reply, the node hears that none came, and
// sends the Fulfill again itself.
#[test]
fn an_unreachable_peer_gives_no_verdict() {
    let (mut network, a, _) = peered(49);
    network.cut.insert((1, 0));
    network.cut.insert((0, 1));
    network.act(1, |node, now| {
        node.send_fulfill(now, &a, 4, &[13, 1, 0], 40).unwrap();
    });
    settle(&mut network, 40_000);
    assert!(verdicts(&network, 1).is_empty());
    let missing = network
        .events
        .iter()
        .any(|(at, event)| *at == 1 && matches!(event, Event::VerdictMissing { tag: 40, .. }));
    assert!(missing);
}
