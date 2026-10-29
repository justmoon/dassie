//! The market maker's loop beside daemons, on tokio's paused time
//! (spec/areas/rate-feed.md#market-maker,
//! spec/areas/testing.md#the-market-maker-and-peer-swaps-on-paused-time):
//! four validators run an XRP hub and a DSUSD hub on the fake XRP Ledger,
//! beside its pool of XRP and DSUSD at the rate; core nodes short of DSUSD
//! join both, and so does the market maker's node, a periphery node pinned
//! to them. The market maker runs in the test's runtime, deciding by the
//! daemons' clock, its ticks tokio's timers.
//!
//! The daemons and the hubs are dassie-node's daemon tests' own
//! (`crates/dassie-node/tests/support/`), compiled into this test from
//! there: those tests cannot depend on this crate, which depends on
//! dassie-node.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

#[path = "../../dassie-node/tests/support/mod.rs"]
mod support;

use std::sync::Arc;

use dassie_cli::market_maker::MarketMaker;
use dassie_cli::market_maker::config;
use dassie_cli::market_maker::strategy::to_units;
use dassie_xrpl::Wallet;
use dassie_xrpl::amount::{Amount, IssuedAsset, TokenValue};
use dassie_xrpl::network::TESTNET;
use support::hubs::{HubAsset, HubNet, Joining, Member, until};
use support::pair::ca_file;
use support::rates::{RatesServer, coinbase};
use support::swaps::{UNIT, XRP_PER_USD, held, peer_snapshot, rated, swap_status};
use tokio_util::sync::CancellationToken;

const BOTH: &[HubAsset] = &[HubAsset::Xrp, HubAsset::Dollar];

/// What a core node is sent of DSUSD beside the faucet's 1000 XRP, worth
/// 500 dollars at the rate: about a twentieth of its value, so its curve
/// takes DSUSD in at its cap, 5% above spot (spec/areas/connector.md#inventory-pricing).
const CORE_DOLLARS: &str = "25";

/// The pool, deep, so that the market maker's swaps hardly move its price:
/// 50 000 DSUSD and 100 000 XRP, at the rate, with a fee of 0.5%.
const POOL_DOLLARS: &str = "50000";
const POOL_DROPS: u64 = 100_000_000_000;
const POOL_FEE: u32 = 500;

/// What the market maker's node is sent of DSUSD beside the faucet's 1000
/// XRP, worth 500 dollars at the rate: an even share of each, so that it
/// trades before it rebalances.
const MAKER_DOLLARS: &str = "500";

/// The most it pays through a core node in one trade, of either asset.
const CAP: u64 = 5 * UNIT;

/// Its curve, 1%, the least gain a trade must make after the cost of
/// undoing it, 0.2%, and how far the pool's price may move against a
/// rebalance, 1% (spec/decisions/market-maker-inventory.md).
const CURVE_PPM: u32 = 10_000;
const MARGIN_PPM: u32 = 2_000;
const SLIPPAGE_PPM: u32 = 10_000;

/// DSUSD, which the pool pairs with XRP.
const DSUSD: IssuedAsset = IssuedAsset::DSUSD;

/// The pool of XRP and DSUSD, as the fake ledger holds it now, in the
/// books' units: XRP and dollars.
fn pool(net: &HubNet) -> (u64, u64) {
    let pool = net.xrpl.ledger().pool(DSUSD).unwrap();
    let dollars = to_units(Amount::Token(DSUSD.amount(pool.value))).unwrap();
    (pool.drops * 1_000, dollars)
}

/// The hubs, and the pool beside them at the rate.
async fn hubs_and_pool() -> HubNet {
    let net = HubNet::start(&TESTNET, BOTH).await;
    net.xrpl
        .ledger()
        .set_pool(
            DSUSD,
            POOL_DROPS,
            TokenValue::parse(POOL_DOLLARS).unwrap(),
            POOL_FEE,
        )
        .unwrap();
    until("the pool made", async || {
        net.xrpl.ledger().pool(DSUSD).is_some()
    })
    .await;
    net
}

/// Has every member read its rates, and waits until `maker` holds a
/// snapshot of each of `cores` that prices both hubs.
async fn quoting(net: &HubNet, maker: &Member, cores: &[&Member]) -> RatesServer {
    let rates = RatesServer::start(coinbase(&[("XRP", XRP_PER_USD)])).await;
    for member in cores.iter().chain([&maker]) {
        rated(&member.node, &rates).await;
    }
    for core in cores {
        until(
            "the market maker's node holds the core node's snapshot",
            async || peer_snapshot(net, &maker.node, &core.key).await.is_some(),
        )
        .await;
    }
    rates
}

