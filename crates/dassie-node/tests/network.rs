//! A simulated network of real engine cores
//! (spec/areas/testing.md#deterministic-simulation): `dassie_node::sim` is the
//! harness, and `dassie_node::sim::scripted` the runs these tests make on fixed
//! seeds. A failure names its seed: `dassie-sim run node-stub <seed>` replays it,
//! `dassie-sim explain node-stub <seed>` prints its trace, and `dassie-sim run
//! node-stub <from>..<to>` runs others (D27); so do `node-xrpl` and
//! `node-currencies`.
//!
//! The targeted tests below set up one situation each: a hop that crashes
//! after posting, a late Fulfill, lying peers, a junior paying its senior
//! in advance, credit growing with fees.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use std::collections::BTreeMap;

use dassie_identity::NodeId;
use dassie_ledger::LedgerId;
use dassie_node::engine::{EngineEvent, Lie, NewIncoming, NewOutgoing};
use dassie_node::node::Policies;
use dassie_node::sim::checks::{check, check_across, check_books, check_events};
use dassie_node::sim::{
    CREDIT, START, Settling, Sim, USD, XRP, key, received, scripted, sent, xrpl_account,
};
use dassie_node::store::{Aim, ForeignAmount};
use dassie_node_core::testing::assert_seeds_pass;
use dassie_settlement::LedgerStatus;

#[test]
fn simulated_networks_pay_across_restarts() {
    assert_seeds_pass(0..6, scripted::run);
}

// M7b: the same runs, settling on the XRP Ledger testnet scheme against
// the fake ledger.
#[test]
fn simulated_networks_settle_on_the_xrp_ledger() {
    assert_seeds_pass(0..6, scripted::run_xrpl);
}

#[test]
fn simulated_networks_pay_across_currencies() {
    assert_seeds_pass(0..6, scripted::run_across);
}

// On this seed, a node ends a peering just after its books took the peer's
// settlement from the XRP Ledger, before the settlement's message arrived.
// The peer's last answer names that settlement, which the node does not
// count, so it waits the whole time before it tells the peer, as D28 lets
// it.
#[test]
fn a_peering_ended_after_its_last_settlement_was_booked_ends_after_the_wait() {
    let run = scripted::run_xrpl(7_000_002_237);
    assert!(!run.outcome.is_failure(), "{}", run.outcome);
}

#[test]
fn a_seed_replays_the_same_run() {
    assert_eq!(scripted::run(2).trace, scripted::run(2).trace);
}

/// Two nodes, peered, and a client of node 0 paying node 1's receiver,
/// run until node 1's receiver took money: it posted a packet, and its
/// Fulfill is on its way. `before` is done to the network once the
/// payment knows its rate, before any money moves.
fn paying(seed: u64, before: impl FnOnce(&mut Sim)) -> Sim {
    let mut sim = Sim::new(seed, 2, 1);
    sim.loss_per_mille = 0;
    sim.run_until(START + 60_000);
    assert!(!sim.peers(0).is_empty(), "the nodes did not peer");
    sim.pay(0, 1, 500_000_000, None, None);
    let rate = |sim: &Sim| sim.payments[0].connection.exchange_rate().is_some();
    assert!(sim.run_until_or(sim.now + 30_000, rate), "no rate");
    before(&mut sim);
    let took = |sim: &Sim| received(sim, &sim.payments[0]) > 0;
    assert!(sim.run_until_or(sim.now + 30_000, took), "nothing received");
    sim
}

// D23: the receiver's node crashes once it posted, before its Fulfill
// arrives. What it booked stays, and the payment still completes.
#[test]
fn a_hop_that_crashes_after_posting_keeps_its_books() {
    let mut sim = paying(70, |_| {});
    sim.crash(1);
    sim.run_until(sim.now + 2_000);
    sim.start(1);
    sim.run_until(sim.now + 120_000);
    check(&sim).unwrap();
}

// D23: the sender's node crashes before the Fulfill reaches it, and
// forgets the packet. The receiver's node keeps what it booked; each
// node's books still hold against what its own clients moved.
#[test]
fn an_upstream_that_forgot_the_packet_changes_nothing_downstream() {
    let mut sim = paying(71, |_| {});
    let before = received(&sim, &sim.payments[0]);
    sim.crash(0);
    sim.run_until(sim.now + 3_000);
    sim.start(0);
    sim.run_until(sim.now + 60_000);
    check_books(&sim).unwrap();
    assert!(received(&sim, &sim.abandoned[0]) >= before);
}

