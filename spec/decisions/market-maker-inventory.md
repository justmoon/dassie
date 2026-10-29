# The market maker prices by its own inventory, on a flatter curve than the nodes, and rebalances on the pool back to even

**Areas:** [Rate feed](../areas/rate-feed.md#market-maker), [Market maker VM](../areas/market-maker-vm.md#settings). Not built yet ([roadmap](../roadmap.md#market-maker-by-inventory)).

## Facts

- The market maker trades between core nodes, by swapping with them over its peerings ([peer-swaps](peer-swaps.md)), and an AMM pool on the XRP Ledger, where it rebalances its own two assets ([passive-rebalancing](passive-rebalancing.md)). A node's curve pays more than spot for the asset it holds least of, so each trade the market maker makes moves the node back toward its targets ([Connector](../areas/connector.md#inventory-pricing)).
- It decides by a band per asset ([Rate feed](../areas/rate-feed.md#bands)). A trade has a fixed size per asset, never takes an asset below its band, and goes from an asset above its band only toward the other while that is within its own. An asset above its band is rebalanced on the pool to the band's middle, delivering no more than the other's band has room for.
- Two bands constrain two amounts apart, and both can end above their tops. A rebalance then has no room to deliver into, and no trade is allowed. On the testnet, after a faucet grant took XRP far above its band and a rebalance filled DSUSD's to a few DSUSD over its top, the market maker held every tick for fourteen hours while the core nodes paid 2.6% for the DSUSD it held.
- Its trades are a fixed two DSUSD or two XRP, one target per tick, the target whose snapshot pays most for that amount. A core node whose ladders were a little thinner than the others' was never chosen, though its price was the same, and a target whose peering could not swap was chosen again tick after tick.
- A swap with a node costs next to nothing beyond the node's own price: a packet's share of the hubs' fees. A trade on the pool costs its trading fee, 0.3% on the testnet's pool, its price impact, which grows with its size, and a ledger fee, and moves the pool, which its keeper then corrects ([keeper-follows-the-pool](keeper-follows-the-pool.md)). The trade that helps a node most is often small: what moves it back the last part of the way.
- A real arbitrageur closes each trade on the other venue at once, so its inventory never drifts. Here that would be a pool trade for every swap, however small, each paying a ledger fee and moving the pool.

## Decision

- **The market maker values its inventory by a curve, as a core node does, but flatter.** It values each asset at the pool's price times `w_i = (1 − k) + k · t_i / s_i`, where `s_i` is the asset's share of what it holds, valued at the pool's price, and `t_i` its target, an even share. Its `k` is a setting, 1% by default, a fifth of a core node's. What it holds of an asset is its hub and its owner book together, as now ([Rate feed](../areas/rate-feed.md#bands)).
- **It trades with every target whose quote beats its own valuation by the margin.** A swap of `a` for `b` through a target is worth it while what the target's snapshot says comes back, valued by the market maker's weights at the state after the swap, exceeds what it pays, valued the same way, by the margin. Each trade is as large as the target's ladders allow with its last unit still clearing, and no larger than a per-trade cap. Every target is considered every tick, and each that clears gets its own trade, up to as many at once as the node runs owner swaps. A flatter curve than the nodes' means it keeps absorbing a node's imbalance as it takes on its own.
- **The margin pays for the rebalance to come.** It is 0.5% by default, above the pool's trading fee and the price impact of a rebalance at the pool's depth. So every trade gains at least that over spot, and the rebalance that later undoes its effect on the market maker's own inventory costs less than the trades it undoes gained.
- **It rebalances on the pool only when its own share is far from even:** once an asset's share is more than a threshold from its target, 10 percentage points of its value by default, it rebalances to even in one Payment, bounded by its slippage share as now. A rebalance comes before any trade, and nothing is decided until its books show it, as now.
- **A target whose trade failed, or paid nothing, is left out** for a wait that doubles from a minute to half an hour, and is back at once after a trade through it that paid.
- **No bands and no fixed trade sizes.** The settings are `k`, the margin, the rebalance threshold, the slippage and the per-trade cap.

## Accepted

- **It holds an imbalance between rebalances,** up to the threshold's share of its value, and XRP's price moving meanwhile changes that imbalance's worth either way. With the default threshold on the testnet market maker's funds, that is about $150, on which a 1% move in XRP is $1.50. A smaller threshold holds less and rebalances more often.
- **A rebalance is one large trade on the pool,** with more price impact than many small ones would have, and fewer ledger fees and corrections by the keeper. The margin covers it at the pool's depth; a shallow pool would want a larger margin or a smaller threshold.
- **Its spot is the pool's.** It prices by what it can rebalance at. A pool off the market misprices its trades and its rebalances by as much, which the keeper bounds to its band.
- **The nodes settle near spot, not at it:** where their bonus is the market maker's margin plus its small skew, about 0.5% from spot by default.
- **A market maker far from even rebalances first.** One started on a faucet grant of XRP alone, or one changed from bands with a skewed inventory, buys the other asset in one trade before it trades.

## Rejected

- *Bands per asset:* two constraints on two amounts deadlock once both are above their tops, and a band is on or off, with no price in between.
- *Closing each trade on the pool at once:* a pool trade, with its ledger fee and its move of the pool, for every swap, however small, so the smallest trades, which a node needs to get the last part of the way back, are not worth making.
- *The nodes' `k`:* the market maker would be as reluctant to take on an imbalance as the node it relieves, so the two would share it rather than the market maker absorbing it.
- *Rebalancing only back to the threshold:* it would be at the threshold again after the next trade, and rebalance again and again in small trades.
- *Valuing at the nodes' spot rates:* the market maker can only realize a price on the pool, so a valuation by any other spot would let it make trades the pool then undoes at a loss.
- *One target per tick, taking turns among those alike:* a target that pays a little less for the fixed amount is never chosen, though it needs the trade as much.
