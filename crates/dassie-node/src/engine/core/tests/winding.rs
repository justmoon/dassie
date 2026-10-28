//! The lifecycle of settlement methods in the node
//! (spec/decisions/settlement-methods.md, spec/areas/network.md#settlement-methods):
//! a hub both nodes join is added to their peering; the owner's removal of a
//! scheme winds its methods down, collecting once within a bound, and the
//! scheme goes after, across a restart too, and the node starts again
//! without it, its methods kept across restarts; a method whose scheme cannot
//! receive, or is no longer run, is dropped at once. A stored peering
//! loading as one method is the store's to test. On a pair that runs the stub and [`StubTwo`](super::StubTwo),
//! each a pretend hub of its own.

use std::collections::BTreeMap;

use dassie_ledger::{LedgerId, Name};

use super::{STUB_TWO, scheme_id, stub_two_unpayable};
use crate::sim::{Settling, Sim, key};

/// The peer protocol's settings, as a node runs with them: a silent peer is
/// kept a day, so a method that cannot receive is dropped after a day.
const PEER: dassie_peer::Config = crate::node::Policies::DEFAULT.peer;

/// The most a method being wound down takes: its two waits, and a review's
/// slack.
const WIND_BOUND_MS: u64 = 2 * PEER.depeer_wait_ms + 10_000;

