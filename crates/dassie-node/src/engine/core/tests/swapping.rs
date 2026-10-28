//! The swap service in two nodes (spec/decisions/peer-swaps.md,
//! spec/areas/connector.md#the-swap-service): node 0 and node 1 peer over
//! the stub with commitments, in dollars, and
//! [`StubTwo`](super::StubTwo), in XRP in a hub, each publishing its own
//! snapshots as a node does ([`OwnSnapshots`](crate::engine::OwnSnapshots));
//! each swaps with the other
//! by itself when the other's quotes, valued at its own curve, clear its
//! costs, and as its owner asks.

use std::collections::BTreeSet;
use std::sync::Arc;

use dassie_feed::FEED_VALIDITY_MS;
use dassie_ilp::ErrorCode;
use dassie_ledger::{LedgerId, Name};

use super::super::{EngineError, OwnerSwap, Quoting, SnapshotSource, SwapEnd, SwapStats};
use super::STUB_TWO;
use super::swaps::{holds, pair};
use crate::sim::{Sim, key};
use crate::store::PeerSettings;

const USD: LedgerId = LedgerId::StubUsd;
/// [`StubTwo`](super::StubTwo)'s ledger.
const HUB: LedgerId = LedgerId::HubXrplTestnetXrp;
/// A dollar, and an XRP, in the books' units.
const UNIT: u64 = 1_000_000_000;

/// The seeds each test runs with: it holds for any.
const SEEDS: [u64; 3] = [101, 102, 103];

/// A snapshot that stays as it is: a node's, frozen.
struct Frozen(Option<Arc<[u8]>>);

impl SnapshotSource for Frozen {
    fn current(&mut self, _: u64, _: &Quoting<'_>) -> Option<Arc<[u8]>> {
        self.0.clone()
    }
}

/// Puts `amount` of the owner's on `scheme`'s ledger, as a deposit.
fn deposit(sim: &mut Sim, index: usize, scheme: &str, amount: u64) {
    sim.step(index, |core| {
        let id = Name::new(scheme).unwrap();
        let ledger = core.schemes[&id].scheme.ledger();
        let mut effects = Vec::new();
        let active = core.schemes.get_mut(&id).unwrap();
        active.scheme.deposit(amount, &mut effects).unwrap();
        core.apply_effects(id, ledger, &effects).unwrap();
    });
}

/// The pair, node 0 heavy in dollars and node 1 in
/// XRP by as much: each holds a hundred dollars and starts with fifty
/// dollars' worth of XRP, and node 1 gets a hundred more, so that between
/// them they hold as much of each.
fn skewed(seed: u64) -> Sim {
    let mut sim = pair(seed);
    deposit(&mut sim, 1, STUB_TWO, 200 * UNIT);
    sim
}

/// How far node `index` is from its targets, an even split of its value
/// between dollars and XRP: the gap between the two, and their sum, in
/// dollars at spot (a dollar buys two XRP).
fn skew(sim: &Sim, index: usize) -> (u128, u128) {
    let (usd, xrp) = holds(sim, index);
    let (usd, xrp) = (u128::from(usd), u128::from(xrp) / 2);
    (usd.abs_diff(xrp), usd + xrp)
}

/// Whether `after` is closer to the targets than `before`, as a share of
/// the value.
fn closer((gap, total): (u128, u128), (gap_before, total_before): (u128, u128)) -> bool {
    gap * total_before < gap_before * total
}

fn stats(sim: &Sim, index: usize) -> SwapStats {
    sim.core(index).swap_stats()
}

// Two core nodes skewed opposite ways swap until neither's quotes clear
// the other's costs, and both end closer to their targets.
#[test]
fn skewed_nodes_swap_toward_their_targets() {
    for seed in SEEDS {
        let mut sim = skewed(seed);
        let before = [skew(&sim, 0), skew(&sim, 1)];
        sim.run_until(sim.now + 600_000);
        let swapped = stats(&sim, 0).fulfilled + stats(&sim, 1).fulfilled;
        assert!(swapped > 0, "seed {seed}: no swap");
        for (index, &before) in before.iter().enumerate() {
            let after = skew(&sim, index);
            assert!(
                closer(after, before),
                "seed {seed}: node {index} went from {before:?} to {after:?}"
            );
            assert!(stats(&sim, index).by_itself);
        }
        // Quiet: each node has a new snapshot of the other, made after the
        // last swap, and nothing clears.
        let sent = [stats(&sim, 0).sent, stats(&sim, 1).sent];
        let seen = [0, 1].map(|index| {
            let other = key(1 - index).node_id();
            sim.core(index)
                .peer_snapshot(&other)
                .map(|snapshot| snapshot.sequence_ms)
        });
        sim.run_until(sim.now + 2 * FEED_VALIDITY_MS);
        for index in 0..2 {
            assert_eq!(stats(&sim, index).sent, sent[index], "seed {seed}");
            assert_eq!(stats(&sim, index).in_flight, 0);
            let other = key(1 - index).node_id();
            let now = sim
                .core(index)
                .peer_snapshot(&other)
                .map(|snapshot| snapshot.sequence_ms);
            assert!(now.is_some() && now != seen[index], "seed {seed}");
        }
        // A step that changes nothing it decides by has it look at nothing.
        for index in 0..2 {
            sim.step(index, |_| {});
            let rounds = sim.core(index).swap_rounds();
            sim.step(index, |_| {});
            assert_eq!(sim.core(index).swap_rounds(), rounds, "seed {seed}");
        }
        crate::sim::checks::check_hub_packets(&sim).unwrap();
    }
}

