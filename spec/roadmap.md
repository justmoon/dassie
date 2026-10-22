# Roadmap

What is not built yet: the open areas of work, the gaps between the spec's contract and `main`, the conditions for real money, smaller open points, and what stays future work. Everything else in the spec describes what is on `main`; an area doc that meets one of these gaps says so in one line and links here.

**New work is an open area of work here, never a numbered milestone.** Areas proceed in parallel, and each says what done means. When an area is built, its behaviour moves into the area docs and its entry here goes.

## Open areas of work

### Settlement hubs

Another way to peer, beside bilateral peering: a ledger kept by n = 3f + 1 validators and backed by an escrow on the XRP Ledger, so that two nodes peered over a hub cannot walk away with what they owe each other. The design is [designs/settlement-hubs.md](designs/settlement-hubs.md); the reasons are [settlement-hubs](decisions/settlement-hubs.md).

- **Parts that can be built side by side:**
  - `dassie-hub`: the hub's ledger, its operations and Simplex consensus, in the simulation without an escrow;
  - the `hub` settlement scheme: holds, commitments and claims, peering over a hub, joining by being paid, against the hub's ledger in the simulation;
  - the escrow on the XRP Ledger testnet: founding, deposits, withdrawals, changing validators, shutdown and payout, against the fake XRP Ledger.