/// Two nodes that peer on the stub's hub, node 1 asking; once they are
/// peered, `join` says whether both join the second stub's hub too.
fn pair(seed: u64, join: bool) -> Sim {
    let mut sim = Sim::with(seed, 2, 1, BTreeMap::new(), Settling::StubHub(4));
    sim.loss_per_mille = 0;
    let peered = |sim: &Sim| {
        (0..2).all(|index| {
            let other = key(1 - index).node_id();
            sim.core(index).peer.peering_of(&other).is_some()
        })
    };
    assert!(
        sim.run_until_or(sim.now + 600_000, peered),
        "the pair did not peer"
    );
    if join {
        for index in 0..2 {
            sim.step(index, |core| {
                core.add_scheme(STUB_TWO, r#"{"commitments":4}"#.into())
                    .unwrap();
            });
        }
        assert!(
            sim.run_until_or(sim.now + 600_000, both_methods),
            "the hub both joined was not added"
        );
    }
    sim
}

/// Whether both nodes hold their peering with both methods.
fn both_methods(sim: &Sim) -> bool {
    (0..2).all(|index| methods(sim, index).len() == 2)
}

/// The schemes of node `index`'s methods with the other node, in order.
fn methods(sim: &Sim, index: usize) -> Vec<String> {
    let other = key(1 - index).node_id();
    sim.core(index)
        .peer
        .peering_of(&other)
        .map(|peering| peering.schemes().map(|scheme| scheme.to_string()).collect())
        .unwrap_or_default()
}

/// The second stub's method of node 0 with node 1, if it has one.
fn second(sim: &Sim) -> Option<dassie_peer::Method> {
    let other = key(1).node_id();
    sim.core(0)
        .peer
        .peering_of(&other)
        .and_then(|peering| peering.method(scheme_id(STUB_TWO)))
        .cloned()
}

/// Whether node `index` runs the second stub.
fn runs_second(sim: &Sim, index: usize) -> bool {
    sim.core(index)
        .schemes
        .contains_key(&Name::new(STUB_TWO).unwrap())
}

/// The place in the trace of the first line that has `what`.
fn line(sim: &Sim, what: &str) -> usize {
    sim.trace
        .iter()
        .position(|line| line.contains(what))
        .unwrap_or_else(|| panic!("no {what}"))
}

/// Whether node 0 has node 1's accounts on the second stub's ledger.
fn accounts_stay(sim: &Sim) -> bool {
    let other = Name::new(key(1).node_id().as_str()).unwrap();
    super::super::peers::has_accounts(&sim.core(0).ledger, LedgerId::HubXrplTestnetXrp, other)
}

// spec/decisions/settlement-methods.md: two nodes peered over one hub that
// both join a second hub add it to their peering, the first method staying
// the one it was made on.
#[test]
fn a_hub_both_nodes_join_is_added_to_their_peering() {
    let sim = pair(51, true);
    for index in 0..2 {
        assert_eq!(methods(&sim, index), ["stub", STUB_TWO], "node {index}");
    }
}

// spec/decisions/settlement-methods.md: the owner's removal of a scheme
// peers settle on is not refused. The scheme is offered to no peer, still
// runs, and its method is wound down: the peer is asked to settle on it
// once, and the method is dropped within its bound. What is left stays on
// its accounts, the peering is kept on the other method, and the scheme
// runs on for the wait for what the peer may still settle on it, then
// goes. Removing it again meanwhile succeeds, though a BTP token now uses
// its ledger: the removal is under way.
#[test]
fn a_scheme_removed_winds_its_method_down_then_goes() {
    let mut sim = pair(52, true);
    let mut removed = Ok(());
    sim.step(0, |core| removed = core.remove_scheme(STUB_TWO));
    removed.unwrap();
    let start = sim.now;
    assert!(
        runs_second(&sim, 0),
        "gone before its method was wound down"
    );
    assert!(second(&sim).is_some_and(|method| method.winding_down()));
    let view = sim
        .core(0)
        .ledger_views()
        .into_iter()
        .find(|view| view.scheme == STUB_TWO)
        .unwrap();
    assert_eq!(view.removing, Some(1));
    assert!(view.removal_blockers.is_empty());
    assert!(
        !sim.core(0).peer.schemes().contains(&scheme_id(STUB_TWO)),
        "still offered"
    );
    // The method's winding as last seen before it was dropped.
    let last = std::cell::Cell::new(None);
    assert!(
        sim.run_until_or(start + WIND_BOUND_MS, |sim| match second(sim) {
            Some(method) => {
                last.set(method.winding);
                false
            }
            None => true,
        }),
        "not dropped in time"
    );
    collected_before_dropped(&sim, last.get());
    let dropped = sim.now;
    let wait = sim.core(0).removal_wait_ms();
    let (mut token, mut again) = (None, Ok(()));
    sim.step(0, |core| {
        let ledger = LedgerId::HubXrplTestnetXrp.to_string();
        token = Some(core.create_token("t".into(), &ledger, None).unwrap());
        again = core.remove_scheme(STUB_TWO);
    });
    again.unwrap();
    let token = token.unwrap().view.id;
    sim.step(0, |core| core.revoke_token(token).unwrap());
    sim.run_until(dropped + wait - 1_000);
    assert!(runs_second(&sim, 0), "gone before its wait was over");
    assert!(
        sim.run_until_or(dropped + wait + 10_000, |sim| !runs_second(sim, 0)),
        "the scheme stayed"
    );
    assert!(
        sim.run_until_or(sim.now + 10_000, |sim| methods(sim, 1) == ["stub"]),
        "the peer kept the method"
    );
    assert_eq!(methods(&sim, 0), ["stub"]);
    assert!(accounts_stay(&sim), "its accounts went");
    assert!(
        sim.core(0)
            .ledger_views()
            .iter()
            .all(|view| view.scheme != STUB_TWO)
    );
    // Not added back, though the peer still runs it.
    sim.run_until(sim.now + 120_000);
    assert_eq!(methods(&sim, 0), ["stub"]);
    assert_eq!(methods(&sim, 1), ["stub"]);
}

/// Node 0's method on the second stub, last seen as `winding` before it
/// was dropped, was dropped once the peer's settlement on it was booked on
/// it and its answer said that was all, not when the wait for the answer
/// ran out; and the peer was asked once.
fn collected_before_dropped(sim: &Sim, winding: Option<dassie_peer::Winding>) {
    // The answer comes in the step the method is dropped in, so the trace
    // tells when that was.
    let winding = winding.expect("never wound down");
    assert!(winding.booked > 0, "nothing collected on it: {winding:?}");
    let asked = winding.asked_ms.expect("never asked");
    let dropped_ms: u64 = sim.trace[line(sim, " 0 PeerNode(MethodDropped {")]
        .split(' ')
        .next()
        .and_then(|ms| ms.parse().ok())
        .expect("a time");
    assert!(
        dropped_ms < asked + PEER.depeer_wait_ms,
        "dropped only when the wait ran out"
    );
    // Asked once, and the peer's settlement was booked before the method
    // was dropped.
    let (winding, dropped) = (
        line(sim, " 0 PeerNode(MethodWinding {"),
        line(sim, " 0 PeerNode(MethodDropped {"),
    );
    let between = &sim.trace[winding..dropped];
    let count = |what: &str| between.iter().filter(|line| line.contains(what)).count();
    assert_eq!(count(" 0 PeerNode(SettlementRequestAnswered {"), 1);
    assert!(
        count(" 0 Settlement(SettlementBooked {") >= 1,
        "nothing collected"
    );
}

// A removal carries on across a restart: the node starts with the scheme
// still being removed and its method still wound down, takes the steps
// again, and the scheme goes within the bound counted from the start.
#[test]
fn a_removal_carries_on_after_a_restart() {
    let mut sim = pair(53, true);
    let mut removed = Ok(());
    sim.step(0, |core| removed = core.remove_scheme(STUB_TWO));
    removed.unwrap();
    sim.crash(0);
    sim.run_until(sim.now + 1_000);
    sim.start(0);
    let start = sim.now;
    assert!(runs_second(&sim, 0), "the removal finished in the restart");
    assert!(
        sim.core(0).schemes[&Name::new(STUB_TWO).unwrap()]
            .removing
            .is_some(),
        "the removal was forgotten"
    );
    assert!(second(&sim).is_some_and(|method| method.winding_down()));
    let wait = sim.core(0).removal_wait_ms();
    assert!(
        sim.run_until_or(start + WIND_BOUND_MS + wait, |sim| !runs_second(sim, 0)),
        "the scheme stayed"
    );
    assert_eq!(methods(&sim, 0), ["stub"]);
    assert!(accounts_stay(&sim));
}

// spec/decisions/settlement-methods.md, spec/areas/node.md#persistence: a
// node restarted from its database keeps both methods of its peering, in
// the order they were agreed, none wound down; and a node whose owner
// removed a scheme its peer settled on starts again from its database once
// the scheme is gone, peered on the other method alone. Found in review:
// removing a scheme that peers used left their peerings without a ledger,
// and the node could not start again.
#[test]
fn both_methods_outlive_a_restart_and_a_removed_scheme_leaves_a_node_that_starts() {
    let mut sim = pair(58, true);
    sim.crash(1);
    sim.run_until(sim.now + 1_000);
    sim.start(1);
    assert_eq!(methods(&sim, 1), ["stub", STUB_TWO], "as stored");
    sim.run_until(sim.now + 60_000);
    for index in 0..2 {
        assert_eq!(methods(&sim, index), ["stub", STUB_TWO], "node {index}");
    }
    assert!(second(&sim).is_some_and(|method| !method.winding_down()));
    let mut removed = Ok(());
    sim.step(0, |core| removed = core.remove_scheme(STUB_TWO));
    removed.unwrap();
    let wait = sim.core(0).removal_wait_ms();
    assert!(
        sim.run_until_or(sim.now + WIND_BOUND_MS + wait, |sim| !runs_second(sim, 0)),
        "the scheme stayed"
    );
    sim.crash(0);
    sim.run_until(sim.now + 1_000);
    sim.start(0);
    assert!(!runs_second(&sim, 0), "the removed scheme came back");
    assert_eq!(methods(&sim, 0), ["stub"]);
    assert!(
        sim.run_until_or(sim.now + 60_000, |sim| methods(sim, 1) == ["stub"]),
        "the peer kept the method"
    );
    assert_eq!(methods(&sim, 0), ["stub"]);
}

// spec/areas/settlement.md#incoming-settlements: a peer's settlement on a
// scheme the node no longer runs, its owner having removed it, is
// acknowledged and books nothing, so the pair's settlement channel, which
// carries one at a time, goes on: the next settlement is taken.
#[test]
fn a_settlement_on_a_scheme_removed_does_not_hold_the_channel() {
    let mut sim = pair(57, true);
    let (zero, one) = (key(0).node_id(), key(1).node_id());
    let mut removed = Ok(());
    sim.step(0, |core| removed = core.remove_scheme(STUB_TWO));
    removed.unwrap();
    let wait = sim.core(0).removal_wait_ms();
    let idle = |sim: &Sim| {
        sim.core(1)
            .peer
            .channel(&zero)
            .is_some_and(|channel| channel.outbox.is_none())
    };
    assert!(
        sim.run_until_or(sim.now + WIND_BOUND_MS + wait + 10_000, |sim| {
            !runs_second(sim, 0) && idle(sim)
        }),
        "the scheme stayed"
    );
    let settled_in = |sim: &Sim| sim.core(0).peer.channel(&one).unwrap().settled_in;
    let before = settled_in(&sim);
    // One the peer sent late, as one whose message got through only after
    // the scheme went.
    for sent in 1..=2 {
        sim.step(1, |core| {
            let now = core.now_ms();
            core.peer
                .send_settlement(now, &zero, scheme_id(STUB_TWO), 5, vec![0; 16])
                .unwrap();
            core.after_peer();
        });
        assert!(
            sim.run_until_or(sim.now + 60_000, idle),
            "the settlement is sent again and again"
        );
        assert_eq!(settled_in(&sim), before + sent);
    }
    assert!(
        sim.trace
            .iter()
            .any(|line| line.contains(" 0 Settlement(PeerSettlementUnbooked {")),
        "not reported"
    );
}

// spec/decisions/settlement-methods.md: a scheme that cannot receive, as
// one over a hub that stalled, is offered to no peer, and its method takes
// no packet; the method is dropped at once, without collecting, once it has
// not been able to for as long as a silent peer is kept; the peering stays
// on the other.
#[test]
fn a_method_that_cannot_receive_is_dropped_after_a_silent_peers_time() {
    let mut sim = pair(54, true);
    stub_two_unpayable(true);
    let two = scheme_id(STUB_TWO);
    for index in 0..2 {
        let other = key(1 - index).node_id();
        sim.step(index, |core| {
            core.update_peer_schemes();
            assert!(
                !core.peer.schemes().contains(&two),
                "node {index} offers it"
            );
            assert!(!core.method_available(&other, two), "node {index}");
            assert!(core.method_available(&other, scheme_id("stub")));
        });
    }
    let start = sim.now;
    sim.run_until(start + PEER.peer_dead_ms - 10_000);
    assert!(both_methods(&sim), "dropped early");
    assert!(
        sim.run_until_or(start + PEER.peer_dead_ms + 20_000, |sim| {
            (0..2).all(|index| methods(sim, index) == ["stub"])
        }),
        "not dropped"
    );
    assert!(second(&sim).is_none());
    stub_two_unpayable(false);
}

// spec/areas/settlement.md#schemes-and-ledgers: a method on a scheme the node
// no longer runs, as one whose stored config no longer makes a scheme, is
// dropped at once, the peering kept on the other.
#[test]
fn a_method_on_a_scheme_no_longer_run_is_dropped_at_once() {
    let mut sim = pair(55, true);
    sim.crash(0);
    let mut batch = crate::store::LedgerBatch::default();
    let id = Name::new(STUB_TWO).unwrap();
    batch.schemes_removed.push(id);
    batch.schemes_added.push((id, "not a config".into()));
    sim.nodes[0].store.commit_ledger(&batch).unwrap();
    sim.start(0);
    assert!(!runs_second(&sim, 0));
    assert!(
        sim.run_until_or(sim.now + 30_000, |sim| {
            (0..2).all(|index| methods(sim, index) == ["stub"])
        }),
        "not dropped"
    );
}

// spec/decisions/settlement-methods.md: a method being wound down takes no
// new packets: a Prepare from the peer on it is refused for want of room
// (T04), as one on a method short of room is.
#[test]
fn a_prepare_on_a_method_wound_down_answers_t04() {
    use dassie_oer::{Decode, Encode};
    use dassie_peer::wire::{Content, Kind};

    use crate::engine::core::Output;

    let mut sim = pair(56, false);
    let (zero, one) = (key(0).node_id(), key(1).node_id());
    let prepare = dassie_ilp::Prepare {
        amount: 10,
        expires_at: dassie_ilp::Timestamp::from_unix_ms(sim.now + 30_000),
        execution_condition: [7; 32],
        destination: dassie_ilp::Address::parse(&format!("test.das.{zero}")).unwrap(),
        data: &[],
    }
    .to_vec();
    let body = Content::MethodPacket {
        request_id: 78,
        scheme: scheme_id("stub"),
        packet: &prepare,
        commitment: &[],
    }
    .to_vec();
    let from = key(1).identity();
    sim.step(0, |core| {
        let now = core.now_ms();
        assert!(core.peer.wind_down(now, &one, scheme_id("stub")));
        assert!(core.peer.method_winding(&one, scheme_id("stub")));
        core.peer_inbound(dassie_peer::InboundId(9_998), from, &body, None);
        let t04 = |packet: &[u8]| {
            matches!(dassie_ilp::Response::decode_exact(packet),
                Ok(dassie_ilp::Response::Reject(reject))
                    if reject.code == dassie_ilp::ErrorCode::T04_INSUFFICIENT_LIQUIDITY)
        };
        let refused = core.outputs_peer.iter().any(|output| match output {
            Output::PeerRequest(request)
                if request.to == one && request.kind == Kind::InterledgerPacket =>
            {
                matches!(
                    Content::decode_exact(&request.body),
                    Ok(Content::InterledgerPacket { request_id: 78, packet }) if t04(packet)
                )
            }
            _ => false,
        });
        assert!(refused, "{:?}", core.outputs_peer);
    });
}
