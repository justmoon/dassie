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

use dassie_feed::{FEED_VALIDITY_MS, MethodTag, Ratio, method_tag};
use dassie_ilp::ErrorCode;
use dassie_ledger::{LedgerId, Name};

use super::super::{
    EngineError, MAX_ROOM_BACKOFF_MS, OwnerSwap, PACKETS_PER_PEER, Quoting, SnapshotSource,
    SwapEnd, SwapStats,
};
use super::swaps::{holds, pair};
use super::{STUB_TWO, stub_two_room_toward};
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

/// Whether each node has a snapshot of the other.
fn both_read_snapshots(sim: &Sim) -> bool {
    (0..2).all(|index| {
        let other = key(1 - index).node_id();
        sim.core(index).peer_snapshot(&other).is_some()
    })
}

/// Node `index`'s name in the books, as its peers' schemes know it.
fn name(index: usize) -> Name {
    Name::new(key(index).node_id().as_str()).unwrap()
}

/// The swaps node `index` sent from its swap endpoints since `since`, in
/// the order they went.
fn swaps_sent(sim: &Sim, index: usize, since: u64) -> Vec<dassie_connector::PacketRecord> {
    let core = sim.core(index);
    let swaps: BTreeSet<_> = [USD, HUB]
        .into_iter()
        .filter_map(|ledger| core.connector.swap_endpoint(ledger))
        .collect();
    core.packets(None, 100_000)
        .into_iter()
        .filter(|record| swaps.contains(&record.source) && record.received_at_ms > since)
        .collect()
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
            // Nothing pending: it backs off from no peer and waits for no
            // snapshot, so its quiet is its decision, as the owner sees.
            assert_eq!(stats(&sim, index).backing_off, 0, "seed {seed}");
            assert_eq!(stats(&sim, index).waiting_for_snapshot, 0, "seed {seed}");
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

// spec/areas/rate-feed.md#market-maker: the owner reads the snapshots the
// service reads (`ListPeerSnapshots`), decoded, each method named by the
// scheme the node runs whose tag it is, as a reader that swaps through
// the node prices them.
#[test]
fn the_owner_reads_the_peers_snapshots_by_scheme() {
    let mut sim = skewed(SEEDS[0]);
    let both = |sim: &Sim| {
        (0..2).all(|index| {
            let other = key(1 - index).node_id();
            sim.core(index).peer_snapshot(&other).is_some()
        })
    };
    assert!(sim.run_until_or(sim.now + 60_000, both));
    let views = sim.core(0).peer_snapshot_views();
    assert_eq!(views.len(), 1);
    let view = &views[0];
    assert_eq!(view.peer, key(1).node_id());
    let read = sim.core(0).peer_snapshot(&view.peer);
    assert_eq!(Some(&view.snapshot), read.as_ref());
    assert_eq!(view.schemes.len(), view.snapshot.methods.len());
    for (method, scheme) in view.snapshot.methods.iter().zip(&view.schemes) {
        let scheme = scheme.as_deref().expect("a scheme the node runs");
        assert_eq!(dassie_feed::method_tag(scheme), method.tag);
    }
    let schemes: BTreeSet<&str> = view.schemes.iter().flatten().map(String::as_str).collect();
    assert_eq!(schemes, BTreeSet::from(["stub", STUB_TWO]));
    // A node that does not swap reads none, and lists none.
    sim.step(0, |core| {
        let settings = PeerSettings {
            swapping: false,
            ..core.peer_settings()
        };
        core.set_peer_settings(settings).unwrap();
    });
    sim.run_until(sim.now + 10_000);
    assert!(sim.core(0).peer_snapshot_views().is_empty());
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
        assert!(
            sim.run_until_or(sim.now + 60_000, both_read_snapshots),
            "seed {seed}"
        );
        sim.transport_refusals.insert((0, 1), 429);
        sim.transport_refusals.insert((1, 0), 429);
        let start = sim.now;
        // When each node sent a swap that was refused for the rate.
        let refused = |sim: &Sim, index: usize| {
            let rate_limited = dassie_connector::PacketState::Rejected(ErrorCode::T05_RATE_LIMITED);
            swaps_sent(sim, index, start)
                .into_iter()
                .filter(|record| record.state == rate_limited)
                .map(|record| record.received_at_ms)
                .collect::<BTreeSet<u64>>()
        };
        // A swap that was in flight as the refusals began is lost with its
        // answer, and waits for its expiry.
        let rounds = |sim: &Sim| refused(sim, 0).len() + refused(sim, 1).len() >= 3;
        assert!(sim.run_until_or(start + 60_000, rounds), "seed {seed}");
        sim.run_until(sim.now + 5_000);
        let (refused_0, refused_1) = (refused(&sim, 0), refused(&sim, 1));
        for rounds in [refused_0, refused_1] {
            // Each round waits at least the backoff, the first wait, then
            // twice the last, up to a hub funding's landing.
            let rounds: Vec<u64> = rounds.into_iter().collect();
            let mut least = 100;
            for window in rounds.windows(2) {
                assert!(window[1] - window[0] >= least, "seed {seed}: {rounds:?}");
                least = (least * 2).min(MAX_ROOM_BACKOFF_MS);
            }
        }
        let backed = stats(&sim, 0).backoffs + stats(&sim, 1).backoffs;
        assert!(backed >= 3, "seed {seed}");
        // The owner sees whom it backs off from now; once the backoff ends
        // it looks again, and the peers take the swaps.
        let backing = |sim: &Sim| stats(sim, 0).backing_off + stats(sim, 1).backing_off > 0;
        assert!(sim.run_until_or(sim.now + 10_000, backing), "seed {seed}");
        sim.transport_refusals.clear();
        let fulfilled = stats(&sim, 0).fulfilled + stats(&sim, 1).fulfilled;
        sim.run_until(sim.now + 60_000);
        assert!(stats(&sim, 0).fulfilled + stats(&sim, 1).fulfilled > fulfilled);
    }
}

