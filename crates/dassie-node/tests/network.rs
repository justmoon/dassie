//! Real engine cores on a simulated network, each test one situation on a
//! fixed seed (spec/areas/testing.md#deterministic-simulation):
//! `dassie_node::sim` is the harness. Its seeded runs, such as
//! `dassie_node::sim::scripted`'s, are no test's: `dassie-sim` and the
//! campaign run them (spec/decisions/random-seed-simulations.md).
//!
//! The tests whose subject is not settlement (a hop that crashes after
//! posting, a late Fulfill, payments racing into one incoming payment) run
//! their nodes on the stub with unlimited trust (`Settling::Trusting`, which
//! `Sim::new` gives), so that no credit runs out and no request to settle or
//! settlement shapes what they check (spec/areas/testing.md#levels). A test
//! of settlement runs over the stub with commitments (`Settling::StubHub`),
//! the path over a hub, as `hub_path.rs` does. No test here settles directly
//! (spec/decisions/hubs-only.md).

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use std::collections::BTreeMap;

use dassie_kit::sim::Outcome;
use dassie_ledger::LedgerId;
use dassie_node::engine::{EngineError, EngineEvent, Lie, NewIncoming, NewOutgoing};
use dassie_node::node::Policies;
use dassie_node::sim::checks::{check, check_books, check_events, check_ledgers, drifted_past};
use dassie_node::sim::{START, Settling, Sim, key, received, sent};
use dassie_node::store::{Aim, ForeignAmount, OutgoingRow, OutgoingState, PaymentId, PeerSettings};
use dassie_payments::amount::Amount;
use dassie_payments::gnap::Limits;
use dassie_settlement::policy;

/// `count` nodes on the stub with unlimited trust, the first a bootstrap
/// node: for a test whose subject is not settlement.
fn trusting(seed: u64, count: usize) -> Sim {
    Sim::new(seed, count, 1)
}

/// Two nodes, peered, and a client of node 0 paying node 1's receiver,
/// run until node 1's receiver took money: it posted a packet, and its
/// Fulfill is on its way. `before` is done to the network once the
/// payment knows its rate, before any money moves.
fn paying(seed: u64, before: impl FnOnce(&mut Sim)) -> Sim {
    let mut sim = trusting(seed, 2);
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

// spec/decisions/own-fulfills.md: the receiver's node crashes once it posted,
// before its Fulfill arrives. What it booked stays, and the payment still
// completes.
#[test]
fn a_hop_that_crashes_after_posting_keeps_its_books() {
    let mut sim = paying(70, |_| {});
    sim.crash(1);
    sim.run_until(sim.now + 2_000);
    sim.start(1);
    sim.run_until(sim.now + 120_000);
    check(&sim).unwrap();
}

// spec/decisions/own-fulfills.md: the sender's node crashes before the
// Fulfill reaches it, and forgets the packet. The receiver's node keeps what
// it booked; each node's books still hold against what its own clients moved.
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
    // The simulation saw the drift the crash made, no more than was in
    // flight, and it is all the books show (spec/decisions/crash-drift.md).
    let drift = sim.drift(0, 1);
    let packet = u128::from(Policies::DEFAULT.connector.max_packet_amount);
    assert!(
        drift.crashes > 0 && drift.crashes * 2 <= packet * 5,
        "{drift:?}"
    );
    assert_eq!(drift.others, 0);
    assert_eq!(drift.crashes, books_drift(&sim));
    assert_eq!(drifted_past(&mut sim), None);
}

/// How far the two nodes' books drifted apart, by their books: a test of
/// what the simulation saw, never of the nodes (spec/decisions/own-books.md).
fn books_drift(sim: &Sim) -> u128 {
    let (zero, one) = (key(0).node_id(), key(1).node_id());
    u128::try_from(-(sim.owes(0, &one) + sim.owes(1, &zero))).unwrap()
}

// spec/decisions/own-fulfills.md: a Fulfill that cannot get through before
// the upstream's expiry counts in the downstream's books, and not in the
// upstream's, which expired the packet. Neither is wrong by its own books,
// and the payment completes.
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

/// How long ending a peering may take (spec/decisions/ending-a-peering.md):
/// waiting for our own settlements, then for what the peer has on its way,
/// each at most the wait, then telling it, then the packets in flight.
const ENDED_WITHIN_MS: u64 = 2 * Policies::DEFAULT.peer.depeer_wait_ms
    + Policies::DEFAULT.peer.termination_timeout_ms
    + Policies::DEFAULT.connector.max_hold_ms;

// spec/decisions/own-fulfills.md: a peering being ended waits only for the
// packets in flight and its bounded waits for settlements, not for Fulfills
// that cannot get through. The peer's answer to the last request to settle
// cannot get through either, and is waited for no longer than its bound.
#[test]
fn a_peering_ends_while_its_fulfills_cannot_arrive() {
    let mut sim = paying(73, |sim| {
        sim.cut.insert((1, 0));
    });
    let peer = key(1).node_id();
    sim.step(0, |core| core.terminate_peering(peer).unwrap());
    sim.run_until(sim.now + ENDED_WITHIN_MS);
    assert!(sim.peers(1).is_empty() && sim.peers(0).is_empty());
    sim.cut.clear();
    sim.run_until(sim.now + 60_000);
    check_books(&sim).unwrap();
}

