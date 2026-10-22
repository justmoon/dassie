# Connector

The node's ILP connector: it knows the node's endpoints (BTP clients, local endpoints, peers), routes each Prepare, answers ILDCP, books the packet's transfers in the [ledger](ledger.md), charges transit, limits what may be in flight, and resolves the transfers when the response comes back or the packet expires. This doc also covers the exchange rates it converts at, how the owner's inventory prices them, the BTP server and its tokens.

**Paths:** `crates/dassie-connector/`, `crates/dassie-node/src/engine/core/packets.rs`, `crates/dassie-node/src/engine/core/tokens.rs`, `crates/dassie-node/src/http/btp.rs`, `crates/dassie-node/src/store/peer_settings.rs`, `crates/dassie-node/src/rates.rs`, `crates/dassie-node/src/node/rates.rs`, `crates/dassie-node/src/store/rates.rs`, `crates/dassie-node/src/rpc/rates.rs`
**Read with:** [Ledger](ledger.md), [ILP](ilp.md), [Peer protocol](peer-protocol.md), [Network](network.md), [Payments](payments.md), [Settlement](settlement.md)

The connector is sans-IO and bounded ([engineering](../engineering.md)), and nothing on the packet path panics on input. Its bounds and timing are `Config` in `config.rs`.

## Addresses and endpoints

Every endpoint has an ILP address, the [ledger](ledger.md) account its packets debit and credit, and that account's ledger (`endpoint.rs`). ILDCP answers each endpoint the address that is routed to it and its ledger's asset.