// D23: a Fulfill that cannot get through before the upstream's expiry
// counts in the downstream's books, and not in the upstream's, which
// expired the packet. Neither is wrong by its own books, and the payment
// completes.
#[test]
fn a_late_fulfill_counts_downstream_only() {
    let mut sim = paying(72, |sim| {
        sim.cut.insert((1, 0));
    });
    sim.run_until(sim.now + 40_000);
    sim.cut.clear();
    sim.run_until(sim.now + 120_000);
    check(&sim).unwrap();
}

// D23: a peering being ended waits only for the packets in flight, not
// for Fulfills that cannot get through.
#[test]
fn a_peering_ends_while_its_fulfills_cannot_arrive() {
    let mut sim = paying(73, |sim| {
        sim.cut.insert((1, 0));
    });
    let peer = key(1).node_id();
    sim.step(0, |core| core.terminate_peering(peer).unwrap());
    sim.run_until(sim.now + 60_000);
    assert!(sim.peers(1).is_empty() && sim.peers(0).is_empty());
    sim.cut.clear();
    sim.run_until(sim.now + 60_000);
    check_books(&sim).unwrap();
}

/// Whether node `index` stopped.
fn stopped(index: usize) -> impl Fn(&Sim) -> bool {
    move |sim: &Sim| sim.nodes[index].core.is_none()
}

// D27: a node whose commit fails stops before anything of the step goes
// out. The receiver's node fails the commit that books its first Fulfill:
// the client hears of no Fulfill and its receiver kept nothing. The
// sender's node had the Prepare answered with the 503 of an engine that
// stopped, which says it had no effect. Once the node is back, the
// payment completes.
#[test]
fn a_receiver_whose_commit_fails_sends_no_fulfill() {
    let mut sim = Sim::new(75, 2, 1);
    sim.loss_per_mille = 0;
    sim.run_until(START + 60_000);
    assert!(!sim.peers(0).is_empty(), "the nodes did not peer");
    sim.pay(0, 1, 500_000_000, None, None);
    sim.fail_commit(1, Aim::Packet, 0);
    assert!(sim.run_until_or(sim.now + 30_000, stopped(1)), "no failure");
    assert_eq!(received(&sim, &sim.payments[0]), 0);
    assert_eq!(sent(&sim.payments[0]), 0);
    assert_eq!(sim.counts["failed commits mid-payment"], 1);
    sim.run_until(sim.now + 5_000);
    sim.start(1);
    sim.run_until(sim.now + 120_000);
    assert!(sim.payments[0].done, "the payment did not finish");
    check(&sim).unwrap();
}

/// Two nodes, peered, the junior paying its senior three units: it settles
/// on the way, and the commit after the next `skip` of `node`'s that
/// holds a settlement fails, `node` picking it from the senior and the
/// junior. It is back a few seconds after it stopped.
fn settling(node: impl FnOnce(usize, usize) -> usize, skip: u32) -> (Sim, usize) {
    let mut sim = Sim::new(3, 2, 1);
    sim.loss_per_mille = 0;
    sim.run_until(START + 60_000);
    let senior = usize::from(!sim.core(0).network().peers[0].peering.inbound);
    let (junior, index) = (1 - senior, node(senior, 1 - senior));
    sim.pay(junior, senior, 3_000_000_000, None, None);
    sim.fail_commit(index, Aim::Settlement, skip);
    let failed = sim.run_until_or(sim.now + 120_000, stopped(index));
    assert!(failed, "no failure");
    assert_eq!(sim.counts["failed commits mid-settlement"], 1);
    sim.run_until(sim.now + 5_000);
    sim.start(index);
    sim.run_until(sim.now + 180_000);
    (sim, index)
}

// D27: the senior fails the commit that books the junior's settlement.
// The junior's message gets the 503 of an engine that stopped, and the
// junior sends it again once the senior is back: it is booked once, and
// the payment completes.
#[test]
fn a_settlement_whose_booking_failed_is_booked_when_sent_again() {
    let (sim, senior) = settling(|senior, _| senior, 0);
    // Where `text` is first traced after line `from`.
    let after = |from: usize, text: &str| {
        from + sim.trace[from..]
            .iter()
            .position(|line| line.contains(text))
            .unwrap_or_else(|| panic!("no {text:?}"))
    };
    let failed = after(0, &format!(" {senior} failed a commit"));
    let back = after(failed, &format!("start {senior}"));
    // The junior's first settlement was paid in advance; its second is
    // the one whose booking failed, delivered once the senior is back.
    let delivered = after(back, "SettlementDelivered");
    assert!(sim.trace[delivered].contains("sequence: 2 }"));
    assert!(
        !sim.trace[..back]
            .iter()
            .any(|line| line.contains("sequence: 2 }"))
    );
    assert!(sim.payments[0].done, "the payment did not finish");
    check(&sim).unwrap();
}