// A swap the node refused itself for want of room in its own hold toward
// the peer backs off from nothing: the method waits, sending nothing, until
// the hold grows, and the swap goes in the step its funding lands in.
#[test]
fn a_swap_short_of_its_own_hold_goes_as_its_funding_lands() {
    for seed in SEEDS {
        let mut sim = skewed(seed);
        assert!(
            sim.run_until_or(sim.now + 60_000, both_read_snapshots),
            "seed {seed}"
        );
        // Node 1, heavy in XRP, pays in the hub: its hold toward node 0
        // there has no room.
        stub_two_room_toward(name(0), Some(0));
        let short = |sim: &Sim| stats(sim, 1).short_of_room > 0;
        assert!(sim.run_until_or(sim.now + 120_000, short), "seed {seed}");
        let waiting =
            |sim: &Sim| stats(sim, 1).waiting_for_room == 1 && stats(sim, 1).in_flight == 0;
        assert!(sim.run_until_or(sim.now + 10_000, waiting), "seed {seed}");
        let (sent, backoffs) = (stats(&sim, 1).sent, stats(&sim, 1).backoffs);
        // While the hold stays as it is, it sends nothing, for far longer
        // than the backoffs it took before.
        sim.run_until(sim.now + 20_000);
        assert_eq!(stats(&sim, 1).sent, sent, "seed {seed}");
        assert_eq!(stats(&sim, 1).waiting_for_room, 1, "seed {seed}");
        // Its funding lands: the swap goes in that step, backing off from
        // nothing.
        let now = sim.now;
        sim.step(1, |_| stub_two_room_toward(name(0), Some(u64::MAX)));
        assert_eq!(sim.now, now);
        assert!(stats(&sim, 1).sent > sent, "seed {seed}");
        assert_eq!(stats(&sim, 1).waiting_for_room, 0, "seed {seed}");
        assert_eq!(stats(&sim, 1).backoffs, backoffs, "seed {seed}");
        stub_two_room_toward(name(0), None);
        let fulfilled = stats(&sim, 1).fulfilled;
        let swapped = |sim: &Sim| stats(sim, 1).fulfilled > fulfilled;
        assert!(sim.run_until_or(sim.now + 30_000, swapped), "seed {seed}");
        crate::sim::checks::check_hub_packets(&sim).unwrap();
    }
}

