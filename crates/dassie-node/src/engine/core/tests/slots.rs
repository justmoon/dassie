//! A peer's connector endpoint has a slot per settlement method of its
//! peering (spec/decisions/settlement-methods.md,
//! spec/areas/connector.md#routing): every method carries packets, each
//! keeping the ledger it came in on, a method short of room answers T04 and
//! moves no packet to another, a converting packet takes its hop's method
//! with the best rate, and what each hop earned weighs per peer and ledger.
//! Three nodes run the stub with commitments, in dollars, and
//! [`StubTwo`](super::StubTwo), in XRP in a hub; nodes 0 and 2 ban each
//! other, so what one pays the other passes node 1.

use std::collections::BTreeMap;

use dassie_connector::{EndpointId, PacketState};
use dassie_identity::NodeId;
use dassie_ilp::ErrorCode;
use dassie_ledger::{AccountName, AccountPath, LedgerId, Name};

use super::{STUB_TWO, scheme_id, stub_two_packets};
use crate::engine::{Core, NewIncoming, NewOutgoing};
use crate::sim::{START, Settling, Sim, key};
use crate::store::{ForeignAmount, OutgoingState, PaymentId, PeerSettings};

const USD: LedgerId = LedgerId::StubUsd;
/// [`StubTwo`](super::StubTwo)'s ledger.
const HUB: LedgerId = LedgerId::HubXrplTestnetXrp;

/// The ledgers of the slots of `node`'s endpoint for `peer` that new
/// packets may take, or are being removed, in order.
fn slots(core: &Core, peer: NodeId) -> Vec<LedgerId> {
    let endpoint = EndpointId::Peer(core.peer_endpoints[&peer]);
    core.connector
        .slots(endpoint)
        .filter(|slot| !slot.removed)
        .map(|slot| slot.ledger)
        .collect()
}

/// Whether node `index` holds credit toward node `peer` on the method on
/// `scheme`: its hold there is funded.
fn holds(sim: &Sim, index: usize, peer: usize, scheme: &str) -> bool {
    let name = Name::new(scheme).unwrap();
    sim.core(index)
        .hub_books(name, key(peer).node_id())
        .is_some_and(|(credit, _, _)| credit > 0)
}

