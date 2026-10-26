# Rate feed

A core node's public, cheap statement of what it converts at: for each ordered pair of its ledgers, the rate a packet gets, with its fee and each ledger's largest packet. It is a signed UDP datagram that the node makes once and sends to every subscriber, with no retries and no state beyond the subscriber list. Market makers watch feeds and trade through nodes whose rates make it worth it, which moves those nodes' holdings back toward balance.

`dassie-feed` holds the contract without IO: the wire format and its signature, the subscriber table and its cookies, when to publish, and a subscriber's side. A core node publishes its feed on a UDP socket of its own and names the socket's port where subscribers look ([the node side](#the-node-side)). `dassie market-maker` is a subscriber that trades on what it reads ([below](#market-maker)).

**Paths:** `crates/dassie-feed/`, `testdata/feed/`, `crates/dassie-node/src/feed.rs`, `resources/systemd/dassie-feed.socket`, `crates/dassie-cli/src/market_maker/`
**Read with:** [Connector](connector.md#exchange-rates), [Identity](identity.md), [Encoding](encoding.md), [Node](node.md#runtimes), [Peer protocol](peer-protocol.md#link-states), [Payments](payments.md#paying-stream-credentials), [XRPL](xrpl.md#amm-pools)

## What a feed says

- **Who and when.** The node's public key, from which its ID derives, a sequence in milliseconds that grows with every feed the node makes, even across a clock that went back, and a time after which the feed no longer counts.
- **The fee,** in parts per million, as the connector takes it ([Connector](connector.md#fees-and-spread)).
- **The ledgers,** by ID string, each with its largest packet, in that ledger's units: every ledger the price table prices, a settlement hub's as any other. The IDs are strings rather than the node's closed list of ledgers, so a subscriber reads a feed that names a ledger it does not know, and a new ledger needs no new feed version. The largest packet is per ledger although the node has one value today ([roadmap](../roadmap.md#not-built-yet)), so that per-ledger values need no new version either.
- **A rate per ordered pair** the node converts between: what one unit in gets out, scaled by 10^15, spread included. A packet of any size that comes in with `amount` goes out with at most `floor((amount − fee) × rate / 10^15)`: each packet pays its own impact on the node's inventory, so how much less it gets depends on its size against the node's value: near target, about `4k` times its share of that value. A pair the node does not convert, for want of a fresh rate, an active scheme that converts, such as a dollar token's off its peg ([XRPL](xrpl.md#the-peg-guard)), or the owner's funds on the way out, is left out; a rate is never zero.

A node publishes the rates its connector's price table holds: what a vanishingly small packet gets, at the worst state the packets in flight could leave ([Connector](connector.md#inventory-pricing)). So a feed promises nothing a packet does not get ([What it publishes](#what-it-publishes)).

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

A subscriber verifies by ZIP-215 over exactly the bytes received ([Identity](identity.md#node-key)), refuses a weak key, and refuses a feed whose key's node ID is not the node it subscribed to. The feed carries no node ID of its own: one beside the key would only be one more field every reader must check against it. It then takes a feed only if its sequence is after the last one it took and it has not expired, so a replayed or delayed datagram never sets rates back.

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

## The node side

`crates/dassie-node/src/feed.rs` is a supervised service, as the rate fetcher is. What a feed says is a pure function of the price table, and the publisher around the table and the schedule decides from its inputs and the node's clock alone; the service carries the decisions out against the socket.

- **Only a core node publishes.** A feed is found at the node's URL's host ([Finding a feed](#finding-a-feed)), so a node without a URL, a periphery node, answers no datagram, publishes nothing and never asks the engine for its table, and neither does a node without its key. A node that gets a hostname starts at once; one that loses it stops.
- **The cookie's secret** is drawn from the node's RNG each time the service starts, and kept in memory only, so a restart voids every cookie, as [The cookie](#the-cookie) says.
- **Counted, not logged.** Every datagram taken, answer sent, subscriber refused and send that failed is counted (`FeedStats`), never logged: a flood of Subscribes costs the node its bytes and a MAC each, and no journal lines. The node logs only whether it has a feed, in the line that says it listens ([Node](node.md#logging)); its owner reads the counters instead ([below](#what-the-owner-sees)).

### What it publishes

- **The price table, as the connector converts at.** The engine counts each rebuild of its price table that changed what a feed says: the charges, the ledgers it prices or the prices ([Connector](connector.md#inventory-pricing)). The service follows that count and fetches the table, at most once per `FEED_MIN_INTERVAL_MS` however busy the node, so that the schedule publishes a change after the least interval at most, and never asks the engine more often than it publishes.
- **What a feed says:** the node's key; the fee the connector takes on transit; each ledger the table prices, by its ID, with the connector's largest packet, which is one value for every ledger today; and each price the table has as a rate, so a pair without one is left out. Rates are the table's exactly, since a feed that promised more than packets get would send market makers into trades that fail.
- **Its validity** is the schedule's, or the moment a rate the table uses ages out, whichever comes first. A feed made after that names no ledgers until the engine builds the table again. While the engine is down, nothing is published, and a subscriber's last feed runs out.
- **The sequence** is the node's clock in milliseconds, made to grow ([Publishing](#publishing)). The service keeps its schedule across its own restarts; across a restart of the node, the clock has moved on. A clock set back across a restart holds a subscriber's next feed back until the clock passes the last sequence it took; nothing persists the sequence, since a feed lost is replaced within a heartbeat.
- **Each publish** encodes and signs once, then sends to each subscriber once, without retries, within half the least interval: a send the socket refuses or that does not fit the time is counted and dropped. A newly subscribed address gets the latest feed at once, if it still counts, so a subscriber need not wait for the heartbeat.

### What the owner sees

Nothing in the journal says whether the feed reaches anyone, so the node's status does: `GetStatus` carries the feed ([RPC](rpc.md)), and `dassie status` and the wallet's network page show it ([Node](node.md#the-dassie-command), [Wallet](wallet.md#pages)).

- **Whether it serves, and where:** the socket's address, and whether the node serves on it now, which is the rule [above](#the-node-side): a URL and a key. A node without the socket reports no feed; a periphery node reports one that does not serve, and `dassie status` and the wallet leave it out, since a periphery node publishing none is no fault.
- **How many subscribers now,** and none while the feed does not serve. It is the table's size when the service last changed it, so a subscription that lapsed counts until the next publish drops it, a heartbeat at most.
- **The counters since the node started:** feeds published; datagrams sent, sends the socket refused and sends a publish ran out of time for; datagrams taken, challenges, subscriptions new and renewed, refusals by reason (a full table, a client's share), datagrams ignored, and subscriptions that lapsed, those dropped to make room for a newcomer included.
- **Cheap and bounded.** The service adds to atomic counters as it goes, and the status reads each on its own, with no lock on the datagram path, so two may be a datagram apart. It is a fixed set of numbers: how many subscribe, never who, so the answer does not grow with the table and names no subscriber's address.
- **They live as long as the socket,** so a restart of the service keeps the counters, though its new table starts empty, and a restart of the node starts them over. Nothing of them is persisted, as nothing of the sequence is.

### Bounds

Everything the service holds is bounded by a named limit: a datagram it reads by `MAX_DATAGRAM_BYTES` (a longer one is read one byte over and refused); the table by `MAX_SUBSCRIBERS` in all and `MAX_SUBSCRIBERS_PER_CLIENT` per client, both in `Policies` so tests can shrink them; the sends of one publish by the table's size; the time a publish sends for; and the time it waits for the engine's table. The ledgers of a feed fit its bound by construction: the connector prices at most the ledgers of the node's realm, hubs' included, and no more than a feed may name (a constant assertion). Since the bound is part of the wire format, a realm with more ledgers than that needs a new feed version.

### The socket

The feed has a UDP socket of its own, apart from QUIC's: its datagrams are not QUIC, and a socket of its own keeps the two from ever reading each other's traffic. Its default port is `DEFAULT_FEED_PORT`, 7768: a fixed port, so that owners open one rule in their firewalls and subscribers find a node that names no port at all; above 1024, so that a node without privileges binds it; and one no service is registered for.

- **Under systemd** it comes from `dassie-feed.socket` (`ListenDatagram=7768`), taken by name like the others ([Node](node.md#runtimes)). It is optional, as QUIC's is: `dassie.service` only wants it, a node without it warns and runs, and an update that brings the unit enables it.
- **Standalone,** the node binds the HTTPS listener's address at that port, or where `--feed-listen` says: the dev environment and its tests give each node a port of its own (`6000+N`, moved by the port offset), since many nodes share one host.
- A socket that cannot be bound leaves the node without a feed, and running.

### Finding a feed

A subscriber needs the feed's port and the node's key; it takes the host from the node's URL.

- **The discovery document,** `/.well-known/dassie/node`, names the port, as `feedPort`, on a core node with a feed, beside the node's ID and key ([HTTP](http.md#the-nodes-discovery-document)). `dassie market-maker` reads it there.
- **The link state** names it too, in extension type 4 ([Peer protocol](peer-protocol.md#link-states)), so a node or tool that reads link states finds every core node's feed without asking each one. Only a node with a URL names one, at its URL's host.
- Neither names a port the node does not serve: a node without the socket, or without a URL, leaves `feedPort` out and issues its link state without the extension.

## Market maker

`dassie market-maker` trades through one or more core nodes, its targets, when the rates a target's feed publishes and an AMM pool on the XRP Ledger disagree by more than a margin, and so moves that target's holdings back toward balance, as passive rebalancing intends ([passive-rebalancing](../decisions/passive-rebalancing.md)). An end-to-end test runs the whole loop against the fake ledger's pool ([Testing](testing.md#end-to-end)), and a VM of our own runs it on the testnet ([Market maker VM](market-maker-vm.md)). It is a client of nodes, not one: it holds no books, and every amount it moves is moved by a node or by the ledger. It reads one JSON config, checked whole before anything connects (`config.rs`), which names its targets, at most as many as a node pins, each once; the decision is a pure function of the feeds, the pool, the holdings and which targets both nodes are peered with (`strategy.rs`), and everything around it is bounded in time and size.

### Two nodes, pinned to every target

The market maker owns two periphery nodes, each running one scheme, the asset of the pair it holds: XRP on one, the pool's token on the other. Both pin every target as an uplink, and no other node ([Network](network.md#pinned-uplinks)). A peering is on one scheme, and a node's own endpoints never convert, so one node could not pay in one asset and be paid in the other through a target; two nodes, each peered with the target on its own ledger, make the target do the conversion its feed prices.

Two nodes suffice for any number of targets, since each keeps an uplink to every target. A trade picks one: the receiving node's incoming payment names its address beneath that target ([Payments](payments.md#receiving)), and the paying node, peered with that target itself, reaches the address through it first. So the payment goes from one node to the target and from the target to the other, and is converted by that target and no one else. Two nodes per target would each hold funds that only that target's trades use; two in all share their funds, and their bands, across the targets.

Each node's passphrase is in a file the config names. The market maker derives from it the node's XRPL account exactly as the node derives it ([XRPL](xrpl.md#the-wallet)), refuses to start unless that is the account the node reports for its ledger and the token node's ledger one of the pool's token's issuer, so that what a rebalance delivers is booked there. It keeps nothing of the phrase but the signing key in memory: no log line or error names a phrase.

### The loop

- **The feeds,** one subscription per target, each on a socket of its own. It reads a target's node ID and key from its discovery document, refuses an ID that is not the key's, and takes the feed's port from there, or from the config for a target that names none. It subscribes as [Subscribing](#subscribing) says, takes datagrams only from that target's address, and takes a feed only verified under that target's key, newer than the last it took from it and unexpired (`feed.rs`), so one target's datagrams never set another's rates. A feed counts until its validity ends; with none current from any target, it holds.
- **Targets that do not answer.** A target whose discovery document fails is asked again a minute later, while the others trade, so one core node down at the start stops none of them. A target that turns out to be the node of an earlier one is traded through once.
- **The pool, the holdings and the peerings.** Each tick it reads the pool with `amm_info` at a validated ledger, over a WebSocket of its own; both nodes' holdings with `ListLedgers`: what each owner's books hold, and what its account holds on the ledger; and which targets each node lists as its neighbors with `ListPeers`, which is when the node takes an incoming payment beneath one.
- **One decision a tick,** logged in one line that names the target a trade goes through, and at most one action. It scores the trade through each target at that target's rates, and takes the one that gains most beyond the margin. Targets that pay alike, as core nodes whose curves are at their cap do, take turns: of those, the first after the one it last traded through, so its trades spread over them rather than wait on one target's settlements. A target that is not a confirmed uplink of both nodes yet is passed over: the payee would refuse a payment beneath it, and the payer could reach it only through another target, which would convert first.

### What it trusts

- **The feed, only to decide to quote.** A feed says what a vanishingly small packet gets; a real payment gets at most that, since the target prices by its inventory, and fees and rounding fall per packet. So a trade that clears the margin at the feed's rate is only quoted.
- **The quote, to decide to pay.** The receiving node creates an incoming payment beneath the target; the paying node quotes its STREAM credentials with a fixed debit ([Payments](payments.md#paying-stream-credentials)); and the market maker pays only if what the quote delivers, swapped back on the pool, still clears the margin. The quote's rate is the least every packet must deliver, so the payment cannot do worse than the quote it was paid on. It follows the payment to its end, within a bound, and completes the incoming payment either way.
- **`SendMax`, to bound a swap.** A rebalance is one Payment from the full node's account to the other's, delivering the other asset with a `SendMax` of its own ([XRPL](xrpl.md#amm-pools)): it delivers all or nothing, and never spends more than `SendMax`, however the pool moved. What it asks the pool for is what a slippage's share less than `SendMax` buys at the price read, so it fails rather than pays a worse price, rounded down to whole drops or millionths of the token, as the other node books it: an exact price takes every digit a token's value has, and an amount that leaves the line or the pool more digits than the ledger keeps is rounded by the ledger, and refused by the fake ([XRPL](xrpl.md#pools-in-the-fake-ledger)). It signs only for a server on the testnet, checked at start and before each rebalance, as the node's account does ([XRPL](xrpl.md#real-money)): the nodes' keys open accounts of the same address on every network.
- **The pool's price, exactly.** Both checks price the swap back with `dassie-xrpl`'s exact pool arithmetic, its trading fee included, and round every conversion to the ledger's units down, so the market maker never counts on more than it gets.

### Bands

Each node has a band of what it holds, in its ledger's units. The band is the node's, whichever target a trade goes through, so the targets share it, and a rebalance is decided as with one target, before any trade and without looking at the targets. A trade never sends a node below the bottom of its band, and is made smaller to stay above it. A node above the top of its band is rebalanced to the band's middle before any trade, spending at most what its account holds on the ledger now, less the fee's headroom for XRP: the books count what the targets owe before they settled, which the account cannot spend yet. It delivers no more than the other node's band has room for, so the two never swap the same funds back and forth, paying the pool's fee each way. A rebalance that can move nothing, since the other node has no room or what it would deliver rounds to nothing, does not stop the trades, or the market maker would hold while the trade that makes room clears the margin: it trades instead, but only from the other node into the one above its band, and only while the other is within its own band. That trade lowers the node without room, so at the next decision the rebalance has room and comes first again; it never takes the other node above its band, so it never leaves both above theirs; and like any trade it takes no node below its band. With both above their bands, it holds. After a rebalance that paid, or may still pay, it decides nothing until the paying node has booked it, within a bound, since until then the node's holding still counts what it spent; a submit whose answer was lost is followed by its hash like any other. The two nodes' signing keys are the market maker's, so the account that withdraws is the one receiving through the targets, and its node rarely signs anything itself; a sequence the two both took is rare, and the node recovers from one ([XRPL](xrpl.md#settling)).

## Test vectors

`testdata/feed/vectors.json` pins the wire format: Subscribes, a Challenge, feeds up to the largest the bounds allow, and feeds that decode but must not verify, all signed with a fixed key. There is no other implementation to take them from, so the crate's own generator makes them (`examples/feed_vectors.rs`, which says how to run it); Ed25519 signs deterministically, so running it again gives the same file. `tests/vectors.rs` decodes each and makes its bytes again. A changed vector is a new feed version.

## Decisions

- [passive-rebalancing](../decisions/passive-rebalancing.md): a signed UDP feed with proof of address, rather than rates over HTTPS on request; market makers as periphery nodes.
- [tigerstyle](../decisions/tigerstyle.md): deterministic, bounded, every decision a value.
