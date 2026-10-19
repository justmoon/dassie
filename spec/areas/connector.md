# Connector

The node's ILP connector: it knows the node's endpoints (BTP clients, local endpoints, peers), routes each Prepare to one of them, answers ILDCP, books the packet's pending transfers in the [ledger](ledger.md), charges transit, limits what may be in flight, and resolves the transfers when the response comes back or the packet expires. This doc also covers the exchange rates it converts at, the BTP server and its tokens.

**Paths:** `crates/dassie-connector/`, `crates/dassie-node/src/engine/core/packets.rs`, `crates/dassie-node/src/engine/core/tokens.rs`, `crates/dassie-node/src/http/btp.rs`, `crates/dassie-node/src/store/peer_settings.rs`, `crates/dassie-node/src/rates.rs`, `crates/dassie-node/src/node/rates.rs`, `crates/dassie-node/src/store/rates.rs`, `crates/dassie-node/src/rpc/rates.rs`
**Read with:** [Ledger](ledger.md), [ILP](ilp.md), [Peer protocol](peer-protocol.md), [Network](network.md), [Payments](payments.md), [Settlement](settlement.md)

The connector is a sans-IO state machine. It never reads a clock: `now` comes in with every call, and the caller polls for the next deadline. Packets are written into a buffer the caller passes, so the packet path does not allocate. Nothing on the packet path panics on input (ILP-3). It counts what it does (Prepares received, forwarded and answered; packets fulfilled, rejected and expired; wrong fulfillments, unexpected responses and duplicates) for instrumentation and simulation checks, and asserts its invariants after every step ([engineering](../engineering.md)).

## Addresses and endpoints

The node's ILP address is `<scheme>.das.<node ID>`, where `<scheme>` is `test` in the test realm and `g` in the live one. Every endpoint has an address, the [ledger](ledger.md) account its packets debit and credit, and that account's ledger.

| Endpoint | ID | Address (what ILDCP hands out) | Account |
|---|---|---|---|
| BTP connection | `btp:<n>`, the engine's number for the connection | `<own>.<6 characters of [a-z0-9]>`, random, free | `equity/owner/btp/<token ID>` |
| STREAM receiver, one per ledger | `local:<n>` | `<own>.stream-<slug>` | `equity/owner` |
| Payer, one per ledger | `local:<n>` | `<own>.pay-<slug>` | `equity/owner/local/pay` |
| Peer | `peer:<n>`, never reused, so a late response cannot resolve a later peering's packet | `<scheme>.das.<peer ID>` | `assets/interledger/<peer>` on its scheme's ledger |

- `<slug>` is the part of the ledger's ID before the `+`, for example `stub` or `xrpl-testnet`. BTP segments have no `-`, so they never collide with a receiver's or a payer's.
- At most 8 local endpoints exist at once: a receiver and a payer for each ledger. Their payments are specified in [Payments](payments.md).
- A child's address (BTP or local) lies below the node's own and is its route. Children's addresses never overlap: neither may be a segment prefix of the other.
- A peer's address is only what ILDCP tells it; packets reach peers through the route table.
- A peer's first outgoing request ID is random, so that after a restart new packets do not reuse IDs the peer still holds from before. Outgoing request IDs are otherwise counted up, skipping IDs in flight to that endpoint, so they are unique per endpoint (ILP-8).
- A request ID a source reuses while it is still in flight is a duplicate: nothing is done. A BTP client gets a BTP Error "a request with this ID is in flight"; a peer's duplicate is dropped.

## Routing

- **Below the node's own address,** only children: the child whose address is a whole-segment prefix of the destination. Anything else below it, and the node's own address itself, is unroutable (F02, NET-34). A child may address itself.
- **Beyond it,** the route table: for each prefix `<scheme>.das.<node ID>`, up to 4 first hops, best first, from the peer protocol's routing ([Network](network.md)). The connector takes the longest prefix that matches on whole segments (NET-33), and its first hop that is an endpoint, is not the packet's source (so packets do not bounce between peers), and is not draining. The table holds at most 1024 routes, none below our own address.
- **The owner's first hop.** For a packet from one of the node's own endpoints, the connector tells which endpoint it would go to first and that endpoint's ledger: what the owner must pay from, since the owner's own endpoints never convert.

## Handling a Prepare

A Prepare from an endpoint goes through these steps, in order. The first that fails answers the source at once with a Reject from this node (`triggeredBy` our address).

