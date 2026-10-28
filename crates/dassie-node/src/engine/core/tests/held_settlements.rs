//! Our settlements on two methods of one peering, which share its
//! settlement channel, one message at a time
//! (spec/areas/settlement.md#outgoing-settlements): one that executes while
//! a message on another method is still on its way to the peer. On a pair
//! that shares three pretend hubs, the stub's and those of the second and
//! third stubs, whose messages book nothing, as a hub's do, or book what
//! they state, as the stub's.

use std::collections::BTreeMap;

use dassie_ledger::LedgerId;

use super::{STUB_THREE, STUB_TWO, scheme_id, stub_two_bare};
use crate::sim::checks::{check_held, check_hub_credit};
use crate::sim::{START, Settling, Sim, key};

/// The ledgers of the second and third stubs, in that order.
const LEDGERS: [LedgerId; 2] = [LedgerId::HubXrplTestnetXrp, LedgerId::HubXrplTestnetDsusdUsd];

/// Two nodes that share three pretend hubs, the stub's and the second and
/// third stubs', whose messages book nothing if `bare`. Node 0 is held off
/// from settling with node 1 on every method, so that only the test
/// settles for it. Runs until each peers with the other on all three, then
/// a minute more, so that what node 1 settles has settled.
fn pair(seed: u64, bare: bool) -> Sim {
    stub_two_bare(bare);
    let mut sim = Sim::with(seed, 2, 1, BTreeMap::new(), Settling::StubHub(4));
    sim.loss_per_mille = 0;
    for index in 0..2 {
        sim.step(index, |core| {
            for scheme in [STUB_TWO, STUB_THREE] {
                core.add_scheme(scheme, r#"{"commitments":4}"#.into())
                    .unwrap();
            }
        });
    }
    let one = key(1).node_id();
    sim.step(0, |core| {
        for scheme in ["stub", STUB_TWO, STUB_THREE] {
            core.settle
                .backoff
                .insert((one, scheme_id(scheme)), (u64::MAX, 1));
        }
    });
    let all = |sim: &Sim| {
        (0..2).all(|index| {
            sim.core(index)
                .peer
                .peering_of(&key(1 - index).node_id())
                .is_some_and(|peering| peering.methods.len() == 3)
        })
    };
    assert!(
        sim.run_until_or(START + 600_000, all),
        "the pair did not peer over three hubs"
    );
    sim.run_until(sim.now + 60_000);
    sim
}

/// What node 0's books hold in settlement on each of [`LEDGERS`], which is
/// what its scheme there holds.
fn held(sim: &Sim) -> [i128; 2] {
    LEDGERS.map(|ledger| check_held(sim, 0, ledger).unwrap())
}

/// Node 1 goes down, and node 0, asked as it were, settles with it on the
/// second and third stubs in one step, before either executes: the first
/// to execute is announced, and not acknowledged while node 1 is down, so
/// the other executes behind that message. Returns what each answer said
/// was on its way, in the order of [`LEDGERS`].
fn settle_on_both(sim: &mut Sim) -> [u64; 2] {
    let one = key(1).node_id();
    assert!(!sim.core(0).settlements_in_flight());
    assert!(!sim.core(0).peer.settling(&one));
    sim.crash(1);
    let mut settled = [0; 2];
    sim.step(0, |core| {
        for (at, scheme) in [STUB_TWO, STUB_THREE].into_iter().enumerate() {
            core.settle.backoff.remove(&(one, scheme_id(scheme)));
            settled[at] = core.settlement_requested(one, scheme_id(scheme));
        }
    });
    sim.run_until(sim.now + 10_000);
    assert!(
        sim.core(0).peer.settling(&one),
        "the first settlement's message was not left on its way"
    );
    settled
}

// spec/areas/settlement.md#outgoing-settlements,
// spec/areas/hubs.md#in-the-node: a node in two hubs settles with a peer
// over both at once, and the second executes while the first's notice is
// still on its way. Its hub paid, and the scheme forgot it as it finalized
// it: it is posted then, and only its notice waits. A restart before that
// notice went finds nothing of it pending, which the scheme, having
// forgotten it, would cancel though it was paid: the books hold, on each
// hub's ledger, what its scheme holds, both settlements booked.
#[test]
fn a_settlement_executed_behind_another_message_survives_a_restart() {
    let mut sim = pair(81, true);
    let before = held(&sim);
    let settled = settle_on_both(&mut sim);
    assert!(settled.iter().all(|amount| *amount > 0), "{settled:?}");
    sim.crash(0);
    sim.start(0);
    sim.run_until(sim.now + 10_000);
    assert!(
        !sim.core(0).settlements_in_flight(),
        "a settlement still pending"
    );
    let after = held(&sim);
    for at in 0..2 {
        assert_eq!(
            before[at] - after[at],
            i128::from(settled[at]),
            "on {}",
            LEDGERS[at]
        );
    }
    // Node 1 back, what is on its way arrives, and the channel is free.
    let (zero, one) = (key(0).node_id(), key(1).node_id());
    sim.start(1);
    assert!(
        sim.run_until_or(sim.now + 120_000, |sim| !sim.core(0).peer.settling(&one)),
        "the message on its way never arrived"
    );
    let sent = sim.core(0).peer.channel(&one).unwrap().settled_out;
    assert_eq!(
        sim.core(1).peer.channel(&zero).unwrap().settled_in,
        sent
    );
    check_hub_credit(&sim).unwrap();
}

// spec/areas/settlement.md#outgoing-settlements: the settlement that
// executed behind another message is booked at once, and its notice goes
// once that message arrived, after it: the peer applies both, in order.
#[test]
fn a_notice_behind_another_message_follows_it_in_order() {
    let mut sim = pair(82, true);
    let before = held(&sim);
    let (zero, one) = (key(0).node_id(), key(1).node_id());
    let first = sim.core(0).peer.channel(&one).unwrap().settled_out + 1;
    let settled = settle_on_both(&mut sim);
    assert!(settled.iter().all(|amount| *amount > 0), "{settled:?}");
    // Node 0 settles nothing more meanwhile.
    sim.step(0, |core| {
        for scheme in [STUB_TWO, STUB_THREE] {
            core.settle
                .backoff
                .insert((one, scheme_id(scheme)), (u64::MAX, 1));
        }
    });
    assert!(
        !sim.core(0).settlements_in_flight(),
        "a settlement waits for the channel"
    );
    let after = held(&sim);
    for at in 0..2 {
        assert_eq!(before[at] - after[at], i128::from(settled[at]));
    }
    assert_eq!(sim.core(0).peer.channel(&one).unwrap().settled_out, first);
    sim.start(1);
    let both = |sim: &Sim| {
        !sim.core(0).peer.settling(&one)
            && sim
                .core(1)
                .peer
                .channel(&zero)
                .is_some_and(|channel| channel.settled_in == first + 1)
    };
    assert!(
        sim.run_until_or(sim.now + 120_000, both),
        "the notice that waited did not follow"
    );
    assert_eq!(
        sim.core(0).peer.channel(&one).unwrap().settled_out,
        first + 1
    );
    check_hub_credit(&sim).unwrap();
}

// spec/areas/settlement.md#outgoing-settlements: on schemes whose messages
// book what they state, a settlement is posted with its message, in one
// batch. So the node settles on one with a peer only while nothing else of
// ours to it is pending or on its way, and on no other method meanwhile:
// its message always finds the channel free, and none executed is ever
// held back, for a restart to hand back to a scheme that forgot it.
#[test]
fn a_settlement_whose_message_books_waits_for_the_channel() {
    let mut sim = pair(83, false);
    let before = held(&sim);
    let settled = settle_on_both(&mut sim);
    assert!(settled[0] > 0, "{settled:?}");
    assert_eq!(settled[1], 0, "settled on another method beside it");
    sim.crash(0);
    sim.start(0);
    sim.run_until(sim.now + 10_000);
    assert!(!sim.core(0).settlements_in_flight());
    let after = held(&sim);
    assert_eq!(before[0] - after[0], i128::from(settled[0]));
    assert_eq!(before[1], after[1]);
    let one = key(1).node_id();
    sim.step(0, |core| {
        assert!(
            !core.can_settle(&one, scheme_id(STUB_THREE)),
            "settles while a message is on its way"
        );
    });
}
