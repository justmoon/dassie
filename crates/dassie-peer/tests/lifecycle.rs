//! The peering lifecycle and settlement delivery (D12,
//! spec/areas/network.md#lifecycle,
//! spec/areas/settlement.md#settlement-channels), one rule at a time, on
//! two simulated nodes.

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
use dassie_peer::sim::{Network, START, key, url};
use dassie_peer::wire::{
    Authentication, Content, Entry, NodeInfo, PeeringRequest, PeeringResponse, RefusalReason,
    ResponseEnvelope, SchemeId, SignedNodeInfo, TerminationReason, encode_envelope, sign_node_info,
};
use dassie_peer::{Config, Event, PeerStatus, Room, SendError, VERDICT_ACCEPTED};

fn settle(network: &mut Network, ms: u64) {
    let until = network.now + ms;
    network.run_until(until);
}

fn stub() -> SchemeId {
    SchemeId::parse("stub").unwrap()
}

/// Two nodes, peered. Their clocks are apart by up to 20 s, and a node
/// takes nothing dated before its start, so it may take a while.
/// The settings the tests run, unless they say otherwise.
const CONFIG: Config = Config::DEV;

fn peered(seed: u64) -> (Network, NodeId, NodeId) {
    peered_with(seed, CONFIG)
}

/// Two nodes, peered, with `config`.
fn peered_with(seed: u64, config: Config) -> (Network, NodeId, NodeId) {
    let mut network = Network::new(seed, 2, 1, config);
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

/// How many times node `index` ended its peering with `peer`.
fn ended(network: &Network, index: usize, peer: &NodeId) -> usize {
    network
        .events
        .iter()
        .filter(|(at, event)| {
            *at == index && matches!(event, Event::PeeringEnded { peer: p } if p == peer)
        })
        .count()
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

// Found by the node simulation (D27): a settlement that could not be
// delivered through a long outage backs off, up to an hour at production
// timings. Once its node answers a request of ours again, it goes at once,
// rather than when its backoff ends, and arrives once.
#[test]
fn a_settlement_goes_as_soon_as_its_node_is_back() {
    let config = Config {
        settlement_retry_max_ms: 3_600_000,
        ..Config::DEV
    };
    let (mut network, a, b) = peered_with(41, config);
    network.crash(1);
    assert!(network.settle(0, b, 5));
    // 25 minutes of failures: the next attempt is due some 17 minutes on.
    settle(&mut network, 25 * 60_000);
    assert!(network.node(0).settling(&b));
    network.start(1);
    settle(&mut network, 60_000);
    assert!(
        !network.node(0).settling(&b),
        "the settlement waited for its backoff"
    );
    assert_eq!(network.applied.len(), 1);
    assert_eq!((network.applied[0].from, network.applied[0].amount), (a, 5));
}

// A node that answers our other requests but is too busy for a settlement
// gets it at the pace of the backoff, not once per answer.
#[test]
fn a_busy_node_gets_a_settlement_at_the_pace_of_the_backoff() {
    // Tries at 0, 1, 3, 7, ... 1023 s.
    refused_settlement(Some(503), 11);
}

// A node that answers our other requests but never a settlement gets it at
// most twice as often as the backoff alone would send it: an early try
// that gets no answer does not make another.
#[test]
fn a_node_that_drops_settlements_gets_them_at_the_pace_of_the_backoff() {
    // Each try times out after 30 s: some 10 in the backoff, and as many
    // early.
    refused_settlement(None, 20);
}

/// Settles while node 1 refuses settlement messages with `status`, or
/// drops them, for 25 minutes at production timings, and checks it got at
/// most `tries`; then that the settlement arrives once node 1 takes it.
fn refused_settlement(status: Option<u16>, tries: usize) {
    let config = Config {
        settlement_retry_max_ms: 3_600_000,
        ..Config::DEV
    };
    let (mut network, a, b) = peered_with(41, config);
    network.refuse_settlements.insert(1, status);
    assert!(network.settle(0, b, 5));
    settle(&mut network, 25 * 60_000);
    assert!(network.node(0).settling(&b));
    assert!(
        network.settlements_refused <= tries,
        "{} tries in 25 minutes",
        network.settlements_refused
    );
    network.refuse_settlements.clear();
    settle(&mut network, 3_600_000);
    assert!(!network.node(0).settling(&b));
    assert_eq!(network.applied.len(), 1);
    assert_eq!((network.applied[0].from, network.applied[0].amount), (a, 5));
}

// NET-13: the original could not end a peering. Here an owner's
// termination ends it on both sides, and the pair does not peer again
// during the cooldown. It begins privately (D28): the peer is still
// advertised, until the node collected and tells it.
#[test]
fn an_owner_ends_a_peering() {
    let (mut network, a, b) = peered(34);
    network.act(0, |node, now| {
        assert!(node.terminate(now, &b, TerminationReason::Owner));
    });
    assert!(network.node(0).is_advertised(&b));
    settle(&mut network, 10_000);
    assert!(network.node(0).peering_of(&b).is_none());
    assert!(network.node(1).peering_of(&a).is_none());
    assert_eq!(ended(&network, 0, &b), 1);
    assert_eq!(ended(&network, 1, &a), 1);
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
    assert_eq!(ended(&network, 0, &b), 1);
    assert_eq!(ended(&network, 1, &a), 1);
}

// A termination outlives a restart of the side that began it: while it
// still collects (D28), and while it tells the peer.
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
    let depeering = network.node(0).peering_of(&b).unwrap().depeering;
    assert_eq!(depeering.map(|d| d.reason), Some(TerminationReason::Owner));
    // Its last request goes nowhere, and is waited out; then it tells.
    settle(&mut network, CONFIG.depeer_wait_ms + 2_000);
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
    assert_eq!(ended(&network, 0, &b), 1);
    assert_eq!(ended(&network, 1, &a), 1);
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

// A peer silent for too long is terminated: each step that needs it times
// out (D28). When it comes back, the two hold the same view again.
#[test]
fn an_unresponsive_peer_is_terminated() {
    let (mut network, a, b) = peered(38);
    network.crash(1);
    settle(
        &mut network,
        CONFIG.peer_dead_ms + CONFIG.depeer_wait_ms + CONFIG.termination_timeout_ms + 10_000,
    );
    assert_eq!(ended(&network, 0, &b), 1);
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
    // Its last request and its message go nowhere, and are waited out.
    settle(
        &mut network,
        CONFIG.depeer_wait_ms + CONFIG.termination_timeout_ms + 5_000,
    );
    assert_eq!(ended(&network, 0, &b), 1);
    assert!(network.node(1).peering_of(&a).is_some());
    network.cut.clear();
    settle(&mut network, 30_000);
    assert_eq!(ended(&network, 1, &a), 1);
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

// NET-13: the original had no bans. A ban ends the peering, and the node
// is refused from then on.
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
// was ending kept it from ever ending. They are refused at the protocol,
// once the peer is told. Before, while the node collects (D28), the peer
// node takes them as before, and the node's connector refuses them, as
// for want of liquidity, which tells the peer nothing.
#[test]
fn a_peering_being_ended_takes_no_new_prepares() {
    let (mut network, _, b) = peered(47);
    network.nodes[0].hooks.busy.insert(b);
    network.act(0, |node, now| {
        node.terminate(now, &b, TerminationReason::Owner);
    });
    let mut sender = Sender::new(&key(1));
    let private = Content::InterledgerPacket {
        request_id: 3,
        packet: &[12, 1, 0],
    };
    let body = sender.request(&network, 0, &private);
    assert_eq!(network.inbound(0, &body).status, 200);
    settle(&mut network, 2_000);
    assert!(
        network
            .node(0)
            .nodes()
            .iter()
            .any(|view| view.termination.is_some())
    );
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

// A peer whose room in our books stays short of a packet past the grace
// is withdrawn, as stuck, with the same damping; while short for less, it
// is waiting for a routine settlement and stays listed (D12).
#[test]
fn a_peer_short_of_room_is_withdrawn_after_the_grace() {
    let (mut network, _, b) = peered(43);
    let view = |network: &Network| {
        network
            .node(0)
            .nodes()
            .into_iter()
            .find(|view| view.id == b)
            .unwrap()
    };
    let grace = Config::DEV.advertise_short_grace_ms;
    report(&mut network, &b, SHORT, grace / 2);
    assert!(view(&network).advertised, "withdrawn within the grace");
    report(&mut network, &b, SHORT, grace);
    let withdrawn = view(&network);
    assert!(!withdrawn.advertised, "still advertised past the grace");
    assert!(withdrawn.online, "withdrawn for room, not for silence");
    assert_eq!(withdrawn.peer_status, PeerStatus::Peered);
    assert_eq!(withdrawn.advertise.flaps, 1);
    assert!(network.events.iter().any(|(at, event)| *at == 0
        && *event
            == Event::Advertised {
                peer: b,
                on: false,
                short: true
            }));
    report(
        &mut network,
        &b,
        Room::Enough,
        Config::DEV.advertise_hold_min_ms + 5_000,
    );
    assert!(view(&network).advertised, "not advertised again");
}

/// What a check every second finds of a short peer, as the node's
/// settlement checks in dev: up to two seconds count.
const SHORT: Room = Room::Short { most_ms: 2_000 };

/// Node 0 reports `room` for `peer` at a check every second, for `ms`, as
/// its settlement checks do in dev.
fn report(network: &mut Network, peer: &NodeId, room: Room, ms: u64) {
    let end = network.now + ms;
    while network.now < end {
        network.act(0, |node, now| node.room(now, peer, room));
        settle(network, 1_000);
    }
    network.act(0, |node, now| node.room(now, peer, room));
}

// A peer that is both silent and short of room when it is withdrawn is
// withdrawn for its silence: `short` names room only when it is the one
// reason (review of #126).
#[test]
fn a_silent_peer_short_of_room_is_withdrawn_for_its_silence() {
    // A grace shorter than the confirmation window, so that both can come
    // due at once after the cut.
    let config = Config {
        advertise_short_grace_ms: 10_000,
        ..CONFIG
    };
    assert!(config.advertise_short_grace_ms < config.peer_confirm_ms);
    let (mut network, _, b) = peered_with(47, config);
    network.cut.insert((0, 1));
    // What was already on its way still arrives.
    settle(&mut network, 5_000);
    // Node 0's clock runs apart from the network's: its deadlines are on
    // its own.
    let at = network.now;
    let mut local = (0, 0);
    network.act(0, |node, now| {
        local = (now, node.peering_of(&b).unwrap().last_seen_ms);
    });
    let (now, last_seen) = local;
    let to_network = |local_ms: u64| local_ms - now + at;
    // Both come due at once: silence just past the confirmation window,
    // and room a grace after it was reported short.
    let due = last_seen + config.peer_confirm_ms + 1;
    // Short at a check every second from a grace before.
    let mut check = due - config.advertise_short_grace_ms;
    while check <= due {
        network.run_until(to_network(check));
        network.act(0, |node, now| node.room(now, &b, SHORT));
        check += 1_000;
    }
    network.run_until(to_network(due) + 1_000);
    let withdrawn: Vec<bool> = network
        .events
        .iter()
        .filter_map(|(at, event)| match event {
            Event::Advertised {
                peer,
                on: false,
                short,
            } if *at == 0 && *peer == b => Some(*short),
            _ => None,
        })
        .collect();
    assert_eq!(withdrawn, [false], "withdrawn for room, though silent");
}

// D33: a link that stayed short of room, settling nothing, is ended, and
// the pair stays apart for the owner's cooldown, by node 0's own clock:
// neither asks the other, and node 0 refuses node 1's request. The next
// peering with it as node 0's senior raises it no credit, which the
// channel remembers. Only past the cooldown do they peer again.
#[test]
fn a_peer_ended_for_room_is_not_peered_again_within_the_cooldown() {
    let (mut network, a, b) = peered(44);
    report(&mut network, &b, SHORT, CONFIG.peer_dead_ms + 2_000);
    let deadline = network.now + CONFIG.depeer_wait_ms * 2 + CONFIG.termination_timeout_ms + 5_000;
    while ended(&network, 0, &b) == 0 && network.now < deadline {
        settle(&mut network, 100);
    }
    assert_eq!(ended(&network, 0, &b), 1, "ended for room");
    assert!(
        network.events.iter().any(|(at, event)| *at == 0
            && *event
                == Event::Depeering {
                    peer: b,
                    reason: TerminationReason::ShortOfRoom
                }),
        "ended for room, not for another reason"
    );
    assert!(network.node(0).channel(&b).unwrap().ended_for_room);
    assert!(!network.node(1).channel(&a).unwrap().ended_for_room);
    let ended_at = network.clock(0);
    let cooldown = CONFIG.owner_termination_cooldown_ms;
    let body = peering_request(&network, &key(1), 1, 0);
    let reply = network.inbound(0, &body);
    assert_eq!(refusal(&reply), RefusalReason::Terminating);
    while network.clock(0) + 1_000 < ended_at + cooldown {
        settle(&mut network, 1_000);
        assert!(
            network.node(0).peering_of(&b).is_none() && network.node(1).peering_of(&a).is_none(),
            "peered again {} ms after ending it for room",
            network.clock(0) - ended_at
        );
    }
    // Past it, the two peer again, on the channel that remembers.
    for _ in 0..60 {
        if network.node(0).peering_of(&b).is_some() {
            break;
        }
        settle(&mut network, 1_000);
    }
    assert!(network.node(0).peering_of(&b).is_some(), "peered again");
    assert!(network.node(0).channel(&b).unwrap().ended_for_room);
}

/// Node 0 checks `peer` every second for `ms`, as its settlement checks do
/// in dev, finds it short, and asks it to settle at each check: a peer
/// short of room is always close enough to be asked, and one that cannot
/// be reached is asked at every check (D12).
fn report_asking(network: &mut Network, peer: &NodeId, ms: u64) {
    let end = network.now + ms;
    while network.now < end {
        network.act(0, |node, now| {
            node.room(now, peer, SHORT);
            // Too much in flight, or a peering being ended, asks nothing.
            let _ = node.request_settlement(now, peer, stub());
        });
        settle(network, 1_000);
    }
}

/// Whether node 0 decided to end its peering with `peer` for `reason`.
fn depeered(network: &Network, peer: &NodeId, reason: TerminationReason) -> bool {
    network.events.iter().any(|(at, event)| {
        *at == 0
            && *event
                == Event::Depeering {
                    peer: *peer,
                    reason,
                }
    })
}

// Cut off one way, node 0's requests never reach node 1, while node 1's
// reach node 0 and are answered, so each still hears the other. Short of
// room for twice as long as a silent peer is kept, node 1 is withdrawn,
// since the link carries nothing, but not ended for room: every request to
// settle failed before it reached node 1, so it had no chance to settle,
// and an end for room would keep the pair apart and raise node 1 no credit
// again (D12, D33). Once it is reached again, only the time it is reached
// counts toward the end.
#[test]
fn a_peer_out_of_reach_is_not_ended_for_room() {
    let (mut network, a, b) = peered(53);
    network.cut_one_way.insert((0, 1));
    report_asking(&mut network, &b, CONFIG.peer_dead_ms * 2);
    assert!(
        network
            .events
            .iter()
            .any(|(at, event)| *at == 0
                && *event == Event::SettlementRequestNotDelivered { peer: b }),
        "a request to settle reached the peer"
    );
    assert!(
        !depeered(&network, &b, TerminationReason::ShortOfRoom),
        "ended for room"
    );
    assert!(
        network.node(1).peering_of(&a).is_some(),
        "ended by the peer"
    );
    let peering = network.node(0).peering_of(&b).unwrap();
    assert!(peering.depeering.is_none(), "ended");
    assert!(peering.short_for_ms >= CONFIG.peer_dead_ms);
    // Up to one check before the first failed request came back.
    assert!(
        peering.short_reached_ms <= 1_000,
        "{}",
        peering.short_reached_ms
    );
    assert!(!network.node(0).is_advertised(&b), "still advertised");
    assert!(
        network.events.iter().any(|(at, event)| *at == 0
            && *event
                == Event::Advertised {
                    peer: b,
                    on: false,
                    short: true
                }),
        "not withdrawn for room"
    );
    network.cut_one_way.clear();
    report_asking(&mut network, &b, CONFIG.peer_dead_ms - 10_000);
    assert!(
        !depeered(&network, &b, TerminationReason::ShortOfRoom),
        "the cut counted"
    );
    report_asking(&mut network, &b, 12_000);
    assert!(
        depeered(&network, &b, TerminationReason::ShortOfRoom),
        "not ended for room"
    );
}

// A peer that is reached and stays short is ended for room, though it
// refuses every request to settle, as an overloaded node would (503):
// only a request that surely never arrived spares it.
#[test]
fn a_peer_that_refuses_to_settle_is_ended_for_room() {
    let (mut network, _, b) = peered(54);
    network.refuse_settlement_requests.insert(1, 503);
    report_asking(&mut network, &b, CONFIG.peer_dead_ms - 2_000);
    assert!(
        !depeered(&network, &b, TerminationReason::ShortOfRoom),
        "ended early"
    );
    report_asking(&mut network, &b, 4_000);
    assert!(
        depeered(&network, &b, TerminationReason::ShortOfRoom),
        "not ended for room"
    );
}

// How long a peer has been short of room is kept across a restart, so a
// node that restarts more often than a day still ends a link that stays
// short; and the time the node was down does not count, since it could not
// ask the peer to settle (review of #126 and #137).
#[test]
fn a_restart_keeps_the_time_short_and_counts_no_downtime() {
    let (mut network, _, b) = peered(48);
    report(&mut network, &b, SHORT, 5_000);
    let short = |network: &Network| network.node(0).peering_of(&b).unwrap().short_for_ms;
    assert_eq!(short(&network), 5_000);
    // Down for far longer than the grace, and less than the peer takes
    // to end a silent peering.
    network.crash(0);
    settle(&mut network, 90_000);
    network.start(0);
    assert_eq!(short(&network), 5_000, "not kept");
    network.act(0, |node, now| node.room(now, &b, SHORT));
    assert_eq!(short(&network), 5_000, "the downtime counted");
    report(&mut network, &b, SHORT, 2_000);
    assert_eq!(short(&network), 7_000);
}

// A node that stalled without restarting (its host suspended, or its
// clock jumped) did not ask the peer to settle meanwhile: one check adds no
// more than a few check intervals (review of #137).
#[test]
fn a_stall_counts_little_as_time_short() {
    let (mut network, _, b) = peered(50);
    let short = |network: &Network| network.node(0).peering_of(&b).unwrap().short_for_ms;
    report(&mut network, &b, SHORT, 3_000);
    assert_eq!(short(&network), 3_000);
    // No check for a minute, then one.
    settle(&mut network, 60_000);
    network.act(0, |node, now| node.room(now, &b, SHORT));
    assert_eq!(short(&network), 5_000, "the stall counted in full");
    assert!(network.node(0).is_advertised(&b), "withdrawn for a stall");
}

// While the node cannot ask a peer to settle (its scheme does not see its
// ledger), nothing is judged: the time short neither grows nor starts
// again, and the peer is not withdrawn for it (review of #137).
#[test]
fn an_outage_of_the_ledger_pauses_the_time_short() {
    let (mut network, _, b) = peered(49);
    let short = |network: &Network| network.node(0).peering_of(&b).unwrap().short_for_ms;
    report(&mut network, &b, SHORT, 3_000);
    assert_eq!(short(&network), 3_000);
    report(&mut network, &b, Room::Unjudged, 60_000);
    assert_eq!(short(&network), 3_000, "the outage counted, or cleared it");
    assert!(
        network.node(0).is_advertised(&b),
        "withdrawn during the outage"
    );
    network.act(0, |node, now| node.room(now, &b, SHORT));
    assert_eq!(short(&network), 3_000);
    report(&mut network, &b, SHORT, 2_000);
    assert_eq!(short(&network), 5_000);
    report(&mut network, &b, Room::Enough, 0);
    assert_eq!(short(&network), 0);
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

// A Fulfill's reply says it was accepted, which nodes of 0.1 read as the
// verdict that keeps their books as they are (D23). A node we hold a
// channel with still gets it after the peering ended; a Prepare does not.
#[test]
fn a_fulfill_is_accepted_before_and_after_the_end() {
    let (mut network, a, b) = peered(48);
    let fulfill = Content::InterledgerPacket {
        request_id: 6,
        packet: &[13, 1, 0],
    };
    let mut sender = Sender::new(&key(1));
    let body = sender.request(&network, 0, &fulfill);
    let reply = network.inbound(0, &body);
    assert_eq!(reply.status, 200);
    assert_eq!(reply.body.last(), Some(&VERDICT_ACCEPTED));
    network.act(0, |node, now| {
        assert!(node.terminate(now, &b, TerminationReason::Owner));
    });
    settle(&mut network, 10_000);
    assert!(network.node(1).peering_of(&a).is_none());
    let body = sender.request(&network, 0, &fulfill);
    let reply = network.inbound(0, &body);
    assert_eq!(reply.status, 200);
    assert_eq!(reply.body.last(), Some(&VERDICT_ACCEPTED));
    let prepare = Content::InterledgerPacket {
        request_id: 7,
        packet: &[12, 1, 0],
    };
    let body = sender.request(&network, 0, &prepare);
    assert_eq!(network.inbound(0, &body).status, 409);
}

// D23: a node asks a peer to settle, naming only the scheme; the peer
// takes it on that scheme while peered, and refuses it on another, or
// once the peering ended.
#[test]
fn a_settlement_request_reaches_the_peer_only() {
    let (mut network, a, b) = peered(50);
    network.act(1, |node, now| {
        node.request_settlement(now, &a, stub()).unwrap();
    });
    settle(&mut network, 1_000);
    assert_eq!(network.settlement_requests, [(0, b)]);
    assert_eq!(network.node(1).stats().settlements_requested, 1);
    // Its answer says what it has on its way: nothing, here (D28).
    let answered = Event::SettlementRequestAnswered {
        peer: a,
        settling: Some(0),
    };
    assert!(network.events.contains(&(1, answered)));
    let mut sender = Sender::new(&key(1));
    let other = Content::SettlementRequest {
        scheme: SchemeId::parse("other").unwrap(),
    };
    let body = sender.request(&network, 0, &other);
    assert_eq!(network.inbound(0, &body).status, 409);
    network.act(0, |node, now| {
        assert!(node.terminate(now, &b, TerminationReason::Owner));
    });
    settle(&mut network, 10_000);
    network.act(1, |node, now| {
        assert!(node.request_settlement(now, &a, stub()).is_err());
    });
    let request = Content::SettlementRequest { scheme: stub() };
    let body = sender.request(&network, 0, &request);
    assert_eq!(network.inbound(0, &body).status, 409);
}

// A request to settle that the peer refuses for not being peered with us,
// as when it has not yet taken in our acceptance, is reported: the node
// may ask again at once rather than a minute later. Here node 0 ends the
// peering, and node 1 asks before it hears.
#[test]
fn a_refused_settlement_request_is_reported() {
    let (mut network, a, b) = peered(51);
    // Cut off, node 0 collects nothing, cannot tell, and ends the peering
    // on its side alone; node 1 asks as soon as it is in reach again.
    network.cut.insert((0, 1));
    network.act(0, |node, now| {
        assert!(node.terminate(now, &b, TerminationReason::Owner));
    });
    settle(
        &mut network,
        CONFIG.depeer_wait_ms + CONFIG.termination_timeout_ms + 5_000,
    );
    assert_eq!(ended(&network, 0, &b), 1);
    network.cut.clear();
    network.act(1, |node, now| {
        node.request_settlement(now, &a, stub()).unwrap();
    });
    settle(&mut network, 1_000);
    let refused = Event::SettlementRequestRefused { peer: a };
    assert!(
        network
            .events
            .iter()
            .any(|(at, event)| *at == 1 && *event == refused),
        "the refusal was not reported"
    );
    assert!(network.settlement_requests.is_empty());
}

/// Three nodes, all peered, the first the bootstrap node. Node 1 ended
/// its peering with node 2 alone, its message cut off for the whole
/// termination timeout, and cut off for so long after that node 2 no
/// longer lists node 1 as a neighbor. The link is back.
fn ended_on_one_side(seed: u64) -> (Network, NodeId, NodeId) {
    let mut network = Network::new(seed, 3, 1, CONFIG);
    let (b, c) = (*network.node(1).own(), *network.node(2).own());
    for _ in 0..30 {
        if network.node(1).is_advertised(&c) && network.node(2).is_advertised(&b) {
            break;
        }
        settle(&mut network, 3_000);
    }
    assert!(network.node(1).is_advertised(&c) && network.node(2).is_advertised(&b));
    network.cut.insert((1, 2));
    network.act(1, |node, now| {
        assert!(node.terminate(now, &c, TerminationReason::Owner));
    });
    settle(
        &mut network,
        CONFIG.depeer_wait_ms + CONFIG.termination_timeout_ms + CONFIG.peer_confirm_ms,
    );
    assert_eq!(ended(&network, 1, &c), 1);
    assert!(network.node(2).peering_of(&b).is_some());
    assert!(
        !network.node(2).is_advertised(&b),
        "node 2 still lists node 1, whose heartbeat rule would do"
    );
    network.cut.clear();
    network.events.clear();
    (network, b, c)
}

/// Whether node 2 ended its peering with `b` before it listed `b` as a
/// neighbor again. A 200 from `b` to any request of its own confirms `b`
/// again, as one to a refresh soon does here, and a heartbeat that lists
/// `b` is answered with a termination: that rule must not be what ends it.
fn ended_unlisted(network: &Network, b: &NodeId) -> bool {
    let relisted = network.events.iter().position(|(at, event)| {
        *at == 2 && matches!(event, Event::Advertised { peer, on: true, .. } if peer == b)
    });
    let ended = network.events.iter().position(|(at, event)| {
        *at == 2 && matches!(event, Event::PeeringEnded { peer } if peer == b)
    });
    ended.is_some_and(|ended| relisted.is_none_or(|relisted| ended < relisted))
}

// A peering only one side still holds is ended there even once it no
// longer lists the other as a neighbor: a request to settle, which a
// creditor short of room sends at every check after a 409, is answered
// with a termination too. Node 2 ends within a termination timeout, not
// when its peer has been silent for a day, and stops asking.
#[test]
fn a_one_sided_peering_asking_to_settle_is_repaired() {
    let (mut network, b, _) = ended_on_one_side(52);
    let until = network.now + CONFIG.termination_timeout_ms;
    let mut asked = 0;
    while network.now < until {
        let mut sent = false;
        network.act(2, |node, now| {
            sent = node.request_settlement(now, &b, stub()).is_ok();
        });
        if !sent {
            break;
        }
        asked += 1;
        settle(&mut network, 250);
    }
    assert!(
        ended_unlisted(&network, &b),
        "node 2 still holds the peering"
    );
    assert!(asked <= 2, "asked {asked} times");
    assert!(network.settlement_requests.iter().all(|(at, _)| *at != 1));
}

// The same for a Prepare: refused as before, and answered with a
// termination.
#[test]
fn a_one_sided_peering_sending_a_prepare_is_repaired() {
    let (mut network, b, _) = ended_on_one_side(53);
    let prepare = Content::InterledgerPacket {
        request_id: 7,
        packet: &[12, 1, 0],
    };
    let body = Sender::new(&key(2)).request(&network, 1, &prepare);
    assert_eq!(network.inbound(1, &body).status, 409);
    settle(&mut network, 1_000);
    assert!(
        ended_unlisted(&network, &b),
        "node 2 still holds the peering"
    );
}

// A request to settle that surely never reached the peer is reported, so
// the node asks again at the next check rather than a minute later: lost
// deliveries do not use up the grace before a peer short of room is
// withdrawn (spec/areas/network.md#advertised-peers). One the peer refused,
// as malformed (400) or while overloaded (503), is not: asked again at
// once, an overloaded peer would be asked at every check.
#[test]
fn only_a_settlement_request_that_never_arrived_is_asked_again_at_once() {
    let (mut network, a, _) = peered(52);
    let again = |network: &Network| {
        network
            .events
            .iter()
            .filter(|(at, event)| {
                *at == 1
                    && matches!(
                        event,
                        Event::SettlementRequestNotDelivered { peer }
                            | Event::SettlementRequestRefused { peer } if *peer == a
                    )
            })
            .map(|(_, event)| event.clone())
            .collect::<Vec<_>>()
    };
    for status in [400, 503] {
        network.refuse_settlement_requests.insert(0, status);
        network.act(1, |node, now| {
            node.request_settlement(now, &a, stub()).unwrap();
        });
        settle(&mut network, 1_000);
        assert_eq!(again(&network), [], "asked again at once after {status}");
    }
    assert!(
        network.settlement_requests.is_empty(),
        "taken while refused"
    );
    network.refuse_settlement_requests.clear();
    network.cut.insert((0, 1));
    network.act(1, |node, now| {
        node.request_settlement(now, &a, stub()).unwrap();
    });
    settle(&mut network, 1_000);
    assert_eq!(
        again(&network),
        [Event::SettlementRequestNotDelivered { peer: a }],
        "a request that never arrived was not reported"
    );
}

// A node wakes when something is due, not every second: with nothing to
// refresh and the peers it wants, it waits for its heartbeats, the
// anti-entropy sweep (every 10 s), its list polls and its route and bridge
// checks.
#[test]
fn an_idle_node_does_not_wake_every_second() {
    let mut network = Network::new(44, 3, 1, Config::DEFAULT);
    settle(&mut network, 3_600_000);
    assert!(
        network.node(0).peers().next().is_some(),
        "node 0 never peered"
    );
    let end = network.now + 3_600_000;
    let mut longest = 0;
    while network.now < end {
        let at = network.node(0).poll_timeout().unwrap();
        longest = longest.max(at.saturating_sub(network.now));
        network.run_until(at.max(network.now + 1).min(end));
    }
    assert!(longest >= 5_000, "node 0 slept {longest} ms at most");
}

// Review of #127: a withdrawn peer heard from again, its hold-down over, is
// listed at once, not at the next sweep or heartbeat (production timings,
// where those are 10 s and more apart).
#[test]
fn a_peer_heard_again_after_its_hold_down_is_listed_at_once() {
    let mut network = Network::new(45, 2, 1, Config::DEFAULT);
    settle(&mut network, 3_600_000);
    let b = *network.node(1).own();
    assert!(network.node(0).is_advertised(&b), "the nodes did not peer");
    let view = |network: &Network| {
        network
            .node(0)
            .nodes()
            .into_iter()
            .find(|view| view.id == b)
            .unwrap()
    };
    // Withdrawn after a minute of silence; its 30 s hold-down ends while it
    // is still cut off.
    network.cut.insert((0, 1));
    network.cut.insert((1, 0));
    settle(&mut network, 120_000);
    assert!(!view(&network).advertised, "not withdrawn");
    network.cut.clear();
    while !view(&network).online {
        settle(&mut network, 10);
    }
    settle(&mut network, 1);
    assert!(view(&network).advertised, "heard from, and not listed");
}

/// What node `index` reported, in order, of ending its peering with `peer`.
fn ending(network: &Network, index: usize, peer: &NodeId) -> Vec<&'static str> {
    network
        .events
        .iter()
        .filter(|(at, _)| *at == index)
        .filter_map(|(_, event)| match event {
            Event::Depeering { peer: p, .. } if p == peer => Some("depeering"),
            Event::SettlementRequestAnswered { peer: p, .. } if p == peer => Some("answered"),
            Event::Terminating { peer: p, .. } if p == peer => Some("terminating"),
            Event::PeeringEnded { peer: p } if p == peer => Some("ended"),
            _ => None,
        })
        .collect()
}

// A node ending a peering collects first, privately (D28): it asks the
// peer to settle once, and tells it only once the answer is in. Until
// then the peer sees nothing change.
#[test]
fn a_node_ending_a_peering_collects_before_it_tells() {
    let (mut network, a, b) = peered(52);
    network.act(0, |node, now| {
        assert!(node.terminate(now, &b, TerminationReason::Owner));
    });
    assert!(network.node(0).is_advertised(&b));
    assert!(network.node(1).peering_of(&a).unwrap().depeering.is_none());
    // Its owner sees it ending; the peer, nothing.
    let status = |network: &Network, index: usize, peer: &NodeId| {
        let views = network.node(index).nodes();
        views
            .into_iter()
            .find(|view| view.id == *peer)
            .unwrap()
            .peer_status
    };
    assert_eq!(status(&network, 0, &b), PeerStatus::Terminating);
    assert_eq!(status(&network, 1, &a), PeerStatus::Peered);
    settle(&mut network, 10_000);
    assert_eq!(
        ending(&network, 0, &b),
        ["depeering", "answered", "terminating", "ended"]
    );
    assert_eq!(ended(&network, 1, &a), 1);
}

// It first waits for its own settlements to the peer under way, so the
// peer counts them; for at most its wait.
#[test]
fn a_node_ending_a_peering_waits_for_its_own_settlements() {
    let (mut network, _, b) = peered(53);
    network.nodes[0].hooks.settling.insert(b);
    network.act(0, |node, now| {
        node.terminate(now, &b, TerminationReason::Owner);
    });
    settle(&mut network, 5_000);
    assert_eq!(ending(&network, 0, &b), ["depeering"]);
    network.nodes[0].hooks.settling.clear();
    settle(&mut network, 5_000);
    assert_eq!(ended(&network, 0, &b), 1);
    // One that never finishes is waited for no longer than the wait.
    let (mut network, _, b) = peered(54);
    network.nodes[0].hooks.settling.insert(b);
    network.act(0, |node, now| {
        node.terminate(now, &b, TerminationReason::Owner);
    });
    settle(&mut network, CONFIG.depeer_wait_ms + 5_000);
    assert_eq!(ended(&network, 0, &b), 1);
}

// It then waits until what the peer said is on its way is booked, so no
// settlement crosses the end; or for at most its wait.
#[test]
fn a_node_ending_a_peering_waits_for_what_the_peer_has_on_its_way() {
    let (mut network, _, b) = peered(55);
    network.promise.insert(1, 5);
    network.act(0, |node, now| {
        node.terminate(now, &b, TerminationReason::Owner);
    });
    settle(&mut network, 5_000);
    assert_eq!(ending(&network, 0, &b), ["depeering", "answered"]);
    network.act(0, |node, now| node.settlement_booked(now, &b, 5));
    settle(&mut network, 5_000);
    assert_eq!(ended(&network, 0, &b), 1);
    let (mut network, _, b) = peered(56);
    network.promise.insert(1, 5);
    network.act(0, |node, now| {
        node.terminate(now, &b, TerminationReason::Owner);
    });
    settle(&mut network, CONFIG.depeer_wait_ms + 5_000);
    assert_eq!(ended(&network, 0, &b), 1);
}

// While it collects, the peer's own request to settle is answered as one
// that settles nothing, and goes no further (D28).
#[test]
fn a_node_ending_a_peering_settles_nothing_the_peer_asks() {
    let (mut network, a, b) = peered(57);
    network.nodes[0].hooks.settling.insert(b);
    network.act(0, |node, now| {
        node.terminate(now, &b, TerminationReason::Owner);
    });
    network.act(1, |node, now| {
        node.request_settlement(now, &a, stub()).unwrap();
    });
    settle(&mut network, 1_000);
    let nothing = Event::SettlementRequestAnswered {
        peer: a,
        settling: Some(0),
    };
    assert!(network.events.contains(&(1, nothing)));
    assert!(!network.settlement_requests.contains(&(0, b)));
}

// If the peer tells it first, the node is the one being ended: it drops
// its own steps and ends the peering at once.
#[test]
fn a_node_told_first_drops_its_own_steps() {
    let (mut network, a, b) = peered(58);
    network.nodes[0].hooks.settling.insert(b);
    network.act(0, |node, now| {
        node.terminate(now, &b, TerminationReason::Owner);
    });
    network.act(1, |node, now| {
        node.terminate(now, &a, TerminationReason::Owner);
    });
    settle(&mut network, 5_000);
    assert_eq!(ended(&network, 0, &b), 1);
    assert_eq!(ended(&network, 1, &a), 1);
    let told = network
        .events
        .iter()
        .any(|(at, event)| *at == 0 && matches!(event, Event::Terminating { by_us: false, .. }));
    assert!(told, "node 0 was told, before its own steps were done");
}

// Found in review: told by the peer, for its own reason, while we collect
// for our owner, the end is still the owner's, and so is its cooldown: we
// refuse the peer's request to peer again (D28).
#[test]
fn an_owners_end_keeps_its_cooldown_when_the_peer_tells_first() {
    let (mut network, a, b) = peered(59);
    network.nodes[0].hooks.settling.insert(b);
    network.act(0, |node, now| {
        node.terminate(now, &b, TerminationReason::Owner);
    });
    network.act(1, |node, now| {
        node.terminate(now, &a, TerminationReason::ShortOfRoom);
    });
    settle(&mut network, 5_000);
    assert_eq!(ended(&network, 0, &b), 1);
    let body = peering_request(&network, &key(1), 1, 0);
    let reply = network.inbound(0, &body);
    assert_eq!(refusal(&reply), RefusalReason::Terminating);
}

// An owner's decision outranks one the node took for silence or room, and
// brings its cooldown.
#[test]
fn an_owners_decision_outranks_the_nodes_own() {
    let (mut network, _, b) = peered(60);
    network.nodes[0].hooks.settling.insert(b);
    network.act(0, |node, now| {
        node.terminate(now, &b, TerminationReason::ShortOfRoom);
        node.terminate(now, &b, TerminationReason::Owner);
    });
    let depeering = network.node(0).peering_of(&b).unwrap().depeering.unwrap();
    assert_eq!(depeering.reason, TerminationReason::Owner);
}

// A ban is the owner's emphatic word: the node withdraws the peer from its
// link state at once and tells it, without waiting for its own settlements
// or asking the peer to settle; what the peer owes stays on the books
// (D28).
#[test]
fn a_ban_withdraws_the_peer_at_once_and_collects_nothing() {
    let (mut network, a, b) = peered(61);
    network.nodes[0].hooks.settling.insert(b);
    let asked = network.settlement_requests.len();
    network.act(0, |node, now| node.ban(now, b).unwrap());
    assert!(!network.node(0).is_advertised(&b), "withdrawn at once");
    assert_eq!(ending(&network, 0, &b), ["terminating"]);
    settle(&mut network, 5_000);
    assert_eq!(ending(&network, 0, &b), ["terminating", "ended"]);
    assert_eq!(network.settlement_requests.len(), asked, "nothing asked");
    let told = network.events.iter().any(|(at, event)| {
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
    assert!(told, "the peer is told the owner ended it");
    assert_eq!(ended(&network, 1, &a), 1);
}

// A ban while the node collects for its owner's own end stops collecting:
// the peer is withdrawn and told at once.
#[test]
fn a_ban_while_collecting_ends_the_peering_at_once() {
    let (mut network, a, b) = peered(62);
    network.nodes[0].hooks.settling.insert(b);
    network.act(0, |node, now| {
        node.terminate(now, &b, TerminationReason::Owner);
    });
    assert!(network.node(0).is_advertised(&b), "still private");
    network.act(0, |node, now| node.ban(now, b).unwrap());
    assert!(!network.node(0).is_advertised(&b));
    assert_eq!(ending(&network, 0, &b), ["depeering", "terminating"]);
    settle(&mut network, 5_000);
    assert_eq!(ended(&network, 0, &b), 1);
    assert!(
        !network.settlement_requests.contains(&(1, a)),
        "nothing asked"
    );
}