// A swap the peer, or its way back, refused for want of room backs off,
// but never longer than a hub funding's landing: once its peer's hold
// toward it has no room, node 0 tries again at most that long after each
// refusal, and at least the first wait.
#[test]
fn a_swap_the_peer_is_short_for_backs_off_at_most_a_landing() {
    for seed in SEEDS {
        let mut sim = skewed(seed);
        assert!(
            sim.run_until_or(sim.now + 60_000, both_read_snapshots),
            "seed {seed}"
        );
        // Node 0's swaps come back in the hub, from node 1's hold toward
        // it, which has no room.
        stub_two_room_toward(name(0), Some(0));
        let start = sim.now;
        sim.run_until(start + 60_000);
        let sent = swaps_sent(&sim, 0, start);
        let short = dassie_connector::PacketState::Rejected(ErrorCode::T04_INSUFFICIENT_LIQUIDITY);
        assert!(
            sent.iter().filter(|record| record.state == short).count() >= 10,
            "seed {seed}: {sent:?}"
        );
        // From the last outcomes it heard, refusals for room, to the next
        // swap it sent. An F08 has it go again at once, smaller, and so
        // does a swap fulfilled; outcomes heard in the moment a swap went
        // may have come after it.
        for (index, record) in sent.iter().enumerate() {
            let went = record.received_at_ms;
            let earlier = &sent[..index];
            let heard = earlier
                .iter()
                .filter_map(|earlier| earlier.resolved_at_ms)
                .filter(|&heard| heard < went)
                .max();
            let next = earlier
                .last()
                .is_some_and(|before| before.received_at_ms < went);
            let only_short = |heard| {
                earlier
                    .iter()
                    .filter(|earlier| earlier.resolved_at_ms == Some(heard))
                    .all(|earlier| earlier.state == short)
            };
            if let Some(heard) = heard
                && only_short(heard)
                && next
            {
                let waited = went - heard;
                assert!(
                    (100..=MAX_ROOM_BACKOFF_MS).contains(&waited),
                    "seed {seed}: waited {waited} ms"
                );
            }
        }
        assert_eq!(stats(&sim, 0).short_of_room, 0, "seed {seed}");
        stub_two_room_toward(name(0), None);
    }
}