// D27: the junior fails the commit of its settlement as it prepares it,
// and, the second time, once it executed it: neither its transfer nor its
// message goes out from what never reached the disk, and the books hold
// after its restart.
#[test]
fn a_settlement_whose_commit_failed_leaves_the_books_holding() {
    for skip in [0, 1] {
        let (sim, _) = settling(|_, junior| junior, skip);
        // The second time, the commit before put the settlement's transfer
        // on disk, pending, and the scheme reconciles it at the restart:
        // the stub never moved what the lost commit said it did.
        let reconciled = sim.trace.iter().any(|line| line.contains("Reconciling"));
        assert_eq!(reconciled, skip == 1, "skip {skip}");
        check(&sim).unwrap_or_else(|problem| panic!("skip {skip}: {problem}"));
    }
}

// D28: a ban withdraws the peer from the node's link state in the step
// that bans it, and ends the peering without asking the peer to settle a
// last time. What the peer owes stays on the books.
#[test]
fn a_ban_withdraws_the_peer_at_once_and_asks_nothing() {
    let mut sim = Sim::new(74, 2, 1);
    sim.loss_per_mille = 0;
    sim.run_until(START + 60_000);
    let peer = key(1).node_id();
    assert!(sim.advertises(0, &peer), "the nodes did not peer");
    sim.pay(1, 0, 500_000_000, None, None);
    let took = |sim: &Sim| received(sim, &sim.payments[0]) > 0;
    assert!(sim.run_until_or(sim.now + 30_000, took), "nothing received");
    let mark = sim.trace.len();
    sim.step(0, |core| core.ban_node(peer).unwrap());
    assert!(!sim.advertises(0, &peer), "still advertised");
    sim.run_until(sim.now + 60_000);
    assert!(sim.peers(0).is_empty() && sim.peers(1).is_empty());
    let lines = &sim.trace[mark..];
    let collecting = format!(" 0 PeerNode(Depeering {{ peer: {peer:?}");
    let answered = format!(" 0 PeerNode(SettlementRequestAnswered {{ peer: {peer:?}");
    let told = format!(" 0 PeerNode(Terminating {{ peer: {peer:?}, reason: Owner");
    assert!(!lines.iter().any(|line| line.contains(&collecting)));
    assert!(!lines.iter().any(|line| line.contains(&answered)));
    assert!(lines.iter().any(|line| line.contains(&told)));
    check_books(&sim).unwrap();
}

// Found in review: a receiver's details may come from another wallet, and
// what the rows cannot hold must be refused before the engine takes it,
// or its commit fails and the engine stops.
#[test]
fn outgoing_payments_refuse_what_their_rows_cannot_hold() {
    let mut sim = Sim::new(80, 1, 1);
    sim.run_until(START + 1_000);
    let long = "g.x".repeat(400);
    let mut refusals = Vec::new();
    for destination in ["", "not an address", long.as_str()] {
        sim.step(0, |core| {
            let created = core.create_outgoing_payment(NewOutgoing {
                ledger: LedgerId::StubUsd,
                wallet_address: "https://sim.example/.well-known/pay".into(),
                client: None,
                grant: None,
                quote_id: None,
                receiver: "https://elsewhere.example/incoming-payments/1".into(),
                destination: destination.to_string(),
                shared_secret: [1; 32],
                debit_amount: 10,
                receive_amount: ForeignAmount {
                    value: 10,
                    asset_code: "USD".into(),
                    asset_scale: 9,
                },
                metadata: None,
                recipient: None,
                fixed_debit: false,
            });
            refusals.push(created.is_err());
        });
    }
    assert_eq!(refusals, [true, true, true]);
    assert!(sim.nodes[0].core.is_some(), "the engine kept running");
}

