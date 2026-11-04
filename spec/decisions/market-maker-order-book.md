# The market maker prices by the order book and the pool together, and rebalances through a standing offer of its own

**Areas:** [Rate feed](../areas/rate-feed.md#trades), [XRPL](../areas/xrpl.md#the-order-book).

## Facts

- The market maker valued what it holds at its AMM pool's price, charged each trade beyond even what selling it back on the pool would cost, and expected a rebalance to deliver what the pool gives ([market-maker-inventory](market-maker-inventory.md)). So the core nodes settle where their bonus is the pool's fee, the price impact of a rebalance and the margin: 0.77% from spot on the testnet, against its pool's 0.3% fee.
- The XRP Ledger keeps an order book beside every pool. On mainnet, the XRP/RLUSD pool held about $2.3M a side at a fee of 0.214%; the book beside it had a spread of 0.021%, and crossing it cost 0.09–0.12% from its mid for $75 to $5,000, 0.2% for $20,000, with about $39,000 of asks and $13,000 of bids within a quarter of a percent. A trade on the book pays no fee, and RLUSD's issuer charges no transfer fee. The pool's price sat 0.17% below the book's mid: arbitrage keeps a pool's price only within its fee of the market.
- A Payment from XRP to a token, or back, with `SendMax` and no paths, is routed by the ledger through the pool and the book alike, each step at the better of them ([XRPL](../areas/xrpl.md#amm-pools)).
- An offer resting on the book costs its owner a ledger fee to place, replace or cancel, a few millionths of an XRP, and an owner reserve while it stands, 0.2 XRP. Whoever crosses it pays its price. The node books an offer of its account crossed by someone else's transaction as it books any transaction it did not sign, by the balance change, and the reserve by the account's owner count ([XRPL](../areas/xrpl.md#following-the-account)).
- The market maker holds up to its threshold's share of its value as an imbalance anyway ([market-maker-inventory](market-maker-inventory.md#accepted)): rebalancing it is not urgent until it passes the threshold.
- The top of a book can be an offer of a few drops, placed by anyone, at any price inside the spread.

## Decision

- **The market's mid, not the pool's price.** The market maker values what it holds at the mid between the market's best bid and ask: on each side the better of the pool's price, its fee counted, and the price at which 100 XRP of the book fill. An offer of a few drops does not move it, and an offer past its expiration, which the ledger lists until a crossing meets it but never fills, counts for nothing.
- **Undoing a trade, and a rebalance, by the better venue.** The cost of undoing a trade beyond even is the less of what selling a rebalance's worth back costs on the pool and on the book, walked at its offers' prices, against the mid; and a rebalance is asked to deliver what the better of them gives for what it pays. The ledger routes it through both, so it delivers at least that.
- **A standing offer while it is off even.** While its share of an asset is more than a band from even, a fifth of its threshold, the market maker keeps one offer on the book that sells what it holds too much of for the other, at the market's mid, as much as takes it back to even. It replaces the offer, in one transaction that names the old one, once the mid has moved by more than a step, a twentieth of a percent, or what takes it back to even by more than a tenth; it cancels it once it is within the band, before a rebalance past the threshold, and when it stops. Each offer expires by itself a few minutes after it is placed, and is replaced before that, so one that a stopped market maker leaves behind does not stand.
- **A rebalance on the pool only past the threshold,** as before, when the standing offer has not brought it back.

## Accepted

- **A quiet book fills a standing offer slowly or never.** On the testnet, where few trade, the market maker rebalances past its threshold much as before; the offer pays where the book is busy.
- **An offer is filled when the market has just moved against it,** by up to a step of its price before it is replaced; and the expiry bounds how long it stands at a price the market has left.
- **Each offer holds an owner reserve, 0.2 XRP, of the market maker's account** while it stands.
- **More transactions from the account that the node pays from,** each a chance to take a sequence the node meant to use, which the node recovers from ([XRPL](../areas/xrpl.md#in-flight)). A step and a tenth bound how often it replaces the offer.
- **The cost of undoing a trade overstates what the ledger charges** where a rebalance splits between the pool and the book; it never understates it.

## Rejected

- *The pool alone:* its fee is the floor of what the nodes settle at, where crossing the book costs half that and resting on it costs nothing.
- *Taking from the book only, without a standing offer:* it pays the spread on every rebalance, when the market maker can wait to be paid it instead.
- *An offer at the mid less an edge:* a better price for the market maker that fills less often, on a book whose spread is already a fiftieth of a percent.
- *A ladder of offers at several prices:* a reserve and a transaction for each, for an imbalance that one offer covers.
- *Buying the pool's auction slot,* whose holder trades through it at a tenth of its fee: worth its price, which competition sets, only for a market maker trading much more than this one.
- *The best offer of the book, however small, as its side's price:* anyone could move the mid with a few drops.