- **Children** lie below the node's own address `<scheme>.das.<node ID>`: a random segment per BTP connection, and per ledger a STREAM receiver and a payer ([Payments](payments.md)). Children's addresses never overlap: neither is a segment prefix of the other.
- **Peers** are reached through the route table; a peer's address is only what ILDCP tells it, `<scheme>.das.<node ID>`. A peer's endpoint ID is never reused, so a late response cannot resolve a later peering's packet.
- **Downlinks** ([Network](network.md#downlinks)) are peers at a child address beneath ours, `<our address>.<node ID>`, which ILDCP tells them and which routes to them as to a child: the rest of the network reaches it through us, with no route of its own. A downlink sends only its own packets, so it has a child's share of the packets in flight, not a peer's; otherwise it is a peer like any other, for fees, conversion and the money in flight.
- **Request IDs** are unique per endpoint while in flight. A peer's first outgoing request ID is random, so that after a restart new packets do not reuse IDs the peer still holds from before. A source that reuses an ID still in flight gets nothing done.

## Routing

- **Below the node's own address,** only children and downlinks, by whole-segment prefix; a downlink whose peering is ending takes no new Prepare. The node's own address itself is unroutable.
- **Beyond it,** the longest whole-segment prefix in the route table, which the peer protocol's routing fills with a few first hops per node, best first ([Network](network.md)). The connector takes the first that is an endpoint, is not the packet's source (so packets do not bounce between peers), and is not draining.
- **The owner's first hop.** For a packet from one of the node's own endpoints, the connector says which ledger its first hop is on: what the owner must pay from, since the owner's endpoints never convert ([Ledger](ledger.md#the-owners-accounts)).

## A periphery node

A periphery node ([Network](network.md#periphery-nodes)) has an address beneath each uplink, `<uplink's address>.<its node ID>`: what that uplink's ILDCP would tell it, which it derives rather than asks for, since an uplink's address and the rule are fixed. Its connector takes them all.

- **Beneath any of them** lie the node's own endpoints, as below its own address. A packet routed there goes on with its destination written below the node's own address, the only one its endpoints know.
- **It forwards nothing.** A Prepare from a peer that lies beneath none of its addresses gets F02. Its own endpoints' packets go out by the routes, through its uplinks.
- **It is paid beneath an uplink it is connected to:** its STREAM receiver's credentials name the address beneath an uplink confirmed now ([Payments](payments.md)), so a payment's packets reach it through that uplink. Credentials handed out earlier keep their address, and reach it while that peering lasts.

## Handling a Prepare

The checks run in the order of `connector/prepare.rs`; the first that fails answers the source at once with a Reject from this node. A packet that would convert is refused unless it comes from a peer, has fresh rates, and still carries money after conversion; transit into a ledger the owner holds nothing on gets T04. The money checks come last: the cap on money in flight with the peer, then a token's spending cap, then the ledger.

**Expiry.** The outgoing packet expires one message window before the incoming one, and at most the longest hold time from now: the window leaves time for the response to travel back, and the hold bounds how long the next hop holds it. A packet that leaves too little time is refused (R02).

## Amounts and conversion

- **Same ledger:** what goes on is what came, less our fee.
- **Across ledgers, from one peer to another:** what came less our fee, at the price table's rate for the pair: spot, moved by the owner's inventory, less the spread ([below](#inventory-pricing)).
- **Across ledgers, from a peer into the owner's own endpoints:** what came, at spot, uncharged. Only a packet from a peer converts.

Conversion is exact integer arithmetic, rounding down, never floating point: at spot on rates relative to the dollar (`fx.rs`), on transit at the table's rate (`pricing.rs`). It uses only rates learned at most an hour before `now` and not after it, so a clock that went back cannot make a rate young; without them the packet gets T00.

## Exchange rates

`rates.rs` holds the decisions without IO, parsing and combining sources' answers; `node/rates.rs` is the fetcher that runs them a minute after the last cycle, and whenever the sources or the currencies wanted change.

- **A rate per currency, relative to the dollar,** kept for the node's ledgers' assets and the currencies an owner may see totals in. Each rate ages on its own.
- **The median of several sources.** A currency's rate is the median of the sources that cover it and answered in this cycle, once more than half of them did; with an even number, the middle two must agree closely, and the rate is their midpoint. A new node's sources cover every currency three times (below). So one wrong source can move a rate only within what the other two say, never beyond, and with one silent, the other two must agree. A currency the owner's sources cover twice gets a rate only while both answer and agree; one they cover once takes what that source says, as the owner chose (the dev environment does). A currency without a rate this cycle keeps its last one until it is stale, and the node warns.
- **Exact decimals.** Answers are read in exact decimal arithmetic; an answer that does not read leaves the source out of the cycle, and it is simply asked again at the next.
- **Across restarts,** each currency's last rate is persisted with when it was learned, so a restarted node converts at once while its rates are fresh.

### Sources

The owner keeps an ordered list of sources of four kinds: Coinbase's format, which covers every currency and which any server may serve (as the dev environment does); Kraken's ticker, the last trade of every currency against the dollar on its market; Bitstamp's ticker, which covers XRP; and the European Central Bank's daily reference rates, which cover the fiat currencies. A new node starts with one of each, so three sources cover every currency, and their data comes from three upstreams: for the fiat currencies Coinbase's data provider, Kraken's market and the ECB, for XRP three exchanges' markets. All four are free and need no key. A node upgraded with the sources it started with gets the ECB; an owner's own list is left alone.

The ECB publishes once a business day, so its rates are up to a day behind the markets; as one of three, a rate of its outside the two live ones leaves the rate at the nearer of them. Its answer is taken only while its newest day is no older than its calendar allows (`MAX_ECB_AGE_DAYS` in `rates.rs`, the gap at Easter), and never one of a day to come: a daily source that stopped reads as silent rather than as a wrong rate.

Sources are asked through the node's outbound client, under its rules for outbound requests ([Peer protocol](peer-protocol.md)).

### Inventory pricing

Transit from one peer to another converts at a rate the owner's inventory moves from spot, so that what flows through a node brings its holdings back toward their targets without the node trading ([passive-rebalancing](../decisions/passive-rebalancing.md)). The curve and the table are in `pricing.rs`.

- **The curve.** Each ledger that converts (an active scheme and a fresh rate) is valued at spot, from what the owner may spend on it; its share of the total against an equal target gives it a weight `w = (1 − k) + k · target / share`, and a pair converts at spot times `w_in / w_out`, less the spread. At target the rate is spot less the spread. The bonus for bringing in what the node lacks is capped at `1 + k`; the penalty for taking it out is not, and out of a ledger the owner holds nothing on, nothing converts (T04). A round trip at one state never gains.
- **`k` is the owner's,** beside the fee and the spread, bounded as they are, and on by default (`DEFAULT_REBALANCE_PPM`).
- **The price table.** The engine builds a table whenever the rates, the owner's funds on a ledger or the peer settings change, and when a rate in it ages out: for each ordered pair of ledgers with fresh rates, a rate out per in and when its older rate was learned. A packet converts exactly `floor(net × rate / 10^15)`, so the table is what a published price would be. A ledger without a fresh rate is left out, so its pairs get T00; two ledgers in the same currency need its rate too, since the curve values every holding.
- **Exact and bounded.** Shares are known to twelve decimals, rounded down, and every product stays within 128 bits or goes through a 256-bit intermediate: a rate is never better for the sender than the curve, and none overflows.

### The owner's total

The total is the sum over the active ledgers of the owner's posted balance ([Ledger](ledger.md#the-owners-accounts)), converted into the owner's display currency. It is shown only while every rate it needs is fresh: with one missing or stale, there is no total, rather than a wrong one.

## Booking a packet

A packet that moves money books linked pending transfers, all or none, each referencing its trace ID ([Ledger](ledger.md#transfers)):

```
same ledger:     Dr source          Cr destination      in − fee
across ledgers:  Dr source          Cr Ls:revenue/fx    in − fee
                 Dr Ld:expenses/fx  Cr destination      out
our fee:         Dr source          Cr Ls:revenue/fees  fee          (only if fee > 0)
```

`Ls` is the source's ledger and `Ld` the destination's. A packet without money, or between two endpoints that share an account, books nothing. The ledger is sized for three pending transfers per packet the connector may hold.

A limit refusal becomes T04, a full table T03. So a peer can push packets to us only while it stays within the credit we extend, and the owner's endpoints only while the owner's group has funds.

## Responses and expiry

- **A Fulfill that matches the condition** posts the transfers, and its bytes go upstream unchanged once the batch that posted them is on disk ([Node](node.md#the-engine)).
- **A peer's Fulfill that does not match** is taken for a stale resend of an earlier packet's with the same request ID: it is dropped, and the packet waits for its own response or its expiry ([connector-timing](../decisions/connector-timing.md)). Any other wrong Fulfill gets F05 at once.
- **A response after the outgoing expiry** counts as a timeout, whatever it says.
- **A Reject, or a response that does not decode,** voids the transfers.

**Failing early.** The connector fails a packet before its expiry only when it surely did not reach its peer (the peer protocol says the connection was refused, or the peer answered with an error that means it did not take it, [Peer protocol](peer-protocol.md)), or when its next hop is removed. A Prepare that may have reached its peer is left to expire: a peer that fulfilled a packet we already failed would bear the loss.

**Fulfills are final.** A packet is posted when it is fulfilled, and stays posted whatever its upstream does with the Fulfill ([Ledger](ledger.md#fulfills-booked-as-final)). The peer protocol sends a response to a peer again until it arrives or a bounded time has passed; a Reject that never arrives has the effect of expiry upstream.

**Draining.** While a peering ends, its endpoint is draining: no new Prepare goes to it or is taken from it, but packets in flight finish. The peering ends once nothing is in flight to or from it ([Network](network.md#termination)). Before that, while the node collects from a peer it decided to end and has not told, the peer's endpoint is refusing: its new Prepares get T04 "insufficient liquidity" before any other check, so a peer that probes with a packet another check would answer can tell, and ours still go to it ([ending-a-peering](../decisions/ending-a-peering.md)).

**Unreached peers.** A peer our messages surely do not reach is refused the same way, on its own: once an ILP packet of ours to it surely did not arrive, its new Prepares get T04 before any other check, until it answers a request of ours, while ours still go to it. Across a link cut one way, a Fulfill of ours would never arrive and count only in our books, so each try of the sender's would drift the books apart by a packet; refused, they drift by at most what was in flight ([unreachable-peers](../decisions/unreachable-peers.md)).

## Money in flight with a peer

The connector caps, for each peer and each way, what the packets in flight carry, at two and a half of the [largest packets](#the-largest-packet) (`IN_FLIGHT_PACKETS` and `Config::max_in_flight`). A Prepare that would pass it gets T05, from which STREAM backs off as from any temporary error.

- **Why it exists.** A node that crashes with Prepares in flight never learns that its downstream fulfilled them, so each one drifts the two peers' books apart by its amount. Without a cap, a crash with many STREAM connections over one peer drifted the books far enough that settlement between them stalled for good. The cap is the most one crash, or one link cut one way ([unreachable-peers](../decisions/unreachable-peers.md)), costs a link, and the [least credit](settlement.md#credit) is derived from it.
- **Two and a half.** A payment's STREAM keeps two of the largest packets in flight; the half packet left takes smaller ones, such as a quote's probes while a payment fills the link. Without it those probes are refused and the quote fails.
- **Amounts, not packets,** so packets from parts of the network with a smaller largest packet just fit more of them.
- **Each way, with our own number.** A crash of either side raises what our books say the peer owes us, so each node bounds both directions with its own cap, knowing nothing of the peer's.
- Packets without money always pass, and a packet passes alone, with nothing else in flight that way, even if a conversion made it larger than the cap.

## The largest packet

A Prepare above the largest packet `P` gets F08, with the amounts. `P` is how much the owner is prepared to lose on a link, so every threshold of a link is counted in it: the cap above, and the settlement trigger, the least settlement and the least credit ([Settlement](settlement.md#positions-and-thresholds)). It is one value for the node, as is the default credit line; per-ledger values are not built ([roadmap](../roadmap.md#not-built-yet)).

## Fees and spread

Only what passes from one peer to another is charged: the owner's fee, and on a conversion the owner's spread, both in parts per million (`charges.rs`), and priced by the [curve](#inventory-pricing). Packets to or from the node's own endpoints are never charged.

- **The fee rounds down,** so a packet too small to carry one passes free. Rounded up, it would take a payment's last packets whole, and strand the payment short.
- **The spread and the curve are not booked per packet:** their gain is the net of the FX accounts, and negative when the node pays above spot for what it lacks.
- **The fee is what a peer earns us,** and the credit we extend it grows from it ([Settlement](settlement.md#credit)). Fees default to 0.
- New charges apply from the engine's next step.

## Spending caps

A BTP token may carry a cap: at most an amount `A` per window `W`, refilling continuously (`cap.rs`).

- **What counts** is a packet that debits the token's account and credits an account outside the owner's group: a peer. A payment to the node's own receiver, or to another token's client, moves no money out of the owner's funds and does not count.
- **The bound.** The cap is a bucket of used amount·time that drains at `A`, with what packets in flight hold counted as if fulfilled. A Prepare that would overfill it is refused (T04; F08 if it alone exceeds `A`). So what a token spends in any period `t` is at most `A + A·t/W`.
- **Per account, in the connector,** so all of a token's connections share one cap, and it is checked before the ledger books the packet.
- **Checkpointed with the journal rows that change it.** The journal alone cannot rebuild a refilling window: two histories with the same recent rows can have used different amounts. A replay of the token's journal since the cap was set is the reference the tests check the checkpoint against.

## The BTP server

ILP clients such as `ilp-plugin-btp` connect with `btp+wss://:<token>@<host>/btp` (BTP 2.0, RFC 23); the codec is in [ILP](ilp.md).

- **The token is the authentication.** Clients are not browsers, so there is no Origin check. Failed logins are rate-limited per client address; only failed ones count, so honest clients may reconnect as often as they need.
- **Money moves only in ILP packets:** a BTP Transfer is refused.
- **A client that goes away** has the packets in flight to it failed upstream; packets it sent keep running, and their responses are dropped. Revoking a token closes its connections, and from that moment they take no new Prepares, so none passes without the token's cap while they close.

### BTP tokens

- A token's secret is shown once at creation; the database keeps its SHA-256 ([secrets-at-rest](../decisions/secrets-at-rest.md)).
- A token is bound to one ledger that an active scheme serves, and gets a sub-account under the owner's pool on it, by the token's number. Numbers are never reused, and a revoked token keeps its account, since the journal refers to it.

## Local endpoints

The STREAM receivers and payers are in-process endpoints that the engine adds when a scheme activates and removes when it goes ([Payments](payments.md)). A packet forwarded to one is delivered in the same step. A payment from the node to its own receiver goes through the connector like any other, through a bounded queue drained at the end of the step, so nothing recurses.

## The packet trace

The connector keeps a bounded record of recent packets, keyed by trace ID, which the journal's packet references point into, and streams its changes to live subscribers. Events nobody polled are dropped and counted.

## ILP-over-HTTP

ILP-over-HTTP (RFC 35) is not built: the connector has no HTTP endpoint ([roadmap](../roadmap.md#not-built-yet)).

## Decisions

- [owner-ledger](../decisions/owner-ledger.md): each endpoint has a ledger, and ILDCP answers its asset; the owner's own endpoints never convert; the display currency.
- [exchange-rates](../decisions/exchange-rates.md): refreshed rates, staleness, exact math, the median of three sources per currency from three upstreams; fees and spread, and why the fee rounds down.
- [passive-rebalancing](../decisions/passive-rebalancing.md): transit priced by the owner's inventory, anchored at spot, rather than the node trading.
- [secrets-at-rest](../decisions/secrets-at-rest.md): BTP tokens are kept hashed.
- [connector-timing](../decisions/connector-timing.md): timing at the packet's expiry, F05 at once, F08 data, F01 and F02, segment-aware prefixes, unique request IDs.
- [own-fulfills](../decisions/own-fulfills.md): Fulfills booked as final; a stale resend from a peer is dropped.
- [packets-in-flight](../decisions/packets-in-flight.md): the cap on money in flight with a peer.
- [thresholds-in-packets](../decisions/thresholds-in-packets.md): the cap counted in the largest packet.
- [unreachable-peers](../decisions/unreachable-peers.md): no Prepare taken from a peer our messages surely do not reach.