// A node whose owner turned swapping off swaps nothing by itself and reads
// no snapshot, while its peer, skewed the other way, still swaps with it.
#[test]
fn a_node_with_swapping_off_never_swaps() {
    for seed in SEEDS {
        let mut sim = skewed(seed);
        sim.step(0, |core| {
            let settings = PeerSettings {
                swapping: false,
                ..core.peer_settings()
            };
            core.set_peer_settings(settings).unwrap();
        });
        sim.run_until(sim.now + 300_000);
        let off = stats(&sim, 0);
        assert!(!off.by_itself);
        assert_eq!((off.sent, off.peers), (0, 0), "seed {seed}");
        assert_eq!(sim.core(0).peer_snapshots().count(), 0);
        assert!(
            stats(&sim, 1).fulfilled > 0,
            "seed {seed}: node 1 swapped nothing"
        );
    }
}

// A periphery node swaps by itself only toward targets its owner set: none
// by default. A core node swaps by itself while its owner lets it.
#[test]
fn a_periphery_node_without_targets_never_swaps_by_itself() {
    let store = Arc::new(crate::store::Store::open_in_memory().unwrap());
    let mut core = super::periphery_core(store);
    assert!(core.peer.is_periphery());
    assert!(core.peer_settings().swapping);
    assert!(!core.swaps_by_itself());
    assert!(!core.swap_stats().by_itself);
    let targets = dassie_connector::Targets::new(&[(USD, 1), (LedgerId::XrplTestnetXrp, 1)]);
    let settings = PeerSettings {
        swap_targets: targets.unwrap(),
        ..core.peer_settings()
    };
    core.set_peer_settings(settings).unwrap();
    assert!(core.swaps_by_itself());
    let off = PeerSettings {
        swapping: false,
        ..settings
    };
    core.set_peer_settings(off).unwrap();
    assert!(!core.swaps_by_itself());
}

// A peer that refuses for its rate (T05, the transport's 429) is backed off
// from, longer each time, as STREAM backs off: no swap goes to it before
// its backoff ends.
#[test]
fn a_rate_limited_swap_backs_off() {
    for seed in SEEDS {
        let mut sim = skewed(seed);
        let snapshots = |sim: &Sim| {
            (0..2).all(|index| {
                let other = key(1 - index).node_id();
                sim.core(index).peer_snapshot(&other).is_some()
            })
        };
        assert!(sim.run_until_or(sim.now + 60_000, snapshots), "seed {seed}");
        sim.transport_refusals.insert((0, 1), 429);
        sim.transport_refusals.insert((1, 0), 429);
        let start = sim.now;
        sim.run_until(start + 10_000);
        // When each node sent a swap that was refused for the rate, from
        // its swap endpoints.
        let refused = |index: usize| {
            let core = sim.core(index);
            let swaps: BTreeSet<_> = [USD, HUB]
                .into_iter()
                .filter_map(|ledger| core.connector.swap_endpoint(ledger))
                .collect();
            let rate_limited = dassie_connector::PacketState::Rejected(ErrorCode::T05_RATE_LIMITED);
            core.packets(None, 100_000)
                .into_iter()
                .filter(|record| swaps.contains(&record.source))
                .filter(|record| record.received_at_ms > start && record.state == rate_limited)
                .map(|record| record.received_at_ms)
                .collect::<BTreeSet<u64>>()
        };
        let (refused_0, refused_1) = (refused(0), refused(1));
        assert!(
            refused_0.len() + refused_1.len() >= 3,
            "seed {seed}: {refused_0:?} {refused_1:?}"
        );
        for rounds in [refused_0, refused_1] {
            // Each round waits at least the backoff, the first wait, then
            // twice the last.
            let rounds: Vec<u64> = rounds.into_iter().collect();
            let mut least = 100;
            for window in rounds.windows(2) {
                assert!(window[1] - window[0] >= least, "seed {seed}: {rounds:?}");
                least *= 2;
            }
        }
        let backed = stats(&sim, 0).backoffs + stats(&sim, 1).backoffs;
        assert!(backed >= 3, "seed {seed}");
    }
}