// Each outgoing payment has a STREAM connection of its own: a second one
// with the credentials of the first is refused, while the first sends and
// after, since the receiver would tell each what both delivered.
#[test]
fn outgoing_payments_never_share_credentials() {
    let mut sim = Sim::new(81, 1, 1);
    sim.run_until(START + 1_000);
    let ledger = sim.settling.ledger();
    let mut incoming = None;
    sim.step(0, |core| {
        incoming = Some(
            core.create_incoming_payment(NewIncoming {
                ledger,
                wallet_address: "https://sim.example/.well-known/pay".into(),
                client: None,
                incoming_amount: Some(20),
                expires_at_ms: None,
                metadata: None,
            })
            .unwrap(),
        );
    });
    let incoming = incoming.unwrap();
    let pay = |destination: &str, shared_secret| NewOutgoing {
        ledger,
        wallet_address: "https://sim.example/.well-known/pay".into(),
        client: None,
        grant: None,
        quote_id: None,
        receiver: format!("https://n0.test/incoming-payments/{}", incoming.id),
        destination: destination.to_string(),
        shared_secret,
        debit_amount: 10,
        receive_amount: ForeignAmount {
            value: 10,
            asset_code: ledger.asset().code.into(),
            asset_scale: 9,
        },
        metadata: None,
        recipient: None,
        fixed_debit: false,
    };
    let mut fresh = None;
    sim.step(0, |core| {
        fresh = Some(core.incoming_credentials(incoming.id, ledger).unwrap());
    });
    let fresh = fresh.unwrap();
    assert_ne!(fresh.destination, incoming.ilp_address);
    let mut created = Vec::new();
    for (destination, secret) in [
        (incoming.ilp_address.as_str(), incoming.shared_secret),
        (incoming.ilp_address.as_str(), incoming.shared_secret),
        (fresh.destination.as_str(), fresh.shared_secret),
    ] {
        sim.step(0, |core| {
            created.push(
                core.create_outgoing_payment(pay(destination, secret))
                    .is_ok(),
            );
        });
    }
    assert_eq!(created, [true, false, true]);
    // Once the first is done too.
    sim.run_until(START + 120_000);
    sim.step(0, |core| {
        let again =
            core.create_outgoing_payment(pay(&incoming.ilp_address, incoming.shared_secret));
        assert!(again.is_err());
    });
}

/// Three nodes where the liar, node 1, reaches node 2 only through node
/// 0: node 2 bans it before they can peer. Node 1 then lies.
fn liar_network(seed: u64, lie: Lie) -> Sim {
    let mut sim = Sim::new(seed, 3, 1);
    sim.loss_per_mille = 0;
    sim.step(2, |core| core.ban_node(key(1).node_id()).unwrap());
    sim.run_until(START + 60_000);
    let (victim, target) = (key(0).node_id(), key(2).node_id());
    assert!(
        sim.peers(1).contains(&victim) && sim.peers(2).contains(&victim),
        "seed {seed}: the nodes did not peer"
    );
    assert!(
        !sim.peers(1).contains(&target),
        "seed {seed}: the ban did not hold"
    );
    sim.step(1, |core| core.lie(lie));
    sim.trace.push(format!("{} 1 lies: {lie:?}", sim.now));
    sim
}

/// What a liar tries to pay: far more than the credit both ways, so the
/// exposure bound is put to the test.
const LIAR_PAYS: u64 = 10_000_000_000;

/// What node 1 owes node 0, by node 0's books.
fn liar_owes(sim: &Sim) -> i128 {
    let position = sim.balances(0)[&format!("assets/interledger/{}", key(1).node_id())];
    CREDIT - position
}

// D23: a peer that never settles uses the credit it was given, and no
// more: past it, its packets are refused. It is not banned for it, but it
// is withdrawn once its room stays short of a packet past the grace (D12),
// and stays peered.
#[test]
fn a_peer_that_never_settles_costs_at_most_its_credit() {
    for seed in 0..20 {
        let mut sim = liar_network(seed, Lie::NeverSettle);
        sim.pay(1, 0, LIAR_PAYS, None, None);
        let spent = |sim: &Sim| liar_owes(sim) > CREDIT * 9 / 10;
        assert!(
            sim.run_until_or(sim.now + 120_000, spent),
            "seed {seed}: it never spent its credit"
        );
        sim.run_until(sim.now + 10 * 60_000);
        let owed = liar_owes(&sim);
        assert!(owed <= CREDIT, "seed {seed}: it owes {owed}");
        assert!(
            !sim.payments[0].done,
            "seed {seed}: it paid past its credit"
        );
        let liar = key(1).node_id();
        assert!(sim.core(0).network().bans.is_empty(), "seed {seed}");
        assert!(sim.peers(0).contains(&liar), "seed {seed}");
        assert!(
            !sim.advertises(0, &liar),
            "seed {seed}: node 0 still advertises it"
        );
        assert!(
            sim.withdrawn
                .iter()
                .any(|(_, by, peer)| *by == 0 && *peer == liar),
            "seed {seed}: node 0 did not withdraw it for room"
        );
    }
}

