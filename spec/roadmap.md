# Roadmap

What is not built yet: the open areas of work, the gaps between the spec's contract and `main`, the conditions for real money, smaller open points, and what stays future work. Everything else in the spec describes what is on `main`; an area doc that meets one of these gaps says so in one line and links here.

**New work is an open area of work here, never a numbered milestone.** Areas proceed in parallel, and each says what done means. When an area is built, its behaviour moves into the area docs and its entry here goes.

## Open areas of work

### Settlement hubs

Another way to peer, beside bilateral peering: a ledger kept by n = 3f + 1 validators and backed by an escrow on the XRP Ledger, so that two nodes peered over a hub cannot walk away with what they owe each other. The design is [designs/settlement-hubs.md](designs/settlement-hubs.md); the reasons are [settlement-hubs](decisions/settlement-hubs.md).

- **Parts that can be built side by side:**
  - `dassie-hub`: the hub's ledger, its operations and Simplex consensus, in the simulation without an escrow;
  - the `hub-xrp-testnet` settlement scheme in the node: its registry and the driver for the validators' endpoints, joining by being paid, a floor and a ceiling the owner sets, against the hub's ledger in the simulation, and the member's account following a change of validators, whose set it now takes from the rules it was built with ([the member's side](areas/hubs.md#the-members-side) and [moving funds](areas/hubs.md#in-the-node) are built);
  - the escrow on the XRP Ledger testnet: founding, deposits, withdrawals, changing validators, shutdown and payout, test network resets, against the fake XRP Ledger;
  - the validator role in the node: gathering the old set's signatures after a change of escrow keys, for the withdrawals sequenced before it and the old set's change of the signer list ([the rest is built](areas/hubs.md#the-validator-in-the-node));
  - founding a hub through the node ([built](areas/hubs.md#founding-a-hub)): a sponsor replacing a validator that refuses or fails before genesis, and the wallet's view of a founding;
  - payout bundles ([built](areas/hubs.md#payout-bundles)): validators passing views and bundles of higher blocks around before and after they sign, a validator that comes back signing from what a member hands it, and the node submitting a quorum's set itself once its validators are gone;
  - the testnet hub: the bootstrap nodes, moved into one region and joined by a fourth ([bootstrap-network](decisions/bootstrap-network.md)), as its validators, and its genesis built into the release as the default hub.
- **Done when:** in the simulation, with up to f faulty validators, members peered over a hub pay each other across hops without trusting each other: a payer that sends no claims, and a payee that claims twice or late, cost the other no more than its hold. f + 1 colluding validators leave evidence. A hub that loses a validator shuts down, and the escrow pays out exactly the final balances, with signatures collected over time. On the testnet, nodes peered over the default hub pay each other, and it carries on while a validator restarts.
- **Later, once hubs run:**
  - fees that follow how full blocks are, so that holding back someone's claim grows dear, and an idle fee that frees the places of abandoned accounts;
  - a setup fee the sponsor pays each validator, in place of the owner's list of sponsors;
  - caps on what new rules may charge, and leaving free of the withdrawal fee during a reconfiguration's notice ([the design](designs/settlement-hubs.md#8-changing-the-validators));
  - being in several hubs per asset, with a limit per validator operator over all of them;
  - a node putting what it earned into its holds, so a peer's room grows with earnings;
  - hub balances and holds in the wallet and the desktop app.
  - stall periods above what the engine's views hold at the longest view (`MAX_STALL_MS`), by keeping long runs of nullified views as ranges ([finality-after-any-outage](decisions/finality-after-any-outage.md)).
- **On mainnet,** as part of the [real money](#real-money) launch: the `hub-xrp` scheme on XRP Ledger mainnet, booking on `hub-xrp+xrp` in the live realm; a default list of mainnet validators, kept with the mainnet bootstrap node list; the default mainnet hub's genesis built in, joined only once an owner turns hubs on; and validators that attest only what they proved through the verifier ([Checking XRP Ledger validators](#checking-xrp-ledger-validators)): deposits and outcomes by their transactions' proofs, the escrow's setup by one state proof, and a testnet reset only once the testnet's UNL validated it ([the design](designs/settlement-hubs.md#6-deposits-and-withdrawals)). **Done when,** besides the above: a validator connected to a lying server attests no deposit, withdrawal, setup or reset that no validated ledger holds, and the validators' following of the escrow (`hub/escrow.rs`) proves through the verifier, in its tests against a lying fake ledger and in the simulation that runs validators against the fake ledger once it exists; in a live-realm simulation and against the mainnet fake ledger, members peer over the default mainnet hub.
- Hubs hold real money only once the conditions of [Real money](#real-money) are met.

### Continuous simulation

The simulations are to run continuously, outside CI, from random seeds; each seed draws its own configuration, and a failure becomes an issue that names its seed ([random-seed-simulations](decisions/random-seed-simulations.md)). The runner, its campaign command, the tripwire and the campaign's supervisor exist ([Testing](areas/testing.md#the-campaign)); a spot VM runs it on `main` (`scripts/sim-campaign/provision.sh`), and releases do not wait for it yet. Not built, in parts that can be built side by side:

- **Swarm testing** in the peer, connector and STREAM simulations, as the `node` simulation has it: before a run starts, its seed draws the whole configuration (nodes, latency, loss, duplication, partitions, crashes biased toward payments in flight, clock skew, faults of the ledger link, the share of lying peers, the workload), and the run prints it first. Some seeds switch a fault off entirely, so rare combinations get their turn; a fixed realistic profile runs alongside. The `node` simulation does not yet draw lying peers, runs across currencies, or RLUSD with its issuer's acts, which only the scripted runs reach.
- **Two phases** in the peer, connector and STREAM simulations, and in the `node` simulation the checks during the chaos (it checks the books once, after the heal). A run first has a stretch of chaos, with every invariant and every [per-node check](areas/testing.md#each-node-judged-by-its-own-books) checked throughout. Then it heals, and must [settle down in production time](areas/testing.md#simulation-time-follows-production). Lying peers are judged only by what they cost the honest ones.
- **Coverage** in the peer, connector and STREAM simulations, as the `node` simulation has it: each run counts what it reached, and a test in CI asserts a minimum for each count over fixed seeds, so that a generator cannot quietly stop reaching a case.
- **The release check, once a live network runs on Dassie's releases** ([random-seed-simulations](decisions/random-seed-simulations.md)). The release workflow's first step refuses to build, sign or publish a tagged commit on which an open `sim-failure` issue was seen, or on which the campaign has not yet run long enough. A refused tag is deleted, and the version is tagged again once the campaign has cleared a commit. The campaign keeps no record of how long it ran a commit, and posts no commit status ([random-seed-simulations](decisions/random-seed-simulations.md)), so the check needs a record of its own, and the campaign a way to run the commit to be tagged.

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
  - the downlink table: downlinks on disk in a table of their own rather than in the node table, in memory only while connected, with rows closed once idle, as channels close, and the XRPL scheme's peers' addresses in a table keyed by account rather than in its state ([§4](designs/periphery.md#4-downlinks-on-a-core-node));
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

### Proof servers

The node proves what it books against rippled servers patched with three public methods: `validations` for a ledger, `tx_proof` for a transaction, and `state_proof` for ledger objects with proofs of absence. Their names and shapes are the contract the node is built against ([the design](designs/xrpl-validation.md#3-the-proof-api)). Every answer proves itself, so a proof server is relied on only to answer ([verified-ledgers](decisions/verified-ledgers.md)). A condition for [real money](#real-money).

- **What it covers:**
  - the patch to rippled: the three methods, public, bounded and charged like rippled's other public methods, serving any ledger the server has, in rippled's own byte formats, with rippled's own unit tests, small enough to carry onto each release;
  - offering it upstream, so that any rippled may serve proofs;
  - a reproducible build, and one public server for mainnet and one for the testnet, each with recent history only;
  - their hostnames in the node's defaults, per network.
- **Done when:** a public patched server for each network answers the three methods, the node's defaults name it, and the patch is offered upstream.

### Checking XRP Ledger validators

The XRPL account takes its server's word on which ledgers are validated and what they hold ([XRPL](areas/xrpl.md#following-the-account)). Acting only on ledgers past the newest seen ([ledger-resets](decisions/ledger-resets.md)) protects against a server that lags, not one that lies: a dishonest server can make a scheme book a settlement that never arrived, cancel one that applied by calling it absent, or, on the testnet, claim a reset that writes off the node's balance. The node is to check the XRP Ledger's validators itself, on both networks, against proofs from [proof servers](#proof-servers), so that nothing is booked, cancelled or written off on a server's word. The design is [designs/xrpl-validation.md](designs/xrpl-validation.md); the reasons are [verified-ledgers](decisions/verified-ledgers.md). It is a condition for [real money](#real-money).

- **Parts, each building on the one before:**
  - the codec: a generic walker of canonical serializations, binary transactions, metadata and the ledger objects the account reads, and the ledger header, pinned field by field against golden captures from mainnet and the testnet;
  - the branches: one check for both trees, proofs of absence, and the skip list;
  - the UNL: signed validator lists fetched from their publishers, built-in publishers per network and the owner's override, manifests, validations and quorum, with validation signatures behind the interface a simulation substitutes;
  - the fake ledger: real headers and a real state tree, the three methods, validators and a publisher, and a server that lies in each way [the design](designs/xrpl-validation.md#11-the-fake-ledger-the-lying-server-and-the-simulation) lists;
  - the account on proofs: anchors, the state it follows proven at each, its history proven and checked complete, a transaction's absence by its account's sequence, the history bound, resets under the UNL, the lists fetched over HTTPS through the node, a default proof server per network, and the kept state and its migration;
  - the `node` simulation's lying server with its coverage minimums, and end-to-end tests against `fake_ledger`'s validators.
- **Done when:** a server that reports a transaction, a balance, an issuer's flag, a pool or an absent account that no validated ledger holds, or a reset no quorum signed, gets nothing booked, written off or cancelled, and the scheme waits and reconnects until an honest server answers; a settlement that applied is never cancelled, whatever a server says of it; a transaction left out of the history is found by the next anchor; an expired or unreachable validator list, or a server that serves no proofs, stalls the scheme with its reason and never falls back to the server's word; with an honest proof server, the scheme settles as before, on both networks. In the simulation every XRPL node anchors, and every kind of lie is reached.
- **Later:** the negative UNL, so that a node rides out a fifth of the UNL offline as rippled does.
- Hub validators use the verifier to observe the escrow for mainnet hubs, as part of the launch: see [Settlement hubs](#settlement-hubs).

### Mainnet schemes

Settling with real money on the XRP Ledger's mainnet, in XRP and in RLUSD ([mainnet-schemes](decisions/mainnet-schemes.md)), with no DSUSD on mainnet, and DSUSD as the testnet's stand-in for RLUSD. A condition for [real money](#real-money); the `live` realm stays refused after it.

- **Parts that can be built side by side:**
  - the account by network: one account type that takes its network as a constant (network ID, asset kind, seed path, default proof server, faucet, realm), the testnet's values unchanged, and the mainnet wallet at `node/settlement-module/xrpl/seed-mainnet` derived for every golden vector;
  - `xrpl` and `xrpl-rlusd` on one mainnet account, with memo types of their own, and a test that every XRPL scheme's memo type, ledger and network are distinct;
  - test RLUSD disabled: the testnet's RLUSD scheme no longer offered or activated, the bootstrap nodes without it, and every run of it moved to mainnet RLUSD in the live realm on the fake, or to DSUSD: dev's and the end-to-end tests' scenarios, the `node` simulation's scripted runs and the in-process tests;
  - the realm gate: a ledger activates only in its node's realm, a node offers only its realm's schemes, a `live` realm stored in the database is refused at start, and the live realm runs in-process in tests, and in dev and the end-to-end tests through a switch only builds with debug assertions honour;
  - the fake ledger's mainnet mode: mainnet's network ID, no faucet, RLUSD's mainnet issuer and a pool, never a reset;
  - **the peg guard** ([issued-assets](decisions/issued-assets.md)), on one state proof of its pool ([above](#checking-xrp-ledger-validators)): mainnet RLUSD priced by mainnet's RLUSD/XRP pool and testnet DSUSD by its DSUSD/XRP pool, the scheme and the connector pausing both ways off the peg and when the reading is stale, the guard's state kept and shown to the owner;
  - the wallet, dev and the end-to-end tests: the mainnet schemes' names, Receive without a faucet and with the reserve to send, the guard's state, a mainnet scenario on the fake, and a pool moved off its peg.
- **Done when:** live-realm nodes against the mainnet fake settle in XRP and in RLUSD at addresses under `g.`; a test-realm node never activates a mainnet scheme, nor a live-realm node a testnet one; no node activates test RLUSD; a release binary refuses the dev switch; a pool moved off its peg pauses the token's scheme and its conversions once the guard's hold has passed, on mainnet RLUSD and on testnet DSUSD, and they resume after recovery, with nothing converted at par meanwhile, while what arrives is still booked; the guard pauses the scheme when its reading is stale.

## Not built yet

Gaps between what the spec's contract or decisions ask for and what `main` does. Each can be picked up on its own.

- **ILP-over-HTTP** (RFC 35, with its synchronous mode): the node serves no `/ilp` route ([Connector](areas/connector.md#ilp-over-http)). **Done when** an RFC 35 client, synchronous mode included, pays through the node over `/ilp`.
- **Hybrid post-quantum TLS.** Confidentiality against "harvest now, decrypt later" belongs to TLS with the hybrid key exchange X25519MLKEM768 ([peer-protocol-v1](decisions/peer-protocol-v1.md)). The node's TLS uses rustls with ring as its only crypto provider, which offers no ML-KEM, so its handshakes use classical key exchange only ([HTTP](areas/http.md#certificates)). **Done when** the node's listeners and its clients (peer client, ACME) offer and prefer X25519MLKEM768, with a crypto provider that still builds the static musl binaries.
- **Per-ledger credit line and largest packet** ([owner-ledger](decisions/owner-ledger.md)). Both, and with the largest packet every threshold counted in it, are one value for the whole node ([Connector](areas/connector.md#the-largest-packet), [Settlement](areas/settlement.md#credit)), so the same thresholds count for more in XRP than in dollars. **Done when** each ledger is configured with its own, and the thresholds and the junior's starting credit follow the ledger's.
- **Draining in-flight packets on shutdown.** On SIGTERM the engine stops at once; it does not wait for the packets in flight to be answered or to expire ([Node](areas/node.md#shutdown)). Their pending transfers were never on disk, so after the restart the books stand as if they had been rejected, and a Fulfill that came back is lost, as in a crash; the cap on money in flight with a peer bounds what that costs ([Connector](areas/connector.md#money-in-flight-with-a-peer)). **Done when** the node stops taking new Prepares on SIGTERM and waits, up to the latest expiry among them, for the ones in flight before it closes the database.
- **`POST /peer` going.** A node that does not take QUIC is asked over HTTPS, and every node still serves `POST /peer` ([Peer protocol](areas/peer-protocol.md#transport)). **Done when** every node of the network takes QUIC, and a release drops `POST /peer` and the fallback.
- **The accounts of closed relationships.** A settlement channel closes once idle, to disk, and frees its place ([closing-channels](decisions/closing-channels.md)), but a peer's two ledger accounts stay in memory for good, since the journal refers to them. The ledger's account bound (`max_accounts`), less the room kept for the node's own accounts, so still caps the nodes a node ever peers with, at about three and a half thousand; past it, a peering is refused for capacity ([Settlement](areas/settlement.md#settlement-channels)). **Done when** the accounts of a node whose channel closed leave memory and its bound, and come back with what they showed when it peers again, or when a late settlement comes over its channel.
- **Issuer transfer fees.** An issuer that charges a fee on transfers between holders makes the RLUSD scheme `Unavailable` ([XRPL](areas/xrpl.md#rlusd)): a Payment without `SendMax` cannot pay the fee. The codec signs a Payment with `SendMax` already ([XRPL](areas/xrpl.md#tokens)); the scheme does not use it, and the fake ledger takes no transfer fee. **Done when** the scheme pays with `SendMax`, books what it sent above what the peer received as a fee, and settles while the issuer charges one.
- **Registration fees:** see [Registration fees](#registration-fees).
- **The rest of [random-seed-simulations](decisions/random-seed-simulations.md):** see [Continuous simulation](#continuous-simulation), including the release check.

## Real money

The `live` realm is refused, and the XRP Ledger mainnet has no scheme ([real-money](decisions/real-money.md), [XRP Ledger](areas/xrpl.md#real-money)). `live` is enabled only when all of these hold:

- the peer protocol's sender authentication is in place ([Peer protocol](areas/peer-protocol.md));
- the ledger is durable: journal and reconciliation ([Ledger](areas/ledger.md));
- XRP Ledger reserve and result handling is complete: the reserve is never booked, an account's creation is a deposit, whole drops with fees booked, results checked with a failure's fee booked and the sender checked ([XRP Ledger](areas/xrpl.md));
- releases are signed ([Release](areas/release.md));
- the node checks the XRP Ledger's validators itself, on both networks ([Checking XRP Ledger validators](#checking-xrp-ledger-validators));
- a public proof server runs for each network, named in the node's defaults ([Proof servers](#proof-servers));
- mainnet schemes in XRP and RLUSD, with memo types of their own and the realm gate, and the dollar token's peg guard ([Mainnet schemes](#mainnet-schemes));
- settlement hubs on mainnet: `hub-xrp`, a default mainnet hub, and validators that observe the escrow through proofs ([Settlement hubs](#settlement-hubs));
- there has been an external review;
- a mainnet bootstrap node list exists.

The first four are on `main`. Until the rest are, the testnet account stops unless its server reports the testnet's network ID, so a mainnet server never gets XRP booked or signed for with a testnet wallet; and the account still takes its server's word on what is validated. The issuer's conditions for a dollar token are decided ([issued-assets](decisions/issued-assets.md)): the issuer is the owner's counterparty, its freezes and clawbacks are booked as built, and the scheme pauses while its pool prices it off its peg, once the guard is built.

## Smaller open points

- **A later peer-protocol version** drops the `credit` field of `peeringRequest` and of its response (sent as 0 and ignored), and the one-byte body of the reply to a Fulfill (always `accepted`, kept for older nodes) ([own-fulfills](decisions/own-fulfills.md)).
- **A post-quantum peer-protocol scheme.** The authentication choice reserves a tag for a scheme on the X-Wing hybrid KEM, and a link-state extension type for its key; neither is built ([Peer protocol](areas/peer-protocol.md#versions), [peer-protocol-v1](decisions/peer-protocol-v1.md)). Signatures and node IDs stay Ed25519.
- **Credit growth** ([earned-trust](decisions/earned-trust.md)): its share and cap are node constants; owner settings may follow. What a conversion gains or loses, the spread and the curve's move from spot, is not booked per packet and does not count toward growth yet.
- **Importing a v0 node's config and BTP tokens** is not planned; it is cheap to add if anyone asks ([database-continuity](decisions/database-continuity.md)).
- **The rate feed's ledgers.** Every ledger the connector prices must fit a feed: the price table's bound is asserted against the bound on the ledgers a feed names (`MAX_PRICED_LEDGERS <= MAX_LEDGERS`, in `crates/dassie-node/src/feed.rs`). The mainnet ledgers press against that bound, so the table is to be bounded by the ledgers of one realm, since a node runs only its realm's ([mainnet-schemes](decisions/mainnet-schemes.md)).

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
- [verified-ledgers](decisions/verified-ledgers.md): checking the XRP Ledger's validators against proofs from patched rippled servers.
- [mainnet-schemes](decisions/mainnet-schemes.md): mainnet's schemes, and a ledger only in its node's realm.
- [secrets-at-rest](decisions/secrets-at-rest.md): frontend signing stays future work.
- [release-integrity](decisions/release-integrity.md): a release is built once until a second build can check it.
- [peering-lifecycle](decisions/peering-lifecycle.md): the channel limit.
- [core-and-periphery](decisions/core-and-periphery.md): the periphery.
- [earned-trust](decisions/earned-trust.md), [own-fulfills](decisions/own-fulfills.md): credit growth's constants; the fields a later protocol version drops.
- [settlement-hubs](decisions/settlement-hubs.md): settlement hubs, on mainnet too.
- [random-seed-simulations](decisions/random-seed-simulations.md): the simulation campaign.
- [ledger-resets](decisions/ledger-resets.md): a lagging server is handled; a reset counts once the testnet's UNL validates it.
- [default-relay](decisions/default-relay.md): the default relay, until an onboarding asks.
- [issued-assets](decisions/issued-assets.md): the peg guard, test RLUSD disabled; transfer fees.