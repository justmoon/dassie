//! Settlement methods that take nothing new (spec/decisions/settlement-methods.md,
//! spec/areas/network.md#settlement-methods): one over a hub that failed,
//! whose scheme can no longer receive and whose methods are dropped as any
//! that cannot; and one being wound down, which the regular settlement
//! check asks nothing of, its one request being the wind-down's.

use std::collections::BTreeMap;

use dassie_ledger::Name;

use super::{STUB_TWO, scheme_id};
use crate::sim::{START, Settling, Sim, key};

/// The peer protocol's settings, as a node runs with them: a silent peer is
/// kept a day, so a method that cannot receive is dropped after a day.
const PEER: dassie_peer::Config = crate::node::Policies::DEFAULT.peer;

/// How often the node asks a peer short of room to settle.
const ASK_EVERY_MS: u64 = crate::node::Policies::DEFAULT.settlement_request_every_ms;

/// The schemes of node `index`'s methods with node `other`, in order.
fn methods(sim: &Sim, index: usize, other: usize) -> Vec<String> {
    sim.core(index)
        .peer
        .peering_of(&key(other).node_id())
        .map(|peering| peering.schemes().map(|scheme| scheme.to_string()).collect())
        .unwrap_or_default()
}

/// When node 0 last asked node 1 to settle on the second stub in its
/// regular check, if it did.
fn asked_on_two(sim: &Sim) -> Option<u64> {
    let other = key(1).node_id();
    sim.core(0)
        .settle
        .asked
        .get(&(other, scheme_id(STUB_TWO)))
        .copied()
}

