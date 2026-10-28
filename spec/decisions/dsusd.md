# A testnet dollar of our own, bought only with XRP through a pool we keep at the market

**Areas:** [DSUSD keeper](../areas/dsusd-keeper.md), [XRPL](../areas/xrpl.md), [Settlement](../areas/settlement.md). The testnet's dollar token, and its stand-in for RLUSD.

## Facts

- A node converts between the ledgers it settles on at its exchange rates, and books a dollar token as `USD` at par ([issued-assets](issued-assets.md)). Market makers trade nodes' rates against the pools on the ledgers themselves, which is what moves a node's holdings back where they ran short.
- Test RLUSD has no market value: Ripple hands it out by hand, and the testnet's XRP/RLUSD pool prices it about thirty times away from par. A market maker would take every node's RLUSD at par and sell it into that pool, and nodes would never get it back. Neither the issuer nor the pool is ours to correct.
- XRP has a market price, which exchanges publish, and the testnet faucet hands out test XRP to anyone.
- A pool on the XRP Ledger prices by the ratio of what it holds. A liquidity provider can deposit or withdraw one of its two assets alone, which moves that side only, and so the price, by exactly the amount.

## Decision

- **DSUSD, a testnet dollar token of our own,** issued by an account of ours, is the testnet's dollar token and its stand-in for RLUSD: nodes settle in it over its settlement hub ([token-hubs](token-hubs.md)) and book it as `USD` at par, and the peg guard runs on it, with its own pool, as it runs on mainnet RLUSD ([issued-assets](issued-assets.md)). Test RLUSD is disabled ([mainnet-schemes](mainnet-schemes.md)).
- **DSUSD is had only by paying XRP for it, on the ledger, through the XRP/DSUSD pool.** The issuer mints only into the pool, never to a person, so it holds no XRP that anyone could drain, and there is no faucet. Every DSUSD outstanding was paid for with XRP that sits in the pool, or is still in the pool.
- **A keeper holds the pool at one exchange's XRP/USD price.** On the testnet nothing of value rides on the price, so one source is enough. An exchange's terms forbid publishing its price so, and [open-rate-sources](open-rate-sources.md) moves the keeper to mainnet's XRP/RLUSD pool; until that is built, it reads Bitstamp's ticker. The keeper refuses a ticker that is stale, crossed, too wide or out of bounds, rather than move the pool on it.
- **The keeper reprices as the pool's main liquidity provider, with DSUSD alone:** it deposits newly minted DSUSD to raise XRP's price, and withdraws DSUSD, which it gives back to the issuer to burn, to lower it. It never buys or sells XRP, so the pool's XRP is what buyers paid in, plus faucet XRP the keeper adds, with DSUSD beside it at the pool's price, to keep the pool deep.

## Accepted

- **DSUSD is worth what the keeper says.** Within its band and the pool's trading fee, and for the few ledgers after a trade until the keeper brings the pool back ([keeper-follows-the-pool](keeper-follows-the-pool.md)), the pool is off the market, and a trade against a stale price costs test XRP only.
- **The issuer's seed sits on a VM,** and whoever has it can mint DSUSD. On the testnet that costs nothing real; the seed lives in a secret only the keeper's account can read.
- **A provider other than the keeper can hold most of the pool.** The keeper then cannot lower the price far alone, and says so; a withdrawal sells the keeper's share of the pool's XRP to the other providers, so it withdraws once a window at most.
- **A testnet reset takes the issuer, the keeper and the pool,** and with them every DSUSD. The keeper makes them again, at the same addresses, from the same seeds.

## Rejected

- *A DSUSD faucet:* free DSUSD buys XRP out of the pool, so a faucet would hand out the pool's XRP to whoever asked most often.
- *Buying DSUSD in the node:* a node that trades on a ledger takes a market risk its owner never chose, and needs paths, slippage and price checks of its own; an owner swaps XRP for DSUSD on the ledger as on any exchange.
- *Several price sources, as the node's rates take:* more requests and more ways to fail, for a price that backs nothing real. Real money would need this decided again.
- *Repricing by trading, or with both assets:* to lower XRP's price by selling XRP the keeper would need XRP it does not have, and to raise it by buying XRP with minted DSUSD it would take the XRP buyers paid in out of the pool. One-sided DSUSD leaves the XRP where buyers put it.
- *Test RLUSD as the testnet's dollar token:* its pool prices it nowhere near a dollar, and neither the issuer nor the pool is ours to correct, so nodes that hold both XRP and RLUSD could not be rebalanced, and a peg guard on it would never let it settle.