// spec/decisions/unreachable-peers.md: the receiver's node cannot reach the
// sender's for ten minutes, while the sender's reaches it, as behind an
// asymmetric firewall. The client tries again after each packet expires. Were
// each try fulfilled, the receiver's books would drift from the sender's by a
// packet a try, with no end: on the stub with unlimited trust, no room runs
// out to stop them. Once a Fulfill of the receiver's node surely did not
// arrive, it takes no new Prepare from the sender's, so the books drift by
// at most what was in flight, two and a half of the largest packets
// (spec/decisions/packets-in-flight.md). Once the cut is gone, its Rejects
// and heartbeats get through, and the payment completes.
#[test]
fn a_peer_that_cannot_hear_us_drifts_by_at_most_what_was_in_flight() {
    let mut sim = trusting(76, 2);
    sim.loss_per_mille = 0;
    sim.run_until(START + 60_000);
    assert!(!sim.peers(0).is_empty(), "the nodes did not peer");
    // Three units, many packets: no credit runs out, so only the rule
    // bounds the drift.
    sim.pay(1, 0, 3_000_000_000, None, None);
    let rate = |sim: &Sim| sim.payments[0].connection.exchange_rate().is_some();
    assert!(sim.run_until_or(sim.now + 30_000, rate), "no rate");
    sim.cut.insert((0, 1));
    sim.run_until(sim.now + 600_000);
    let (sender, receiver) = (key(1).node_id(), key(0).node_id());
    let drift = -(sim.owes(0, &sender) + sim.owes(1, &receiver));
    let packet = i128::from(Policies::DEFAULT.connector.max_packet_amount);
    assert!(
        drift > 0 && drift * 2 <= packet * 5,
        "the books drifted by {drift}, past 2.5 packets of {packet}"
    );
    // The simulation saw it all, and none of it as a crash's, so it never
    // excuses a link this drifted (spec/decisions/crash-drift.md).
    let seen = sim.drift(0, 1);
    assert_eq!(seen.crashes, 0);
    assert_eq!(i128::try_from(seen.others).unwrap(), drift);
    sim.cut.clear();
    sim.run_until(sim.now + 180_000);
    assert!(sim.payments[0].done, "the payment did not finish");
    check(&sim).unwrap();
}

// spec/areas/peer-protocol.md#authentication: nothing is dated, so a node
// that just started refuses no message for its age. Node 0's clock is ahead
// of node 1's. Its outgoing payment cannot reach node 1, and it restarts: its
// payer goes on as it starts, and node 1's Fulfills, sent as it starts,
// arrive, so the books do not drift.
#[test]
fn fulfills_to_a_node_that_just_started_still_arrive() {
    let mut sim = Sim::with_skews(
        77,
        1,
        BTreeMap::new(),
        Settling::Trusting,
        Policies::DEFAULT.peer,
        &[5_000, 0],
    );
    sim.loss_per_mille = 0;
    sim.run_until(START + 60_000);
    assert!(!sim.peers(0).is_empty(), "the nodes did not peer");
    sim.cut.extend([(0, 1), (1, 0)]);
    sim.pay_outgoing(0, 1, 500_000_000).unwrap();
    sim.run_until(sim.now + 1_000);
    sim.crash(0);
    sim.cut.clear();
    sim.start(0);
    sim.run_until(sim.now + 120_000);
    let (payer, payee) = (key(0).node_id(), key(1).node_id());
    let drift = -(sim.owes(0, &payee) + sim.owes(1, &payer));
    assert_eq!(drift, 0, "the books drifted");
    check(&sim).unwrap();
}

/// Whether node `index` stopped.
fn stopped(index: usize) -> impl Fn(&Sim) -> bool {
    move |sim: &Sim| sim.nodes[index].core.is_none()
}