// D12: a peer whose room stays short of a packet, settling nothing, for as
// long as a silent peer is kept, is ended with reason 4: a link that stuck
// or drifted past what it survives is not saved.
#[test]
fn a_peer_short_of_room_for_a_day_is_ended() {
    // Each seed simulates a day, about 2.5 s of wall time.
    for seed in 0..3 {
        let mut sim = liar_network(seed, Lie::NeverSettle);
        sim.pay(1, 0, LIAR_PAYS, None, None);
        let spent = |sim: &Sim| liar_owes(sim) > CREDIT * 9 / 10;
        assert!(
            sim.run_until_or(sim.now + 120_000, spent),
            "seed {seed}: it never spent its credit"
        );
        let peer = Policies::DEFAULT.peer;
        sim.run_until(sim.now + peer.advertise_short_grace_ms + peer.peer_dead_ms + 10 * 60_000);
        let liar = key(1).node_id();
        let ended = format!(" 0 PeerNode(Terminating {{ peer: {liar:?}, reason: ShortOfRoom");
        assert!(
            sim.trace.iter().any(|line| line.contains(&ended)),
            "seed {seed}: node 0 did not end the peering for room"
        );
        assert!(sim.core(0).network().bans.is_empty(), "seed {seed}");
        assert!(liar_owes(&sim) <= CREDIT, "seed {seed}");
    }
}

// D19: a peer that spends the credit it was given and vanishes, as a
// retiring node may, costs the peer it spent through no more than that
// credit. That peer ends the peering once it stops answering, and the rest
// of the network loses nothing.
#[test]
fn a_peer_that_spends_its_credit_and_vanishes_costs_at_most_that_credit() {
    // Each seed simulates the day a silent peer is kept, about 2.5 s of
    // wall time.
    for seed in 0..5 {
        let mut sim = liar_network(seed, Lie::NeverSettle);
        sim.pay(1, 2, LIAR_PAYS, None, None);
        assert!(
            sim.run_until_or(sim.now + 120_000, |sim| liar_owes(sim) > CREDIT * 9 / 10),
            "seed {seed}: it never spent its credit"
        );
        sim.crash(1);
        // A peer not heard from for a day is ended.
        let dead = Policies::DEFAULT.peer.peer_dead_ms;
        sim.run_until(sim.now + dead + 10 * 60_000);
        assert!(
            !sim.peers(0).contains(&key(1).node_id()),
            "seed {seed}: node 0 still peers with it"
        );
        let owed = liar_owes(&sim);
        assert!(owed <= CREDIT, "seed {seed}: it left owing {owed}");
        assert!(sim.peers(0).contains(&key(2).node_id()), "seed {seed}");
    }
}

/// Runs until node `index`'s scheme first saw its ledger, before it
/// peered, then restarts the node with its ledger out of reach. Having seen
/// it once, the node peers (D31), but sees nothing its ledger does until
/// it is back.
fn restart_without_ledger(sim: &mut Sim, index: usize) {
    let synced = |sim: &Sim| {
        sim.core(index)
            .ledger_views()
            .iter()
            .all(|ledger| ledger.status == LedgerStatus::Synced)
    };
    assert!(sim.run_until_or(sim.now + 30_000, synced), "never synced");
    assert!(sim.peers(index).is_empty(), "peered before the restart");
    sim.crash(index);
    sim.ledger_down.insert(index);
    sim.start(index);
}

// Review of #118: a node asks no peer to settle while its scheme cannot see
// its ledger: it could not book what the peer pays, and would ask again.
// Once the ledger is back, the node asks, and books what the peer pays as
// its settlement.
#[test]
fn a_node_asks_no_one_to_settle_while_it_cannot_see_its_ledger() {
    // Node 0 cannot pay its senior in advance while its ledger is down, so
    // node 1 would withdraw it, and end the peering after a day (the next
    // test): here the grace outlasts the outage.
    let peer = dassie_peer::Config {
        advertise_short_grace_ms: 4 * 60_000,
        peer_dead_ms: 10 * 60_000,
        ..Policies::DEFAULT.peer
    };
    let mut sim = Sim::with_peer(7, 2, 1, BTreeMap::new(), Settling::Xrpl, peer);
    sim.loss_per_mille = 0;
    restart_without_ledger(&mut sim, 0);
    sim.run_until(START + 60_000);
    let (zero, one) = (key(0).node_id(), key(1).node_id());
    assert!(sim.peers(0).contains(&one), "the nodes did not peer");
    // Node 1 pays node 0 more than the credit node 0 extends to it.
    sim.pay(1, 0, 3_000_000_000, None, None);
    sim.run_until(sim.now + 60_000);
    let owes = -sim.owes(0, &one);
    assert!(owes > CREDIT * 9 / 10, "node 1 owes node 0 only {owes}");
    let settled = |sim: &Sim| {
        sim.core(1)
            .network()
            .channels
            .iter()
            .find(|(node, _)| *node == zero)
            .map_or(0, |(_, channel)| channel.settled_out)
    };
    assert_eq!(
        settled(&sim),
        0,
        "node 1 settled before node 0 could see it"
    );
    sim.ledger_down.clear();
    sim.run_until(sim.now + 180_000);
    assert!(settled(&sim) > 0, "node 1 never settled");
    assert!(sim.payments[0].done, "the payment did not finish");
    check(&sim).unwrap();
}