// spec/decisions/settlement-methods.md: a method being wound down is asked
// to settle once, by the wind-down; the regular check, which asks a peer
// short of room every minute, asks nothing more on it, while the wind-down
// waits for an answer that does not come.
#[test]
fn a_method_wound_down_is_asked_only_by_its_wind_down() {
    let mut sim = Sim::with(61, 2, 1, BTreeMap::new(), Settling::StubHub(4));
    sim.loss_per_mille = 0;
    let (zero, one) = (key(0).node_id(), key(1).node_id());
    for index in 0..2 {
        sim.step(index, |core| {
            core.add_scheme(STUB_TWO, r#"{"commitments":4}"#.into())
                .unwrap();
        });
    }
    // Node 1 never settles with node 0 on the second stub, so node 0's
    // room there stays short, and it asks again at every check.
    sim.step(1, |core| {
        core.settle
            .backoff
            .insert((zero, scheme_id(STUB_TWO)), (u64::MAX, 1));
    });
    let both = |sim: &Sim| methods(sim, 0, 1).len() == 2 && methods(sim, 1, 0).len() == 2;
    assert!(
        sim.run_until_or(START + 600_000, |sim| both(sim)
            && asked_on_two(sim).is_some()),
        "the pair did not peer on both stubs, or node 0 never asked"
    );
    let first = asked_on_two(&sim).unwrap();
    assert!(
        sim.run_until_or(sim.now + 2 * ASK_EVERY_MS, |sim| {
            asked_on_two(sim).is_some_and(|at| at > first)
        }),
        "node 0 did not ask again: the check below would show nothing"
    );
    // Node 0 winds the method down; its request to settle reaches node 1
    // no more, so the wind-down waits for the answer.
    sim.cut.insert((0, 1));
    sim.step(0, |core| {
        let now = core.now_ms();
        assert!(core.peer.wind_down(now, &one, scheme_id(STUB_TWO)));
    });
    let start = sim.now;
    // Within the wind-down's wait for the answer, the regular check comes
    // round at least once.
    const _: () = assert!(PEER.depeer_wait_ms > ASK_EVERY_MS + 2_000);
    sim.run_until(start + PEER.depeer_wait_ms - 1_000);
    assert!(
        sim.core(0).peer.method_winding(&one, scheme_id(STUB_TWO)),
        "no longer wound down: the check below would show nothing"
    );
    // Each request to settle that never reached node 1 is asked again at
    // the next check, so the regular check would ask every few seconds:
    // only the wind-down asks, once. The stub has room, and is not asked.
    let winding = sim
        .trace
        .iter()
        .position(|line| line.contains(" 0 PeerNode(MethodWinding {"))
        .expect("never wound down");
    let asks = sim.trace[winding..]
        .iter()
        .filter(|line| line.ends_with(" 0 asks 1 to settle"))
        .count();
    assert_eq!(
        asks, 1,
        "the regular check asked on a method being wound down"
    );
    // The stub, which is not wound down, is still asked as it was.
    sim.cut.clear();
    assert!(
        sim.run_until_or(sim.now + 2 * PEER.depeer_wait_ms, |sim| {
            methods(sim, 0, 1) == ["stub"]
        }),
        "the method was not dropped within its bound"
    );
}

/// The hub scheme of the run's one hub, by its ID.
fn hub_scheme() -> Name {
    Name::new(dassie_hub::member::SCHEME_ID).unwrap()
}

/// Whether node `index`'s hub scheme can receive a peer's settlement.
fn hub_receives(sim: &Sim, index: usize) -> bool {
    sim.core(index).schemes[&hub_scheme()].scheme.can_receive()
}

// spec/areas/hubs.md#the-members-side, spec/decisions/settlement-methods.md:
// a pair peered over a hub and over the stub with commitments. The hub
// loses f + 1 validators and stalls: once the validators still up have kept
// naming the same final block for as long as the hub's stall, each member's
// hub scheme can no longer receive, the node offers it to no peer, and its
// method is dropped, without collecting, once it could not receive for as
// long as a silent peer is kept, the peering kept on the stub.
#[test]
fn a_failed_hubs_method_is_dropped_and_the_peering_kept() {
    let mut sim = Sim::with(62, 2, 0, BTreeMap::new(), Settling::Hub(4));
    sim.loss_per_mille = 0;
    for index in 0..2 {
        sim.step(index, |core| {
            core.add_scheme("stub", r#"{"commitments":4}"#.into())
                .unwrap();
        });
    }
    assert!(
        sim.run_until_or(START + 600_000, crate::sim::hubbed::joined),
        "the members did not join the hub"
    );
    sim.join_network(1);
    let both = |sim: &Sim| methods(sim, 0, 1).len() == 2 && methods(sim, 1, 0).len() == 2;
    assert!(
        sim.run_until_or(sim.now + 600_000, both),
        "the pair did not peer over the hub and the stub: {:?}",
        methods(&sim, 0, 1)
    );
    assert!((0..2).all(|index| hub_receives(&sim, index)));
    let hub = sim.hub_at(0);
    let stall = hub.rules.terms.timers.stall;
    let lost: Vec<usize> = hub.honest().take(hub.faults() + 1).collect();
    for index in lost {
        sim.crash_validator(index);
    }
    let failed = sim.now;
    // The hub stalls within the stall and a view; each member judges it a
    // stall after.
    let judged = |sim: &Sim| (0..2).all(|index| !hub_receives(sim, index));
    sim.run_until(failed + stall);
    assert!(
        (0..2).all(|index| hub_receives(&sim, index)),
        "judged failed before a stall"
    );
    assert!(
        sim.run_until_or(
            failed + 2 * stall + dassie_hub::consensus::MAX_VIEW_TIMEOUT_MS,
            judged
        ),
        "a member's hub scheme can still receive"
    );
    let since = sim.now;
    let hub_id = scheme_id(dassie_hub::member::SCHEME_ID);
    for index in 0..2 {
        assert!(
            !sim.core(index).peer.schemes().contains(&hub_id),
            "node {index} still offers the failed hub"
        );
    }
    // The judgment holds once the validators are gone too, as they may be
    // once a hub that stalled paid out.
    let left: Vec<usize> = sim.hub_at(0).honest().collect();
    for index in left {
        if sim.hub_at(0).validators[index].driver.is_some() {
            sim.crash_validator(index);
        }
    }
    sim.run_until(since + PEER.peer_dead_ms - 60_000);
    assert_eq!(methods(&sim, 0, 1).len(), 2, "dropped early");
    assert!(
        sim.run_until_or(since + PEER.peer_dead_ms + 60_000, |sim| {
            methods(sim, 0, 1) == ["stub"] && methods(sim, 1, 0) == ["stub"]
        }),
        "the failed hub's method was not dropped: {:?}",
        methods(&sim, 0, 1)
    );
}