// spec/decisions/random-seed-simulations.md: a node whose commit fails stops
// before anything of the step goes out. The receiver's node fails the commit
// that books its first Fulfill: the client hears of no Fulfill and its
// receiver kept nothing. The sender's node had the Prepare answered with the
// 503 of an engine that stopped, which says it had no effect. Once the node
// is back, the payment completes.
#[test]
fn a_receiver_whose_commit_fails_sends_no_fulfill() {
    let mut sim = trusting(75, 2);
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

/// Two nodes peered over the stub with commitments, each having funded its
/// hold toward the other once asked, and node 0 paying node 1 three units:
/// as its hold is taken, node 1 asks it to fund the hold again on the way,
/// and the commit after the next `skip` of node `index`'s that holds a
/// settlement fails. It is back a few seconds after it stopped.
fn settling(index: usize, skip: u32) -> Sim {
    let mut sim = Sim::with(3, 2, 1, BTreeMap::new(), Settling::StubHub(1));
    sim.loss_per_mille = 0;
    sim.run_until(START + 60_000);
    assert!(!sim.peers(0).is_empty(), "the nodes did not peer");
    sim.pay(0, 1, 3_000_000_000, None, None);
    sim.fail_commit(index, Aim::Settlement, skip);
    let failed = sim.run_until_or(sim.now + 120_000, stopped(index));
    assert!(failed, "no failure");
    assert_eq!(sim.counts["failed commits mid-settlement"], 1);
    sim.run_until(sim.now + 5_000);
    sim.start(index);
    sim.run_until(sim.now + 180_000);
    sim
}

// spec/decisions/random-seed-simulations.md: the payee fails the commit that
// books the payer's funding of its hold over the hub. The payer's message
// gets the 503 of an engine that stopped, and the payer sends it again once
// the payee is back: it is booked once, and the payment completes.
#[test]
fn a_settlement_whose_booking_failed_is_booked_when_sent_again() {
    let payee = 1;
    let sim = settling(payee, 0);
    // Where `text` is first traced after line `from`.
    let after = |from: usize, text: &str| {
        from + sim.trace[from..]
            .iter()
            .position(|line| line.contains(text))
            .unwrap_or_else(|| panic!("no {text:?}"))
    };
    let failed = after(0, &format!(" {payee} failed a commit"));
    let back = after(failed, &format!("start {payee}"));
    // The payer's first funding was asked for as they peered; its second
    // is the one whose booking failed, delivered once the payee is back.
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

// spec/decisions/random-seed-simulations.md: the payer fails the commit of
// its funding of its hold as it prepares it, and, the second time, once it
// executed it: neither its transfer nor its message goes out from what
// never reached the disk, and the books hold after its restart.
#[test]
fn a_settlement_whose_commit_failed_leaves_the_books_holding() {
    for skip in [0, 1] {
        let sim = settling(0, skip);
        // The second time, the commit before put the funding's transfer on
        // disk, pending, and the scheme reconciles it at the restart: the
        // stub never moved what the lost commit said it did.
        let reconciled = sim.trace.iter().any(|line| line.contains("Reconciling"));
        assert_eq!(reconciled, skip == 1, "skip {skip}");
        check(&sim).unwrap_or_else(|problem| panic!("skip {skip}: {problem}"));
    }
}

// spec/decisions/ending-a-peering.md: a ban withdraws the peer from the
// node's link state in the step that bans it, and ends the peering without
// asking the peer to settle a last time. What the peer owes stays on the
// books.
#[test]
fn a_ban_withdraws_the_peer_at_once_and_asks_nothing() {
    let mut sim = trusting(74, 2);
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
    let mut sim = trusting(80, 1);
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
    let mut sim = trusting(81, 1);
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
                uplink: None,
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

// spec/areas/payments.md#what-a-grant-allows: a grant whose limits cap
// what is received but not what is debited pays nothing, though a store
// restored from elsewhere still holds it; with a debit limit beside its
// receive limit, the same payment is made.
#[test]
fn a_grant_that_limits_only_what_is_received_pays_nothing() {
    let mut sim = trusting(90, 1);
    sim.run_until(START + 1_000);
    let ledger = sim.settling.ledger();
    let asset = ledger.asset();
    let mut incoming = None;
    sim.step(0, |core| {
        incoming = Some(
            core.create_incoming_payment(NewIncoming {
                ledger,
                wallet_address: "https://sim.example/.well-known/pay".into(),
                client: Some("https://client.example".into()),
                incoming_amount: Some(20),
                expires_at_ms: None,
                metadata: None,
                uplink: None,
            })
            .unwrap(),
        );
    });
    let incoming = incoming.unwrap();
    let pay = |debit_limit: Option<u64>| NewOutgoing {
        ledger,
        wallet_address: "https://sim.example/.well-known/pay".into(),
        client: Some("https://client.example".into()),
        grant: Some((
            PaymentId([9; 16]),
            Limits {
                receiver: None,
                interval: None,
                debit_amount: debit_limit.map(|value| Amount::new(value, asset.code, asset.scale)),
                receive_amount: Some(Amount::new(100, asset.code, asset.scale)),
            },
        )),
        quote_id: None,
        receiver: format!("https://n0.test/incoming-payments/{}", incoming.id),
        destination: incoming.ilp_address.clone(),
        shared_secret: incoming.shared_secret,
        debit_amount: 10,
        receive_amount: ForeignAmount {
            value: 10,
            asset_code: asset.code.into(),
            asset_scale: asset.scale,
        },
        metadata: None,
        recipient: None,
        fixed_debit: false,
    };
    sim.step(0, |core| {
        let refused = core.create_outgoing_payment(pay(None));
        assert!(
            matches!(&refused, Err(EngineError::Refused(why)) if why.contains("not what is debited")),
            "{refused:?}"
        );
        assert!(core.create_outgoing_payment(pay(Some(100))).is_ok());
    });
}

// spec/areas/payments.md#outgoing-payments: a payment whose packets keep
// failing stops its connection's money, and connects again for as long as
// its deadline lets it, not the few times a connection that closed may.
// The peers cannot reach each other for six minutes: node 0 soon refuses
// the payment's packets itself, without a reply from the receiver. Once
// the path is back, the payment completes, before its ten minutes are up.
#[test]
fn a_payment_whose_money_stopped_tries_until_its_deadline() {
    let mut sim = trusting(90, 2);
    sim.loss_per_mille = 0;
    sim.run_until(START + 60_000);
    assert!(!sim.peers(0).is_empty(), "the nodes did not peer");
    sim.cut.extend([(0, 1), (1, 0)]);
    sim.pay_outgoing(0, 1, 500_000_000).unwrap();
    let id = sim.outgoing[0].3;
    let created = sim.now;
    let row = |sim: &Sim| sim.nodes[0].store.outgoing_payment(id).unwrap().unwrap();
    sim.run_until(created + 6 * 60_000);
    assert_eq!(row(&sim).state, OutgoingState::Sending);
    let stops = sim
        .trace
        .iter()
        .filter(|line| {
            line.contains(" 0 Payment(OutgoingReconnects") && line.contains("kept failing")
        })
        .count();
    // More than the eight times a connection that closed is opened again.
    assert!(stops > 8, "{stops} stops");
    sim.cut.clear();
    let done = |sim: &Sim| row(sim).state != OutgoingState::Sending;
    assert!(
        sim.run_until_or(created + 10 * 60_000, done),
        "still sending"
    );
    assert_eq!(row(&sim).state, OutgoingState::Completed, "{:?}", row(&sim));
    check(&sim).unwrap();
}

// Two outgoing payments race into one incoming payment, each on a
// connection of its own, each to deliver all it asks for: the receiver takes
// at most what it asked for, split between them. Both end at once rather
// than at their deadline, since a receiver that takes no more closes the
// connections that offer it money (spec/decisions/full-or-stopped-short.md),
// and so does one whose packet another payment left too little room for
// (spec/decisions/close-rather-than-lower.md). One that delivered less fails,
// saying so, and what they sent is what the receiver took. The payer whose
// packet filled it had never seen its room fall when another payer took its
// share first: until it was closed, it waited for its idle timeout, and heard
// why on a new connection a minute later.
#[test]
fn payments_racing_into_one_incoming_payment_end_at_once() {
    for seed in 0..6 {
        let (sim, received, rows) = race_into_one_incoming_payment(82 + seed, RACED);
        let sent: u64 = rows.iter().map(|row| row.sent_amount).sum();
        assert_eq!(sent, received, "seed {seed}: {rows:?}");
        // Neither payer can be told the last units another took, so the
        // payment can end short of its amount, never over it.
        assert!(received <= RACED, "seed {seed}: {received}");
        for row in &rows {
            if row.sent_amount < RACED {
                assert_eq!(row.state, OutgoingState::Failed, "seed {seed}");
                assert_eq!(
                    row.failure.as_deref(),
                    Some("the receiver takes no more"),
                    "seed {seed}"
                );
            } else {
                assert_eq!(row.state, OutgoingState::Completed, "seed {seed}");
            }
        }
        // The harness's checks pair each outgoing payment with an incoming
        // payment of its own, so the books are checked here: the receiver's
        // owner gained what its payment received, and the payer paid what
        // both sent.
        let gained = sim.balances(1)["equity/owner"] - sim.deposit(1);
        assert_eq!(gained, i128::from(received), "seed {seed}");
        let paid = -sim.balances(0)["equity/owner/local/pay"];
        assert_eq!(paid, i128::from(sent), "seed {seed}");
        check_events(&sim).unwrap_or_else(|problem| panic!("seed {seed}: {problem}"));
        check_ledgers(&sim).unwrap_or_else(|problem| panic!("seed {seed}: {problem}"));
    }
}

// Two outgoing payments pay one incoming payment at once, each on a
// connection of its own, and together pay just what it asks for. Each was
// told the room the incoming payment had when it started, and keeps being
// told it, more than is left once the other paid; no packet of either ever
// finds too little room, so both complete.
#[test]
fn payments_that_fit_together_into_one_incoming_payment_both_complete() {
    for seed in 0..6 {
        let (sim, received, rows) = race_into_one_incoming_payment(82 + seed, 2 * RACED);
        assert_eq!(received, 2 * RACED, "seed {seed}: {rows:?}");
        for row in &rows {
            assert_eq!(row.state, OutgoingState::Completed, "seed {seed}");
            assert_eq!(row.sent_amount, RACED, "seed {seed}");
        }
        check_events(&sim).unwrap_or_else(|problem| panic!("seed {seed}: {problem}"));
        check_ledgers(&sim).unwrap_or_else(|problem| panic!("seed {seed}: {problem}"));
    }
}

/// What each payment racing into one incoming payment sends, and what the
/// incoming payment asks for when they race: three of the largest packets.
const RACED: u64 = 3 * Policies::DEFAULT.connector.max_packet_amount;

/// Node 0 pays an incoming payment of node 1 for `amount` twice at once,
/// [`RACED`] each, each payment with credentials of its own, and runs until
/// both ended, for at most ten seconds: a packet's round trip and a back-off
/// or two. Returns the network, what the incoming payment received, and the
/// outgoing payments.
fn race_into_one_incoming_payment(seed: u64, amount: u64) -> (Sim, u64, Vec<OutgoingRow>) {
    let mut sim = trusting(seed, 2);
    sim.run_until(START + 60_000);
    assert!(
        !sim.peers(0).is_empty(),
        "seed {seed}: the nodes did not peer"
    );
    let ledger = sim.settling.ledger();
    let mut incoming = None;
    sim.step(1, |core| {
        let payment = core
            .create_incoming_payment(NewIncoming {
                ledger,
                wallet_address: "https://sim.example/.well-known/pay".into(),
                client: None,
                incoming_amount: Some(amount),
                expires_at_ms: None,
                metadata: None,
                uplink: None,
            })
            .unwrap();
        let other = core.incoming_credentials(payment.id, ledger).unwrap();
        incoming = Some((payment, other));
    });
    let (incoming, other) = incoming.unwrap();
    let pay = |destination: &str, shared_secret| NewOutgoing {
        ledger,
        wallet_address: "https://sim.example/.well-known/pay".into(),
        client: None,
        grant: None,
        quote_id: None,
        receiver: format!("https://n1.test/incoming-payments/{}", incoming.id),
        destination: destination.to_string(),
        shared_secret,
        debit_amount: RACED,
        receive_amount: ForeignAmount {
            value: RACED,
            asset_code: ledger.asset().code.into(),
            asset_scale: 9,
        },
        metadata: None,
        recipient: None,
        fixed_debit: false,
    };
    let mut ids: Vec<PaymentId> = Vec::new();
    sim.step(0, |core| {
        for (destination, secret) in [
            (incoming.ilp_address.as_str(), incoming.shared_secret),
            (other.destination.as_str(), other.shared_secret),
        ] {
            let created = core.create_outgoing_payment(pay(destination, secret));
            ids.push(created.unwrap().row.id);
        }
    });
    let rows = |sim: &Sim| {
        ids.iter()
            .map(|id| sim.nodes[0].store.outgoing_payment(*id).unwrap().unwrap())
            .collect::<Vec<_>>()
    };
    let ended = |sim: &Sim| {
        rows(sim)
            .iter()
            .all(|row| row.state != OutgoingState::Sending)
    };
    let ended = sim.run_until_or(sim.now + 10_000, ended);
    assert!(ended, "seed {seed}: still sending: {:?}", rows(&sim));
    let paid_in = sim.nodes[1].store.incoming_payment(incoming.id).unwrap();
    let received = paid_in.unwrap().received_amount;
    let rows = rows(&sim);
    (sim, received, rows)
}

// spec/decisions/peering-lifecycle.md: a payer over a hub that never funds
// its hold toward us spends what is in it, and is then short of room for a
// packet: past the grace it is withdrawn, and once its room has stayed
// short, settling nothing, for as long as a silent peer is kept, the
// peering is ended for room. Nobody is banned for it. Over the stub with
// commitments, node 1 pays node 0 far more than its hold and never settles.
// A silent peer is kept half an hour here, not a day, so that the test
// runs the case and not a day of heartbeats.
#[test]
fn a_payer_short_of_room_for_a_silent_peers_time_is_ended_for_room() {
    let peer = dassie_peer::Config {
        peer_dead_ms: 30 * 60_000,
        ..Policies::DEFAULT.peer
    };
    let mut sim = Sim::with_peer(0, 2, 1, BTreeMap::new(), Settling::StubHub(1), peer);
    sim.loss_per_mille = 0;
    let peered = |sim: &Sim| !sim.peers(0).is_empty() && !sim.peers(1).is_empty();
    assert!(
        sim.run_until_or(START + 120_000, peered),
        "the nodes did not peer"
    );
    sim.step(1, |core| core.lie(Lie::NeverSettle));
    sim.pay(1, 0, 10_000_000_000, None, None);
    let payer = key(1).node_id();
    let withdrawn = |sim: &Sim| {
        sim.withdrawn
            .iter()
            .any(|withdrawal| withdrawal.by == 0 && withdrawal.peer == payer)
    };
    assert!(
        sim.run_until_or(
            sim.now + peer.advertise_short_grace_ms + 10 * 60_000,
            withdrawn
        ),
        "node 0 did not withdraw it for room"
    );
    assert!(!sim.payments[0].done, "it paid past its hold");
    assert!(sim.peers(0).contains(&payer), "ended at once");
    let ended = format!(" 0 PeerNode(Terminating {{ peer: {payer:?}, reason: ShortOfRoom");
    let is_ended = |sim: &Sim| sim.trace.iter().any(|line| line.contains(&ended));
    let short_from = sim.now;
    sim.run_until(short_from + peer.peer_dead_ms / 2);
    assert!(!is_ended(&sim), "ended long before a silent peer's time");
    assert!(
        sim.run_until_or(short_from + peer.peer_dead_ms + 10 * 60_000, is_ended),
        "node 0 did not end the peering for room"
    );
    assert!(sim.core(0).network().bans.is_empty());
}

// spec/decisions/peering-lifecycle.md: requests to settle that never arrived
// withdraw no one. Over a hub, each node asks the other to fund its hold
// toward it as they peer (spec/areas/hubs.md#in-the-node). For over two
// request intervals, nothing node 1 sends reaches node 0, which is still
// heard from and funds its hold as soon as it is asked: each request that
// fails to arrive is asked again at the next check, not an interval later,
// so node 0 is asked as soon as the link is back, well within the grace.
#[test]
fn requests_to_settle_that_never_arrived_withdraw_no_honest_payer() {
    let mut sim = Sim::with(3, 2, 1, BTreeMap::new(), Settling::StubHub(1));
    sim.loss_per_mille = 0;
    let peered = |sim: &Sim| !sim.peers(0).is_empty() && !sim.peers(1).is_empty();
    assert!(
        sim.run_until_or(START + 120_000, peered),
        "the nodes did not peer"
    );
    let (payer, payee) = (0, 1);
    let payer_id = key(payer).node_id();
    // What the payer holds toward the payee, by its own stub.
    let hold = |sim: &mut Sim| {
        let payee_id = key(payee).node_id();
        sim.nodes[payer]
            .core
            .as_mut()
            .unwrap()
            .hub_peers()
            .into_iter()
            .find(|hub| hub.peer.as_str() == payee_id.as_str())
            .map_or(0, |hub| hub.holds.toward)
    };
    let policies = Policies::DEFAULT;
    let cut_for = 2 * policies.settlement_request_every_ms + 6_000;
    sim.cut.insert((payee, payer));
    let cut_at = sim.now;
    sim.run_until(cut_at + cut_for);
    assert_eq!(
        hold(&mut sim),
        0,
        "the payer funded its hold while the payee could not reach it"
    );
    sim.cut.clear();
    sim.run_until(cut_at + policies.peer.advertise_short_grace_ms + 60_000);
    let for_room: Vec<u64> = sim
        .withdrawn
        .iter()
        .filter(|withdrawal| withdrawal.by == payee && withdrawal.peer == payer_id)
        .map(|withdrawal| withdrawal.at - cut_at)
        .collect();
    assert!(
        for_room.is_empty(),
        "the payee withdrew the payer for room, {for_room:?} ms after the cut"
    );
    assert!(hold(&mut sim) > 0, "the payer funded no hold");
    assert!(sim.advertises(payee, &payer_id));
}

// A run that reports an error fails, whatever else it checks.
#[test]
fn an_error_event_fails_the_run() {
    let mut sim = trusting(1, 2);
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

// spec/decisions/crash-drift.md: over a pretend hub, the payer crashes again
// and again while it pays the payee, each time with Fulfills on their way
// to it, until the books drifted past what the link survives and the
// payee's room for the payer is short, below one largest packet. What the
// link survives is the default credit the payer answers with, plus the
// payee's hold toward the payer, less two triggers and twice the least
// settlement. The payee funded that hold when the payer asked it at the
// start, the more if it had booked the payer's opening settlement first,
// so it is read, never assumed. The payer's books leave it nothing to
// settle, and the payee withdraws it: the link is not saved
// (spec/decisions/own-fulfills.md). A network that does not settle down
// for that is a run the simulation rules out, not a failure. It runs each
// way, so it holds whichever of the two has the lower node ID, which wins
// when both ask to peer at once.
#[test]
fn a_link_crashes_drifted_past_what_it_survives_is_ruled_out() {
    crashes_past_what_it_survives(0, 1);
    crashes_past_what_it_survives(1, 0);
}

/// [`a_link_crashes_drifted_past_what_it_survives_is_ruled_out`], with
/// node `payer` paying node `payee`.
fn crashes_past_what_it_survives(payer: usize, payee: usize) {
    let mut sim = Sim::with(78, 2, 1, BTreeMap::new(), Settling::StubHub(4));
    sim.loss_per_mille = 0;
    let peered = |sim: &Sim| !sim.peers(0).is_empty() && !sim.peers(1).is_empty();
    assert!(
        sim.run_until_or(START + 600_000, peered),
        "the nodes did not peer"
    );
    let withdrawn = key(payer).node_id();
    let packet = Policies::DEFAULT.connector.max_packet_amount;
    let settles = 2 * u128::from(policy::trigger(packet)) + 2 * u128::from(policy::minimum(packet));
    // The payee's hold toward the payer, which is the credit it extends,
    // and its room for the payer, by its books.
    let standing = |sim: &Sim| {
        let peer = sim
            .core(payee)
            .network()
            .peers
            .into_iter()
            .find(|peer| peer.id == withdrawn)
            .unwrap();
        let hold = peer.peering.first().credit_out;
        (hold, peer.owed.unwrap() + i128::from(hold))
    };
    // What the link survives (spec/areas/settlement.md#credit), and whether
    // the drift is past it with the payee's room for the payer short
    // (spec/areas/network.md#advertised-peers).
    let survives = |sim: &Sim| {
        let (hold, _) = standing(sim);
        (u128::from(Policies::DEFAULT.peer_credit) + u128::from(hold)).saturating_sub(settles)
    };
    let stuck = |sim: &mut Sim| {
        let (_, room) = standing(sim);
        sim.drift(payer, payee).crashes > survives(sim)
            && room < i128::from(policy::minimum(packet))
    };
    // The payee's receiver took money: it posted a packet, and its Fulfill
    // is on its way.
    let took = |sim: &Sim| sim.payments.last().is_some_and(|p| received(sim, p) > 0);
    for _ in 0..60 {
        if stuck(&mut sim) {
            break;
        }
        sim.pay(payer, payee, 3_000_000_000, None, None);
        if sim.run_until_or(sim.now + 30_000, took) {
            sim.crash(payer);
            sim.run_until(sim.now + 2_000);
            sim.start(payer);
        }
        sim.run_until(sim.now + 30_000);
    }
    assert!(
        stuck(&mut sim),
        "{:?} {:?}",
        sim.drift(payer, payee),
        standing(&sim)
    );
    sim.run_until(sim.now + 600_000);
    assert!(
        sim.withdrawn
            .iter()
            .any(|withdrawal| withdrawal.by == payee && withdrawal.peer == withdrawn),
        "the payee did not withdraw the payer"
    );
    match dassie_node::sim::checks::not_settled_down(&mut sim, 600, "stuck") {
        Outcome::Unsupported(finding) => assert_eq!(finding.at, "drifted-past-what-it-survives"),
        outcome => panic!("{outcome}"),
    }
}

// spec/decisions/paid-within-a-tolerance.md: a payment is paid in full
// only once it has its amount. One whose last packet carries less than its
// tolerance gets that packet too: its incoming payment completes at its
// amount, its outgoing payment completes, and its client counts it done.
#[test]
fn a_payments_last_packet_below_its_tolerance_is_paid_too() {
    let mut sim = trusting(76, 2);
    sim.loss_per_mille = 0;
    sim.run_until(START + 60_000);
    assert!(!sim.peers(0).is_empty(), "the nodes did not peer");
    // Messages arrive in the order they were sent, so the largest packets
    // reach the receiver before the last one, however the run's other
    // traffic draws its delays.
    let latency_ms = sim.latency_ms.1;
    sim.latency_ms = (latency_ms, latency_ms);
    // Three of the largest packets and twenty thousand units, less than
    // the tolerance of 49,999.
    let amount = 3 * Policies::DEFAULT.connector.max_packet_amount + 20_000;
    assert!(sim.pay(0, 1, amount, None, Some(amount)));
    sim.pay_outgoing(0, 1, amount).unwrap();
    sim.run_until(sim.now + 300_000);
    let paid = received(&sim, &sim.payments[0]);
    assert_eq!(paid, amount, "{}", sim.trace.join("\n"));
    assert!(sim.payments[0].done, "{}", sim.trace.join("\n"));
    check(&sim).unwrap();
}

// spec/areas/settlement.md#the-stub-scheme: two nodes on the stub with
// unlimited trust each extend the other the unlimited credit, whichever
// asked for the peering. They pay each other dozens of times the default credit, both
// ways, and neither asks the other to settle, nor settles.
#[test]
fn on_unlimited_trust_nodes_pay_without_settling() {
    let mut sim = trusting(7, 2);
    sim.loss_per_mille = 0;
    let (zero, one) = (key(0).node_id(), key(1).node_id());
    let peered = |sim: &Sim| sim.peers(0).contains(&one) && sim.peers(1).contains(&zero);
    assert!(
        sim.run_until_or(START + 5 * 60_000, peered),
        "the nodes did not peer"
    );
    for index in 0..2 {
        for peer in sim.core(index).network().peers {
            assert_eq!(peer.peering.first().credit_out, policy::UNLIMITED_CREDIT);
        }
    }
    let credit = Policies::DEFAULT.peer_credit;
    let (there, back) = (30 * credit, 50 * credit);
    assert!(sim.pay(0, 1, there, None, Some(there)));
    assert!(sim.pay(1, 0, back, None, Some(back)));
    let done = |sim: &Sim| sim.payments.iter().all(|payment| payment.done);
    assert!(
        sim.run_until_or(sim.now + 30 * 60_000, done),
        "the payments did not finish"
    );
    assert_eq!(received(&sim, &sim.payments[0]), there);
    assert_eq!(received(&sim, &sim.payments[1]), back);
    // By its own books, node 1 owes node 0 what it paid through node 0
    // beyond what node 0 paid through it, far past the default credit, and
    // settles none of it.
    let net = i128::from(back - there);
    assert!(net > 10 * i128::from(credit));
    assert_eq!(sim.owes(1, &zero), net);
    assert_eq!(sim.owes(0, &one), -net);
    sim.run_until(sim.now + 10 * 60_000);
    assert_eq!(sim.owes(1, &zero), net);
    let settled = sim
        .trace
        .iter()
        .find(|line| line.contains("Settlement(Settling") || line.contains("SettlementRequest"));
    assert!(settled.is_none(), "a node settled or asked: {settled:?}");
    // Node 0 ends the peering, and asks a last time: node 1 answers that
    // nothing is on its way, and prepares nothing, so the end collects
    // nothing.
    let mark = sim.trace.len();
    sim.step(0, |core| core.terminate_peering(one).unwrap());
    let ended = |sim: &Sim| !sim.peers(0).contains(&one) && !sim.peers(1).contains(&zero);
    assert!(
        sim.run_until_or(sim.now + 10 * 60_000, ended),
        "the peering did not end"
    );
    let answered = format!(" 0 PeerNode(SettlementRequestAnswered {{ peer: {one:?}, settling: ");
    let answers: Vec<&String> = sim.trace[mark..]
        .iter()
        .filter(|line| line.contains(&answered))
        .collect();
    assert!(
        !answers.is_empty(),
        "node 1 did not answer the last request"
    );
    for answer in answers {
        assert!(answer.contains(&format!("{answered}0 }}")), "{answer}");
    }
    let settled = sim.trace[mark..].iter().find(|line| {
        line.contains("Settlement(Settling") || line.contains("Settlement(PrepareFailed")
    });
    assert!(settled.is_none(), "the end settled: {settled:?}");
    assert_eq!(sim.owes(1, &zero), net);
    check(&sim).unwrap();
}

/// Where the first line of `sim`'s trace after line `from` that has
/// `text` is, if one does.
fn first_after(sim: &Sim, from: usize, text: &str) -> Option<usize> {
    sim.trace[from..]
        .iter()
        .position(|line| line.contains(text))
        .map(|at| from + at)
}

/// The number in `line` right after `text`.
fn number_after(line: &str, text: &str) -> u64 {
    line[line.find(text).unwrap() + text.len()..]
        .split(|c: char| !c.is_ascii_digit())
        .next()
        .and_then(|digits| digits.parse().ok())
        .unwrap()
}

// spec/decisions/ending-a-peering.md: a settlement that lands after the
// peering ended is credited to the peer's account, once. Over the stub with
// commitments, the payee's owner ends the peering, and the payer answers the
// last request with a settlement on its way, which does not reach the payee:
// the payee waits out its bound and ends the peering, and books the
// settlement only once it arrives, after the end.
#[test]
fn a_settlement_that_lands_after_the_end_is_credited_once() {
    let mut sim = Sim::with(7, 2, 1, BTreeMap::new(), Settling::StubHub(4));
    sim.loss_per_mille = 0;
    let peered = |sim: &Sim| !sim.peers(0).is_empty() && !sim.peers(1).is_empty();
    assert!(
        sim.run_until_or(START + 120_000, peered),
        "the nodes did not peer"
    );
    // The node that asked for the peering is paid.
    let payee = usize::from(sim.core(0).network().peers[0].peering.inbound);
    let payer = 1 - payee;
    let payer_id = key(payer).node_id();
    sim.pay(payer, payee, 500_000_000, None, None);
    sim.run_until(sim.now + 60_000);
    assert!(sim.payments[0].done, "the payment did not finish");
    let before = sim.owes(payee, &payer_id);
    // Nothing the payer sends the payee gets there: its answer to the
    // last request does, as the reply to the payee's request.
    sim.cut.insert((payer, payee));
    let mark = sim.trace.len();
    sim.step(payee, |core| core.terminate_peering(payer_id).unwrap());
    let found = first_after;
    let answered =
        format!(" {payee} PeerNode(SettlementRequestAnswered {{ peer: {payer_id:?}, settling: ");
    assert!(
        sim.run_until_or(sim.now + 60_000, |sim| found(sim, mark, &answered)
            .is_some()),
        "the payer did not answer the last request"
    );
    let line = &sim.trace[found(&sim, mark, &answered).unwrap()];
    let amount = number_after(line, &answered);
    assert!(amount > 0, "nothing on its way: {line}");
    let ended = format!(" {payee} PeerNode(PeeringEnded {{ peer: {payer_id:?}");
    assert!(
        sim.run_until_or(sim.now + 600_000, |sim| found(sim, mark, &ended).is_some()),
        "the peering did not end"
    );
    let end = found(&sim, mark, &ended).unwrap();
    let booked = format!(" {payee} Settlement(SettlementBooked {{ from: {payer_id:?}");
    assert!(
        found(&sim, mark, &booked).is_none(),
        "booked before the end"
    );
    assert_eq!(sim.owes(payee, &payer_id), before);
    sim.cut.clear();
    assert!(
        sim.run_until_or(sim.now + 30 * 60_000, |sim| found(sim, end, &booked)
            .is_some()),
        "the settlement was not booked after the end"
    );
    sim.run_until(sim.now + 120_000);
    let after: Vec<&String> = sim.trace[end..]
        .iter()
        .filter(|line| line.contains(&booked))
        .collect();
    assert_eq!(after.len(), 1, "not booked once after the end: {after:?}");
    assert!(
        after[0].contains(&format!("amount: {amount} ")),
        "{}",
        after[0]
    );
    assert_eq!(
        sim.owes(payee, &payer_id),
        before + i128::from(amount),
        "not credited to the payer's account"
    );
    check_books(&sim).unwrap();
}

/// Three nodes on the stub with unlimited trust in a line: node 0 and node
/// 2 ban each other, so what one pays the other passes node 1, which
/// charges a fee of `fee_ppm`. Runs until node 0 has a route to node 2.
fn charging_line(seed: u64, fee_ppm: u32) -> Sim {
    let mut sim = trusting(seed, 3);
    sim.loss_per_mille = 0;
    let (zero, two) = (key(0).node_id(), key(2).node_id());
    sim.step(0, |core| core.ban_node(two).unwrap());
    sim.step(2, |core| core.ban_node(zero).unwrap());
    charge(&mut sim, fee_ppm);
    let routed = |sim: &Sim| {
        sim.core(0)
            .network()
            .nodes
            .iter()
            .any(|node| node.id == two && !node.first_hops.is_empty())
    };
    assert!(sim.run_until_or(START + 600_000, routed), "no route");
    sim
}

/// Node 1 charges a fee of `connector_fee_ppm` for what it forwards.
fn charge(sim: &mut Sim, connector_fee_ppm: u32) {
    sim.step(1, |core| {
        core.set_peer_settings(PeerSettings {
            connector_fee_ppm,
            ..PeerSettings::DEFAULT
        })
        .unwrap();
    });
}

/// An outgoing payment of node 0 of `debit` to a receiver of node 2 that
/// asks for no amount, at no less than `least`: its incoming payment's ID
/// and its own.
fn pay_at_least(sim: &mut Sim, debit: u64, least: u64) -> (PaymentId, PaymentId) {
    let ledger = sim.settling.ledger();
    let mut incoming = None;
    sim.step(2, |core| {
        let payment = core
            .create_incoming_payment(NewIncoming {
                ledger,
                wallet_address: "https://sim.example/.well-known/pay".into(),
                client: None,
                incoming_amount: None,
                expires_at_ms: None,
                metadata: None,
                uplink: None,
            })
            .unwrap();
        incoming = Some(payment);
    });
    let incoming = incoming.unwrap();
    let mut outgoing = None;
    sim.step(0, |core| {
        let created = core
            .create_outgoing_payment(NewOutgoing {
                ledger,
                wallet_address: "https://sim.example/.well-known/pay".into(),
                client: None,
                grant: None,
                quote_id: None,
                receiver: format!("https://n2.test/incoming-payments/{}", incoming.id),
                destination: incoming.ilp_address.clone(),
                shared_secret: incoming.shared_secret,
                debit_amount: debit,
                receive_amount: ForeignAmount {
                    value: least,
                    asset_code: ledger.asset().code.into(),
                    asset_scale: 9,
                },
                metadata: None,
                recipient: None,
                fixed_debit: true,
            })
            .unwrap();
        outgoing = Some(created.row.id);
    });
    let outgoing = outgoing.unwrap();
    sim.outgoing.push((0, 2, incoming.id, outgoing));
    (incoming.id, outgoing)
}

// spec/areas/payments.md#outgoing-payments, spec/areas/stream.md#responses:
// a path that gets dearer mid-payment stops an outgoing payment's money
// once its packets arrive short of its rate, its quote's. The node keeps
// that rate, so the receiver never gets less than it promises, and connects
// again until the deadline: once the path's price is back, the payment
// completes. Node 1, the only hop between node 0 and node 2, triples its
// fee once the payment is under way.
#[test]
fn a_payment_whose_path_got_dearer_keeps_its_rate_until_it_is_back() {
    let mut sim = charging_line(91, 10_000);
    // At no less than 98%: at the 1% fee, and not at 3%.
    let debit = 50 * Policies::DEFAULT.connector.max_packet_amount;
    let least = debit / 100 * 98;
    let (incoming, outgoing) = pay_at_least(&mut sim, debit, least);
    let row = |sim: &Sim| {
        sim.nodes[0]
            .store
            .outgoing_payment(outgoing)
            .unwrap()
            .unwrap()
    };
    let received = |sim: &Sim| {
        sim.nodes[2]
            .store
            .incoming_payment(incoming)
            .unwrap()
            .map_or(0, |row| row.received_amount)
    };
    // Never less than its rate promises, at every step.
    let at_rate = |sim: &Sim| {
        u128::from(received(sim)) * u128::from(debit)
            >= u128::from(row(sim).sent_amount) * u128::from(least)
    };
    assert!(
        sim.run_until_or(sim.now + 60_000, |sim| received(sim) > 0),
        "nothing received"
    );
    assert_eq!(row(&sim).state, OutgoingState::Sending, "paid at once");
    charge(&mut sim, 30_000);
    let mark = sim.trace.len();
    let short = |sim: &Sim| {
        sim.trace[mark..].iter().any(|line| {
            line.contains(" 0 Payment(OutgoingReconnects") && line.contains("short of its rate")
        })
    };
    assert!(
        sim.run_until_or(sim.now + 120_000, |sim| short(sim) || !at_rate(sim)),
        "its money never stopped"
    );
    assert!(at_rate(&sim), "delivered below its rate");
    let stalled = received(&sim);
    sim.run_until(sim.now + 60_000);
    assert!(at_rate(&sim), "delivered below its rate");
    assert_eq!(row(&sim).state, OutgoingState::Sending);
    assert!(received(&sim) <= stalled + Policies::DEFAULT.connector.max_packet_amount);
    charge(&mut sim, 10_000);
    let done = |sim: &Sim| row(sim).state != OutgoingState::Sending;
    assert!(
        sim.run_until_or(sim.now + 300_000, done),
        "still sending once the path's price was back"
    );
    assert_eq!(row(&sim).state, OutgoingState::Completed, "{:?}", row(&sim));
    assert!(received(&sim) >= least, "{} < {least}", received(&sim));
    check_books(&sim).unwrap();
}