// D12: a peer whose room stays short of a packet past the grace, settling
// nothing, is withdrawn, and advertised again once it settles. Node 0, the
// junior, cannot pay its senior in advance while it cannot see its ledger:
// node 1 asks, and withdraws it once the grace is over. Both stay peered.
// Once the ledger is back, node 0 pays in advance, node 1 lists it again,
// and a payment goes through.
#[test]
fn a_peer_short_of_room_is_withdrawn_until_it_settles() {
    let mut sim = Sim::with(7, 2, 1, BTreeMap::new(), Settling::Xrpl);
    sim.loss_per_mille = 0;
    restart_without_ledger(&mut sim, 0);
    // Past the grace, 3 min, counted from when node 1 could first ask.
    sim.run_until(START + 4 * 60_000);
    assert!(!sim.peers(0).is_empty(), "the nodes did not peer");
    let (senior, junior) = (1, 0);
    assert!(
        !sim.core(junior).network().peers[0].peering.inbound,
        "node 0 is not the junior"
    );
    let (senior_id, junior_id) = (key(senior).node_id(), key(junior).node_id());
    assert!(
        !sim.advertises(senior, &junior_id),
        "the senior advertises a junior that has not paid in advance"
    );
    assert_eq!(
        sim.withdrawn
            .iter()
            .map(|(_, by, peer)| (*by, *peer))
            .collect::<Vec<_>>(),
        [(senior, junior_id)],
        "only the senior withdraws"
    );
    assert!(
        sim.advertises(junior, &senior_id),
        "the junior withdrew its senior"
    );
    assert!(sim.peers(senior).contains(&junior_id), "the peering ended");
    sim.ledger_down.clear();
    let back = |sim: &Sim| sim.advertises(senior, &junior_id);
    assert!(
        sim.run_until_or(sim.now + 120_000, back),
        "the senior never listed the junior again"
    );
    assert!(sim.owes(senior, &junior_id) > 0, "not paid in advance");
    sim.pay(junior, senior, 200_000_000, None, None);
    sim.run_until(sim.now + 60_000);
    assert!(sim.payments[0].done, "the payment did not finish");
    check(&sim).unwrap();
}

// D12: requests to settle that never arrived withdraw no one. For over two
// request intervals, nothing the senior sends reaches its junior, which
// is still heard from and pays in advance as soon as it is asked: each
// request that fails to arrive is asked again at the next check, not an
// interval later, so the junior is asked as soon as the link is back,
// well within the grace.
#[test]
fn requests_to_settle_that_never_arrived_withdraw_no_honest_junior() {
    let mut sim = Sim::new(3, 2, 1);
    sim.loss_per_mille = 0;
    let peered = |sim: &Sim| !sim.peers(0).is_empty() && !sim.peers(1).is_empty();
    assert!(
        sim.run_until_or(START + 120_000, peered),
        "the nodes did not peer"
    );
    // The senior is the node that accepted the request: its peering is
    // inbound.
    let senior = usize::from(!sim.core(0).network().peers[0].peering.inbound);
    let junior = 1 - senior;
    assert!(sim.core(senior).network().peers[0].peering.inbound);
    let junior_id = key(junior).node_id();
    let policies = Policies::DEFAULT;
    let cut_for = 2 * policies.settlement_request_every_ms + 6_000;
    sim.cut.insert((senior, junior));
    let cut_at = sim.now;
    sim.run_until(cut_at + cut_for);
    assert!(
        sim.owes(senior, &junior_id) <= 0,
        "the junior paid in advance while its senior could not reach it"
    );
    sim.cut.clear();
    sim.run_until(cut_at + policies.peer.advertise_short_grace_ms + 60_000);
    let for_room: Vec<u64> = sim
        .withdrawn
        .iter()
        .filter(|(_, by, peer)| *by == senior && *peer == junior_id)
        .map(|(at, _, _)| at - cut_at)
        .collect();
    assert!(
        for_room.is_empty(),
        "the senior withdrew its junior for room, {for_room:?} ms after the cut"
    );
    assert!(sim.owes(senior, &junior_id) > 0, "not paid in advance");
    assert!(sim.advertises(senior, &junior_id));
}

