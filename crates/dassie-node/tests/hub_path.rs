//! The packet path over a settlement hub (spec/areas/hubs.md#in-the-node),
//! through real engine cores in the node simulation: the nodes settle over
//! the stub with commitments, which signs and takes promises per packet as a
//! hub's member does, without a hub (spec/areas/settlement.md#the-stub-scheme).
//! A failure names its seed: `dassie-sim run node-stub-hub <seed>` replays a
//! scripted run, and the campaign runs it from random seeds.
//!
//! After every step, the simulation checks that no node's stub holds a
//! packet its connector let go, and that the credit each node extends over
//! the hub is what the holds call for (`Sim::watch_hub`): the targeted tests
//! below set up the situations where a packet was held for good, and each
//! checks the run with it.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use std::collections::BTreeMap;

use dassie_connector::{EndpointId, PacketRecord, PacketState};
use dassie_ilp::ErrorCode;
use dassie_ledger::Name;
use dassie_node::engine::HubPeer;
use dassie_node::sim::checks::{check, check_hub_credit, check_hub_packets};
use dassie_node::sim::{START, Settling, Sim, key, received, scripted};
use dassie_kit::sim::assert_passes;
use dassie_settlement::Held;

#[test]
fn simulated_networks_pay_over_a_stub_with_commitments() {
    assert_passes(0, scripted::run_stub_hub);
}

/// `count` nodes over the stub with commitments, room for `limit` packets
/// per peer each way, losing no message. The first node is the only
/// bootstrap node.
fn hub_network(seed: u64, count: usize, limit: usize) -> Sim {
    let mut sim = Sim::with(seed, count, 1, BTreeMap::new(), Settling::StubHub(limit));
    sim.loss_per_mille = 0;
    sim
}

/// Runs until every node has a peer, at most a minute.
fn peered(sim: &mut Sim) {
    let all = |sim: &Sim| (0..sim.nodes.len()).all(|index| !sim.peers(index).is_empty());
    assert!(
        sim.run_until_or(START + 60_000, all),
        "the nodes did not peer"
    );
}