- **Done when:** in the simulation, with up to f faulty validators, members peered over a hub pay each other across hops without trusting each other: a payer that sends no claims, and a payee that claims twice or late, cost the other no more than its hold. f + 1 colluding validators leave evidence. A hub that loses a validator shuts down, and the escrow pays out exactly the final balances, with signatures collected over time.
- Hubs hold real money only once the conditions of [Real money](#real-money) are met.

### Continuous simulation

The simulations are to run continuously, outside CI, from random seeds; each seed draws its own configuration, and a failure becomes an issue that names its seed ([random-seed-simulations](decisions/random-seed-simulations.md)). The runner, its campaign command, the tripwire and the campaign's supervisor exist ([Testing](areas/testing.md#the-campaign)); a spot VM runs it on `main` (`scripts/sim-campaign/provision.sh`), and releases do not wait for it yet. Not built, in parts that can be built side by side:

- **Swarm testing** in the peer, connector and STREAM simulations, as the `node` simulation has it: before a run starts, its seed draws the whole configuration (nodes, latency, loss, duplication, partitions, crashes biased toward payments in flight, clock skew, faults of the ledger link, the share of lying peers, the workload), and the run prints it first. Some seeds switch a fault off entirely, so rare combinations get their turn; a fixed realistic profile runs alongside. The `node` simulation does not yet draw lying peers, runs across currencies, or RLUSD with its issuer's acts, which only the scripted runs reach.
- **Two phases** in the peer, connector and STREAM simulations, and in the `node` simulation the checks during the chaos (it checks the books once, after the heal). A run first has a stretch of chaos, with every invariant and every [per-node check](areas/testing.md#each-node-judged-by-its-own-books) checked throughout. Then it heals, and must [settle down in production time](areas/testing.md#simulation-time-follows-production). Lying peers are judged only by what they cost the honest ones.
- **Coverage** in the peer, connector and STREAM simulations, as the `node` simulation has it: each run counts what it reached, and a test in CI asserts a minimum for each count over fixed seeds, so that a generator cannot quietly stop reaching a case.
- **The release check, once a live network runs on Dassie's releases** ([random-seed-simulations](decisions/random-seed-simulations.md)). The release workflow's first step refuses to build, sign or publish a tagged commit on which an open `sim-failure` issue was seen, or on which the campaign has not yet run long enough. A refused tag is deleted, and the version is tagged again once the campaign has cleared a commit.

**Done when:** the campaign runs unattended on `main`, files one issue per failure signature with a command that reproduces it, and its tripwire keeps showing up. CI asserts the coverage minimums.

**Not yet decided:** coverage-guided fuzzing of the decoders that face untrusted input (outside CI, [Engineering](engineering.md#no-fuzzing-or-random-seeds-in-ci)), real binaries behind a proxy that injects faults, and shrinking failing runs.

### Rebuilding releases

A release is built once, on the Vorstand Nix cloud, and the release workflow signs what that build made ([Release](areas/release.md#the-build)). Its archives are deterministic, but nothing builds the release a second time to compare: whether the binaries reproduce on another builder is untested, and the release key vouches for whatever the one builder and its cache produced ([release-integrity](decisions/release-integrity.md)). Building again to verify would be welcome, but GitHub's runners are very slow, and the project is early and wants to move fast.

- **The likely shape:** the project may move its builds off GitHub, onto a build system that is Nix throughout. A dedicated builder that builds nothing but releases then runs `nix build --rebuild` on the release before it is signed: it builds the store path again, from the source and the pinned inputs, and fails if the bytes differ from what the Nix cloud made. Signing waits for it.
- **A bypass for urgent hotfixes:** a hotfix that cannot wait for the second build may be signed without it, by an explicit choice that the release run records.
- **Done when:** every release, but a hotfix that took the bypass, is signed only once a second build on the release builder has produced the same bytes.

### Registration fees

Registration is free, and throttled instead ([Network](areas/network.md#registration)). The design calls for a paid registration whose fee grows with the size of the list ([membership-economics](decisions/membership-economics.md)).

- **What it covers:**
  - **The fee quote and payment,** as a v1.x peer-protocol addition: a quote message, and payment by ILP with a signed quote condition (the "naive" variant of the [token payment protocol](original/05-accounting-and-settlement.md)), to the cheapest 75% of bootstrap nodes.
  - **The fee schedule,** set when the quote and payment are built.
- **Done when:** a node registers by paying the quoted fee, and is refused without it.

### Periphery

Nodes without a public IP join as periphery nodes: they connect only to core nodes, get child addresses beneath them, and never forward. The design is [designs/periphery.md](designs/periphery.md); the reasons are [core-and-periphery](decisions/core-and-periphery.md). Periphery nodes with their uplinks, over the QUIC connections they open, with child addresses and downlinks under a bound of their own, are built ([Network](areas/network.md#periphery-nodes), [Connector](areas/connector.md#a-periphery-node)).

- **Parts that can be built side by side:**
  - QUIC for periphery links: dialling an uplink again with a backoff when its connection is lost, and the fallbacks where UDP is blocked (TLS over TCP, WebSocket, WebTransport for browser nodes);
  - the downlink table: downlinks on disk in a table of their own rather than in the node table, in memory only while connected, with rows closed once nothing is owed either way, and the XRPL scheme's peers' addresses in a table keyed by account rather than in its state ([§4](designs/periphery.md#4-downlinks-on-a-core-node));
  - BTP clients of a periphery node, whose ILDCP address is still beneath the node's own address rather than an uplink's;
  - periphery nodes in the `peer` and `node` simulations' chaos runs, losing and regaining uplinks;
  - records and home nodes, so that clients find the relays that reach a node. Relays are built: every QUIC endpoint relays streams to the nodes connected to it, so every uplink is already a relay for its downlinks, and a node keeps a connection to a relay it chooses ([Peer protocol](areas/peer-protocol.md#relayed-streams)). Relayed streams carry Open Payments and paired owner apps' RPC;
  - owner apps. The node pairs them and serves their RPC on relayed streams ([RPC](areas/rpc.md#paired-devices)), and the wallet on `localhost` is to show the pairing code; left are the apps themselves (a remote mode of the desktop app, a mobile app), consent to grants in the app as it connects (listing and following pending grants), the front door's consent page that sends the owner there, and finding the node on the local network;
  - the [desktop app](areas/desktop.md) beyond its Nix package: packages for other Linux distributions (AppImage, .deb, Flatpak), the app in the mirror's flake from a signed build, its updates, and macOS and Windows;
  - front doors. Front doors are built, for the nodes that keep one as their relay, which a node without a hostname does with `dassiepay.net`'s by default ([HTTP](areas/http.md#front-doors), [Node](areas/node.md#a-chosen-relay)), and so are a node's names and its signed responses ([Payments](areas/payments.md#the-nodes-names)); left are their consent page (the front door answers the consent route with a fixed error until then), an owner's own domain as a CNAME to a front door with a certificate for that name, the DNS TXT record that names the node and its check in Dassie's own client, the owner's names as a wallet setting rather than an environment variable, a node's rate limits per client behind its chosen relay (today a node counts its front door as one client, so a few clients use up its budget for every client through it: [HTTP](areas/http.md#front-doors)), and the proposal to Open Payments;
  - free receivers, with the XRPL scheme's rules for accounts that do not exist yet; on RLUSD a payment can create neither an account nor a trust line, so a free receiver there needs XRP first;
  - browser nodes;
  - **becoming core** ([promotion-to-core](decisions/promotion-to-core.md)). On `main` a node with a hostname is core and one without is periphery; under [promotion-to-core](decisions/promotion-to-core.md) every node starts in the periphery and is core only when the signed data says so: registered, in the 2-core of the graph of edges both ends advertise, in its component with the most bootstrap nodes. The parts:
    - computing the core from the majority list and fresh link states, the same in every node, and routing to a node's own address only once it is core;
    - promotion: an uplink advertises its downlink as a neighbor once the downlink is registered, answers at its URL with a publicly issued certificate, has an hour of good standing and its heartbeats, twenty of the largest packets of room in the uplink's books and a completed settlement, a clock within bounds, and no ban; a link-state entry for a core node's free core relationships, so a node chooses uplinks that can promote it;
    - a peering's kind following the peer's status in place, both ways, with its accounts and channel, and the uplink routing the child address for as long as they are peered;
    - a node using its own address after a set wait, and its child addresses again at once when it stops being core;
    - demotion: bootstrap nodes dropping a registered node whose URL or certificate fails for longer than becoming core takes, and uplinks no longer advertising a peer that fails the promotion criteria;
    - the first core: bootstrap nodes peering as core peers and advertising each other without the checks, and fewer edges asked of networks with fewer than three bootstrap nodes;
    - a peering's kind decided by the peer's status rather than its link state, with the periphery extension meaning only that a node has no URL.

    **Done when** in the simulation a node with a URL stays periphery until registered and promoted by two uplinks, then is routed at its own address by every node after the wait, and drops back when it goes away; and two networks that share no bootstrap node majority never both count as the core.
- **It needs** [the channel limit](#not-built-yet) lifted, so that a core node keeps few relationships with core nodes.
- **Done when:** a periphery node behind a NAT peers with two core nodes, gets an address beneath each, pays and is paid through them, and keeps working when it loses one. Its owner's paired app reaches it through a relay, and a third-party Open Payments client pays it through a front door, whose response a Dassie client checks against the node ID. A free receiver is paid, spends through its uplink, and has its account created by the first settlement. In the simulation, home nodes that withhold, and ground core keys, hide no node from a relay it chose.

### Onboarding

A new owner opens a setup URL, enters a passphrase and chooses a first ledger ([Overview](overview.md#life-of-a-node)); every other choice is a default in the node's environment. An onboarding in the wallet is to walk the owner through the choices that matter, explaining each. It is not designed yet; what it must hold so far:

- **Choosing a relay** is one of its steps. A node without a hostname keeps a connection to the `dassiepay.net` front door by default, so that it is reached at its default name ([default-relay](decisions/default-relay.md)). The step explains what that costs: a machine the project runs learns when the node is online, and reads the Open Payments requests it passes on, as any front door does. The owner keeps it, names another relay, or keeps none and is not reached at that name.
- **Done when:** an owner sets a node up in the wallet without the command line, and makes each choice the node otherwise defaults, the relay among them, knowing what it costs.

### Rate feed and market makers

A core node that converts between ledgers fills up with one asset and runs out of the other. Rebalancing is passive ([passive-rebalancing](decisions/passive-rebalancing.md)): the node prices transit by its inventory ([Connector](areas/connector.md#inventory-pricing)), publishes what it converts at, and market makers trade through it when that pays, which moves it back toward its targets. Inventory pricing, the feed's contract and its sans-IO core are built ([Rate feed](areas/rate-feed.md)), and so is the market maker, `dassie market-maker`, which subscribes to a node's feed, verifies it, pays through the node when its rate and an AMM pool on the XRP Ledger leave a margin, and moves its own funds back through the pool ([Rate feed](areas/rate-feed.md#market-maker)). Not built, in parts that can be built side by side:

- **The node's feed service:** a core node publishing its feed from the rates its connector applies, signed with its node key, on a UDP socket of its own (an optional systemd socket, handled as QUIC's is, and its own port when standalone), with the subscriber table and the publish schedule of `dassie-feed`.
- **Finding a feed:** a link-state extension that names the feed's port, and the port in `GET /stats`, which the market maker reads; until then its config names the port.
- **The whole loop, end to end:** a core node with XRP and DSUSD skewed to one side, the market maker's two nodes pinned to it, and the fake ledger's pool, run until the core node's holdings move toward balance and the market maker's value at the pool's price grows.

**Done when:** a core node publishes its feed to subscribers who proved their address, and a market maker that watches it trades through the node, moving the node's holdings toward balance.

### Checking XRP Ledger validators

The XRPL scheme takes its server's word on which ledgers are validated and what they hold ([XRPL](areas/xrpl.md#following-the-account)). Acting only on ledgers past the newest seen ([ledger-resets](decisions/ledger-resets.md)) protects against a server that lags, not one that lies. A dishonest server can make the scheme book a settlement that never arrived, cancel one that applied by calling the account absent, or, on the testnet, claim a reset by reporting a lower ledger that closed later, so that the node writes off its balance. The node is to check the validators' signatures itself, so that no server can fake validated state or a reset.

- **What it covers:**
  - **A UNL of the node's own:** the validator list it trusts, signed by a list publisher whose key the node is configured with, and the validators' manifests, which bind each validator's signing key to its master key and can revoke it.
  - **Validations checked on every ledger the scheme acts on:** a ledger counts as validated only once a quorum of the UNL has signed its hash. What the scheme reads from it, its transactions and the account's state, counts only once it is tied to that hash; how each answer is tied to it is part of the work.
  - **A reset** is a new chain that the UNL validates, so a server alone can no longer claim one.
  - **The fake ledger** gets validators that sign its ledgers, and a server that lies, so the simulation can show the scheme ignoring it.
- **It matters before real money.** A mainnet scheme that took one server's word would let that server's operator have the node book XRP that never arrived. [real-money](decisions/real-money.md)'s conditions do not name this check; whether they should is decided with the mainnet scheme ([Real money](#real-money)).
- **Done when:** a server that reports a transaction, a balance or an absent account that no validated ledger holds, or a reset no quorum signed, gets nothing booked, written off or cancelled, and the scheme waits until an honest server answers; with an honest server, the scheme settles as before.

## Not built yet

Gaps between what the spec's contract or decisions ask for and what `main` does. Each can be picked up on its own.

- **ILP-over-HTTP** (RFC 35, with its synchronous mode): the node serves no `/ilp` route ([Connector](areas/connector.md#ilp-over-http)). **Done when** an RFC 35 client, synchronous mode included, pays through the node over `/ilp`.
- **Hybrid post-quantum TLS.** Confidentiality against "harvest now, decrypt later" belongs to TLS with the hybrid key exchange X25519MLKEM768 ([peer-protocol-v1](decisions/peer-protocol-v1.md)). The node's TLS uses rustls with ring as its only crypto provider, which offers no ML-KEM, so its handshakes use classical key exchange only ([HTTP](areas/http.md#certificates)). **Done when** the node's listeners and its clients (peer client, ACME) offer and prefer X25519MLKEM768, with a crypto provider that still builds the static musl binaries.
- **Per-ledger credit line and largest packet** ([owner-ledger](decisions/owner-ledger.md)). Both, and with the largest packet every threshold counted in it, are one value for the whole node ([Connector](areas/connector.md#the-largest-packet), [Settlement](areas/settlement.md#credit)), so the same thresholds count for more in XRP than in dollars. **Done when** each ledger is configured with its own, and the thresholds and the junior's starting credit follow the ledger's.
- **Draining in-flight packets on shutdown.** On SIGTERM the engine stops at once; it does not wait for the packets in flight to be answered or to expire ([Node](areas/node.md#shutdown)). Their pending transfers were never on disk, so after the restart the books stand as if they had been rejected, and a Fulfill that came back is lost, as in a crash; the cap on money in flight with a peer bounds what that costs ([Connector](areas/connector.md#money-in-flight-with-a-peer)). **Done when** the node stops taking new Prepares on SIGTERM and waits, up to the latest expiry among them, for the ones in flight before it closes the database.
- **`POST /peer` going.** A node that does not take QUIC is asked over HTTPS, and every node still serves `POST /peer` ([Peer protocol](areas/peer-protocol.md#transport)). **Done when** every node of the network takes QUIC, and a release drops `POST /peer` and the fallback.
- **The channel limit.** A node never closes a peer's accounts or settlement channel, and keeps a bounded number of channels, one per node it has ever peered with; once they are taken, peering with a new node is refused for capacity ([Settlement](areas/settlement.md), [peering-lifecycle](decisions/peering-lifecycle.md)). **Done when** a relationship whose peering has ended and whose balance is settled can be closed, freeing its channel, without a late settlement being lost or credited twice.
- **Issuer transfer fees.** An issuer that charges a fee on transfers between holders makes the RLUSD scheme `Unavailable` ([XRPL](areas/xrpl.md#rlusd)): a Payment without `SendMax` cannot pay the fee. The codec signs a Payment with `SendMax` already ([XRPL](areas/xrpl.md#tokens)); the scheme does not use it, and the fake ledger takes no transfer fee. **Done when** the scheme pays with `SendMax`, books what it sent above what the peer received as a fee, and settles while the issuer charges one.
- **Registration fees:** see [Registration fees](#registration-fees).
- **The rest of [random-seed-simulations](decisions/random-seed-simulations.md):** see [Continuous simulation](#continuous-simulation), including the release check.

## Real money

The `live` realm is refused, and the XRP Ledger mainnet has no scheme ([real-money](decisions/real-money.md), [XRP Ledger](areas/xrpl.md#real-money)). `live` is enabled only when all of these hold:

- the peer protocol's sender authentication is in place ([Peer protocol](areas/peer-protocol.md));
- the ledger is durable: journal and reconciliation ([Ledger](areas/ledger.md));
- XRP Ledger reserve and result handling is complete: the reserve is never booked, an account's creation is a deposit, whole drops with fees booked, results checked with a failure's fee booked and the sender checked ([XRP Ledger](areas/xrpl.md));
- releases are signed ([Release](areas/release.md));
- there has been an external review.

The first four are on `main`; the external review has not happened. Mainnet also needs a scheme of its own, with its own memo type, versioned with the peer protocol, and a mainnet bootstrap node list must exist before `live` makes sense. Until then, the testnet scheme stops unless its server reports the testnet's network ID, so a mainnet server never gets XRP booked or signed for with a testnet wallet. The scheme still takes its server's word on what is validated; checking the validators itself is an [open area](#checking-xrp-ledger-validators). Settlement hubs hold real money only under the same conditions. A scheme in an issued asset needs, besides, the issuer's conditions decided again: a dollar token priced at par, and what freezes and clawbacks cost ([issued-assets](decisions/issued-assets.md)).

## Smaller open points

- **A later peer-protocol version** drops the `credit` field of `peeringRequest` and of its response (sent as 0 and ignored), and the one-byte body of the reply to a Fulfill (always `accepted`, kept for older nodes) ([own-fulfills](decisions/own-fulfills.md)).
- **A post-quantum peer-protocol scheme.** The authentication choice reserves a tag for a scheme on the X-Wing hybrid KEM, and a link-state extension type for its key; neither is built ([Peer protocol](areas/peer-protocol.md#versions), [peer-protocol-v1](decisions/peer-protocol-v1.md)). Signatures and node IDs stay Ed25519.
- **Credit growth** ([earned-trust](decisions/earned-trust.md)): its share and cap are node constants; owner settings may follow. What a conversion gains or loses, the spread and the curve's move from spot, is not booked per packet and does not count toward growth yet.
- **Importing a v0 node's config and BTP tokens** is not planned; it is cheap to add if anyone asks ([database-continuity](decisions/database-continuity.md)).
- **RLUSD on the public testnet.** The end-to-end tests and the simulations settle in the fake ledger's test RLUSD, whose issuer mints, freezes and claws back on request ([XRPL](areas/xrpl.md#tokens-in-the-fake-ledger)). Two nodes funded by hand, with RLUSD from Ripple, are still to settle on the public testnet, as `xrpl_test.ts` lets XRP be checked there by hand.

## Future work, design only

Designs that stay future work. The architecture must not preclude them.

- **The token payment protocol** ([original 5 §9](original/05-accounting-and-settlement.md)).
- **A private UDP peer transport** ([original 3 §18](original/03-peer-network.md)).
- **Signed or independent frontends,** so that the wallet need not trust the node it is served by ([original 2 §8.8](original/02-node-runtime.md), [secrets-at-rest](decisions/secrets-at-rest.md)).

## Decisions

- [peer-protocol-v1](decisions/peer-protocol-v1.md): the post-quantum path: X-Wing for the peer protocol, hybrid TLS for confidentiality.
- [database-continuity](decisions/database-continuity.md): no import of v0 data for now.
- [membership-economics](decisions/membership-economics.md): registration fees and why registration is free and throttled meanwhile.
- [owner-ledger](decisions/owner-ledger.md): per-ledger credit line and largest packet.
- [real-money](decisions/real-money.md): the conditions for real money.
- [secrets-at-rest](decisions/secrets-at-rest.md): frontend signing stays future work.
- [release-integrity](decisions/release-integrity.md): a release is built once until a second build can check it.
- [peering-lifecycle](decisions/peering-lifecycle.md): the channel limit.
- [core-and-periphery](decisions/core-and-periphery.md): the periphery.
- [earned-trust](decisions/earned-trust.md), [own-fulfills](decisions/own-fulfills.md): credit growth's constants; the fields a later protocol version drops.
- [settlement-hubs](decisions/settlement-hubs.md): settlement hubs.
- [random-seed-simulations](decisions/random-seed-simulations.md): the simulation campaign.
- [ledger-resets](decisions/ledger-resets.md): a lagging server is handled; one that lies needs the validators checked.
- [default-relay](decisions/default-relay.md): the default relay, until an onboarding asks.
- [issued-assets](decisions/issued-assets.md): an issued asset on mainnet needs the issuer's conditions decided again; transfer fees.
- [passive-rebalancing](decisions/passive-rebalancing.md): the rate feed and the market makers.
