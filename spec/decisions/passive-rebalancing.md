# Nodes rebalance passively: inventory pricing, a rate feed and market makers

**Areas:** [Connector](../areas/connector.md#inventory-pricing), [Settlement](../areas/settlement.md#credit). Inventory pricing is built; the rate feed and the market makers are not ([roadmap](../roadmap.md#rate-feed-and-market-makers)).

## Facts

- A core node that converts between two ledgers drifts. Every conversion takes in one asset and pays out the other, so a node through which more flows one way fills up with one and runs out of the other. Once its owner holds nothing on a ledger, the owner's group limit refuses every conversion out of it (T04), and nothing moves the node back.
- A node holds its assets on ledgers it settles on, and only peers can move them in its books: it cannot trade on its own without an exchange account, a ledger's decentralised exchange, or keys and logic for each of them, and every one of those is a new way to lose the owner's funds.
- Automated market makers on layer-1 ledgers rebalance passively: their price follows their inventory, so arbitrageurs, who profit from any gap to the market, bring them back. Their price is their inventory alone, though, so an arbitrageur can push them as far as the market moves, and they trust no oracle.
- A node already has an oracle: its spot rate, the median of three independent sources ([exchange-rates](exchange-rates.md)). One wrong source cannot move it past what the other two say.
- Peering is one per node pair, on one scheme, chosen automatically, and the owner's own endpoints never convert ([owner-ledger](owner-ledger.md)). One node that wanted to trade through a core node in both directions would need two peerings with it, on two schemes.
- An arbitrageur needs every core node's prices, often and cheaply; a node that answers price requests over HTTPS pays a handshake per asker and per poll.

## Decision

- **Each core node prices conversions by its inventory**, anchored at spot. For each ledger that converts, the node values the owner's funds on it at spot; `s_i` is its share of the total, `t_i` its target share. A unit of the ledger is worth `w_i = (1 − k) + k · t_i / s_i` against spot, and transit from `a` to `b` converts at `spot · w_a / w_b · (1 − spread)`. Bringing in what the node lacks gets better than spot; taking it out gets worse. So whoever profits from the gap, a market maker or an ordinary payment, moves the node back toward its targets, and the node itself never trades.
- **The bonus is capped at `1 + k`; the penalty is not.** However skewed the node, it never pays more than `k` above spot for what it lacks, so a market whose price differs from the node's oracle by less than `k` cannot drain it; what the node lacks gets dearer the scarcer it is, and nothing converts out of a ledger the owner holds nothing on. A round trip at one state never gains: the two curve factors multiply to one before the cap and the spread, which only take away.
- **The parameters are starting points.** `k` is an owner setting beside the spread, on by default at 5%, and the targets are equal shares by value. At 5% a 25/75 split costs about 6% on the way out of the scarce ledger, a 5/95 split about a third. Both are to be tuned against what the network shows, and targets may become owner settings.
- **The connector converts at a price table**, rebuilt whenever the rates, the owner's funds or the settings change: for each ordered pair of ledgers with fresh rates, a rate in exact integer arithmetic, rounded down. A packet only multiplies by it, so what a node publishes is exactly what its packets get. Conversions into the owner's own endpoints stay at spot, uncharged: the owner's own payments do not pay the owner.
- **A public rate feed, over UDP.** A core node publishes its table as one signed datagram, generated and signed once and sent to every subscriber, with no retries and no state beyond a bounded list of subscribers. A subscriber proves its address with a cookie the node derives from a boot secret and the address before it gets anything larger than its request, so a forged source address cannot aim the feed at a victim. Signatures are by the node key under an OID of their own, verified by ZIP-215 as everything else.
- **Market makers are periphery nodes.** A market maker runs two periphery nodes, each on one scheme and pinned to the core node it trades with: one pays the core node on one ledger, the other is paid on the other, and the market maker rebalances its own side on a ledger's exchange. That fits one peering per node pair, needs no change to how peers convert, and keeps the market maker's keys on its own nodes.

## Accepted

- **The spread's gain can be negative.** The curve moves the rate past spot in the sender's favour, so a node short of an asset pays for getting it back. Like the spread, this is not booked per packet: it is the net of the FX accounts, and does not count toward a peer's credit growth ([Settlement](../areas/settlement.md#credit)).
- **A node may pay up to `k` above spot** for what it lacks, which an arbitrageur takes when the market is that far from the oracle. That is the price of rebalancing, bounded by `k` per unit.
- **The oracle's errors pass through.** A spot rate off by less than `k` from the market leaves a node's curve exploitable by that much; the median of three sources keeps it close.
- **A table, not a curve per packet.** Within one engine step, packets convert at the table of the last commit, so a burst sees the inventory as it was at the step's start. The group limit still refuses what the owner does not have.
- **A ledger whose rate is stale is left out**, so its pairs convert nothing, as before; a pair of ledgers in the same currency now needs that currency's rate too, since the curve values holdings across all of them.

## Rejected

- *The node trading on a ledger's exchange or an exchange account:* keys and logic per venue, slippage and counterparty risk on the owner's funds, and a node that acts on its own rather than on what its peers bring.
- *A pure AMM curve, priced by the inventory alone:* without the oracle, an arbitrageur could move a node's price as far as the market moves, and the node would follow any market, a manipulated one included.
- *An uncapped bonus:* a node with nothing left of an asset would pay without bound to get it back, and a single trade could take the rest of its funds.
- *Rates over HTTPS on request:* a handshake per asker and per poll, and state per connection, for data that is the same for everyone.
- *A feed without proof of address:* a reply larger than the request, sent to any source address, is a reflection amplifier.
- *One market-maker node trading through a core node both ways:* it would need two peerings with the same node, on two schemes, and the owner's own endpoints never convert.
