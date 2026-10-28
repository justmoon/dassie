//! Peer swaps between daemons over settlement hubs, on tokio's paused time
//! (spec/decisions/peer-swaps.md, spec/areas/connector.md#the-swap-service,
//! spec/areas/testing.md#members-of-settlement-hubs-on-paused-time): four
//! validators run an XRP hub and a DSUSD hub on the fake XRP Ledger, and
//! two core nodes join both and peer over both, a method each, each
//! reading the other's snapshots and swapping with it by itself.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

mod support;

use dassie_feed::{FEED_HEARTBEAT_MS, FEED_MIN_INTERVAL_MS};
use dassie_node::clock::Clock;
use dassie_node::rpc::proto;
use dassie_xrpl::network::TESTNET;
use support::hubs::{HubAsset, HubNet, Joining, Member, pass, until};
use support::rates::{RatesServer, coinbase};
use support::swaps::{XRP_PER_USD, gain_ppm, held, peer_snapshot, rated, set_spread, swap_status};

const BOTH: &[HubAsset] = &[HubAsset::Xrp, HubAsset::Dollar];

/// Each owner's spread, in parts per million: 0.5%. The curves of nodes so
/// skewed differ by about 0.8% at the start, so each swap gains a little
/// beyond the spread, and they stop once the curves are within it: after a
/// few dollars, some score of the largest packets each way, rather than
/// once their curves meet.
const SPREAD_PPM: u32 = 5_000;

/// The most either gains, in parts per million, on a swap at the top of
/// the other's book once they stopped. A node swaps while what the other's
/// snapshot promises, valued at its own curve, clears one plus its costs;
/// the top of the book promises a little more than any step of the walk,
/// the grid's first drop on each ladder, and a node's own bids, spread and
/// all, value the two assets as its curve does, the spread folded into
/// both. So once they stopped, the gain here is at most a node's costs over
/// these hubs, a fee per largest packet, and those two drops: well below
/// this, and this below the gain the skew starts them at.
const QUIET_PPM: i128 = 2_000;

/// How long a node takes, at most, to push the other a snapshot made after
/// a given moment: its heartbeat, and the least interval between two.
const FRESH_SNAPSHOT_MS: u64 = FEED_HEARTBEAT_MS + FEED_MIN_INTERVAL_MS;

/// The most times the test looks for the two to have stopped: swapping
/// that goes on through so many heartbeats never stops.
const CALM_ROUNDS: usize = 20;

/// How far `member` is from its targets, an even split of its value: the
/// gap between its dollars and its XRP in dollars at the rate, and their
/// sum, by its own books.
async fn skew(net: &HubNet, member: &Member) -> (u64, u64) {
    let dollars = held(net, member, HubAsset::Dollar).await;
    let xrp = held(net, member, HubAsset::Xrp).await / 2;
    (dollars.abs_diff(xrp), dollars + xrp)
}

/// Whether the node's swap service has nothing pending: no swap in flight,
/// no peer backed off from or waited on for its next snapshot. Each of
/// those only defers its next decision.
fn idle(status: &proto::GetSwapStatusResponse) -> bool {
    status.in_flight == 0 && status.backing_off == 0 && status.waiting_for_snapshot == 0
}

/// Each way of a swap between `d1` and `d2`, and what it gains the node
/// that pays, by their snapshots as each holds the other's (`None` where a
/// side is closed, as while a rate the node converts at is stale); and the
/// two snapshots' sequences. `None` while either holds none of the other.
async fn gains(net: &HubNet, d1: &Member, d2: &Member) -> Option<([Way; 4], (u64, u64))> {
    let (xrp, dollars) = (
        net.hub(HubAsset::Xrp).scheme,
        net.hub(HubAsset::Dollar).scheme,
    );
    let of_d2 = peer_snapshot(net, &d1.node, &d2.key).await?;
    let of_d1 = peer_snapshot(net, &d2.node, &d1.key).await?;
    let ways = [
        ("d1 pays XRP", gain_ppm(&of_d1, &of_d2, xrp, dollars)),
        ("d1 pays dollars", gain_ppm(&of_d1, &of_d2, dollars, xrp)),
        ("d2 pays XRP", gain_ppm(&of_d2, &of_d1, xrp, dollars)),
        ("d2 pays dollars", gain_ppm(&of_d2, &of_d1, dollars, xrp)),
    ];
    Some((ways, (of_d2.sequence, of_d1.sequence)))
}