// A peer that stays short is tried at most a round of swaps per hub
// funding's landing, however long it stays so.
#[test]
fn a_peer_that_stays_short_is_tried_once_a_landing() {
    for seed in SEEDS {
        let mut sim = skewed(seed);
        assert!(
            sim.run_until_or(sim.now + 60_000, both_read_snapshots),
            "seed {seed}"
        );
        stub_two_room_toward(name(0), Some(0));
        // Past the first waits, which double up to a landing.
        sim.run_until(sim.now + 10_000);
        let (sent, backoffs) = (stats(&sim, 0).sent, stats(&sim, 0).backoffs);
        let window = 300_000;
        sim.run_until(sim.now + window);
        let rounds = window / MAX_ROOM_BACKOFF_MS + 1;
        let most = PACKETS_PER_PEER as u64 * rounds;
        let tried = stats(&sim, 0).sent - sent;
        assert!(tried <= most, "seed {seed}: {tried} swaps in {window} ms");
        assert!(stats(&sim, 0).backoffs - backoffs <= most, "seed {seed}");
        // And it still tries: it backs off from it now.
        assert!(tried > 0, "seed {seed}");
        stub_two_room_toward(name(0), None);
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
        // The owner sees that it waits for the peer's next snapshot.
        assert_eq!(stats(&sim, 0).waiting_for_snapshot, 1, "seed {seed}");
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

/// `a / b` in lowest terms.
fn reduced(a: u128, b: u128) -> (u128, u128) {
    let (mut x, mut y) = (a, b);
    while y != 0 {
        (x, y) = (y, x % y);
    }
    (a / x, b / x)
}

/// The rate the first step of `snapshot`'s walk from `from` to `to`
/// promises, exactly, as an owner names one: the bid less the first drop,
/// times one less it again, over the ask (spec/areas/rate-feed.md#reading-a-snapshot).
fn first_step_rate(snapshot: &dassie_feed::Snapshot, from: MethodTag, to: MethodTag) -> (u64, u64) {
    let (bid, ask) = (
        snapshot.method(from).unwrap().bid,
        snapshot.method(to).unwrap().ask,
    );
    let keep = u128::from(10_000 - snapshot.grid_bp[0]);
    let (mut numerator, mut denominator) = (
        u128::from(bid.mantissa()) * keep * keep,
        u128::from(ask.mantissa()) * 100_000_000,
    );
    let shift = bid.exponent() - ask.exponent();
    if shift >= 0 {
        numerator <<= shift;
    } else {
        denominator <<= -shift;
    }
    let (numerator, denominator) = reduced(numerator, denominator);
    (
        u64::try_from(numerator).unwrap(),
        u64::try_from(denominator).unwrap(),
    )
}

// An owner's swap at exactly the rate the peer's snapshot promises meets
// an F08 and goes on in smaller packets until all of it is paid. Each
// packet's minimum is its size at the owner's rate, rounded up, a unit
// above what the snapshot promises it, rounded down, but no more than the
// peer's own price gives. Node 0 takes packets larger than the connectors
// pass, so that the F08 names exactly the most that does.
#[test]
fn an_owner_swap_at_the_promised_rate_goes_on_after_an_f08() {
    let largest = crate::node::Policies::DEFAULT.connector.max_packet_amount;
    for seed in SEEDS {
        let mut sim = pair(seed);
        sim.step(1, |core| {
            let settings = PeerSettings {
                swapping: false,
                ..core.peer_settings()
            };
            core.set_peer_settings(settings).unwrap();
        });
        sim.step(0, |core| core.settle.max_packet = 2 * largest);
        let one = key(1).node_id();
        let has_snapshot = |sim: &Sim| sim.core(0).peer_snapshot(&one).is_some();
        assert!(
            sim.run_until_or(sim.now + 60_000, has_snapshot),
            "seed {seed}"
        );
        let snapshot = sim.core(0).peer_snapshot(&one).unwrap().clone();
        let (from, to) = (method_tag(STUB_TWO), method_tag("stub"));
        let rate = first_step_rate(&snapshot, from, to);
        // Within the first step, all of it at its rate: the largest packet
        // and a quarter, too much for a rounding to take.
        let amount = largest + largest / 4;
        let promised = snapshot.swap_at_least(from, to, Ratio::new(rate.0, rate.1).unwrap());
        assert!(
            promised.unwrap().amount_in >= amount,
            "seed {seed}: {promised:?}"
        );
        // Exactly: a rate any higher gets nothing.
        let dearer = Ratio::new(rate.0 + 1, rate.1).unwrap();
        let dearer = snapshot.swap_at_least(from, to, dearer);
        assert_eq!(dearer.map(|swap| swap.amount_in), Some(0), "seed {seed}");
        let before = holds(&sim, 0);
        let swap = OwnerSwap {
            peer: one,
            pay_on: HUB,
            back_on: USD,
            amount,
            rate,
        };
        sim.step(0, |core| core.start_owner_swap(7, swap).unwrap());
        let done = |sim: &Sim| !sim.swaps_done.is_empty();
        assert!(sim.run_until_or(sim.now + 200_000, done), "seed {seed}");
        let (node, id, report) = sim.swaps_done[0];
        assert_eq!((node, id), (0, 7));
        let stats = stats(&sim, 0);
        assert!(stats.too_large > 0, "seed {seed}: no F08, {stats:?}");
        assert_eq!(report.ended, SwapEnd::Done, "seed {seed}: {report:?}");
        assert_eq!(report.paid, swap.amount, "seed {seed}: {report:?}");
        let (paid, received) = (u128::from(report.paid), u128::from(report.received));
        assert!(
            received * u128::from(rate.1) >= paid * u128::from(rate.0),
            "seed {seed}: {report:?}"
        );
        let after = holds(&sim, 0);
        assert_eq!(after.0 - before.0, report.received, "seed {seed}");
        assert_eq!(before.1 - after.1, report.paid, "seed {seed}");
    }
}