/// The market maker of `maker`'s node, as its config file says it,
/// rebalancing once a share is `threshold_ppm` from even, deciding by the
/// daemons' clock.
async fn market_maker(net: &HubNet, maker: &Member, threshold_ppm: u32) -> MarketMaker {
    let ledgers = |asset: HubAsset| {
        (
            net.hub(asset).ledger.clone(),
            asset.owner_ledger(net.network),
        )
    };
    let ((xrp_hub, xrp_book), (dollar_hub, dollar_book)) =
        (ledgers(HubAsset::Xrp), ledgers(HubAsset::Dollar));
    let file = serde_json::json!({
        "socket": maker.node.ipc,
        // Not read here: the test hands it the node's keys.
        "passphraseFile": "unread",
        "xrpl": {
            "server": net.xrpl.url,
            "token": { "currency": "DSUSD", "issuer": DSUSD.issuer.to_address() },
        },
        "xrp": {
            "ledger": xrp_hub,
            "ownerBook": xrp_book,
            "cap": CAP,
        },
        "token": {
            "ledger": dollar_hub,
            "ownerBook": dollar_book,
            "cap": CAP,
        },
        "curvePpm": CURVE_PPM,
        "marginPpm": MARGIN_PPM,
        "thresholdPpm": threshold_ppm,
        "slippagePpm": SLIPPAGE_PPM,
        "caFile": ca_file(),
        "allowPrivate": true,
    });
    let config = config::parse(&file.to_string()).unwrap();
    // The node's first XRPL testnet account, as the node derives it.
    let entropy = maker.key.xrpl_account_entropy(TESTNET.id, 0);
    let wallet = Wallet::from_entropy(entropy.as_bytes());
    MarketMaker::start(config, wallet, Arc::new(net.clock))
        .await
        .unwrap()
}

/// Runs `maker` until `done`, and stops it as SIGTERM does.
async fn run_until(what: &str, mut maker: MarketMaker, done: impl AsyncFnMut() -> bool) {
    let shutdown = CancellationToken::new();
    let running = tokio::spawn({
        let shutdown = shutdown.clone();
        async move { maker.run(&shutdown).await }
    });
    until(what, done).await;
    shutdown.cancel();
    running.await.unwrap();
}

/// What `member` holds of DSUSD and XRP, in the books' units.
async fn holdings(net: &HubNet, member: &Member) -> (u64, u64) {
    (
        held(net, member, HubAsset::Dollar).await,
        held(net, member, HubAsset::Xrp).await,
    )
}

/// Whether `after` holds a larger share of its value in DSUSD than
/// `before`: DSUSD over XRP rose, which at one rate is its share.
fn more_dollars((dollars, xrp): (u64, u64), (dollars_before, xrp_before): (u64, u64)) -> bool {
    u128::from(dollars) * u128::from(xrp_before) > u128::from(dollars_before) * u128::from(xrp)
}

/// `held`'s value at `pool`'s price, in dollars times what the pool holds
/// of XRP: exact, and as good to compare at one price.
fn value((dollars, xrp): (u64, u64), (pool_xrp, pool_dollars): (u64, u64)) -> u128 {
    u128::from(dollars) * u128::from(pool_xrp) + u128::from(xrp) * u128::from(pool_dollars)
}