/// Three nodes over the stub with commitments in a line: node 0 and node 2
/// ban each other at once, so they never peer and what one pays the other
/// goes through node 1. Node 1's stub has room for one packet per peer
/// each way, and node 0's for `payer`. Runs until node 0 has a route to
/// node 2, at most ten minutes, as a network takes to be ready.
fn line(seed: u64, payer: usize) -> Sim {
    let mut sim = hub_network(seed, 3, 1);
    let (zero, two) = (key(0).node_id(), key(2).node_id());
    sim.step(0, |core| {
        core.ban_node(two).unwrap();
        core.remove_scheme("stub").unwrap();
        core.add_scheme("stub", format!(r#"{{"commitments":{payer}}}"#))
            .unwrap();
    });
    sim.step(2, |core| core.ban_node(zero).unwrap());
    let routed = |sim: &Sim| {
        sim.core(0)
            .network()
            .nodes
            .iter()
            .any(|node| node.id == two && !node.first_hops.is_empty())
    };
    assert!(
        sim.run_until_or(START + 600_000, routed),
        "no route from node 0 to node 2"
    );
    sim
}

/// Node `index`'s peerings over the stub with commitments.
fn hub_peers(sim: &mut Sim, index: usize) -> Vec<HubPeer> {
    sim.nodes[index].core.as_mut().unwrap().hub_peers()
}

/// What node `index`'s stub holds of each peer's packets, for each peer it
/// holds any of.
fn held(sim: &mut Sim, index: usize) -> Vec<(Name, Held)> {
    hub_peers(sim, index)
        .into_iter()
        .filter(|peer| peer.held != Held::default())
        .map(|peer| (peer.peer, peer.held))
        .collect()
}

/// Node `index`'s peering over the stub with commitments with node `peer`.
fn hub_peer(sim: &mut Sim, index: usize, peer: usize) -> Option<HubPeer> {
    let name = key(peer).node_id();
    hub_peers(sim, index)
        .into_iter()
        .find(|hub| hub.peer.as_str() == name.as_str())
}

/// What node `index`'s stub holds of the packets between it and node
/// `peer`.
fn held_with(sim: &mut Sim, index: usize, peer: usize) -> Held {
    hub_peer(sim, index, peer).map_or(Held::default(), |hub| hub.held)
}

/// The packets node `index` sent to a peer, as its connector traced them.
fn sent_to_peers(sim: &Sim, index: usize) -> Vec<PacketRecord> {
    sim.core(index)
        .packets(None, 100_000)
        .into_iter()
        .filter(|record| matches!(record.destination, Some(EndpointId::Peer(_))))
        .collect()
}

/// Whether node `index` traced a packet to a peer that ended `state`.
fn ended(sim: &Sim, index: usize, state: PacketState) -> bool {
    sent_to_peers(sim, index)
        .iter()
        .any(|record| record.state == state)
}

/// Starts a payment of half a unit from a client of node `from` to node
/// `to`'s receiver, and runs until it knows its rate.
fn paying(sim: &mut Sim, from: usize, to: usize) {
    assert!(sim.pay(from, to, 500_000_000, None, None));
    let index = sim.payments.len() - 1;
    let rate = move |sim: &Sim| sim.payments[index].connection.exchange_rate().is_some();
    assert!(sim.run_until_or(sim.now + 30_000, rate), "no rate");
}

/// Runs until every payment is done, at most `ms`.
fn paid(sim: &mut Sim, ms: u64) {
    let done = |sim: &Sim| sim.payments.iter().all(|payment| payment.done);
    assert!(
        sim.run_until_or(sim.now + ms, done),
        "the payments did not finish"
    );
}

// spec/areas/hubs.md#in-the-node: a Prepare to a peer over a hub goes with
// the scheme's commitment, which the peer takes before it forwards; the
// payer's reply to the Fulfill carries its claim, which the payee takes;
// a Reject, such as those that answer the client's probes of the rate,
// makes the payer forget what it kept. Once the payment is done, neither
// stub holds anything of the other's packets.
#[test]
fn a_payment_over_a_hub_resolves_every_commitment() {
    let mut sim = hub_network(1, 2, 1);
    peered(&mut sim);
    paying(&mut sim, 0, 1);
    paid(&mut sim, 120_000);
    assert_eq!(received(&sim, &sim.payments[0]), 500_000_000);
    assert!(sim.counts["prepares over a hub"] > 0);
    assert!(ended(&sim, 0, PacketState::Fulfilled));
    assert!(sent_to_peers(&sim, 0).iter().any(|record| matches!(
        record.state,
        PacketState::Rejected(code) if code != ErrorCode::T04_INSUFFICIENT_LIQUIDITY
    )));
    let claims = format!(" 1 PeerNode(FulfillReply {{ peer: {:?}", key(0).node_id());
    assert!(sim.trace.iter().any(|line| line.contains(&claims)));
    sim.run_until(sim.now + 10_000);
    assert_eq!(held(&mut sim, 0), []);
    assert_eq!(held(&mut sim, 1), []);
    check(&sim).unwrap();
}

// spec/areas/hubs.md#in-the-node: a Prepare from a peer over a hub goes on
// only if its commitment covers it, and is refused with T04 otherwise, as
// one beyond a peer's credit is. Node 1 forwards node 0's packets to node
// 2, and its stub takes one of them at a time, while node 0's commits to
// eight: node 1 refuses those node 0 sends while it holds one, and never
// holds more. The payment still completes.
#[test]
fn a_payee_refuses_with_t04_what_the_commitments_do_not_cover() {
    let mut sim = line(2, 8);
    // Several units, so that the client sends many packets at once.
    assert!(sim.pay(0, 2, 3_000_000_000, None, None));
    let deadline = sim.now + 600_000;
    let mut most = 0;
    while !sim.payments[0].done {
        assert!(sim.now < deadline, "the payment did not finish");
        sim.run_until(sim.now + 50);
        most = most.max(held_with(&mut sim, 1, 0).theirs);
    }
    assert_eq!(most, 1);
    assert!(ended(
        &sim,
        0,
        PacketState::Rejected(ErrorCode::T04_INSUFFICIENT_LIQUIDITY)
    ));
    // What node 1 refused for its commitments never reached its connector,
    // which traced fewer of node 0's packets than node 0 sent it.
    let taken = sim
        .core(1)
        .packets(None, 100_000)
        .iter()
        .filter(|record| matches!(record.source, EndpointId::Peer(_)))
        .count();
    assert!(taken < sent_to_peers(&sim, 0).len());
    sim.run_until(sim.now + 10_000);
    for index in 0..3 {
        assert_eq!(held(&mut sim, index), [], "node {index}");
    }
    check(&sim).unwrap();
}

// spec/areas/hubs.md#in-the-node: a packet to a peer over a hub that timed
// out waits for the hub, and is settled by what the scheme decided: the
// stub decides at once that no claim paid it, so it is rejected upstream.
// Node 1's Fulfills cannot reach node 0 for a while: node 0 forgets each
// packet once decided, and node 1 keeps each fulfillment, which no claim
// covers. Once the cut is gone, the payment completes.
#[test]
fn a_packet_that_timed_out_is_rejected_once_the_hub_decided() {
    let mut sim = hub_network(3, 2, 1);
    peered(&mut sim);
    paying(&mut sim, 0, 1);
    sim.cut.insert((1, 0));
    // The client keeps sending meanwhile, so at any moment a packet may be
    // in flight: node 0 forgets the ones decided, so once one timed out it
    // comes to hold nothing.
    let deadline = sim.now + 120_000;
    loop {
        sim.run_until(sim.now + 100);
        let timed_out = ended(
            &sim,
            0,
            PacketState::Rejected(ErrorCode::R00_TRANSFER_TIMED_OUT),
        );
        if timed_out && held_with(&mut sim, 0, 1).ours == 0 {
            break;
        }
        assert!(sim.now < deadline, "node 0 kept what the hub decided");
    }
    assert!(held_with(&mut sim, 1, 0).unclaimed > 0);
    sim.cut.clear();
    paid(&mut sim, 180_000);
    check(&sim).unwrap();
}

// Found in review: while a peering was being ended, the hub's hooks did
// nothing, so a packet in flight on it was never resolved in the scheme.
// Node 0 bans node 1 while node 1's Fulfill of its Prepare is on its way:
// the peering is ending at once, and the Fulfill still comes back with node
// 0's claim, which node 1 takes. Neither stub keeps anything of the packet.
#[test]
fn a_ban_while_a_fulfill_is_on_its_way_still_resolves_it_over_the_hub() {
    let mut sim = hub_network(4, 2, 1);
    peered(&mut sim);
    paying(&mut sim, 0, 1);
    let took = |sim: &Sim| received(sim, &sim.payments[0]) > 0;
    assert!(sim.run_until_or(sim.now + 30_000, took), "nothing received");
    assert_eq!(held_with(&mut sim, 0, 1).ours, 1);
    let peer = key(1).node_id();
    sim.step(0, |core| core.ban_node(peer).unwrap());
    sim.run_until(sim.now + 60_000);
    assert!(sim.peers(0).is_empty() && sim.peers(1).is_empty());
    check_hub_packets(&sim).unwrap();
    assert_eq!(held(&mut sim, 0), []);
    assert_eq!(held(&mut sim, 1), []);
}

// Found in review, the payee's side: node 1 forwards node 0's Prepare to
// node 2, and bans node 0 while node 2's Fulfill is on its way back. Node 1
// still tells its stub of the fulfillment, and takes node 0's claim for it,
// though the peering is being ended. Nodes 0 and 2 never peer, so the
// payment goes through node 1.
#[test]
fn a_peering_being_ended_still_resolves_its_packets_over_the_hub() {
    let mut sim = line(5, 1);
    paying(&mut sim, 0, 2);
    let took = |sim: &Sim| received(sim, &sim.payments[0]) > 0;
    assert!(sim.run_until_or(sim.now + 30_000, took), "nothing received");
    assert_eq!(held_with(&mut sim, 1, 0).theirs, 1);
    let zero = key(0).node_id();
    sim.step(1, |core| core.ban_node(zero).unwrap());
    sim.run_until(sim.now + 60_000);
    check_hub_packets(&sim).unwrap();
    assert_eq!(held_with(&mut sim, 1, 0), Held::default());
    assert_eq!(held_with(&mut sim, 0, 1), Held::default());
}

// Found in review: a Prepare the payer's scheme committed to that never
// reached the peer stayed in flight in the scheme for good. Node 0's stub
// commits to one packet to node 1 at a time, and node 0's requests to node
// 1 do not get through for a while: each Prepare surely did not arrive, and
// is failed at once. The scheme forgets each, so once the cut is gone the
// next Prepare goes out, and the payment completes.
#[test]
fn a_prepare_that_never_reached_the_peer_frees_its_place() {
    let mut sim = hub_network(6, 2, 1);
    peered(&mut sim);
    paying(&mut sim, 0, 1);
    sim.cut.insert((0, 1));
    sim.run_until(sim.now + 10_000);
    assert!(ended(
        &sim,
        0,
        PacketState::Rejected(ErrorCode::T01_PEER_UNREACHABLE)
    ));
    check_hub_packets(&sim).unwrap();
    let hub = hub_peer(&mut sim, 0, 1).unwrap();
    assert!(hub.held.ours <= hub.pending_to);
    sim.cut.clear();
    paid(&mut sim, 180_000);
    check(&sim).unwrap();
}

// spec/areas/hubs.md#in-the-node: the credit a node extends to a peer over a
// hub is its hold toward the peer, with its settlements to it on their way.
// Node 1 asks node 0 to settle at once, since it extends nothing: node 0
// funds its hold toward node 1, raising its credit by as much, and node 1
// books the funding as node 0's settlement. Each packet node 1 is paid is
// taken from node 0's hold, which node 0's credit follows down, until node
// 1 asks again. The credit is what the holds call for after every step
// (`Sim::watch_hub`).
#[test]
fn the_credit_over_a_hub_follows_the_holds() {
    let mut sim = hub_network(7, 2, 1);
    peered(&mut sim);
    let books = |sim: &mut Sim, index: usize, peer: usize| {
        let hub = hub_peer(sim, index, peer).unwrap();
        (hub.credit.unwrap(), hub.holds, hub.settling)
    };
    let funded = |sim: &Sim| {
        let id = key(1).node_id();
        sim.core(0)
            .network()
            .peers
            .iter()
            .any(|peer| peer.id == id && peer.peering.credit_out > 0)
    };
    assert!(sim.run_until_or(sim.now + 120_000, funded), "no hold");
    let (credit, holds, settling) = books(&mut sim, 0, 1);
    assert!(holds.toward > 0, "node 0 holds nothing toward node 1");
    assert_eq!(credit, holds.toward + settling);
    // Node 1 books the funding once node 0's settlement arrives.
    let deadline = sim.now + 30_000;
    while books(&mut sim, 1, 0).1.from == 0 {
        assert!(sim.now < deadline, "node 1 never heard of the hold");
        sim.run_until(sim.now + 100);
    }
    assert_eq!(books(&mut sim, 1, 0).1.from, holds.toward);
    paying(&mut sim, 0, 1);
    paid(&mut sim, 120_000);
    sim.run_until(sim.now + 120_000);
    let (credit, holds, settling) = books(&mut sim, 0, 1);
    assert_eq!(credit, holds.toward + settling);
    // Node 0's hold is what it funded, less what node 1 was paid from it.
    // Node 0 asks node 1 to settle too, so node 1 funds a hold toward node
    // 0, which node 0's stub holds besides: what node 0 funded is what left
    // the stub, with what came in for that hold.
    let funded = 100_000_000_000 + sim.balances(0)["assets/settlement"] + i128::from(holds.from);
    assert_eq!(i128::from(holds.toward), funded - 500_000_000);
    check_hub_credit(&sim).unwrap();
    check(&sim).unwrap();
}

// A payment over hubs crosses hops in full: each payer funds its hold
// toward the next hop when asked, and is paid from it.
#[test]
fn a_payment_over_hubs_is_paid_in_full_across_hops() {
    let mut sim = line(8, 1);
    assert!(sim.pay(0, 2, 3_000_000_000, None, None));
    paid(&mut sim, 600_000);
    assert_eq!(received(&sim, &sim.payments[0]), 3_000_000_000);
    for (index, peer) in [(0, 1), (1, 2)] {
        let hub = hub_peer(&mut sim, index, peer).unwrap();
        assert!(
            hub.holds.toward > 0,
            "node {index} holds nothing toward {peer}"
        );
    }
    check_hub_credit(&sim).unwrap();
    check(&sim).unwrap();
}
