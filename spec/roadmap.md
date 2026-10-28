# Roadmap

What is not built yet: the open areas of work, the gaps between the spec's contract and `main`, the conditions for real money, smaller open points, and what stays future work. Everything else in the spec describes what is on `main`; an area doc that meets one of these gaps says so in one line and links here.

**New work is an open area of work here, never a numbered milestone.** Areas proceed in parallel, and each says what done means. When an area is built, its behaviour moves into the area docs and its entry here goes.

## Open areas of work

### Settlement hubs

A ledger kept by n = 3f + 1 validators and backed by an escrow on the XRP Ledger, so that two nodes peered over a hub cannot walk away with what they owe each other; peers settle only over hubs, over every hub both are in ([hubs-only](decisions/hubs-only.md), [Network](areas/network.md#settlement-methods)). The design is [designs/settlement-hubs.md](designs/settlement-hubs.md); the reasons are [settlement-hubs](decisions/settlement-hubs.md). What is built is in [Settlement hubs](areas/hubs.md).

- **Parts that can be built side by side:**
  - the member's side in the node: closing by the owner, a floor and a ceiling the owner sets, the member's account following a change of validators, whose set it now takes from the rules it was built with, and moving a scheme to another hub, as to a default hub founded again, which needs the old account closed and its state let go ([In the node](areas/hubs.md#in-the-node), [hub-named-when-added](decisions/hub-named-when-added.md));
  - the validator role in the node: gathering the old set's signatures after a change of escrow keys, for the withdrawals sequenced before it and the old set's change of the signer list ([The validator in the node](areas/hubs.md#the-validator-in-the-node));
  - founding a hub: a sponsor replacing a validator that refuses or fails before genesis, and the wallet's view of a founding ([Founding a hub](areas/hubs.md#founding-a-hub));
  - payout bundles: validators passing views and bundles of higher blocks around before and after they sign, a validator that comes back signing from what a member hands it, and the node submitting a quorum's set itself once its validators are gone. Before it submits a token hub's set, a member checks its own proof of the escrow's trust line, and the first to submit a dissolving token hub's set tops the escrow's XRP up to 2 XRP: the reserve of its signer list and trust line, which the set's change of signer list needs, and a margin for the set's fees ([Payout bundles](areas/hubs.md#payout-bundles), [token-hubs](decisions/token-hubs.md));
  - validators keeping their history across restarts, so that a restarted honest validator never says `Trimmed`: a member then reads its state anew only on f + 1 of them, and no longer on one and minutes without its cursor moving ([When the history is gone](areas/hubs.md#when-the-history-is-gone)), which costs it the fulfillments of claims made in the blocks it skips;
- **Done when:** in the simulation, a hub that loses a validator for as long as the design allows shuts down, and its escrow pays out exactly the final balances, with signatures collected over time. On the testnet, nodes peered over the default hub pay each other, and it carries on while a validator restarts.
- **Later, once hubs run:**
  - fees that follow how full blocks are, so that holding back someone's claim grows dear, and an idle fee that frees the places of abandoned accounts;
  - a setup fee the sponsor pays each validator, in place of the owner's list of sponsors;
  - caps on what new rules may charge, and leaving free of the withdrawal fee during a reconfiguration's notice ([the design](designs/settlement-hubs.md#8-changing-the-validators));
  - a node putting what it earned into its holds, so a peer's room grows with earnings;
  - several hubs of one asset per node, with a limit per validator operator over all of them, once running hubs show it is worth it;
  - hub balances and holds in the desktop app;
  - receipts as proofs a member fetches from any one validator: the certificate of a final block and Merkle paths from its roots, to an operation's result in the results root and to the account in the state root; a count per account of the operations that touched it, kept in the state tree, so a member knows it holds every one since a height; and a state tree kept between blocks, so a validator builds a path without hashing the whole state again. One validator then answers a read, its answer proving itself by the certificate, with no signature per request, where today f + 1 validators each sign every statement ([the validator's service](areas/hubs.md#the-validators-service)). The block format keeps the results root for them;
  - stall periods above what the engine's views hold at the longest view (`MAX_STALL_MS`), by keeping long runs of nullified views as ranges ([finality-after-any-outage](decisions/finality-after-any-outage.md)).
- Hubs hold real money only once the conditions of [Real money](#real-money) are met.

### Token hubs

Hubs that hold an issued token: DSUSD on the XRP Ledger testnet, RLUSD on mainnet ([token-hubs](decisions/token-hubs.md)). The issuer's freeze is a state of the hub's ledger, a clawback ends the hub with a payout pro rata, and the escrow's fees are paid from XRP anyone may send it; what is built is in [Settlement hubs](areas/hubs.md), [A frozen hub](areas/hubs.md#a-frozen-hub) among it, and in the `node-hub-dsusd` simulation ([Testing](areas/testing.md#over-a-settlement-hub)).

- **What is left:**
  - a payer's hold reduction pending at a freeze, in `node-hub-dsusd`: the `hub` simulation shows that such a reduction falls due only after the thaw, but no member's node runs one there yet. It waits for a decision on how a payer's node books a reduction of its hold that it did not make itself;
  - the operator's step on the testnet: the market maker's VM starts again, on a release that builds in the testnet's default hubs, from a fresh node database ([Market maker VM](areas/market-maker-vm.md)), as below.
- RLUSD's hub comes with [Mainnet hubs](#mainnet-hubs).
- **Done when:** in a `node-hub-dsusd` simulation, members lose nothing they had not redeemed when a reduction was pending at a freeze; and on the testnet, nodes peered over the DSUSD hub pay each other, and a payment crosses from the XRP hub to the DSUSD hub.

### Mainnet hubs

Hubs on the XRP Ledger's mainnet, in XRP and in RLUSD, as part of the [real money](#real-money) launch ([settlement-hubs](decisions/settlement-hubs.md), [real-money](decisions/real-money.md)). Their schemes, `hub-xrpl-xrp` and `hub-xrpl-rlusd`, the places of their default hubs and the default list of mainnet validators are built ([In the node](areas/hubs.md#in-the-node)), and so are validators that follow the escrow only through a verifier of the XRP Ledger, whose lists and anchor they keep across a restart ([What validators attest](areas/hubs.md#what-validators-attest), [The validator in the node](areas/hubs.md#the-validator-in-the-node)); the live-realm simulations run members over hubs each run founds ([Testing](areas/testing.md#over-a-settlement-hub)). No hub runs on mainnet yet.

- **The operator's steps:**
  - the project's mainnet validators run, and are entered in the default list of mainnet validators (`member::DEFAULT_MAINNET_VALIDATORS`);
  - they found the default mainnet hubs, whose rules go in `default-mainnet-hub.hex` and `default-mainnet-rlusd-hub.hex`, with `member::DEFAULT_MAINNET_HUB` and `member::DEFAULT_MAINNET_RLUSD_HUB` set to them.
- **Done when:** members peer over the default mainnet hubs, in the live-realm simulations or an end-to-end test, once their genesis is built in.

### Continuous simulation

The simulations are to run continuously, outside CI, from random seeds; each seed draws its own configuration, and a failure becomes an issue that names its seed ([random-seed-simulations](decisions/random-seed-simulations.md)). The runner, its campaign command, the tripwire and the campaign's supervisor exist ([Testing](areas/testing.md#the-campaign)); a spot VM runs it on `main` (`scripts/sim-campaign/provision.sh`), and releases do not wait for it yet. Not built, in parts that can be built side by side:

- **Swarm testing** in the peer, connector and STREAM simulations, as the `node` simulation has it: before a run starts, its seed draws the whole configuration (nodes, latency, loss, duplication, partitions, crashes biased toward payments in flight, clock skew, faults of the ledger link, the share of lying peers, the workload), and the run prints it first. Some seeds switch a fault off entirely, so rare combinations get their turn; a fixed realistic profile runs alongside. The `node` simulation draws the stub with unlimited trust or with commitments, and in a share of its seeds hubs run in the simulation, a hub of XRP or, in a share of those, a hub of XRP and a hub of DSUSD with every node in both, with the faults of its members' ledger links and lying servers, its partitions bounded so that a run stays within what a campaign gives it, and its books judged hub by hub by the checks the scripted runs over hubs share ([Testing](areas/testing.md#the-simulations)). It does not yet draw lying peers, runs across currencies, a token hub's issuer freezing or clawing back, faulty validators or lying servers under them, which only the scripted runs reach.
- **Two phases** in the peer, connector and STREAM simulations, and in the `node` simulation the checks during the chaos (it checks the books once, after the heal). A run first has a stretch of chaos, with every invariant and every [per-node check](areas/testing.md#each-node-judged-by-its-own-books) checked throughout. Then it heals, and must [settle down in production time](areas/testing.md#simulation-time-follows-production). Lying peers are judged only by what they cost the honest ones.
- **Coverage** in the peer, connector and STREAM simulations, as the `node` simulation has it: each run counts what it reached, so that the runner's counts over a range of seeds show a generator that stops reaching a case.
- **The release check, once a live network runs on Dassie's releases** ([random-seed-simulations](decisions/random-seed-simulations.md)). The release workflow's first step refuses to build, sign or publish a tagged commit on which an open `sim-failure` issue was seen, or on which the campaign has not yet run long enough. A refused tag is deleted, and the version is tagged again once the campaign has cleared a commit. The campaign keeps no record of how long it ran a commit, and posts no commit status ([random-seed-simulations](decisions/random-seed-simulations.md)), so the check needs a record of its own, and the campaign a way to run the commit to be tagged.

**Done when:** the campaign runs unattended on `main`, files one issue per failure signature with a command that reproduces it, and its tripwire keeps showing up.

**Not yet decided:** coverage-guided fuzzing of the decoders that face untrusted input (outside CI, [Engineering](engineering.md#no-fuzzing-or-random-seeds-in-ci)), real binaries behind a proxy that injects faults, and shrinking failing runs.

### Rebuilding releases

A release is built once, on the Vorstand Nix cloud, and the release workflow signs what that build made ([Release](areas/release.md#the-build)). Its archives are deterministic, but nothing builds the release a second time to compare: whether the binaries reproduce on another builder is untested, and the release key vouches for whatever the one builder and its cache produced ([release-integrity](decisions/release-integrity.md)). Building again to verify would be welcome, but GitHub's runners are very slow, and the project is early and wants to move fast.

- **The likely shape:** the project may move its builds off GitHub, onto a build system that is Nix throughout. A dedicated builder that builds nothing but releases then runs `nix build --rebuild` on the release before it is signed: it builds the store path again, from the source and the pinned inputs, and fails if the bytes differ from what the Nix cloud made. Signing waits for it.
- **A bypass for urgent hotfixes:** a hotfix that cannot wait for the second build may be signed without it, by an explicit choice that the release run records.
- **Done when:** every release, but a hotfix that took the bypass, is signed only once a second build on the release builder has produced the same bytes.

### Registration fees

Registration is free, and throttled instead ([Network](areas/network.md#registration)). The design calls for a paid registration whose fee grows with the size of the list ([membership-economics](decisions/membership-economics.md)).

- **What it covers:**
  - **The fee quote and payment,** as a v1.x peer-protocol addition: a quote message, and payment by ILP with a signed quote condition (the "naive" variant of the token payment protocol, below). A registration pays the cheapest 75% of bootstrap nodes.
  - **The fee schedule,** set when the quote and payment are built.
- **Done when:** a node registers by paying the quoted fee, and is refused without it. [Names](#names) use the same quote message and way of paying, but pay every registrar, per commitment and per renewal, since a registrant needs every registrar's receipt.

### Names

Names that people can say, held for a node ID by a quorum of the bootstrap nodes' lists: a registrant commits to a name sealed, and paid, at every registrar, and unseals it once it holds every receipt; the earliest commitment wins. The design is [designs/names.md](designs/names.md); the reasons are [names-by-earliest-commitment](decisions/names-by-earliest-commitment.md). Names, commitments and their messages are built ([Peer protocol](areas/peer-protocol.md#names)), and so are the registrar's side, its log, the rule, granting, renewal and grace, its list and releasing a split ([Network](areas/network.md#names)), every node's view of who holds a name, by a quorum of the lists ([Network](areas/network.md#holders)), and the registrant, which registers the node's names and renews them, as the owner asks in the wallet ([Network](areas/network.md#the-registrant)). The `names` simulation holds them to the design's guarantees with one faulty registrar of four, equivocating, withholding receipts or tipping off a front-runner, on a rough network with crashes while names are registered and again while they are granted and read, with an honest registrar cut off from the others; splits are released once it is calm ([Testing](areas/testing.md#names)). A node answers Open Payments under the names it holds, and its client takes an answer under a name only from the name's holder in its view ([Payments](areas/payments.md#the-nodes-names)); a front door reads the registrars' lists itself and passes a name's requests to its holder ([HTTP](areas/http.md#front-doors)). Names run on the testnet only, where they may be reset at any time: a node of the live realm has them off ([Network](areas/network.md#names)).

- **Parts that can be built side by side:**
  - the fees: each registrar quoted and paid per commitment and per renewal, by the quote message and way of paying of [registration fees](#registration-fees); with them, names on the live realm, which has none while names are free;
  - the holders in the owner's view: the names the node takes holders for, over RPC, in the wallet and `dassie status`;
  - the address a name gives, `$<name>.<domain>` at each front door the node is named at, shown to its owner in the wallet beside the name;
  - moving a name to another node ID, signed by its holder, for an owner who moves to a new node ([designs/names.md §8](designs/names.md#8-open-questions));
  - commitments that do not show who makes them, sent and paid through another node, so that a faulty registrar can neither refuse one node's commitments nor tip off a guesser by who commits ([designs/names.md §8](designs/names.md#8-open-questions)).
- **Open points:**
  - **The name space is bounded.** A registrar lists at most `MAX_NAME_LIST` names, since every node downloads each registrar's list whole, at every change, in one response. While names are free, one actor within the throttles, with enough node IDs and networks, can take a large share of them over time. Fees make each name cost something; paged lists, read in parts, would lift the ceiling itself.
  - **A node with a hostname** may register a name, and its registrars grant it, but it never answers under it, since names are for nodes reached through a front door ([Payments](areas/payments.md#the-nodes-names)). Whether such a registration is refused, or the node answers under its names too, is open; for now it registers and does not answer.
  - **A name that passes to another holder,** once it expires unrenewed or is released. A front door then passes its requests to the new holder, so wallet addresses and payment requests handed out under it reach a node that did not make them, and the old holder's grants and resources keep URLs at a name it no longer answers to ([Payments](areas/payments.md#the-nodes-names)). What such a URL means then is open: whether the old holder's grants and resources stay reachable under another of its names, and how a client that kept a URL learns that the holder changed, as a Dassie-aware one can by the node ID a wallet address document carries.
- **Done when:** a name is paid for at every registrar, the owner sees the holders its node takes and the address each name gives, a name moves to another node ID, and the open points are decided, with the `names` simulation's checks still holding once commitments are paid. Commitments that do not show who makes them are not needed for it: if they are not built by then, they move to [future work](#future-work-design-only).

### Periphery

Nodes without a public IP join as periphery nodes: they connect only to core nodes, get child addresses beneath them, and never forward. The design is [designs/periphery.md](designs/periphery.md); the reasons are [core-and-periphery](decisions/core-and-periphery.md). Periphery nodes with their uplinks, over the QUIC connections they open, over UDP or, where UDP is blocked, a WebSocket ([quic-over-websocket](decisions/quic-over-websocket.md)), with child addresses and downlinks under a bound of their own, of which free receivers hold at most half, are built ([Network](areas/network.md#periphery-nodes), [Connector](areas/connector.md#a-periphery-node), [free-receivers-half](decisions/free-receivers-half.md)).

- **Parts that can be built side by side:**
  - QUIC for periphery links: dialling an uplink again with a backoff when its connection is lost;
  - the downlink table: downlinks on disk in a table of their own rather than in the node table, in memory, with their channels and accounts, only while connected or while something still needs them, their timers run from the disk, with rows closed once idle, as channels close ([§4](designs/periphery.md#4-downlinks-on-a-core-node), [resident-peers](decisions/resident-peers.md));
  - BTP clients of a periphery node, whose ILDCP address is still beneath the node's own address rather than an uplink's;
  - periphery nodes in the `peer` and `node` simulations' chaos runs, losing and regaining uplinks;
  - records and home nodes, so that clients find the relays that reach a node. Relays are built: every QUIC endpoint relays streams to the nodes connected to it, so every uplink is already a relay for its downlinks, and a node keeps a connection to a relay it chooses ([Peer protocol](areas/peer-protocol.md#relayed-streams)). Relayed streams carry Open Payments and paired owner apps' RPC;
  - owner apps. The node pairs them and serves their RPC on relayed streams ([RPC](areas/rpc.md#paired-devices)), and the wallet on `localhost` is to show the pairing code; left are the apps themselves (a remote mode of the desktop app, a mobile app), consent to grants in the app as it connects (listing and following pending grants), the front door's consent page that sends the owner there, finding the node on the local network, and, with the first app, freezing what it relies on: `buf breaking` against the last release in CI, and the pairing code and the owner's contract among the [frozen surfaces](overview.md#frozen-surfaces);
  - the [desktop app](areas/desktop.md) beyond its Nix package: packages for other Linux distributions (AppImage, .deb, Flatpak), the app in the mirror's flake from a signed build, its updates, and macOS and Windows;
  - front doors. Front doors are built, for the nodes that keep one as their relay, over UDP or, where UDP is blocked, a WebSocket, which a node without a hostname does with `dassiepay.net`'s by default ([HTTP](areas/http.md#front-doors), [Node](areas/node.md#a-chosen-relay)), and so are a node's names and its signed responses ([Payments](areas/payments.md#the-nodes-names)); left are their consent page (the front door answers the consent route with a fixed error until then), an owner's own domain as a CNAME to a front door with a certificate for that name, the DNS TXT record that names the node and its check in Dassie's own client, the owner's names as a wallet setting rather than an environment variable, a node's rate limits per client behind its chosen relay (today a node counts its front door as one client, so a few clients use up its budget for every client through it: [HTTP](areas/http.md#front-doors)), and the proposal to Open Payments;
  - free receivers (their half of an uplink's downlinks is built), paid into their uplink's hub without a deposit ([Settlement hubs](areas/hubs.md#the-members-side));
  - browser nodes;
  - **becoming core** ([promotion-to-core](decisions/promotion-to-core.md)). On `main` a node with a hostname is core and one without is periphery; under [promotion-to-core](decisions/promotion-to-core.md) every node starts in the periphery and is core only when the signed data says so: registered, in the 2-core of the graph of edges both ends advertise, in its component with the most bootstrap nodes. The parts:
    - routing to a node's own address only once it is core. Every node computes the core already ([Network](areas/network.md#the-core));
    - promotion: an uplink advertises its downlink as a neighbor once the downlink is registered, answers at its URL with a publicly issued certificate, has an hour of good standing and its heartbeats, twenty of the largest packets of room in the uplink's books and a completed settlement, a clock within bounds, and no ban; a link-state entry for a core node's free core relationships, so a node chooses uplinks that can promote it;
    - a peering's kind following the peer's status in place, both ways, with its accounts and channel, and the uplink routing the child address for as long as they are peered;
    - a node using its own address after a set wait, and its child addresses again at once when it stops being core;
    - demotion: bootstrap nodes dropping a registered node whose URL or certificate fails for longer than becoming core takes, and uplinks no longer advertising a peer that fails the promotion criteria;
    - the first core: bootstrap nodes peering as core peers and advertising each other without the checks, and fewer edges asked of networks with fewer than three bootstrap nodes;
    - a peering's kind decided by the peer's status rather than its link state, with the periphery extension meaning only that a node has no URL.

    **Done when** in the simulation a node with a URL stays periphery until registered and promoted by two uplinks, then is routed at its own address by every node after the wait, and drops back when it goes away; and two networks that share no bootstrap node majority never both count as the core.
- **It needs** [the channel limit](#not-built-yet) lifted, so that a core node keeps few relationships with core nodes.
- **Done when:** a periphery node behind a NAT peers with two core nodes, gets an address beneath each, pays and is paid through them, and keeps working when it loses one. Its owner's paired app reaches it through a relay, and a third-party Open Payments client pays it through a front door, whose response a Dassie client checks against the node ID. A free receiver is paid, spends through its uplink, and has its hub account opened once its uplink owes it the hub's minimum for a new account. In the simulation, home nodes that withhold, and ground core keys, hide no node from a relay it chose.

### Onboarding

A new owner opens a setup URL, enters a passphrase and chooses a first ledger ([Overview](overview.md#life-of-a-node)); every other choice is a default in the node's environment. An onboarding in the wallet is to walk the owner through the choices that matter, explaining each. It is not designed yet; what it must hold so far:

- **Choosing a relay** is one of its steps. A node without a hostname keeps a connection to the `dassiepay.net` front door by default, so that it is reached at its default name ([default-relay](decisions/default-relay.md)). The step explains what that costs: a machine the project runs learns when the node is online, and reads the Open Payments requests it passes on, as any front door does. The owner keeps it, names another relay, or keeps none and is not reached at that name.
- **Done when:** an owner sets a node up in the wallet without the command line, and makes each choice the node otherwise defaults, the relay among them, knowing what it costs.

### Proof servers

The node proves what it books against rippled servers patched with three public methods: `validations` for a ledger, `tx_proof` for a transaction, and `state_proof` for ledger objects with proofs of absence. Their names and shapes are the contract the node is built against ([the design](designs/xrpl-validation.md#3-the-proof-api)). Every answer proves itself, so a proof server is relied on only to answer ([verified-ledgers](decisions/verified-ledgers.md)). A condition for [real money](#real-money).

The patch, its build and a public server per network run ([Proof servers](areas/proof-servers.md)), and each network's server is its account's default ([XRPL](areas/xrpl.md)).

- **What is left:** offering the patch upstream, so that any rippled may serve proofs.
- **Done when:** the node's defaults name each network's proof server, and the patch is offered upstream.

### Checking XRP Ledger validators

The XRPL account anchors on ledgers its own UNL validated, and proves at each anchor what it reads ([XRPL](areas/xrpl.md#following-the-account)): a server that took its word could have a scheme book a settlement that never arrived, or cancel one that applied by calling it absent. The node checks the XRP Ledger's validators itself, on both networks, against proofs from [proof servers](#proof-servers), so that nothing is booked, cancelled or written off on a server's word. The design is [designs/xrpl-validation.md](designs/xrpl-validation.md); the reasons are [verified-ledgers](decisions/verified-ledgers.md). It is a condition for [real money](#real-money).

- **Left to build:** signing only from a fresh anchor; anchoring on fewer hints than each closed ledger's, as the design's triggers have it, once the schemes no longer need to hear of each ledger; and in the `node` simulation, testnet resets under the UNL, one of them while a node is down until the new chain has passed its anchor. The rest is built: each network's proof server as its default ([Proof servers](#proof-servers)), the verifier ([XRPL](areas/xrpl.md#the-validators-records)), the account on proofs ([XRPL](areas/xrpl.md#anchors), [XRPL](areas/xrpl.md#following-the-account)), the fake ledger's proofs and lying server, and the simulation and daemon tests against them ([XRPL](areas/xrpl.md#proofs-in-the-fake-ledger), [Testing](areas/testing.md#the-simulations)).
- **Done when:** a server that reports a transaction, a balance, an issuer's flag, a pool or an absent account that no validated ledger holds, or a reset no quorum signed, gets nothing booked, written off or cancelled, and the scheme waits and reconnects until an honest server answers; a settlement that applied is never cancelled, whatever a server says of it; a transaction left out of the history is found by the next anchor; an expired or unreachable validator list, or a server that serves no proofs, stalls the scheme with its reason and never falls back to the server's word; with an honest proof server, the scheme settles as before, on both networks. In the simulation every XRPL node anchors, and every kind of lie is reached.
- **Later:** the negative UNL, so that a node rides out a fifth of the UNL offline as rippled does.
- Hub validators prove what they attest under the UNL of the hub's network, through the same verifier as the account ([XRPL](areas/xrpl.md#the-verifier)); what is left for mainnet hubs is in [Mainnet hubs](#mainnet-hubs).

### Mainnet schemes

Settling with real money on the XRP Ledger's mainnet, in XRP and in RLUSD ([mainnet-schemes](decisions/mainnet-schemes.md)), with no DSUSD on mainnet, and DSUSD as the testnet's stand-in for RLUSD. A condition for [real money](#real-money); the `live` realm stays refused after it.

- **Left to build:** a live node that takes none of the test network's defaults, its bootstrap nodes and its relay.
- **Done when:** live-realm nodes against the mainnet fake settle in XRP and in RLUSD at addresses under `g.`, over mainnet hubs ([Mainnet hubs](#mainnet-hubs)); a test-realm node never activates a mainnet scheme, nor a live-realm node a testnet one; no node activates test RLUSD; a release binary refuses the dev switch; a pool moved off its peg pauses the token's scheme and its conversions, those of the token's hub ledger included, once the guard's hold has passed, on mainnet RLUSD and on testnet DSUSD, and they resume after recovery, with nothing converted at par meanwhile, while what arrives is still booked; the guard pauses the scheme when its reading is stale, until [Open rate sources](#open-rate-sources) turns the guard off.

### Open rate sources

A node's default rate sources are exchanges whose terms license their market data for personal or internal use, and a node converts for its owner's fees. [open-rate-sources](decisions/open-rate-sources.md) replaces them with sources whose terms allow that use: XRP's rate from mainnet's XRP/RLUSD pool, proven at validated ledgers, and the fiat currencies' from four central banks. With XRP's rate from that pool, the peg guard could only read RLUSD as pegged, so it is turned off. DSUSD's keeper takes its target from the same pool.

- **Left to build:** following mainnet's validated ledgers for the pool's price on a node of either realm, with no key; the pool as XRP's only source, with its floor and window; Norges Bank's, the Bank of Canada's and the Czech National Bank's sources beside the ECB's; the new defaults, for new nodes and for nodes upgraded with the sources they started with; the peg guard turned off on RLUSD and DSUSD, with their hubs' ledgers converting as the XRP hub's does; the sources named in the wallet; the keeper's target.
- **Done when:** a new node's sources are only these; XRP's rate follows mainnet's XRP/RLUSD pool, and a pool pushed for less than half the window leaves it unmoved; a test node signs nothing on mainnet; no dollar token's scheme or hub pauses for its price; the fiat currencies' rates come from the four banks, each dropped once older than its calendar allows; the wallet names the sources; and the keeper holds DSUSD's pool at mainnet's XRP/RLUSD price.
- **Later:** a source of XRP's price in dollars outside every dollar token's pool, close to real time and with terms that allow a node's use, and the peg guard on it again.

### Kopje across Dassie

The brand page holds Dassie's visual identity, Kopje ([visual-identity](decisions/visual-identity.md)), and dassie.land draws in it; the wallet and the desktop app do not yet. Its rocks are still Terracotta mounds, which read as droppings; [granite-rocks](decisions/granite-rocks.md) makes them granite, drawn by hand to rules the brand page states.

- **Parts that can be built side by side:** the wallet, in its light theme and dusk; the desktop app's icons and tray; the rules for rocks on the brand page, and every drawing with rocks drawn again to them, with a composition of its own for phones where it is shown there.
- **Done when:** every screen of the wallet and the desktop app draws in Kopje's palette, type and mark, light and dark; and every rock on dassie.land and the brand page is gray granite, drawn to the brand page's rules, with a composition of its own wherever a scene is shown on phones.

## Not built yet

Gaps between what the spec's contract or decisions ask for and what `main` does. Each can be picked up on its own.

- **ILP-over-HTTP** (RFC 35, with its synchronous mode): the node serves no `/ilp` route ([Connector](areas/connector.md#ilp-over-http)). **Done when** an RFC 35 client, synchronous mode included, pays through the node over `/ilp`.
- **Hybrid post-quantum TLS.** Confidentiality against "harvest now, decrypt later" belongs to TLS with the hybrid key exchange X25519MLKEM768 ([peer-protocol-v1](decisions/peer-protocol-v1.md)). The node's TLS uses rustls with ring as its only crypto provider, which offers no ML-KEM, so its handshakes use classical key exchange only ([HTTP](areas/http.md#certificates)). **Done when** the node's listeners and its clients (QUIC, the outbound client, ACME) offer and prefer X25519MLKEM768, with a crypto provider that still builds the static musl binaries.
- **Per-ledger credit line and largest packet** ([owner-ledger](decisions/owner-ledger.md)). Both, and with the largest packet every threshold counted in it, are one value for the whole node ([Connector](areas/connector.md#the-largest-packet), [Settlement](areas/settlement.md#credit)), so the same thresholds count for more in XRP than in dollars. The credit line left the code with direct settlement ([hubs-only](decisions/hubs-only.md)); what remains of it is the size one request to settle funds a hold by, over a hub ([Settlement hubs](areas/hubs.md#in-the-node)). **Done when** each ledger is configured with its own largest packet and size of a hold's funding, and the thresholds follow the ledger's.
- **A stream of the node's status and its hubs.** The owner's contract streams live data ([rpc-contract](decisions/rpc-contract.md)), but has no `Subscribe` method for the node's status or its hubs (`proto/dassie/owner/v1/hub_member.proto`), so the wallet asks for them again every few seconds while it shows them ([Wallet](areas/wallet.md#live-data)). **Done when** the owner's contract streams each hub's account, holds, moves and payout, and the node's status, and the wallet's Hubs page follows them by those streams rather than asking again.
- **Draining in-flight packets on shutdown.** On SIGTERM the engine stops at once; it does not wait for the packets in flight to be answered or to expire ([Node](areas/node.md#shutdown)). Their pending transfers were never on disk, so after the restart the books stand as if they had been rejected, and a Fulfill that came back is lost, as in a crash; the cap on money in flight with a peer bounds what that costs ([Connector](areas/connector.md#money-in-flight-with-a-peer)). **Done when** the node stops taking new Prepares on SIGTERM and waits, up to the latest expiry among them, for the ones in flight before it closes the database.
- **The accounts of closed relationships.** A settlement channel closes once idle, to disk, and frees its place ([closing-channels](decisions/closing-channels.md)), but a peer's two ledger accounts stay in memory for good, since the journal refers to them. The ledger's account bound (`max_accounts`), less the room kept for the node's own accounts, so still caps the nodes a node ever peers with, at about three and a half thousand, half of them core nodes and half downlinks ([account-budgets](decisions/account-budgets.md)); past its kind's half, a peering is refused for capacity ([Settlement](areas/settlement.md#settlement-channels)). The design is [resident-peers](decisions/resident-peers.md): account numbers from a counter, never given again; only the accounts in memory, with each ledger's totals of those on disk; one way to a peer's accounts, from memory, disk or new; a core peer's accounts closing with its channel; and a migration that marks the accounts of closed channels. A downlink's accounts leaving memory when it disconnects come with [the downlink table](#periphery). **Done when** the accounts of a node whose channel closed leave memory and its bound, and come back with what they showed when it peers again, or when a late settlement comes over its channel; and in the simulation, nodes that peer and leave, more than the account bound holds, are settled exactly once, with every ledger balanced.
- **Issuer transfer fees.** While an issuer charges a fee on transfers between holders, an owner book of its token pays none of the owner's payments, a deposit into its hub included ([XRPL](areas/xrpl.md#the-token-scheme)): a Payment without `SendMax` cannot pay the fee. Its hub's validators hold the hub as at a freeze meanwhile ([token-hubs](decisions/token-hubs.md)). The codec signs a Payment with `SendMax` already ([XRPL](areas/xrpl.md#tokens)); the scheme does not use it, and the fake ledger takes no transfer fee. **Done when** an owner book pays the owner's payments with `SendMax` while the issuer charges a fee, and books the fee.
- **Registration fees:** see [Registration fees](#registration-fees).
- **The rest of [random-seed-simulations](decisions/random-seed-simulations.md):** see [Continuous simulation](#continuous-simulation), including the release check.

## Real money

The `live` realm is refused, so the XRP Ledger mainnet's scheme, `xrpl-xrp`, runs only by a dev switch no release build honours ([real-money](decisions/real-money.md), [XRP Ledger](areas/xrpl.md#real-money)). `live` is enabled only when all of these hold:

- the peer protocol's sender authentication is in place ([Peer protocol](areas/peer-protocol.md));
- the ledger is durable: journal and reconciliation ([Ledger](areas/ledger.md));
- XRP Ledger reserve and result handling is complete: the reserve is never booked, an account's creation is a deposit, whole drops with fees booked, results checked with a failure's fee booked and the sender checked ([XRP Ledger](areas/xrpl.md));
- releases are signed ([Release](areas/release.md));
- the node checks the XRP Ledger's validators itself, on both networks ([Checking XRP Ledger validators](#checking-xrp-ledger-validators));
- a public proof server runs for each network, named in the node's defaults ([Proof servers](#proof-servers));
- mainnet schemes in XRP and RLUSD, and the realm gate ([Mainnet schemes](#mainnet-schemes));
- settlement hubs on mainnet: a default mainnet hub for `hub-xrpl-xrp` and for `hub-xrpl-rlusd`, founded by the project's validators ([Mainnet hubs](#mainnet-hubs));
- there has been an external review;
- a mainnet bootstrap node list exists.

Names are not among these: the live realm runs without them until registrations pay fees ([Names](#names)).

The first four are on `main`, and so are each network's proof server, named in the node's defaults, the mainnet hub schemes and validators that observe the escrow through proofs. Until the rest are, the testnet account stops unless its server reports the testnet's network ID, so a mainnet server never gets XRP booked or signed for with a testnet wallet; and the account proves what it reads against ledgers its UNL validated, from the project's proof servers by default ([Proof servers](#proof-servers)). The issuer's conditions for a dollar token are decided ([issued-assets](decisions/issued-assets.md)): the issuer is the owner's counterparty, its freezes and clawbacks are booked as built, and the scheme pauses while its pool prices it off its peg, until [open-rate-sources](decisions/open-rate-sources.md) turns the guard off.

## Smaller open points

- **A post-quantum signing key.** Every node ID commits to an SLH-DSA key beside the Ed25519 key, which signs nothing yet ([post-quantum-node-id](decisions/post-quantum-node-id.md)). Not built: a node's post-quantum signing key, the SLH-DSA statement that vouches for it under the node's ID, the version that carries both, and a post-quantum key in the QUIC handshake, which proves a peer's key ([Peer protocol](areas/peer-protocol.md#versions)). **Done when** a node can prove its identity, and its peers check it, without trusting Ed25519.
- **The testnet's new phrases.** A node reads only phrases of the [phrase format](decisions/phrase-format.md), so every machine of the testnet that keeps a phrase made before gets a new one. The bootstrap nodes have theirs, and the node's default bootstrap list names them by their new IDs and both their keys (`crates/dassie-node/src/env.rs`). The market maker's node is left: each new phrase is a new node ID and new XRP Ledger accounts, which change the market maker's targets, and the accounts are funded again.
- **The testnet's names under `test.dassiepay.net`.** The live realm takes `dassiepay.net` and the testnet moves to `<node ID>.test.dassiepay.net` ([public-domains](decisions/public-domains.md)): it needs the subdomain's DNS, a wildcard certificate and a testnet front door serving it, and then the testnet's default domain in the code.
- **Pre-releases,** if wanted, are `vX.Y.Z-rc.N` tags that the release workflow builds and signs as it does a release, on a channel of their own that a node joins only when its owner asks; the version parser, the installer and the updater accept none yet ([release-integrity](decisions/release-integrity.md)).
- **The market maker's VM on one node.** The VM runs one node in both hubs where it ran a node per asset ([Market maker VM](areas/market-maker-vm.md)), which deploying takes an operator's steps: make the secret `mm-phrase` from the DSUSD node's phrase in `mm-dsusd-phrase`, whose account holds XRP and a DSUSD trust line already, and grant the VM's service account access to it, as `provision.sh` does; send what the XRP node's account and hub hold to that account, or leave it; put the new `startup.sh` in the instance metadata and reset the VM, which stops the two nodes and starts the one; move `/var/lib/dassie-mm/xrp` and `/var/lib/dassie-mm/dsusd` away once the node runs; and then delete the secrets `mm-xrp-phrase` and `mm-dsusd-phrase` with their bindings. **Done when** the VM's log shows the market maker swapping through boot1–3 with one node.
- **Importing a v0 node's config and BTP tokens** is not planned; it is cheap to add if anyone asks ([database-continuity](decisions/database-continuity.md)).

## Future work, design only

Designs that stay future work. The architecture must not preclude them.

- **The token payment protocol** (the original's `docs/implementation-notes/token-payment-protocol.md`, at the tag `original`).
- **A private UDP peer transport** (the original's `docs/implementation-notes/peer-to-peer-communication.md`).
- **Signed or independent frontends,** so that the wallet need not trust the node it is served by (the original's `docs/implementation-notes/frontend-security.md`, [secrets-at-rest](decisions/secrets-at-rest.md)).

## Decisions

- [peer-protocol-v1](decisions/peer-protocol-v1.md): the post-quantum path: hybrid TLS for confidentiality, and a key kept for in link states.
- [post-quantum-node-id](decisions/post-quantum-node-id.md): the node ID commits to an SLH-DSA key, and how a node is to prove continuity with it.
- [database-continuity](decisions/database-continuity.md): no import of v0 data for now.
- [membership-economics](decisions/membership-economics.md): registration fees and why registration is free and throttled meanwhile.
- [names-by-earliest-commitment](decisions/names-by-earliest-commitment.md): names, sealed and held by a quorum of the bootstrap nodes' lists.
- [owner-ledger](decisions/owner-ledger.md): per-ledger credit line and largest packet.
- [real-money](decisions/real-money.md): the conditions for real money.
- [verified-ledgers](decisions/verified-ledgers.md): checking the XRP Ledger's validators against proofs from patched rippled servers.
- [mainnet-schemes](decisions/mainnet-schemes.md): mainnet's schemes, and a ledger only in its node's realm.
- [secrets-at-rest](decisions/secrets-at-rest.md): frontend signing stays future work.
- [release-integrity](decisions/release-integrity.md): a release is built once until a second build can check it.
- [peering-lifecycle](decisions/peering-lifecycle.md): the channel limit.
- [core-and-periphery](decisions/core-and-periphery.md): the periphery.
- [settlement-hubs](decisions/settlement-hubs.md): settlement hubs, on mainnet too.
- [hubs-only](decisions/hubs-only.md): peers settle only over hubs, in one hub per asset until several of one asset are worth it.
- [token-hubs](decisions/token-hubs.md): hubs for DSUSD and RLUSD, the issuer's freeze and clawback, and the escrow's gas.
- [hub-scheme-names](decisions/hub-scheme-names.md): hub schemes named for their ledger and asset.
- [random-seed-simulations](decisions/random-seed-simulations.md): the simulation campaign.
- [ledger-resets](decisions/ledger-resets.md): a lagging server is handled; a reset counts once the testnet's UNL validates it.
- [default-relay](decisions/default-relay.md): the default relay, until an onboarding asks.
- [issued-assets](decisions/issued-assets.md): the peg guard; transfer fees.