// A peer that refuses swaps (F02) is not asked again until its snapshot
// changes.
#[test]
fn a_refused_swap_waits_for_the_next_snapshot() {
    for seed in SEEDS {
        let mut sim = skewed(seed);
        // Node 1 publishes the snapshot it has, and refuses every swap.
        sim.step(1, |core| {
            let own = core.own_snapshot().cloned();
            assert!(own.is_some(), "seed {seed}");
            core.set_snapshot_source(Box::new(Frozen(own)));
            core.connector.set_offer_swaps(false);
        });
        let refused = |sim: &Sim| stats(sim, 0).refused > 0;
        assert!(sim.run_until_or(sim.now + 120_000, refused), "seed {seed}");
        // What was in flight beside it comes back refused too.
        sim.run_until(sim.now + 5_000);
        let (sent, refusals) = (stats(&sim, 0).sent, stats(&sim, 0).refused);
        assert_eq!(stats(&sim, 0).in_flight, 0);
        sim.run_until(sim.now + 30_000);
        assert_eq!(stats(&sim, 0).sent, sent, "seed {seed}: asked again");
        assert_eq!(stats(&sim, 0).refused, refusals, "seed {seed}");
        // Its next snapshot is asked again.
        sim.step(1, |core| {
            let own = crate::engine::OwnSnapshots::new(key(1));
            core.set_snapshot_source(Box::new(own));
            let settings = PeerSettings {
                rebalance_ppm: 40_000,
                ..core.peer_settings()
            };
            core.set_peer_settings(settings).unwrap();
            core.connector.set_offer_swaps(false);
        });
        let again = |sim: &Sim| stats(sim, 0).refused > refusals;
        assert!(sim.run_until_or(sim.now + 120_000, again), "seed {seed}");
        assert!(stats(&sim, 0).sent > sent);
    }
}

// The owner swaps on a peer from one method to another, up to an amount,
// at a least rate, and hears what moved.
#[test]
fn the_owner_swaps_at_a_rate() {
    for seed in SEEDS {
        let mut sim = pair(seed);
        // Node 1 only offers swaps, so that nothing but the owner's swap
        // moves node 0's holdings.
        sim.step(1, |core| {
            let settings = PeerSettings {
                swapping: false,
                ..core.peer_settings()
            };
            core.set_peer_settings(settings).unwrap();
        });
        let one = key(1).node_id();
        let has_snapshot = |sim: &Sim| sim.core(0).peer_snapshot(&one).is_some();
        assert!(
            sim.run_until_or(sim.now + 60_000, has_snapshot),
            "seed {seed}"
        );
        let before = holds(&sim, 0);
        // Five dollars for at least 1.9 XRP each: a dollar buys two.
        let swap = OwnerSwap {
            peer: one,
            pay_on: USD,
            back_on: HUB,
            amount: 5 * UNIT,
            rate: (19, 10),
        };
        sim.step(0, |core| core.start_owner_swap(7, swap).unwrap());
        let done = |sim: &Sim| !sim.swaps_done.is_empty();
        assert!(sim.run_until_or(sim.now + 200_000, done), "seed {seed}");
        let (node, id, report) = sim.swaps_done[0];
        assert_eq!((node, id), (0, 7));
        assert!(
            report.paid > 0 && report.paid <= swap.amount,
            "seed {seed}: {report:?}"
        );
        assert!(report.received * 10 >= report.paid * 19, "{report:?}");
        if report.paid < swap.amount {
            assert_ne!(report.ended, SwapEnd::Done, "{report:?}");
        }
        let after = holds(&sim, 0);
        assert_eq!(before.0 - after.0, report.paid, "seed {seed}");
        assert_eq!(after.1 - before.1, report.received, "seed {seed}");
        // At a rate no snapshot promises, nothing moves.
        let dear = OwnerSwap {
            rate: (3, 1),
            ..swap
        };
        sim.step(0, |core| core.start_owner_swap(8, dear).unwrap());
        assert!(sim.run_until_or(sim.now + 10_000, |sim| sim.swaps_done.len() > 1));
        let (_, id, report) = sim.swaps_done[1];
        assert_eq!(id, 8);
        assert_eq!((report.paid, report.received), (0, 0));
        assert_eq!(report.ended, SwapEnd::Exhausted);
        // A rate of nothing, as one left out reads, would take whatever the
        // peer's snapshot says: refused, as is a rate over nothing.
        for rate in [(0, 1), (0, 0), (1, 0)] {
            let free = OwnerSwap { rate, ..swap };
            sim.step(0, |core| {
                let refused = core.start_owner_swap(10, free);
                assert!(matches!(refused, Err(EngineError::Invalid(_))), "{rate:?}");
            });
        }
        // Without a snapshot of the pair, none starts.
        let other = OwnerSwap {
            back_on: LedgerId::XrplTestnetXrp,
            ..swap
        };
        sim.step(0, |core| assert!(core.start_owner_swap(9, other).is_err()));
    }
}