// D20: the node whose peering request was accepted, the junior, extends
// its senior the default credit; the senior extends it nothing. So the
// senior asks at once, and the junior pays half the way to its trigger in
// advance, (1.1 - 0.15) / 2 = 0.475, before anything flows. Then the
// junior pays through its senior, settling as it goes.
#[test]
fn a_junior_pays_its_senior_in_advance() {
    let mut sim = Sim::new(3, 2, 1);
    sim.loss_per_mille = 0;
    sim.run_until(START + 60_000);
    let peering = |sim: &Sim, index: usize| sim.core(index).network().peers[0].peering.clone();
    let (senior, junior) = if peering(&sim, 0).inbound {
        (0, 1)
    } else {
        (1, 0)
    };
    assert!(!peering(&sim, junior).inbound, "both sides accepted");
    assert_eq!(peering(&sim, senior).credit_out, 0);
    assert_eq!(i128::from(peering(&sim, junior).credit_out), CREDIT);
    let (senior_id, junior_id) = (key(senior).node_id(), key(junior).node_id());
    assert_eq!(sim.owes(senior, &junior_id), 475_000_000, "paid in advance");
    assert_eq!(sim.owes(junior, &senior_id), -475_000_000);
    sim.pay(junior, senior, 3_000_000_000, None, None);
    sim.run_until(sim.now + 180_000);
    assert!(sim.payments[0].done, "the payment did not finish");
    check(&sim).unwrap();
}

// D20: what a peer earned us raises the credit we extend to it, by a tenth
// of the fees, counted on the peer's own ledger. Node 1 charges for what
// passes from node 0, on dollars, to node 2, on XRP: the fees are booked
// on dollars, so they raise its credit to node 0, and not to node 2.
#[test]
fn credit_grows_with_the_fees_a_peer_earned() {
    let mut sim = Sim::with(9, 3, 2, BTreeMap::new(), Settling::Cross);
    sim.loss_per_mille = 0;
    sim.run_until(START + 120_000);
    let (zero, two) = (key(0).node_id(), key(2).node_id());
    let trust = |sim: &Sim, ledger, peer: &NodeId| {
        -sim.balances_on(1, ledger)[&format!("contra/trust/{peer}")]
    };
    let fees = |sim: &Sim| {
        sim.balances_on(1, USD)
            .get("revenue/fees")
            .copied()
            .unwrap_or(0)
    };
    let (to_zero, to_two) = (trust(&sim, USD, &zero), trust(&sim, XRP, &two));
    let before = fees(&sim);
    sim.pay(0, 2, 2_000_000_000, None, None);
    sim.run_until(sim.now + 180_000);
    assert!(sim.payments[0].done, "the payment did not finish");
    let earned = fees(&sim) - before;
    assert!(earned > 0, "node 1 earned nothing");
    // A tenth of the fees, less what falls short of a whole unit.
    let grown = trust(&sim, USD, &zero) - to_zero;
    assert!(
        grown <= earned / 10 && earned / 10 - grown <= 1,
        "grew {grown} on {earned}"
    );
    assert_eq!(
        trust(&sim, XRP, &two),
        to_two,
        "the fees were booked on dollars"
    );
    // Restarted, node 1 reads on after its last raise: nothing twice.
    let raised = trust(&sim, USD, &zero);
    sim.crash(1);
    sim.run_until(sim.now + 2_000);
    sim.start(1);
    sim.run_until(sim.now + 60_000);
    assert_eq!(trust(&sim, USD, &zero), raised, "fees credited twice");
    check_across(&sim).unwrap();
}

// A run that reports an error fails, whatever else it checks.
#[test]
fn an_error_event_fails_the_run() {
    let mut sim = Sim::new(1, 2, 1);
    let node = key(1).node_id();
    sim.record(
        0,
        &dassie_node::engine::PeerEvent::NoEndpoint {
            node,
            error: "gone".into(),
        }
        .into(),
    );
    assert!(check_events(&sim).is_err());
    sim.record(0, &EngineEvent::EventsDropped { count: 1 }.into());
    assert_eq!(sim.errors.len(), 2);
    sim.errors.clear();
}