/// Three nodes in a line, each running the stub with commitments, unless
/// `two_only` says node 2 runs [`StubTwo`](super::StubTwo) alone, and
/// [`StubTwo`](super::StubTwo); nodes 0 and 2 ban each other at once. With
/// the rates every node learns, so packets may convert. Runs until each
/// pair in the line has a peering on every hub both are in, and every hold
/// on them is funded, at most twenty minutes.
fn line(seed: u64, two_only: bool) -> Sim {
    let mut sim = Sim::with(seed, 3, 1, BTreeMap::new(), Settling::StubHub(4));
    sim.loss_per_mille = 0;
    let (zero, two) = (key(0).node_id(), key(2).node_id());
    for index in 0..3 {
        let now = sim.now;
        sim.step(index, |core| {
            core.add_scheme(STUB_TWO, r#"{"commitments":4}"#.into())
                .unwrap();
            if index == 2 && two_only {
                core.remove_scheme("stub").unwrap();
            }
            let rates = crate::sim::RATES.map(|(code, rate)| (code, rate, now));
            core.set_rates(&rates);
            match index {
                0 => core.ban_node(two).unwrap(),
                2 => core.ban_node(zero).unwrap(),
                _ => {}
            }
        });
    }
    let methods = |sim: &Sim, a: usize, b: usize| {
        let other = key(b).node_id();
        sim.core(a)
            .peer
            .peering_of(&other)
            .map_or(0, |peering| peering.methods.len())
    };
    let shared = if two_only { 1 } else { 2 };
    let ready = |sim: &Sim| {
        let schemes = |index: usize| -> &[&str] {
            if index == 2 && two_only {
                &[STUB_TWO]
            } else {
                &["stub", STUB_TWO]
            }
        };
        [(0, 1, 2), (1, 0, 2), (1, 2, shared), (2, 1, shared)]
            .into_iter()
            .all(|(a, b, count)| {
                methods(sim, a, b) == count
                    && schemes(a)
                        .iter()
                        .filter(|scheme| schemes(b).contains(scheme))
                        .all(|scheme| holds(sim, a, b, scheme))
            })
    };
    assert!(
        sim.run_until_or(START + 1_200_000, ready),
        "the line did not peer on its hubs"
    );
    sim
}

/// An outgoing payment of `amount` from node `from` on `ledger` to a
/// receiver of node `to` on `into`, of at least `least`.
fn pay(
    sim: &mut Sim,
    (from, ledger): (usize, LedgerId),
    (to, into): (usize, LedgerId),
    amount: u64,
    least: u64,
) {
    let mut incoming = None;
    sim.step(to, |core| {
        let payment = core
            .create_incoming_payment(NewIncoming {
                ledger: into,
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
    sim.step(from, |core| {
        let created = core
            .create_outgoing_payment(NewOutgoing {
                ledger,
                wallet_address: "https://sim.example/.well-known/pay".into(),
                client: None,
                grant: None,
                quote_id: None,
                receiver: format!("https://n{to}.test/incoming-payments/{}", incoming.id),
                destination: incoming.ilp_address.clone(),
                shared_secret: incoming.shared_secret,
                debit_amount: amount,
                receive_amount: ForeignAmount {
                    value: least,
                    asset_code: into.asset().code.into(),
                    asset_scale: 9,
                },
                metadata: None,
                recipient: None,
                fixed_debit: true,
            })
            .unwrap();
        outgoing = Some(created.row.id);
    });
    sim.outgoing
        .push((from, to, incoming.id, outgoing.unwrap()));
}

/// The state of the `nth` outgoing payment, and what its receiver got.
fn outcome(sim: &Sim, nth: usize) -> (OutgoingState, u64) {
    let (from, to, incoming, outgoing): (usize, usize, PaymentId, PaymentId) = sim.outgoing[nth];
    let state = sim.nodes[from]
        .store
        .outgoing_payment(outgoing)
        .unwrap()
        .unwrap()
        .state;
    let received = sim.nodes[to]
        .store
        .incoming_payment(incoming)
        .unwrap()
        .map_or(0, |row| row.received_amount);
    (state, received)
}

/// Runs until the `nth` outgoing payment ended, at most ten minutes, and
/// says how.
fn ended(sim: &mut Sim, nth: usize) -> (OutgoingState, u64) {
    let done = |sim: &Sim| outcome(sim, nth).0 != OutgoingState::Sending;
    sim.run_until_or(sim.now + 600_000, done);
    outcome(sim, nth)
}

/// What node `index` booked on its FX accounts on `ledger`: nothing
/// unless a packet converted there.
fn converted(sim: &Sim, index: usize, ledger: LedgerId) -> i128 {
    let core = sim.core(index);
    [AccountName::RevenueFx, AccountName::ExpensesFx]
        .into_iter()
        .filter_map(|name| core.ledger.find(&AccountPath::new(ledger, name)))
        .filter_map(|id| core.ledger.account(id))
        .map(|account| account.balance().net_credit().abs())
        .sum()
}

// Each of a pair's two methods carries packets: a payment in dollars and one
// in XRP in a hub from node 0 to node 2 each cross node 1 on the method of
// their own ledger, so node 1 converts nothing, and the second method's
// scheme commits to and checks packets.
#[test]
fn a_pair_on_two_methods_carries_packets_on_both_without_converting() {
    let mut sim = line(51, false);
    for (a, b) in [(0, 1), (1, 0), (1, 2), (2, 1)] {
        let other = key(b).node_id();
        assert_eq!(slots(sim.core(a), other), [USD, HUB], "node {a}");
    }
    let before = stub_two_packets();
    pay(&mut sim, (0, USD), (2, USD), 1_000_000, 1_000_000);
    assert_eq!(ended(&mut sim, 0), (OutgoingState::Completed, 1_000_000));
    assert_eq!(
        stub_two_packets(),
        before,
        "a dollar packet went on the hub"
    );
    pay(&mut sim, (0, HUB), (2, HUB), 1_000_000, 1_000_000);
    assert_eq!(ended(&mut sim, 1), (OutgoingState::Completed, 1_000_000));
    assert!(stub_two_packets() > before, "no packet went on the hub");
    for ledger in [USD, HUB] {
        assert_eq!(
            converted(&sim, 1, ledger),
            0,
            "node 1 converted on {ledger:?}"
        );
    }
    crate::sim::checks::check_hub_packets(&sim).unwrap();
}

// A method is chosen by structure, never by room: node 1 never funds its
// hold toward node 2 on the stub, so a dollar payment from node 0 gets
// nowhere, refused for want of room there, and not one packet moves to the
// method over the other hub, which has room.
#[test]
fn a_method_short_of_room_answers_t04_and_moves_no_packet() {
    let mut sim = line(52, false);
    let two = key(2).node_id();
    // Spent, as a node whose settlements on it keep failing: the hold has
    // no room left, and is never funded again.
    sim.step(1, |core| {
        core.settle
            .backoff
            .insert((two, scheme_id("stub")), (u64::MAX, 1));
    });
    let before = stub_two_packets();
    // Far more than the hold covers, so it is spent within the payment.
    let amount = 50_000_000_000;
    pay(&mut sim, (0, USD), (2, USD), amount, 1);
    sim.run_until(sim.now + 300_000);
    let (_, received) = outcome(&sim, 0);
    assert!(received < amount, "the hold covered it all");
    // Node 1 refused packets to node 2 for want of room on the stub.
    let core = sim.core(1);
    let endpoint = EndpointId::Peer(core.peer_endpoints[&two]);
    let t04 = PacketState::Rejected(ErrorCode::T04_INSUFFICIENT_LIQUIDITY);
    let refused = core
        .packets(None, 100_000)
        .into_iter()
        .any(|record| record.destination == Some(endpoint) && record.state == t04);
    assert!(refused, "no packet to node 2 answered T04");

    assert_eq!(
        stub_two_packets(),
        before,
        "a packet moved to the other hub"
    );
    assert_eq!(converted(&sim, 1, HUB), 0);
    crate::sim::checks::check_hub_packets(&sim).unwrap();
}

// A packet whose ledger its next hop has no method on converts, on the
// hop's method the price table gives the best rate for: node 2 runs only
// the second hub, so node 1 converts node 0's dollars into XRP in it.
#[test]
fn a_converting_packet_takes_its_hops_method() {
    let mut sim = line(53, true);
    let two = key(2).node_id();
    assert_eq!(slots(sim.core(1), two), [HUB]);
    let debit = 1_000_000;
    let least = crate::sim::least_across(USD, HUB, debit);
    pay(&mut sim, (0, USD), (2, HUB), debit, least);
    let (state, received) = ended(&mut sim, 0);
    assert_eq!(state, OutgoingState::Completed);
    assert!(received >= least, "{received} < {least}");
    assert!(converted(&sim, 1, HUB) > 0, "node 1 did not convert");
    crate::sim::checks::check_hub_packets(&sim).unwrap();
}

// What a hop earned weighs per peer and ledger: node 1 charges a fee, and
// the fees of a dollar payment count toward node 2's method in dollars
// alone, those of a payment in the hub toward its method there.
#[test]
fn weights_stay_per_peer_and_ledger() {
    let mut sim = line(54, false);
    let two = key(2).node_id();
    sim.step(1, |core| {
        core.set_peer_settings(PeerSettings {
            connector_fee_ppm: 10_000,
            ..PeerSettings::DEFAULT
        })
        .unwrap();
    });
    let earned = |sim: &Sim, ledger| {
        let core = sim.core(1);
        let endpoint = EndpointId::Peer(core.peer_endpoints[&two]);
        core.connector
            .earnings(endpoint, ledger, core.now_ms())
            .unwrap()
    };
    pay(&mut sim, (0, USD), (2, USD), 1_000_000, 900_000);
    assert_eq!(ended(&mut sim, 0).0, OutgoingState::Completed);
    assert!(earned(&sim, USD) > 0, "no weight on dollars");
    assert_eq!(earned(&sim, HUB), 0, "weight in the hub from dollars");
    pay(&mut sim, (0, HUB), (2, HUB), 1_000_000, 900_000);
    assert_eq!(ended(&mut sim, 1).0, OutgoingState::Completed);
    assert!(earned(&sim, HUB) > 0, "no weight in the hub");
}