// spec/areas/rate-feed.md#market-maker, spec/decisions/passive-rebalancing.md,
// spec/decisions/market-maker-inventory.md: d1 lacks DSUSD, so its curve
// takes DSUSD in at better than spot, and its snapshot says so; the market
// maker's node holds it, as it reads its peers' snapshots. The market
// maker, even at the start, swaps DSUSD for XRP through d1, and once its
// share of XRP is two points from even, swaps it back to even through the
// pool from its owner book. So d1's share of its value in DSUSD
// rises, toward its target; the market maker's value, at the pool's price,
// grows; and both members' books are as they were after both restart.
#[tokio::test(start_paused = true)]
async fn a_market_maker_moves_a_skewed_core_node_toward_balance_and_gains() {
    let net = hubs_and_pool().await;
    let members = net
        .members(
            &[
                Joining::of(81, BOTH).with_dollars(CORE_DOLLARS),
                Joining::of(91, BOTH)
                    .with_dollars(MAKER_DOLLARS)
                    .pinned_to(&[81]),
            ],
            &[],
        )
        .await;
    let [d1, maker] = [&members[0], &members[1]];
    net.peered(&[d1, maker]).await;
    let _rates = quoting(&net, maker, &[d1]).await;
    // A periphery node without targets swaps nothing by itself.
    assert!(!swap_status(&maker.node).await.by_itself);
    net.settled(&[d1, maker]).await;
    let seeded = pool(&net);
    let (d1_before, maker_before) = (holdings(&net, d1).await, holdings(&net, maker).await);

    let market_maker = market_maker(&net, maker, 20_000).await;
    // A swap's DSUSD reached d1, and the node's XRP went back through the
    // pool: the only swaps on it are the market maker's. d1 pays its own
    // costs in DSUSD, such as the DSUSD hub's fee on each claim it redeems,
    // so what a swap brings shows as a rise that may fall short of a whole
    // one; while one of d1's moves between its hub and its book is on its
    // way, the sum reads short, never above.
    run_until(
        "the market maker swaps through d1 and back through the pool",
        market_maker,
        async || {
            let dollars = held(&net, d1, HubAsset::Dollar).await;
            pool(&net).0 > seeded.0 && dollars > d1_before.0
        },
    )
    .await;
    // What it paid may still be on its way: the nodes finish it, and move
    // funds between their hubs and books as they need.
    net.settled(&[d1, maker]).await;
    assert!(swap_status(&maker.node).await.fulfilled > 0, "it swapped");

    let d1_after = holdings(&net, d1).await;
    let went = format!("d1 went from {d1_before:?} to {d1_after:?} (DSUSD, XRP)");
    assert!(more_dollars(d1_after, d1_before), "{went}");
    assert!(d1_after.0 > d1_before.0, "d1 took DSUSD in: {went}");
    assert!(d1_after.1 < d1_before.1, "d1 paid XRP out: {went}");

    // The market maker holds more than it did, at the pool's price now.
    let now = pool(&net);
    assert!(now.0 > seeded.0 && now.1 < seeded.1, "it swapped back");
    let maker_after = holdings(&net, maker).await;
    assert!(
        value(maker_after, now) > value(maker_before, now),
        "the market maker went from {maker_before:?} to {maker_after:?} at {now:?}"
    );

    // Restarted, both catch up and book nothing twice.
    let kept = net.claimed(&[d1, maker]).await;
    let members = net.restart_all(members).await;
    let again = net.settled(&members.iter().collect::<Vec<_>>()).await;
    assert_eq!(again, kept, "the books after a restart");
    net.stop(members).await;
}

// spec/areas/rate-feed.md#market-maker, spec/decisions/market-maker-inventory.md:
// both core nodes lack DSUSD, so both snapshots price it above the pool,
// alike, at their curve's cap. The market maker reads both as its node
// holds them, and swaps DSUSD for XRP through each that clears, at once.
// So both take DSUSD in, through one node of the market maker's pinned to
// both, and each holds a larger share of its value in DSUSD.
#[tokio::test(start_paused = true)]
async fn a_market_maker_moves_each_of_two_core_nodes_toward_balance() {
    let net = hubs_and_pool().await;
    let members = net
        .members(
            &[
                Joining::of(81, BOTH).with_dollars(CORE_DOLLARS),
                Joining::of(82, BOTH).with_dollars(CORE_DOLLARS),
                Joining::of(91, BOTH)
                    .with_dollars(MAKER_DOLLARS)
                    .pinned_to(&[81, 82]),
            ],
            &[],
        )
        .await;
    let [d1, d2, maker] = [&members[0], &members[1], &members[2]];
    net.peered(&[d1, d2, maker]).await;
    let _rates = quoting(&net, maker, &[d1, d2]).await;
    net.settled(&[d1, d2, maker]).await;
    let before = [holdings(&net, d1).await, holdings(&net, d2).await];

    // The default threshold, far enough from even for trades through both
    // before a rebalance.
    let market_maker = market_maker(&net, maker, 50_000).await;
    run_until(
        "the market maker swaps through each core node",
        market_maker,
        async || {
            held(&net, d1, HubAsset::Dollar).await > before[0].0
                && held(&net, d2, HubAsset::Dollar).await > before[1].0
        },
    )
    .await;
    net.settled(&[d1, d2, maker]).await;
    for (index, core) in [d1, d2].into_iter().enumerate() {
        let after = holdings(&net, core).await;
        assert!(
            more_dollars(after, before[index]),
            "d{} went from {:?} to {after:?} (DSUSD, XRP)",
            index + 1,
            before[index]
        );
    }
    net.stop(members).await;
}