// A peer asked to settle answers with what it has on its way (D28): the
// settlement it makes, or one already under way, which under the
// one-at-a-time rule is the whole answer. Every amount a node is told is
// one its peer made to it.
#[test]
fn a_peer_asked_to_settle_tells_what_it_has_on_its_way() {
    let id = |index: usize| format!("{:?}", key(index).node_id());
    let mut told = 0;
    for seed in 0..6 {
        let run = scripted::run(seed);
        assert!(!run.outcome.is_failure(), "seed {seed}: {}", run.outcome);
        // (who settled, to whom, how much)
        let mut made = std::collections::BTreeSet::new();
        for line in &run.trace {
            let mut words = line.split(' ');
            let (Some(_), Some(index)) = (words.next(), words.next()) else {
                continue;
            };
            let Ok(index) = index.parse::<usize>() else {
                continue;
            };
            let field = |name: &str| {
                let start = line.find(name)? + name.len();
                let rest = &line[start..];
                Some(rest[..rest.find([',', ' ', '}'])?].to_string())
            };
            if line.contains("Settlement(Settling {") {
                let node = line[line.find("node: ").unwrap() + 6..]
                    .split(',')
                    .next()
                    .unwrap();
                let amount = field("amount: ").unwrap();
                made.insert((index, node.to_string(), amount));
            }
            if line.contains("SettlementRequestAnswered") {
                let Some(amount) =
                    field("settling: Some(").map(|a| a.trim_end_matches(')').to_string())
                else {
                    panic!("seed {seed}: an answer without an amount: {line}");
                };
                if amount == "0" {
                    continue;
                }
                let peer = line[line.find("peer: ").unwrap() + 6..]
                    .split(',')
                    .next()
                    .unwrap();
                let from = (0..8).find(|i| id(*i) == peer).unwrap();
                assert!(
                    made.contains(&(from, id(index), amount.clone())),
                    "seed {seed}: node {index} was told of {amount} that node {from} did not make: {line}"
                );
                told += 1;
            }
        }
    }
    assert!(told > 0, "no peer answered with an amount on its way");
}

// D31: a node peers on a scheme only once the scheme has seen its ledger.
// Its first sync books what the account holds as the owner's deposit, so a
// peer's settlement that reached the ledger before it, such as one a node
// ending the peering asks for (D28), would be taken for that deposit while
// the peer still owed it on our books. Node 0's ledger is down from its
// first start, its account funded: it does not peer, and node 1 does not
// peer with it. Once the ledger is back, they peer; node 1 pays node 0 past
// the credit node 0 extends, and node 0's owner ends the peering. What node
// 1 settles is booked as its settlement.
#[test]
fn a_node_peers_only_once_it_has_seen_its_ledger() {
    let mut sim = Sim::with(7, 2, 1, BTreeMap::new(), Settling::Xrpl);
    sim.loss_per_mille = 0;
    sim.ledger_down.insert(0);
    sim.ledger.fund(xrpl_account(0));
    *sim.fundings.entry(0).or_default() += 1;
    sim.run_until(START + 5 * 60_000);
    let (zero, one) = (key(0).node_id(), key(1).node_id());
    assert!(
        sim.peers(0).is_empty(),
        "node 0 peered before it saw its ledger"
    );
    assert!(!sim.peers(1).contains(&zero), "node 1 peered with it");
    sim.ledger_down.clear();
    let peered = |sim: &Sim| sim.peers(0).contains(&one) && sim.peers(1).contains(&zero);
    assert!(
        sim.run_until_or(sim.now + 5 * 60_000, peered),
        "the nodes did not peer once node 0 saw its ledger"
    );
    sim.pay(1, 0, 3_000_000_000, None, None);
    sim.run_until(sim.now + 60_000);
    let owes = -sim.owes(0, &one);
    assert!(owes > 0, "node 1 owes node 0 nothing");
    sim.step(0, |core| core.terminate_peering(one).unwrap());
    sim.run_until(sim.now + 180_000);
    assert!(!sim.peers(0).contains(&one), "the peering did not end");
    check_books(&sim).unwrap();
}

// D31: having seen its ledger once, a node peers on the scheme after a
// restart while its ledger is out of reach, and keeps the peerings it has:
// the first sync is kept with the scheme's state, and what reaches the
// ledger meanwhile is read from where the scheme left off once it is back.
#[test]
fn a_node_that_saw_its_ledger_once_peers_while_it_is_down() {
    let mut sim = Sim::with(7, 2, 1, BTreeMap::new(), Settling::Xrpl);
    sim.loss_per_mille = 0;
    restart_without_ledger(&mut sim, 0);
    let one = key(1).node_id();
    let peered = |sim: &Sim| sim.peers(0).contains(&one);
    assert!(
        sim.run_until_or(sim.now + 5 * 60_000, peered),
        "the nodes did not peer"
    );
    sim.crash(0);
    sim.start(0);
    sim.run_until(sim.now + 60_000);
    assert!(sim.peers(0).contains(&one), "the peering did not survive");
    sim.ledger_down.clear();
    sim.run_until(sim.now + 120_000);
    check_books(&sim).unwrap();
}
