# Rate feed

A core node's public, cheap statement of what it converts at: for each ordered pair of its ledgers, the rate a packet gets, with its fee and each ledger's largest packet. It is a signed UDP datagram that the node makes once and sends to every subscriber, with no retries and no state beyond the subscriber list. Market makers watch feeds and trade through nodes whose rates make it worth it, which moves those nodes' holdings back toward balance.

`dassie-feed` holds the contract without IO: the wire format and its signature, the subscriber table and its cookies, when to publish, and a subscriber's side. `dassie market-maker` is a subscriber that trades on what it reads ([below](#market-maker)). The node service that publishes a feed, its socket and how a feed is found are not built yet ([roadmap](../roadmap.md#rate-feed-and-market-makers)).

**Paths:** `crates/dassie-feed/`, `testdata/feed/`, `crates/dassie-cli/src/market_maker/`
**Read with:** [Connector](connector.md#exchange-rates), [Identity](identity.md), [Encoding](encoding.md), [Node](node.md), [Payments](payments.md#paying-stream-credentials), [XRPL](xrpl.md#amm-pools)

## What a feed says

- **Who and when.** The node's ID and public key, a sequence in milliseconds that grows with every feed the node makes, even across a clock that went back, and a time after which the feed no longer counts.
- **The fee,** in parts per million, as the connector takes it ([Connector](connector.md#fees-and-spread)).
- **The ledgers,** by ID string, each with its largest packet, in that ledger's units. The IDs are strings rather than the node's closed list of ledgers, so a subscriber reads a feed that names a ledger it does not know, and a new ledger needs no new feed version. The largest packet is per ledger although the node has one value today ([roadmap](../roadmap.md#not-built-yet)), so that per-ledger values need no new version either.
- **A rate per ordered pair** the node converts between: what one unit in gets out, scaled by 10^15, spread included. A packet of any size that comes in with `amount` goes out with at most `floor((amount − fee) × rate / 10^15)`: each packet pays its own impact on the node's inventory, so how much less it gets depends on its size against the node's value: near target, about `4k` times its share of that value. A pair the node does not convert, for want of a fresh rate, an active scheme or the owner's funds on the way out, is left out; a rate is never zero.

The node is to publish the rates its connector's price table holds: what a vanishingly small packet gets, at the worst state the packets in flight could leave ([Connector](connector.md#inventory-pricing)). That is the node service's to keep.

## Wire format

Canonical OER ([Encoding](encoding.md)), starting with a version byte. `src/wire.rs` has the layouts; three messages share them:

- **Subscribe,** from a subscriber, with the cookie the node handed it if it has one, padded with zeros to a fixed size;
- **Challenge,** from the node, handing out a cookie;
- **Feed,** from the node, signed.

Every datagram fits in `MAX_DATAGRAM_BYTES`, which one packet carries on any path IPv6 runs on, so a feed is never fragmented. The bounds on the ledgers and rates are chosen so that the largest feed they allow fits, which a constant assertion checks; signing never meets a feed that does not.

**Every value has one encoding.** Ledgers are in ascending order of their IDs and rates of their pairs, each once; padding is zeros; and decoding refuses anything else, so a feed's bytes are exactly what was signed, and a decoded datagram re-encodes to its bytes (a property test). Decoders never panic, whatever the bytes, and every list is bounded before it is read.

**Versions.** A datagram of another version is refused. Within a version, no layout may change: subscribers such as market makers run software of their own, outside the node's release ([overview](../overview.md#frozen-surfaces)). A change is a new version byte, and a feed of the new version is signed under a new OID.

## Signing

A feed is signed by the node key (Ed25519) over its own bytes, which start with the OID `…1399.3.1` ([Identity](identity.md#object-identifiers)), so a feed's signature never passes for a link state's or anything else the node signs.

A subscriber verifies by ZIP-215 over exactly the bytes received ([Identity](identity.md#node-key)), refuses a weak key, and refuses a feed whose node ID is not its key's or not the node it subscribed to. It then takes a feed only if its sequence is after the last one it took and it has not expired, so a replayed or delayed datagram never sets rates back.

A feed is public, signed but not encrypted: it says nothing the node's packets would not.

## Subscribers

A node sends its feed to each address in its subscriber table, once per feed, and forgets it.

### The cookie

UDP's source address is whatever the sender writes, so a node that sent its feed to every address a Subscribe came from could be aimed at a victim. An address is subscribed only once it has shown that it receives there:

- **A Subscribe without a valid cookie** gets a Challenge carrying one, and nothing else. A Subscribe is padded to at least a Challenge's size, so a node never answers an address it has not proven with more bytes than it got from it, and cannot amplify an attack.
- **The cookie is a MAC** (HMAC-SHA256, truncated) of the address and the epoch, under a secret the node draws when it starts and keeps in memory only (`src/cookie.rs`). So the node keeps nothing for an address until it is proven, nobody computes a cookie for an address whose datagrams they do not receive, and a cookie proves only the address and port it was made for. An IPv4 address mapped into IPv6 is the IPv4 address.
- **A cookie is taken in its epoch and the next,** so it lives one to two epochs. One from the epoch before is still taken, and the node hands out a fresh one with the answer, so a subscriber that renews in time is never challenged again.

### The table

- **Bounded in all and per client** (an IPv4 address, or an IPv6 /56, as the node's other per-client limits: `ClientKey` in `dassie-node-core`), so one host cannot take every place by rotating ports or IPv6 addresses. A party with many clients, such as the 256 /56s of one IPv6 /48, can still fill the table and hold it by renewing: those subscribed before keep the feed, and newcomers are refused until entries lapse.
- **A full table refuses newcomers** and evicts nobody: every entry is a proven address that asked within its lifetime, and evicting one for a newcomer would let a party with many addresses push the others out. A refused subscriber is sent nothing, and tries again at its next renewal.
- **An entry lasts a fixed time,** renewed by every Subscribe with a valid cookie, and expires otherwise. Expiry follows the time the caller passes in.
- **In memory only.** After a restart the secret is new, every cookie is void, and subscribers are challenged and come back at their next renewal.

Every answer is a value the caller carries out: challenge, subscribed (perhaps with a fresh cookie), refused or ignored. A datagram that is not a Subscribe, does not decode, or comes from an address a feed cannot go to (port 0, or an IP that is unspecified, multicast or broadcast) gets nothing.

### Subscribing

A subscriber sends a Subscribe to the node at a fixed interval, well within an entry's lifetime and a cookie's, with the last cookie it was handed, and again at once when a Challenge hands it a new one (`src/receiver.rs`). It takes datagrams only from the node's address.

## Publishing

A node's rates move with every conversion, but a feed costs a signature and a datagram per subscriber. So a node publishes at most once per `FEED_MIN_INTERVAL_MS` while its rates change, and otherwise a heartbeat every `FEED_HEARTBEAT_MS`, so subscribers know the node is alive and its rates still hold. A change within the interval is published when the interval ends, never lost. A feed counts for a few heartbeats, so that one or two lost on the way do not leave a subscriber without rates (`src/schedule.rs`).

## Market maker

`dassie market-maker` trades through one core node, the target, when the rates its feed publishes and an AMM pool on the XRP Ledger disagree by more than a margin, and so moves the target's holdings back toward balance, as passive rebalancing intends ([roadmap](../roadmap.md#rate-feed-and-market-makers)). It is a client of nodes, not one: it holds no books, and every amount it moves is moved by a node or by the ledger. It reads one JSON config, checked whole before anything connects (`config.rs`); the decision is a pure function of the feed, the pool and the holdings (`strategy.rs`), and everything around it is bounded in time and size.

### Two nodes, pinned to the target

The market maker owns two periphery nodes, each running one scheme, the asset of the pair it holds: XRP on one, the pool's token on the other. Both pin the target as their only uplink ([Network](network.md#pinned-uplinks)). A peering is on one scheme, and a node's own endpoints never convert, so one node could not pay in one asset and be paid in the other through the target; two nodes, each peered with the target on its own ledger, make the target do the conversion its feed prices. Pinned, they reach the network only through the target, so a payment between them is converted by the target and no one else.

Each node's passphrase is in a file the config names. The market maker derives from it the node's XRPL account exactly as the node derives it ([XRPL](xrpl.md#the-wallet)), refuses to start unless that is the account the node reports for its ledger and the token node's ledger one of the pool's token's issuer, so that what a rebalance delivers is booked there. It keeps nothing of the phrase but the signing key in memory: no log line or error names a phrase.

### The loop

- **The feed.** It reads the target's node ID and key from its `GET /stats`, refuses an ID that is not the key's, and takes the feed's port from there, or from the config while `/stats` names none. It subscribes as [Subscribing](#subscribing) says, takes datagrams only from the target's address, and takes a feed only verified under that key, newer than the last and unexpired (`feed.rs`). A feed counts until its validity ends; with none, it holds.
- **The pool and the holdings.** Each tick it reads the pool with `amm_info` at a validated ledger, over a WebSocket of its own, and both nodes' holdings with `ListLedgers`: what each owner's books hold, and what its account holds on the ledger.
- **One decision a tick,** logged in one line, and at most one action.

### What it trusts

- **The feed, only to decide to quote.** A feed says what a vanishingly small packet gets; a real payment gets at most that, since the target prices by its inventory, and fees and rounding fall per packet. So a trade that clears the margin at the feed's rate is only quoted.
- **The quote, to decide to pay.** The receiving node creates an incoming payment; the paying node quotes its STREAM credentials with a fixed debit ([Payments](payments.md#paying-stream-credentials)); and the market maker pays only if what the quote delivers, swapped back on the pool, still clears the margin. The quote's rate is the least every packet must deliver, so the payment cannot do worse than the quote it was paid on. It follows the payment to its end, within a bound, and completes the incoming payment either way.
- **`SendMax`, to bound a swap.** A rebalance is one Payment from the full node's account to the other's, delivering the other asset with a `SendMax` of its own ([XRPL](xrpl.md#amm-pools)): it delivers all or nothing, and never spends more than `SendMax`, however the pool moved. What it asks the pool for is what a slippage's share less than `SendMax` buys at the price read, so it fails rather than pays a worse price. It signs only for a server on the testnet, checked at start and before each rebalance, as the node's account does ([XRPL](xrpl.md#real-money)): the nodes' keys open accounts of the same address on every network.
- **The pool's price, exactly.** Both checks price the swap back with `dassie-xrpl`'s exact pool arithmetic, its trading fee included, and round every conversion to the ledger's units down, so the market maker never counts on more than it gets.

### Bands

Each node has a band of what it holds, in its ledger's units. A trade never sends a node below the bottom of its band, and is made smaller to stay above it. A node above the top of its band is rebalanced to the band's middle before any trade, spending at most what its account holds on the ledger now, less the fee's headroom for XRP: the books count what the target owes before it settled, which the account cannot spend yet. It delivers no more than the other node's band has room for, so the two never swap the same funds back and forth, paying the pool's fee each way; with both above their bands, it holds. After a rebalance that paid, or may still pay, it decides nothing until the paying node has booked it, within a bound, since until then the node's holding still counts what it spent; a submit whose answer was lost is followed by its hash like any other. The two nodes' signing keys are the market maker's, so the account that withdraws is the one receiving through the target, and its node rarely signs anything itself; a sequence the two both took is rare, and the node recovers from one ([XRPL](xrpl.md#settling)).

## Test vectors

`testdata/feed/vectors.json` pins the wire format: Subscribes, a Challenge, feeds up to the largest the bounds allow, and feeds that decode but must not verify, all signed with a fixed key. There is no other implementation to take them from, so the crate's own generator makes them (`examples/feed_vectors.rs`, which says how to run it); Ed25519 signs deterministically, so running it again gives the same file. `tests/vectors.rs` decodes each and makes its bytes again. A changed vector is a new feed version.

## Decisions

- [passive-rebalancing](../decisions/passive-rebalancing.md): a signed UDP feed with proof of address, rather than rates over HTTPS on request; market makers as periphery nodes.
- [tigerstyle](../decisions/tigerstyle.md): deterministic, bounded, every decision a value.