/// A way of swapping, and what it gains the node that pays, in parts per
/// million.
type Way = (&'static str, Option<i128>);

/// Waits until `d1` and `d2` stopped swapping, and returns what each way
/// gains then. They stopped once neither sent a swap since a moment when
/// nothing was pending, and each has since been pushed a snapshot of the
/// other made after that moment: each decided again on the other's quotes
/// as they are after every swap, and sent nothing. And both snapshots
/// price every way, so that they decided on open quotes.
async fn stopped(net: &HubNet, d1: &Member, d2: &Member) -> [Way; 4] {
    for _ in 0..CALM_ROUNDS {
        until("both idle, a swap made", async || {
            let (one, two) = (swap_status(&d1.node).await, swap_status(&d2.node).await);
            idle(&one) && idle(&two) && one.fulfilled + two.fulfilled > 0
        })
        .await;
        let calm_at = net.clock.now_ms();
        let sent = [
            swap_status(&d1.node).await.sent,
            swap_status(&d2.node).await.sent,
        ];
        pass(FRESH_SNAPSHOT_MS).await;
        let (one, two) = (swap_status(&d1.node).await, swap_status(&d2.node).await);
        if !idle(&one) || !idle(&two) || [one.sent, two.sent] != sent {
            continue;
        }
        let Some((ways, (of_d2, of_d1))) = gains(net, d1, d2).await else {
            continue;
        };
        if of_d2 > calm_at && of_d1 > calm_at && ways.iter().all(|(_, gain)| gain.is_some()) {
            return ways;
        }
    }
    panic!("they kept swapping through {CALM_ROUNDS} heartbeats");
}

// spec/decisions/peer-swaps.md, spec/areas/connector.md#the-swap-service:
// each owner sets a spread, and the issuer sends the two nodes dollars
// beside the faucet's 1000 XRP, worth 500 dollars at the rate: d1 less than
// its XRP is worth and d2 more, so they are skewed opposite ways, d1 short
// of dollars and d2 short of XRP. Each reads the other's snapshot and swaps
// with it by itself while the other's quotes, valued at its own curve,
// clear its costs: d1 pays XRP and is paid back dollars, d2 the other way.
// Which of them makes a swap is a race, since either one's closes the
// gap: one may make them all. They swap until neither's quotes clear the
// other's costs, and both end closer to their targets.
#[tokio::test(start_paused = true)]
async fn two_core_nodes_skewed_opposite_ways_swap_until_neither_gains() {
    let net = HubNet::start(&TESTNET, BOTH).await;
    let members = net
        .members(
            &[
                Joining::of(81, BOTH).with_dollars("462"),
                Joining::of(82, BOTH).with_dollars("542"),
            ],
            &[],
        )
        .await;
    let [d1, d2] = [&members[0], &members[1]];
    net.peered(&[d1, d2]).await;
    for member in [d1, d2] {
        set_spread(&member.node, SPREAD_PPM).await;
    }
    // Before they read rates, which their snapshots need: nothing swaps.
    net.settled(&[d1, d2]).await;
    let before = [skew(&net, d1).await, skew(&net, d2).await];
    let dollars_and_xrp = async |member| {
        (
            held(&net, member, HubAsset::Dollar).await,
            held(&net, member, HubAsset::Xrp).await / 2,
        )
    };
    let (dollars, xrp) = dollars_and_xrp(d1).await;
    assert!(dollars < xrp, "d1 short of dollars: {dollars} and {xrp}");
    let (dollars, xrp) = dollars_and_xrp(d2).await;
    assert!(dollars > xrp, "d2 short of XRP: {dollars} and {xrp}");

    let rates = RatesServer::start(coinbase(&[("XRP", XRP_PER_USD)])).await;
    for member in [d1, d2] {
        rated(&member.node, &rates).await;
    }
    let ways = stopped(&net, d1, d2).await;
    for member in [d1, d2] {
        assert!(swap_status(&member.node).await.by_itself);
    }
    // Neither's quotes clear the other's costs: at the top of the other's
    // book, valued at the swapping node's own curve, no swap gains more
    // than its costs, well below the spread they started beyond.
    for (way, gain) in ways {
        let gain = gain.unwrap();
        assert!(gain < QUIET_PPM, "{way} gains {gain} ppm");
    }
    net.settled(&[d1, d2]).await;
    // Each is closer to an even split of its value: the gap between its
    // dollars and its XRP, as a share of the whole, fell.
    for (index, member) in [d1, d2].into_iter().enumerate() {
        let (gap_before, total_before) = before[index];
        let (gap, total) = skew(&net, member).await;
        assert!(
            u128::from(gap) * u128::from(total_before) < u128::from(gap_before) * u128::from(total),
            "d{} went from a gap of {gap_before} in {total_before} to {gap} in {total}",
            index + 1
        );
    }
    net.stop(members).await;
}
