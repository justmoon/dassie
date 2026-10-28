//! A peer's connector endpoint has a slot per settlement method of its
//! peering (spec/decisions/settlement-methods.md,
//! spec/areas/connector.md#routing): every method carries packets, each
//! keeping the ledger it came in on, a method short of room answers T04 and
//! moves no packet to another, a converting packet takes its hop's method
//! with the best rate, what each hop earned weighs per peer and ledger, and
//! the packets in flight on a method dropped resolve on its slot, its hub's
//! scheme hearing of each.
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
    pay_with(sim, (from, ledger), (to, into), amount, least, false);
}

/// [`pay`], with a shared secret the receiver does not know if `forged`:
/// the receiver then rejects every packet.
fn pay_with(
    sim: &mut Sim,
    (from, ledger): (usize, LedgerId),
    (to, into): (usize, LedgerId),
    amount: u64,
    least: u64,
    forged: bool,
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
                shared_secret: if forged {
                    incoming.shared_secret.map(|byte| !byte)
                } else {
                    incoming.shared_secret
                },
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

// spec/areas/hubs.md#in-the-node, spec/decisions/settlement-methods.md: what
// a peer can cost a node on a method is bounded by the holds on that
// method's hub. While a payment in dollars and one in XRP cross node 1 in
// opposite directions, each on its own method, the credit node 1 extends
// each neighbor on each method is, after every step, its hold toward it on
// that method's hub and its settlements to it there on their way, never
// what the other method holds.
#[test]
fn each_methods_credit_is_its_own_hubs_holds() {
    let mut sim = line(58, false);
    pay(&mut sim, (0, USD), (2, USD), 1_000_000, 1_000_000);
    pay(&mut sim, (2, HUB), (0, HUB), 1_000_000, 1_000_000);
    assert_eq!(ended(&mut sim, 0), (OutgoingState::Completed, 1_000_000));
    assert_eq!(ended(&mut sim, 1), (OutgoingState::Completed, 1_000_000));
    for peer in [0, 2] {
        let other = key(peer).node_id();
        let name = Name::new(other.as_str()).unwrap();
        for scheme in ["stub", STUB_TWO] {
            let id = Name::new(scheme).unwrap();
            let core = sim.nodes[1].core.as_mut().unwrap();
            let (credit, _, settling) = core.hub_books(id, other).unwrap();
            let toward = core.with_commitments(id, |c| c.holds(name).toward);
            assert!(credit > 0, "node 1 extends node {peer} nothing on {scheme}");
            assert_eq!(
                Some(credit),
                toward.map(|toward| toward + settling),
                "node 1's credit to node {peer} on {scheme}"
            );
        }
    }
    crate::sim::checks::check_hub_credit(&sim).unwrap();
}

// A method dropped while a packet is in flight on it, and added again
// before the packet resolves, takes its slot back with the packet, last, as
// the peering then lists the method; every check of the endpoints holds
// after each step. Node 2's messages to node 1 are lost, so once node 1
// forwarded what node 0 sends node 2, it stays in flight, on node 0's
// dollar slot for node 1 too.
#[test]
fn a_method_back_before_its_packets_resolve_takes_its_slot_back() {
    let mut sim = line(55, false);
    let (one, two) = (key(1).node_id(), key(2).node_id());
    sim.cut.insert((2, 1));
    let pending = |sim: &Sim, index: usize, to: NodeId| {
        let core = sim.core(index);
        let endpoint = EndpointId::Peer(core.peer_endpoints[&to]);
        core.packets(None, 100_000).into_iter().any(|record| {
            record.destination == Some(endpoint) && record.state == PacketState::Pending
        })
    };
    let in_flight = |sim: &Sim| pending(sim, 0, one);
    pay(&mut sim, (0, USD), (2, USD), 1_000_000, 1_000_000);
    assert!(
        sim.run_until_or(sim.now + 10_000, |sim| pending(sim, 1, two)),
        "no packet in flight to node 2"
    );
    assert!(in_flight(&sim), "no packet in flight to node 1");
    sim.step(0, |core| {
        let now = core.now_ms();
        assert!(core.peer.drop_method(now, &one, scheme_id("stub")));
        core.after_peer();
        let endpoint = EndpointId::Peer(core.peer_endpoints[&one]);
        let waiting = core
            .connector
            .slots(endpoint)
            .any(|slot| slot.ledger == USD && slot.removed);
        assert!(waiting, "the dropped slot did not wait for its packet");
    });
    let methods = |sim: &Sim| {
        sim.core(0)
            .peer
            .peering_of(&one)
            .unwrap()
            .schemes()
            .collect::<Vec<_>>()
    };
    let back = |sim: &Sim| methods(sim).contains(&scheme_id("stub"));
    assert!(
        sim.run_until_or(sim.now + 30_000, back),
        "the method did not come back"
    );
    assert!(
        in_flight(&sim),
        "the packet resolved before the method came back"
    );
    assert_eq!(methods(&sim), [scheme_id(STUB_TWO), scheme_id("stub")]);
    assert_eq!(slots(sim.core(0), one), [HUB, USD]);
    sim.cut.clear();
    sim.run_until(sim.now + 120_000);
    assert!(!in_flight(&sim), "the packet never resolved");
    crate::sim::checks::check_hub_packets(&sim).unwrap();
}

/// What node `index`'s scheme `scheme` holds of the packets between it and
/// node `peer`.
fn held(sim: &mut Sim, index: usize, peer: usize, scheme: &str) -> dassie_settlement::Held {
    let peer = Name::new(key(peer).node_id().as_str()).unwrap();
    let core = sim.nodes[index].core.as_mut().unwrap();
    core.hub_peers()
        .into_iter()
        .find(|of| of.peer == peer && of.scheme.as_str() == scheme)
        .map(|of| of.held)
        .unwrap_or_default()
}

/// Whether node `index` has a packet to `to` in flight.
fn pending(sim: &Sim, index: usize, to: NodeId) -> bool {
    let core = sim.core(index);
    let endpoint = EndpointId::Peer(core.peer_endpoints[&to]);
    core.packets(None, 100_000)
        .into_iter()
        .any(|record| record.destination == Some(endpoint) && record.state == PacketState::Pending)
}

/// Whether node `index`'s peering with `other` has its method on the stub.
fn has_method(sim: &Sim, index: usize, other: NodeId) -> bool {
    sim.core(index)
        .peer
        .peering_of(&other)
        .unwrap()
        .schemes()
        .any(|scheme| scheme == scheme_id("stub"))
}

/// Node 0 pays node 2 in dollars while node 2's messages to node 1 are
/// lost, with a shared secret node 2 does not know if `forged`, and with
/// packets in flight node 0 and node 1 drop their dollar method, over the
/// stub's pretend hub; then the messages go through again. Checks that each
/// packet resolved on the dropped method's slot before the method came
/// back, and that neither node's scheme still holds a packet between them,
/// and says what became of node 0's packets to node 1.
fn dropped_with_packets_in_flight(seed: u64, forged: bool) -> Vec<PacketState> {
    let mut sim = line(seed, false);
    let (zero, one, two) = (key(0).node_id(), key(1).node_id(), key(2).node_id());
    sim.cut.insert((2, 1));
    pay_with(&mut sim, (0, USD), (2, USD), 1_000_000, 1_000_000, forged);
    assert!(
        sim.run_until_or(sim.now + 10_000, |sim| pending(sim, 1, two)),
        "no packet in flight to node 2"
    );
    assert!(pending(&sim, 0, one), "no packet in flight to node 1");
    assert!(
        held(&mut sim, 0, 1, "stub").ours > 0,
        "node 0 committed to none"
    );
    assert!(held(&mut sim, 1, 0, "stub").theirs > 0, "node 1 took none");
    for (index, other) in [(0, one), (1, zero)] {
        sim.step(index, |core| {
            let now = core.now_ms();
            assert!(core.peer.drop_method(now, &other, scheme_id("stub")));
            core.after_peer();
        });
    }
    let dropped = |sim: &Sim| !has_method(sim, 0, one) && !has_method(sim, 1, zero);
    assert!(dropped(&sim), "the method was not dropped");
    sim.cut.clear();
    assert!(
        sim.run_until_or(sim.now + 30_000, |sim| !pending(sim, 0, one)),
        "the packets never resolved"
    );
    assert!(
        dropped(&sim),
        "the method came back before its packets resolved"
    );
    assert_eq!(held(&mut sim, 0, 1, "stub").ours, 0, "node 0 kept a packet");
    assert_eq!(
        held(&mut sim, 1, 0, "stub").theirs,
        0,
        "node 1 kept a packet"
    );
    let endpoint = EndpointId::Peer(sim.core(0).peer_endpoints[&one]);
    let answered = sim
        .core(0)
        .packets(None, 100_000)
        .into_iter()
        .filter(|record| record.destination == Some(endpoint))
        .map(|record| record.state)
        .collect();
    // The payer's claim follows its reply to the Fulfill, within a few
    // seconds of the simulation's time.
    let mut claimed = false;
    for _ in 0..50 {
        claimed = held(&mut sim, 1, 0, "stub").unclaimed == 0;
        if claimed {
            break;
        }
        sim.run_until(sim.now + 100);
    }
    assert!(claimed, "node 1 kept a fulfillment no claim covers");
    crate::sim::checks::check_hub_packets(&sim).unwrap();
    answered
}

// A method over a hub dropped while a packet is in flight on it still
// hears what becomes of it, on its slot (spec/decisions/settlement-methods.md):
// fulfilled, the payee's scheme keeps the fulfillment and takes the payer's
// claim for it, and the payer's forgets the commitment it claimed.
#[test]
fn a_dropped_methods_fulfilled_packet_is_claimed_over_its_hub() {
    let answered = dropped_with_packets_in_flight(56, false);
    assert!(
        answered.contains(&PacketState::Fulfilled),
        "no packet fulfilled: {answered:?}"
    );
}

// Rejected, the payer's scheme releases the commitment, and the payee's the
// packet it took.
#[test]
fn a_dropped_methods_rejected_packet_is_released_over_its_hub() {
    let answered = dropped_with_packets_in_flight(56, true);
    assert!(
        answered
            .iter()
            .any(|state| matches!(state, PacketState::Rejected(_))),
        "no packet rejected: {answered:?}"
    );
    assert!(!answered.contains(&PacketState::Fulfilled));
}

/// Whether node `index` runs scheme `scheme`.
fn runs(sim: &Sim, index: usize, scheme: &str) -> bool {
    sim.core(index)
        .schemes
        .contains_key(&Name::new(scheme).unwrap())
}

/// What node `index`'s scheme `scheme` holds of the packets between it and
/// node `peer`, if the scheme runs.
fn held_if_run(
    sim: &mut Sim,
    index: usize,
    peer: usize,
    scheme: &str,
) -> Option<dassie_settlement::Held> {
    runs(sim, index, scheme).then(|| held(sim, index, peer, scheme))
}

// spec/decisions/settlement-methods.md, spec/areas/connector.md#settlement-method-slots:
// the owner's removal of a scheme drops its methods, but the scheme runs on
// while packets are in flight on their slots: what becomes of each is
// heard over its hub, both ways. Node 1, in the middle, removes the stub
// while node 0's payment to node 2 is in flight through it, and its two
// methods on it are dropped at once, as when collecting them was quick.
// Node 1's reply to node 2's Fulfill carries its claim, and node 0's claim
// in its reply to node 1's Fulfill is taken. The scheme goes only once the
// wait for what peers may still settle on it is over.
#[test]
fn a_scheme_removed_runs_until_its_dropped_methods_packets_are_claimed() {
    let mut sim = line(57, false);
    let (zero, one, two) = (key(0).node_id(), key(1).node_id(), key(2).node_id());
    sim.cut.insert((2, 1));
    pay(&mut sim, (0, USD), (2, USD), 1_000_000, 1_000_000);
    assert!(
        sim.run_until_or(sim.now + 10_000, |sim| pending(sim, 1, two)),
        "no packet in flight to node 2"
    );
    assert!(pending(&sim, 0, one), "no packet in flight to node 1");
    assert!(held(&mut sim, 1, 0, "stub").theirs > 0, "node 1 took none");
    assert!(
        held(&mut sim, 1, 2, "stub").ours > 0,
        "node 1 committed none"
    );
    sim.step(1, |core| {
        core.remove_scheme("stub").unwrap();
        let now = core.now_ms();
        for other in [zero, two] {
            assert!(core.peer.drop_method(now, &other, scheme_id("stub")));
        }
        core.after_peer();
    });
    let dropped_ms = sim.now;
    assert!(!has_method(&sim, 1, zero) && !has_method(&sim, 1, two));
    assert!(runs(&sim, 1, "stub"), "gone while its slots held packets");
    sim.cut.clear();
    assert!(
        sim.run_until_or(sim.now + 30_000, |sim| !pending(sim, 0, one)
            && !pending(sim, 1, two)),
        "the packets never resolved"
    );
    let endpoint = EndpointId::Peer(sim.core(0).peer_endpoints[&one]);
    let fulfilled = sim
        .core(0)
        .packets(None, 100_000)
        .into_iter()
        .any(|record| {
            record.destination == Some(endpoint) && record.state == PacketState::Fulfilled
        });
    assert!(fulfilled, "no packet fulfilled");
    // Each claim follows its reply to a Fulfill, within a few seconds of the
    // simulation's time: node 2 takes node 1's, node 1 node 0's.
    let mut claimed = false;
    for _ in 0..50 {
        let ours = held_if_run(&mut sim, 1, 0, "stub");
        let theirs = held(&mut sim, 2, 1, "stub");
        claimed = ours.is_some_and(|held| held.unclaimed == 0 && held.theirs == 0)
            && theirs.unclaimed == 0;
        if claimed {
            break;
        }
        sim.run_until(sim.now + 100);
    }
    assert!(claimed, "a fulfillment no claim covers was left");
    assert_eq!(held(&mut sim, 1, 2, "stub").ours, 0, "node 1 kept a packet");
    let wait = sim.core(1).removal_wait_ms();
    sim.run_until(dropped_ms + wait - 1_000);
    assert!(runs(&sim, 1, "stub"), "gone before its wait was over");
    assert!(
        sim.run_until_or(dropped_ms + wait + 10_000, |sim| !runs(sim, 1, "stub")),
        "the scheme stayed"
    );
    crate::sim::checks::check_hub_packets(&sim).unwrap();
}