| Step | Refusal |
|---|---|
| The source is no endpoint (any more) | nothing is done |
| The request ID is in flight from the source | a duplicate (above) |
| The source is draining: its peering is ending | T01 "the peering is ending" |
| The packet does not decode | F01 "invalid packet" |
| The destination is `peer.config` or below it | answered as [ILDCP](#ildcp) |
| The connector holds as many packets as it may | T03 "connector busy" |
| The source has as many packets in flight as it may | T05 "too many packets in flight" |
| No next hop | F02 "no route found" |
| `expiresAt − 1 s < now + 1 s` | R02 "insufficient timeout" |
| `amount` > the largest packet | F08 "packet amount exceeds maximum allowed amount", with the amounts (ILP-4) |
| It would convert, and its source is not a peer | F02 "pay from the route's ledger" |
| It would convert, and a rate it needs is missing, or older than an hour | T00 "exchange rates are stale" |
| The conversion has no result | T00 "no exchange rate" |
| A non-zero amount whose output is zero | F02 "amount too small" |
| Money in flight with a peer would pass its cap | T05 "too much money in flight" ([below](#money-in-flight-with-a-peer)) |
| The source account's spending cap refuses it | F08 or T04 ([below](#spending-caps)) |
| The ledger refuses a transfer | T04, T03 or T00 ([below](#booking-a-packet)) |

**Expiry.** The outgoing packet expires at `min(expiresAt − 1 s, now + 30 s)`: the message window of 1 s leaves time for the response to travel back, and the next hop holds it at most 30 s. At least 2 s must remain of the incoming packet.

**What is forwarded.** The outgoing Prepare carries the outgoing amount and expiry; its condition, destination and data are copied unchanged. The packet is then in flight, keyed by its destination and the outgoing request ID.

## ILDCP

A Prepare to `peer.config` (or below it) is an ILDCP request only if it is addressed to `peer.config` exactly, carries the ILDCP condition and no money; anything else gets F01 "not an ILDCP request". One already expired gets R02 "expired". Otherwise the node fulfills it at once, with 32 zero bytes and the response `{address, assetScale, assetCode}`: the source endpoint's address as above, and its ledger's asset (ILP-7, NET-25). The codec is in [ILP](ilp.md).

## Amounts and conversion

- **Same ledger:** what goes on is what came, less our fee.
- **Across ledgers:** `convert(in − fee)`, less the spread ([below](#fees-and-spread)). Only a packet from a peer converts: the owner's own endpoints never do ([Ledger](ledger.md#the-owners-accounts)).

`convert` uses exact integer arithmetic, rounding down. Rates are fixed-point with 15 decimal places: units of the currency one US dollar buys, times 10^15 ([below](#exchange-rates)). For an amount `a` from asset `F` (scale `sF`) to asset `T` (scale `sT`):

```
same currency:   rate_F = rate_T = 1
out = floor( a · rate_T · 10^(sT − sF) / rate_F )        when sT ≥ sF
out = floor( a · rate_T / (rate_F · 10^(sF − sT)) )      when sT < sF
```

There is no result for a missing rate, or one that does not fit 64 bits. Powers of ten are exact integers (ILP-6). A conversion uses only rates learned at most an hour before `now`, and not after it: a clock that went back cannot make a rate young. The rate table holds at most 64 currencies.

## Exchange rates

How the node learns the rates it converts at. `rates.rs` holds the decisions, without IO: parsing a source's answer and combining the answers; `node/rates.rs` is the fetcher that carries them out.

### What a rate is

- Every rate is relative to the US dollar: how many units of a currency one dollar buys, times 10^15, an exact integer. The dollar's rate is 1 by definition.
- The node keeps rates for eight currencies: `USD`, `XRP`, `EUR`, `GBP`, `JPY`, `CHF`, `CAD` and `AUD`. Those are its ledgers' assets and the currencies an owner may see totals in.
- Each currency's rate carries the time it was learned, and ages on its own.
- A rate is good for an hour. A conversion that needs a rate older than that, or learned after `now`, is refused with T00 "exchange rates are stale" ([above](#handling-a-prepare)). The wallet shows each rate with when it was learned and whether it is stale.

### Sources

The owner keeps a list of at most 8 sources, in order. A new node starts with all three kinds, so that one wrong or silent source does not move a rate.

| Kind | Answers | Default URL | Covers |
|---|---|---|---|
| `coinbase` | `{data: {currency, rates: {CODE: "decimal"}}}`: units of each currency per unit of `currency` | `https://api.coinbase.com/v2/exchange-rates?currency=USD` | every currency |
| `kraken` | its ticker: `result.<pair>.c[0]`, the last trade, in dollars per XRP | `https://api.kraken.com/0/public/Ticker?pair=XRPUSD` | `XRP` |
| `bitstamp` | its ticker: `last`, in dollars per XRP | `https://www.bitstamp.net/api/v2/ticker/xrpusd/` | `XRP` |

- Only a `coinbase` source takes a URL of its own, since others serve that format too (such as the dev environment's `rates.json`). The URL must be `https://` with a host, at most 2048 bytes.
- The list is persisted in `rate_sources (position 0–7, kind, url)`; `url` is NULL for a kind's own server.

### Reading an answer

Every number is read in exact decimal arithmetic, never through floating point:

- A decimal is digits, optionally a `.` and more digits: no sign, exponent, leading `.`, trailing `.` or spaces, and at most 21 digits before the point. It is read to 15 decimal places, rounding down; a number too large is not one.
- **Coinbase's format** may have any base. The rate of each kept currency is its rate over the dollar's, `rate(code) · 10^15 / rate(USD)`, where the base's own rate is 1. An answer with no rate for the dollar is unreadable. A rate of zero, or one that does not read, is left out.
- **Kraken and Bitstamp** give a price in dollars per XRP; the rate is its inverse, `10^30 / price`. A price of zero is unreadable.
- An answer that is not JSON of the kind's shape is unreadable, and the source counts as not having answered.

### A cycle

The fetcher runs a cycle when the engine starts, when the owner changes the sources or the display currency or adds a settlement scheme, and a minute after the last cycle ends. A source that failed is simply asked again at the next cycle; there is no backoff.

1. **What is wanted:** the assets of the active ledgers, and the display currency. If that is only the dollar, nothing is fetched.
2. **Every source is asked at once,** with `GET` and `Accept: application/json`, through the node's outbound client, under its rules for outbound requests ([Peer protocol](peer-protocol.md)). A source has 10 s to answer with status 200 and at most 64 KiB.
3. **The rate per currency** is the median of the sources that cover it and gave a rate for it in this cycle, once more than half of the sources that cover it did. With an even number, the two in the middle must agree within 1% (`(high − low) · 100 ≤ high`), and the rate is their mean, rounded down; otherwise there is none. A currency only Coinbase's format covers, such as those of display, relies on the `coinbase` sources alone.
4. **A currency without a rate** this cycle keeps its last one, until it is an hour old, and the node warns.
5. **The new rates** are persisted, then handed to the engine.

### Rates across restarts

`exchange_rates (asset, rate, as_of_ms)` holds each currency's last rate, as a decimal string of the integer (it may not fit 64 bits), with the time it was learned. Rates are written as each cycle takes them, and restored at start, so a restarted node converts at once while they are less than an hour old. Stored rates of currencies the node does not keep are left out.

### The owner's total

The owner may choose a display currency, one of the eight, or none. It is persisted in `payment_settings.display_currency`.

The total is the sum over the active ledgers of the owner's posted balance on each ([Ledger](ledger.md#the-owners-accounts)), converted into the display currency at scale 9. It is shown only while every rate it needs is fresh: with one missing or stale, there is no total.

## Booking a packet

A packet that moves money books linked pending transfers, all or none ([Ledger](ledger.md#operations)), each referencing its trace ID:

```
same ledger:     Dr source          Cr destination      in − fee
across ledgers:  Dr source          Cr Ls:revenue/fx    in − fee
                 Dr Ld:expenses/fx  Cr destination      out
our fee:         Dr source          Cr Ls:revenue/fees  fee          (only if fee > 0)
```

`Ls` is the source's ledger and `Ld` the destination's. A packet of amount 0, or between two endpoints that share an account, books nothing. The ledger has room for three pending transfers for every packet in flight.

A ledger refusal becomes:

| Ledger error | Reject |
|---|---|
| `ExceedsDebits`, `ExceedsCredits`, `Overflow` | T04 "insufficient liquidity" |
| `PendingFull`, `JournalFull` | T03 "connector busy" |
| `UnknownAccount`, `SameAccount`, `DifferentLedgers`, `ZeroAmount` | T00 "internal error" |

So a peer can push packets to us only while it stays within the credit we extend (the limit on its `assets/interledger` account), and the owner's endpoints only while the owner's group has funds.

## Responses and expiry

A response comes from the endpoint the Prepare went to, with the outgoing request ID.

- **No packet in flight** with that ID: dropped, and counted.
- **A peer's Fulfill whose fulfillment is not the packet's:** a stale resend of an earlier packet's with the same request ID. It is dropped, and the packet waits for its own response or its expiry (D13).
- **After the outgoing expiry:** the packet times out, R00 "packet timed out", whatever the response says (ILP-1).
- **A Fulfill whose SHA-256 is the condition:** the transfers are posted, and the Fulfill's bytes go upstream unchanged, once the batch that posted them is on disk ([Node](node.md#the-engine)).
- **Any other Fulfill:** F05 "invalid fulfillment", at once (ILP-2); the transfers are voided.
- **A Reject:** the transfers are voided, and the Reject's bytes go upstream unchanged.
- **A response that does not decode:** T00 "invalid response from the next hop"; the transfers are voided.

**Deadlines.** A packet still in flight at its outgoing expiry is rejected upstream with R00 "packet timed out", and its transfers voided. The connector also fails a packet early:

- when its next hop is removed: T01 "next hop went away". A child may already have fulfilled it; that loss is on the child's own account, since we never got the fulfillment;
- when it surely did not reach its peer: T01 "the peer could not be reached". That is a refused connection, or 400, 401, 409 or 503 in reply ([Peer protocol](peer-protocol.md)). A Prepare that may have reached its peer is left to expire: a peer that fulfilled a packet we already failed would bear its loss.

A packet whose source went away is still resolved by its response, without output.

**Fulfills are final.** A packet from a peer is posted when it is fulfilled, and stays posted whatever its upstream does with the Fulfill ([Ledger](ledger.md#fulfills-booked-as-final)). The peer protocol sends a Fulfill or Reject to a peer again until it arrives or 30 s have passed ([Peer protocol](peer-protocol.md)); a Reject that never arrives has the effect of expiry upstream.

**Draining.** While a peering ends, its endpoint is draining: the connector routes no new Prepare to it and refuses its new Prepares (T01; from a peer they never get that far, since the peer protocol answers 409 "the peering is ending" first, so T01 is a backstop), but packets in flight finish, and their responses still flow. The peering ends once no packet is in flight to or from it ([Network](network.md)).

## Money in flight with a peer

The connector keeps, for each peer, what the packets in flight from it carry in, and what those to it carry out. Each is capped at

```
max_in_flight = 2 · P + P / 2          -- P = the largest packet; 0.25 units at the default
```

- Two of the largest packets fit, a third does not, and the half packet left takes smaller ones, such as a quote's probes while a payment fills the link.
- A Prepare that would take either way past the cap gets T05 "too much money in flight", from which STREAM backs off as from any temporary error.
- The cap counts amounts, not packets: packets from parts of the network with a smaller largest packet just fit more of them under it.
- Packets without money always pass, and a packet passes alone, with nothing else in flight that way, even if a conversion made it larger than the cap.
- A crash loses the Fulfills of what was in flight, so this is the most one crash drifts two peers' books apart. Each node bounds both directions with its own cap, knowing nothing of the peer's.

## Fees and spread

What passes from one peer to another is charged the owner's fee and spread, in parts per million, each from 0 to 100,000 (10%), default 0. Packets to or from the node's own endpoints are never charged.

```
fee = floor(in · fee_ppm / 10^6)
out = in − fee                                                   -- same ledger
out = floor(convert(in − fee) · (10^6 − spread_ppm) / 10^6)      -- across ledgers
```

- The fee rounds down: a packet too small to carry one passes free. Rounded up, it would take a payment's last packets whole, and strand it short.
- The spread applies only to a conversion. Its gain is the net of the FX accounts; it is not booked per packet.
- A packet whose output rounds to nothing is refused (F02 "amount too small").
- The fee is the third linked transfer, Dr the source's account, Cr `revenue/fees` on the source's ledger. Fees are what a peer earns us, from which the credit we extend it grows ([Settlement](settlement.md#credit)).
- New charges apply from the next packet. They are persisted in a single-row table, `peer_settings (connector_fee_ppm, fx_spread_ppm)`, each `CHECK`ed to 0–100,000.

## Spending caps

A BTP token may carry a cap: at most `A` per window `W`, refilling continuously. `A` is from 1 to 2^63 − 1 in the token's ledger's units, `W` from 1 s to 366 days. The connector holds at most 256 caps.

- **What counts** is a packet that debits the token's account and credits an account outside the owner's group (or on another ledger): a peer. A payment to the node's own receiver, or to another token's client, moves no money out of the owner's funds and does not count.
- **The bucket**, in exact integers: `U` (amount·ms) as of time `τ`, and `R`, what packets in flight hold. `U` drains at `A` per ms: `U(t) = max(0, U − (t − τ)·A)`; a time before `τ` changes nothing.
  - A Prepare of `a` is refused with F08 "packet amount exceeds the spending cap" (maximum `A`) when `a > A`, and with T04 "the spending cap is reached" when `U(t) + (R + a)·W > A·W`. Otherwise `R` grows by `a`.
  - A fulfilled packet adds `a·W` to `U`; a rejected one only frees `R`.
  - So `U + R·W ≤ A·W` always, and what a token spends in any period `t` is at most `A + A·t/W`.
- **It is enforced in the connector**, per account, so all of a token's connections share one cap. The cap is checked before the ledger books the packet.
- **The usage is checkpointed** with the journal rows that change it: `cap_used_amount · W + cap_used_remainder` and `cap_as_of_ms` in `btp_tokens`. The journal alone cannot rebuild it from a recent window: two histories with the same recent rows can have used different amounts. A replay of the token's journal since the cap was set is the reference the tests check the checkpoint against.
- **Changes:** setting a cap where there was none starts from nothing used; changing one carries the usage over to the new cap, rounding the amount up; removing it, or revoking the token, drops it. Packets in flight count against the cap either way.
- **What the owner sees:** the cap, what could be spent now (`(A·W − U(t) − R·W) / W`), what packets in flight hold, and when the bucket is full again.

## The BTP server

ILP clients such as `ilp-plugin-btp` connect with `btp+wss://:<token>@<host>/btp`: a WebSocket upgrade of `GET /btp` (RFC 23, BTP 2.0). Clients are not browsers, so there is no Origin check: the token is the authentication. The BTP codec is in [ILP](ilp.md).

**Upgrade.**

| Condition | Answer |
|---|---|
| The engine is not running | 503 "The ledger is not running" |
| The client's address failed to authenticate too often | 429 "Too many failed attempts" |
| 64 BTP clients are connected | 503 "Too many BTP clients" |

Failed authentications count per client address (an IPv4 address or an IPv6 /56): a burst of 5, refilled one every 12 s, over 256 addresses. Only failed logins count, so honest clients may reconnect as often as they need.

**Authentication.** Within 10 s of the upgrade (pings and pongs may come first), the client sends a BTP Message whose first protocol-data entry is `auth` with empty data, and which carries an `auth_token` entry of content type 1 (UTF-8 text) holding the token. The node hashes it, looks the hash up among tokens not revoked, and answers a Response with the same request ID and empty protocol data. Anything else closes the socket with code 1008 and counts as a failed login: no message in time, a non-binary frame, a frame that is not BTP, no `auth`, or an unknown token.

**After authentication.**
- A **Message** whose `ilp` entry holds a packet: a Prepare for the connector, with the BTP request ID.
- A **Response** with an `ilp` entry: the response to one of our Prepares. One without is an acknowledgement, and ignored.
- An **Error**: it answers our Message as a failure. The Prepare it answers is rejected at once with the Error's code, if it is an ILP error code (else T00), no `triggeredBy`, and the message "the BTP client answered with an error".
- A **Transfer** is refused with a BTP Error: money moves only in ILP packets.
- A Message without an `ilp` entry gets a BTP Error "expected ILP data".
- A text frame closes the socket (1003 "BTP is binary"); a frame that is not BTP closes it as a protocol error (1002).

**What the node sends.** A Prepare as a Message `[{ilp, octet-stream, packet}]` with the outgoing request ID; a Fulfill or Reject as a Response with the client's request ID. A BTP Error the node sends has code `F00`, name `NotAcceptedError`, an empty `triggeredAt`, and the reason as its data.

**Bounds and endings.**
- A BTP message is at most 64 KiB.
- At most 256 messages wait for a client; a client that does not read them is disconnected.
- A client has at most 60 packets in flight.
- Revoking a token closes its connections (1008 "disconnected"), after the messages already queued. The node's shutdown closes them with 1001 "node shutting down", an engine restart with 1011 "ledger restarting".
- When a client goes away, its packets in flight are failed upstream; packets it sent keep running, and their responses are dropped.

### BTP tokens

- **Creating** a token takes a label of at most 64 bytes, a ledger that an active scheme serves, and optionally a spending cap. Its secret is 16 random bytes, 22 base64url characters, shown once at creation. The database keeps its SHA-256, never the token ([D10](../decisions/D10-secrets-at-rest.md)), with the label, the ledger, the time it was made, when it was revoked, and its cap and usage.
- Each token gets a sub-account, `equity/owner/btp/<id>`, where `<id>` is the token's number: never reused, starting at 1. Its client spends from the owner's pool through it.
- At most 64 tokens are not revoked, and at most 1024 have been made: each keeps its account, which the journal refers to.
- The owner sees, for each token not revoked: its label, ledger, creation time, what was received less sent through it (posted), how many clients are connected, and its cap.

## Local endpoints

The STREAM receivers and payers are in-process endpoints that the engine adds when a scheme activates and removes when it goes. A packet the connector forwards to a local endpoint is delivered in the same step; a payment from the node to its own receiver goes through the connector like any other, through a bounded queue drained at the end of the step, so nothing recurses. Their behaviour is in [Payments](payments.md).

## The packet trace

The connector keeps a bounded record of recent packets, which the journal's packet references point into: the trace ID, source, destination endpoint, destination address (up to 96 bytes), the amount in and out, when it arrived and was resolved, what became of it (pending, fulfilled, or rejected with a code), and its transfers. It holds 4096 records, evicting the oldest, and queues at most 1024 events for live subscribers; events nobody polled are dropped and counted.

## Bounds

| What | Value |
|---|---|
| Endpoints | 256 |
| Packets in flight | 8448 |
| Packets in flight from one BTP client or local endpoint | 60 |
| Packets in flight from one peer | 512 (a peer carries other nodes' traffic too) |
| Routes | 1024, up to 4 first hops each |
| Largest packet `P` | 0.1 units (100,000,000 at scale 9), node-wide |
| Money in flight with a peer, each way | `2.5 · P` |
| Message window, least timeout, longest hold | 1 s, 1 s, 30 s |
| Exchange rates usable for | 1 h |
| Spending caps | 256 |
| Trace | 4096 records, 1024 events |

The largest packet, and with it everything that follows from it, is one value for the node. Per-ledger values, for it and for the credit line ([Settlement](settlement.md#credit)), are not built ([roadmap](../roadmap.md#not-built-yet)).

## ILP-over-HTTP

ILP-over-HTTP (RFC 35, with its synchronous mode) is part of the compatibility contract, but it is not built: the node serves no `/ilp` route, and the connector has no HTTP endpoint ([roadmap](../roadmap.md#not-built-yet)).

## Decisions

- [D6](../decisions/D06-owner-ledger-and-multi-currency-accounts.md): each endpoint has a ledger; ILDCP answers its asset; the owner's own endpoints never convert; the display currency.
- [D7](../decisions/D07-exchange-rates-fees-and-spread.md): refreshed rates, staleness, exact math, the median of several sources; fees and spread, and why the fee rounds down.
- [D10](../decisions/D10-secrets-at-rest.md): BTP tokens are kept hashed.
- [D13](../decisions/D13-connector-timing-and-packet-limits.md): timing at the packet's expiry, F05 at once, F08 data, F01 and F02, segment-aware prefixes, unique request IDs.
- [D23](../decisions/D23-each-node-books-its-own-fulfills.md): Fulfills booked as final; a stale resend from a peer is dropped.
- [D24](../decisions/D24-money-in-flight-with-a-peer-stays-within-a-few-packets.md): the cap on money in flight with a peer.
- [D25](../decisions/D25-a-links-thresholds-are-counted-in-its-largest-packet.md): the cap counted in the largest packet.

## Original

- [4 §3.3–§4](../original/04-interledger.md): the original's endpoint types, routing interaction and connector pipeline, with its fixed 5 s timeout.
- [4 §5](../original/04-interledger.md), [5 §1.3](../original/05-accounting-and-settlement.md): the original fetched Coinbase once, at start, and converted through JavaScript doubles; it never refreshed.
- [4 §7–§8](../original/04-interledger.md): the original's local endpoints and its ILP-over-HTTP stub.
- [4 §12](../original/04-interledger.md): the original's constants.
- [3 §14](../original/03-peer-network.md): the original's BTP server.
- [2 §9](../original/02-node-runtime.md): the original's BTP tokens, stored in plaintext.
