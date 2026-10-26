# Roadmap

What is not built yet: the open areas of work, the gaps between the spec's contract and `main`, the conditions for real money, smaller open points, and what stays future work. Everything else in the spec describes what is on `main`; an area doc that meets one of these gaps says so in one line and links here.

**New work is an open area of work here, never a numbered milestone.** Areas proceed in parallel, and each says what done means. When an area is built, its behaviour moves into the area docs and its entry here goes.

## Open areas of work

### Hubs only

Peers settle only over settlement hubs, and direct settlement leaves the code ([hubs-only](decisions/hubs-only.md)). Today every scheme on the XRP Ledger peers directly, through the credit machinery ([Settlement](areas/settlement.md#credit)); the plain stub that most tests and simulations run on settles directly through it too; and dev, the end-to-end tests and the wallet have no hub.

- **Parts that can be built side by side:**
  - the owner's books: each scheme on the XRP Ledger, in XRP, DSUSD and RLUSD, runs as the owner's book of its asset and peers with no one; a scheme says whether it peers; a token scheme pays the owner's funds out as the XRP scheme does; and a hub's funding takes from and pays to the owner's book of the hub's asset;
  - the tests: a stub with unlimited trust, which bounds room but never asks to settle and never settles, for the tests and simulations that are not about settlement; those about settlement on the stub with commitments or on hubs in the simulation; and the coverage of proofs and lying servers under a whole node, which only the direct simulations and end-to-end tests have, moved onto hub runs before those go;
  - dev, the end-to-end tests and the wallet on hubs: a dev scenario with validators and an escrow on the dev fake ledger and members that fund from their owner's books, with an end-to-end test; the direct dev scenarios and end-to-end tests moved onto hubs or the unlimited stub; the wallet's view of the node's hubs, and `dassie setup` joining the default hub;
  - the market maker on hubs: its periphery nodes members of the XRP and DSUSD hubs, its dev scenario, end-to-end tests and VM back once they are; it is parked until then. Inventory pricing and the rate feed on the hubs' ledgers, with rates compared across ledgers at spot;
  - removing direct settlement: the peer half of the XRP Ledger schemes, entry payments ([pay-to-enter](decisions/pay-to-enter.md)), and the credit machinery only direct peerings need: the senior's credit, growth ([earned-trust](decisions/earned-trust.md)), the least credit, an end for room's lower credit ([ended-for-room](decisions/ended-for-room.md)), the fallback to a direct scheme when no hub is shared, and the plain stub that settles. What stays: the size one request to settle funds a hold by, which is the default credit line today ([Settlement hubs](areas/hubs.md#in-the-node)), trust transfers, settling on request and the end for room's mark;
  - a design doc of direct settlement, written as it leaves: how it worked, and the credit per method it would need to come back, never below the least credit a peering needs;
  - the XRP hub scheme renamed from `hub-xrp-testnet` to `hub-xrpl-testnet-xrp`, with its ledger, by a migration of the database ([hub-scheme-names](decisions/hub-scheme-names.md)), and in the wallet.
- **Done when:** no scheme peers but a hub's, in the code and in the `peer` and `node` simulations; dev and the end-to-end tests run on hubs, the market maker's included; every test that is not about settlement runs on the unlimited stub; proofs and lying servers are covered under hub runs; and the spec describes direct settlement only in its design doc.

### Settlement hubs

A ledger kept by n = 3f + 1 validators and backed by an escrow on the XRP Ledger, so that two nodes peered over a hub cannot walk away with what they owe each other; peers settle only over hubs ([Hubs only](#hubs-only)). The design is [designs/settlement-hubs.md](designs/settlement-hubs.md); the reasons are [settlement-hubs](decisions/settlement-hubs.md).

- **Parts that can be built side by side:**
  - `dassie-hub`: the hub's ledger, its operations and Simplex consensus, in the simulation without an escrow;
  - the `hub-xrpl-testnet-xrp` settlement scheme in the node: its registry and the driver for the validators' endpoints, a floor and a ceiling the owner sets, against the hub's ledger in the simulation, and the member's account following a change of validators, whose set it now takes from the rules it was built with ([the member's side](areas/hubs.md#the-members-side) and [moving funds](areas/hubs.md#in-the-node) are built);
  - the escrow on the XRP Ledger testnet: founding, deposits, withdrawals, changing validators, shutdown and payout, test network resets, against the fake XRP Ledger;
  - the validator role in the node: gathering the old set's signatures after a change of escrow keys, for the withdrawals sequenced before it and the old set's change of the signer list ([the rest is built](areas/hubs.md#the-validator-in-the-node));
  - founding a hub through the node ([built](areas/hubs.md#founding-a-hub)): a sponsor replacing a validator that refuses or fails before genesis, and the wallet's view of a founding;
  - payout bundles ([built](areas/hubs.md#payout-bundles)): validators passing views and bundles of higher blocks around before and after they sign, a validator that comes back signing from what a member hands it, and the node submitting a quorum's set itself once its validators are gone;
  - validators keeping their history across restarts, so that a restarted honest validator never says `Trimmed`: a member then reads its state anew only on f + 1 of them, and no longer on one and minutes without its cursor moving ([When the history is gone](areas/hubs.md#when-the-history-is-gone)), which costs it the fulfillments of claims made in the blocks it skips;
  - the testnet hub ([its rules are the default hub](areas/hubs.md#in-the-node), boot1–4 its validators): joining it by default, with no check of distance ([hubs-only](decisions/hubs-only.md));
  - validators that attest only what they proved through the verifier: see [Mainnet hubs](#mainnet-hubs).
- **Done when:** in the simulation, with up to f faulty validators, members peered over a hub pay each other across hops without trusting each other: a payer that sends no claims, and a payee that claims twice or late, cost the other no more than its hold. f + 1 colluding validators leave evidence. A hub that loses a validator shuts down, and the escrow pays out exactly the final balances, with signatures collected over time. On the testnet, nodes peered over the default hub pay each other, and it carries on while a validator restarts.
  - **Met in the simulation** (`node-hub`, [Testing](areas/testing.md#over-a-settlement-hub)), but for this: the hub that loses validators stalls there, and its escrow pays out the final balances with signatures gathered over time; a shutdown after a validator's absence, days in the design's terms, is not simulated.
  - **Met on the testnet:** two nodes peered over the default hub, `4c5ef2ba…`, pay each other, and it carries on while one of its validators restarts.
- **Later, once hubs run:**
  - fees that follow how full blocks are, so that holding back someone's claim grows dear, and an idle fee that frees the places of abandoned accounts;
  - a setup fee the sponsor pays each validator, in place of the owner's list of sponsors;
  - caps on what new rules may charge, and leaving free of the withdrawal fee during a reconfiguration's notice ([the design](designs/settlement-hubs.md#8-changing-the-validators));
  - a node putting what it earned into its holds, so a peer's room grows with earnings;
  - hub balances and holds in the desktop app;
  - stall periods above what the engine's views hold at the longest view (`MAX_STALL_MS`), by keeping long runs of nullified views as ranges ([finality-after-any-outage](decisions/finality-after-any-outage.md)).
- Hubs hold real money only once the conditions of [Real money](#real-money) are met.

### Joining by being paid

A node with no account in a hub is paid into it by a peer, which opens its account with `pay`, as [the design](designs/settlement-hubs.md#5-joining) has it, so a node need not deposit before it can peer ([hubs-only](decisions/hubs-only.md)). Today a node joins a hub only by its own deposit ([Settlement hubs](areas/hubs.md#in-the-node)).

- **Parts that can be built side by side:**
  - the member's side in `dassie-hub`: a membership with no account yet, which learns its account by its key once a peer's `pay` opened it, and sets its disbursal address before any other operation;
  - the node: a peering over a hub with a peer that has no account in it yet, whichever of them asked to peer, whose packets go without commitments, on what the paying peer owes the new member, until that reaches the hub's minimum for a new account; then the paying peer's `pay` of that minimum, never before, booked as a settlement on both sides. The new member spends only what it is owed, through the same peer.
- **Done when:** in the `node-hub` simulation, a node with no account in the hub peers with a member, is paid into the hub without depositing once its peer owes it the hub's minimum for a new account, and not before, sets its disbursal address first, and pays and is paid over the hub from then on; the `pay` is booked once on each side, across crashes; and a node that cycles fresh keys through a member costs that member nothing.

### Token hubs

Hubs that hold an issued token: DSUSD on the XRP Ledger testnet, RLUSD on mainnet. The issuer's freeze is a state of the hub's ledger, a clawback ends the hub with a payout pro rata, and the escrow's fees are paid from XRP anyone may send it ([token-hubs](decisions/token-hubs.md)). Today a hub's rules name no asset, and its escrow holds XRP alone.

- **Parts that can be built side by side:**
  - the rules: the asset and its issuer in the terms, in an encoding versioned so that the testnet's hub keeps its ID;
  - the escrow: its trust line to the issuer, set at founding, and in the validators' check with the issuer's account charging no transfer fee; token amounts in deposits, withdrawals and payouts; and XRP sent to a token escrow credited to no one, its fees paid from the escrow's XRP;
  - the hub's ledger: frozen and clawed-back states; while frozen, only claims for commitments that expire within the freeze's window (its stamp plus M, `MAX_HOLD_MS` and the hub's precision), each before its own expiry, and the validators' attestations, with every other operation waiting for the thaw; the hub's clocks stopped while frozen, every notice (a hold reduction's, a reconfiguration's, the shutdown's) and a validator's absence pushed back by the freeze's length, so that a hold reduction pending at a freeze falls due only after the thaw, its notice extended by the freeze; after a clawback, the withdrawals already sequenced paid first and the rest pro rata, with an exact rule for the remainder; and the `hub` simulation drawing freezes, thaws, transfer fees and clawbacks;
  - the validators: attesting the issuer's freeze, thaw, transfer fee and clawback from what they proved of the escrow's line and the issuer's account; submitting nothing to a frozen escrow, and signing no payout set until the thaw;
  - the member's side: `hub-xrpl-testnet-dsusd`, booking on `hub-xrpl-testnet-dsusd+usd`, its scheme and ledger taken from the hub's asset ([hub-scheme-names](decisions/hub-scheme-names.md)), funded through the owner's DSUSD book; a frozen hub's method unavailable from the freeze's block until the thaw, with no Prepare sent or taken over it and no deposit; a payee over any hub refusing a commitment, and its Prepare, that expires more than `MAX_HOLD_MS` plus the hub's precision ahead of its clock ([the design](designs/settlement-hubs.md#4-peering-over-a-hub)); and before a withdrawal, the escrow's XRP topped up to the least gas when it is short, a constant of the hub's crate;
  - the DSUSD testnet hub, founded by boot1–4, with its genesis built in and offered by default beside the XRP hub. RLUSD's hub comes with [Mainnet hubs](#mainnet-hubs).
- **Done when:** in the `hub` simulation, a freeze halts every member's operation but claims for commitments that expire within its window, which go through until they expire, a hold reduction pending at a freeze falls due only after the thaw, its notice extended by the freeze, and no notice or absence falls due while it lasts, nothing is submitted to the escrow and no payout set is signed while it lasts, and the hub resumes on the thaw; a transfer fee freezes the hub as a freeze does; after a clawback, the payout pays the withdrawals already sequenced in full and each account its share of what the escrow holds beyond them, to the last unit and never more. In a `node-hub-dsusd` simulation, members peered over a DSUSD hub pay each other until a freeze, send and take no packet over it while it lasts, keep the claims for packets they took before it, refuse commitments that expire more than `MAX_HOLD_MS` plus the precision ahead, lose nothing they had not redeemed when a reduction was pending, and pay each other again after the thaw; they are paid out after a clawback; XRP sent to the escrow credits no account, and a withdrawal tops up the escrow's XRP first. On the testnet, nodes peered over the DSUSD hub pay each other, and a payment crosses from the XRP hub to the DSUSD hub.

### Several hubs per node

A node is in one hub per asset, and in hubs of several assets ([hubs-only](decisions/hubs-only.md)). Today a node is in one hub, of XRP.

- **Parts that can be built side by side:**
  - the node: funding, payouts and moves per hub, each from the owner's book of its asset, with each hub's funding kept in the database;
  - advertising: the link state names every hub the node is in, each with its scheme and asset, in a bounded list, and still reads the one hub of nodes that name only one;
  - the owner's view: each hub's account, holds and moves over RPC, in the wallet and in `dassie hub`;
  - a pair in two hubs peering over both, each a method ([Settlement methods](#settlement-methods)).
- **Later:** several hubs of one asset, with a limit per validator operator over all of them, once running hubs show it is worth it.
- **Done when:** in the `node` simulation, nodes in an XRP hub and a DSUSD hub fund, advertise and pay out each on its own; a pair in both carries packets over both; and one hub's failure leaves the other's method working.

### Settlement methods

A peering settles over every hub both nodes are in, each a method of the peering, and a packet keeps the ledger it came in on where a first hop has a method on it ([settlement-methods](decisions/settlement-methods.md), [hubs-only](decisions/hubs-only.md)). Today a peering is on one scheme, picked alphabetically, and stays on it.

- **Parts that can be built side by side:**
  - the peer protocol, within v1: a link-state extension saying a node has methods, and messages that agree, add and drop them and carry a Prepare that names its method; with a node without methods, one method and Prepares as now; a stored peering loaded as one method;
  - the node: accounts, credit, thresholds, requests to settle and withdrawal for room per method, the end for room and its mark per peering, a hub both come to be in added to their peerings, and a method dropped as a peering ends, on that method alone: collected once for a bounded time when its scheme is removed, without collecting when its hub cannot receive for as long as a silent peer is kept or is no longer run; an owner's removal of a scheme no longer refused for its methods;
  - the connector: among a destination's first hops, one with a method on the packet's incoming ledger first, and otherwise the drawn hop's method with the best rate in the price table, rates on different ledgers compared at spot; a method chosen by structure only, so one short of room gives T04.
- **Done when:** in the `node` simulation, a pair in two hubs carries packets over both, each on the ledger it came in on; a payment whose two ends share a ledger crosses core nodes that also share it without converting; a method short of room answers T04 and moves no packet to another method; a hub both nodes join is added to their peering, and one removed is dropped within a bounded time, what is left staying on its accounts, the peering kept; a node of today and one with methods peer and carry packets on one method, and a stored peering carries on as one method after an upgrade; and the per-node oracles hold the bound on what each peer can cost, summed over its methods.

### Random next hops

A packet's next hop drawn at random among its first hops, weighted by the fees each earned us, so that one node cannot stop a payment and a node with a poor rate earns nothing ([weighted-next-hops](decisions/weighted-next-hops.md)). Today the connector takes the first usable hop by node ID.

- **Parts that can be built side by side:**
  - routing: the few first hops kept per destination cut by a hash keyed by the node, rather than the lowest node IDs, so that no node can choose an ID that makes it everyone's first hop;
  - the connector: a draw per packet from the node's random number generator, among the usable first hops that keep the packet's ledger, or among all of them, unweighted, when none does; the owner's own packets only among those on the ledger their route starts on, picked once per quote or payment;
  - the weights: per peer and ledger, the fees earned on packets that kept that ledger and were fulfilled through the peer, decaying, in memory, with a fixed share of the draws even.
- **Done when:** in the `node` simulation, a payment to a destination with two paths, one through a node that refuses its packets or converts below the sender's minimum, completes through the other, and that node earns nothing; packets an attacker sends itself through a hop and rejects leave its weight as it was, and raising a hop's weight costs the node's fee on every unit; a hop short of room answers T04 and keeps its weight for longer than a settlement takes.

### Mainnet hubs

Hubs on the XRP Ledger's mainnet, in XRP and in RLUSD, as part of the [real money](#real-money) launch ([settlement-hubs](decisions/settlement-hubs.md), [real-money](decisions/real-money.md)). Today hubs run on the testnet only, and their validators take their server's word on the escrow.

- **Parts that can be built side by side:**
  - a verifier of the XRP Ledger that stands alone, for validators, from the account's ([XRPL](areas/xrpl.md#the-validators-records)): proving a transaction, proving state, and the validators' records; and validators that follow the escrow only through it: deposits and outcomes by their transactions' proofs, the escrow's setup and the issuer's acts by state proofs, and a testnet reset only once the testnet's UNL validated it; deposits found with no budget that can drop one, from hints as long as none is missed and each is proven; and what they judge as of the latest ledger they proved, such as a frozen escrow, judged only from an anchor younger than a bound ([the design](designs/settlement-hubs.md#6-deposits-and-withdrawals), [Checking XRP Ledger validators](#checking-xrp-ledger-validators));
  - the network named in the path that derives a hub's escrow and keys, for every network but the testnet, so that the testnet's hubs keep their keys, one passphrase run on both networks never makes the same ones for a nonce, and a payout set keeps to one network by itself rather than by the escrow's sequence ([One network per escrow](designs/settlement-hubs.md#2-founding-a-hub)); and a validator's network taken from its config;
  - the schemes `hub-xrpl-xrp` and `hub-xrpl-rlusd`, booking on `hub-xrpl-xrp+xrp` and `hub-xrpl-rlusd+usd` in the live realm, funded from the owner's `xrpl` and `xrpl-rlusd` books ([hub-scheme-names](decisions/hub-scheme-names.md));
  - a default list of mainnet validators, kept with the mainnet bootstrap node list, whose first validators the project runs;
  - the default mainnet hubs' genesis built in, which the project provides once its validators founded them, joined only once an owner turns hubs on.
- **Done when:** a validator connected to a lying server attests no deposit, withdrawal, setup, reset, freeze, thaw or clawback that no validated ledger holds, and the validators' following of the escrow (`hub/escrow.rs`) proves through the verifier, in its tests against a lying fake ledger and in the simulation that runs validators against the fake ledger; a validator misses no deposit, however many small payments anyone sends the escrow; a validator whose server stops closing ledgers judges nothing from its old anchor once it is older than the bound, and attests again once a newer one is proven; and in a live-realm simulation against the mainnet fake ledger, with a lying server, members peer over the default mainnet hubs in XRP and in RLUSD.

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
  - **The fee quote and payment,** as a v1.x peer-protocol addition: a quote message, and payment by ILP with a signed quote condition (the "naive" variant of the [token payment protocol](original/05-accounting-and-settlement.md)). A registration pays the cheapest 75% of bootstrap nodes.
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
  - the downlink table: downlinks on disk in a table of their own rather than in the node table, in memory, with their channels and accounts, only while connected or while something still needs them, their timers run from the disk, with rows closed once idle, as channels close, and the XRPL scheme's peers' addresses in a table keyed by account rather than in its state ([§4](designs/periphery.md#4-downlinks-on-a-core-node), [resident-peers](decisions/resident-peers.md));
  - BTP clients of a periphery node, whose ILDCP address is still beneath the node's own address rather than an uplink's;
  - periphery nodes in the `peer` and `node` simulations' chaos runs, losing and regaining uplinks;
  - records and home nodes, so that clients find the relays that reach a node. Relays are built: every QUIC endpoint relays streams to the nodes connected to it, so every uplink is already a relay for its downlinks, and a node keeps a connection to a relay it chooses ([Peer protocol](areas/peer-protocol.md#relayed-streams)). Relayed streams carry Open Payments and paired owner apps' RPC;
  - owner apps. The node pairs them and serves their RPC on relayed streams ([RPC](areas/rpc.md#paired-devices)), and the wallet on `localhost` is to show the pairing code; left are the apps themselves (a remote mode of the desktop app, a mobile app), consent to grants in the app as it connects (listing and following pending grants), the front door's consent page that sends the owner there, and finding the node on the local network;
  - the [desktop app](areas/desktop.md) beyond its Nix package: packages for other Linux distributions (AppImage, .deb, Flatpak), the app in the mirror's flake from a signed build, its updates, and macOS and Windows;
  - front doors. Front doors are built, for the nodes that keep one as their relay, over UDP or, where UDP is blocked, a WebSocket, which a node without a hostname does with `dassiepay.net`'s by default ([HTTP](areas/http.md#front-doors), [Node](areas/node.md#a-chosen-relay)), and so are a node's names and its signed responses ([Payments](areas/payments.md#the-nodes-names)); left are their consent page (the front door answers the consent route with a fixed error until then), an owner's own domain as a CNAME to a front door with a certificate for that name, the DNS TXT record that names the node and its check in Dassie's own client, the owner's names as a wallet setting rather than an environment variable, a node's rate limits per client behind its chosen relay (today a node counts its front door as one client, so a few clients use up its budget for every client through it: [HTTP](areas/http.md#front-doors)), and the proposal to Open Payments;
  - free receivers (their half of an uplink's downlinks is built), paid into their uplink's hub without a deposit ([Joining by being paid](#joining-by-being-paid)). Paying to enter on an XRPL scheme ([pay-to-enter](decisions/pay-to-enter.md)) goes with direct settlement ([Hubs only](#hubs-only));
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

- **Left to build:** signing only from a fresh anchor; anchoring on fewer hints than each closed ledger's, as the design's triggers have it, once the schemes no longer need to hear of each ledger; and in the `node` simulation, testnet resets under the UNL, one of them while a node is down until the new chain has passed its anchor. The rest is built: each network's proof server as its default ([Proof servers](#proof-servers)), the verifier ([XRPL](areas/xrpl.md#the-validators-records)), the account on proofs ([XRPL](areas/xrpl.md#anchors), [XRPL](areas/xrpl.md#following-the-account)), the fake ledger's proofs and lying server, and the simulation and end-to-end tests against them ([XRPL](areas/xrpl.md#proofs-in-the-fake-ledger), [Testing](areas/testing.md#the-simulations)).
- **Done when:** a server that reports a transaction, a balance, an issuer's flag, a pool or an absent account that no validated ledger holds, or a reset no quorum signed, gets nothing booked, written off or cancelled, and the scheme waits and reconnects until an honest server answers; a settlement that applied is never cancelled, whatever a server says of it; a transaction left out of the history is found by the next anchor; an expired or unreachable validator list, or a server that serves no proofs, stalls the scheme with its reason and never falls back to the server's word; with an honest proof server, the scheme settles as before, on both networks. In the simulation every XRPL node anchors, and every kind of lie is reached.
- **Later:** the negative UNL, so that a node rides out a fifth of the UNL offline as rippled does.
- Hub validators use the verifier to observe the escrow for mainnet hubs, as part of the launch: see [Mainnet hubs](#mainnet-hubs).

### Mainnet schemes

Settling with real money on the XRP Ledger's mainnet, in XRP and in RLUSD ([mainnet-schemes](decisions/mainnet-schemes.md)), with no DSUSD on mainnet, and DSUSD as the testnet's stand-in for RLUSD. A condition for [real money](#real-money); the `live` realm stays refused after it.

- **Left to build:** a live node that takes none of the test network's defaults, its bootstrap nodes and its relay.
- **Done when:** live-realm nodes against the mainnet fake settle in XRP and in RLUSD at addresses under `g.`, over the mainnet hubs once peers settle only over hubs ([Mainnet hubs](#mainnet-hubs)); a test-realm node never activates a mainnet scheme, nor a live-realm node a testnet one; no node activates test RLUSD; a release binary refuses the dev switch; a pool moved off its peg pauses the token's scheme and its conversions once the guard's hold has passed, on mainnet RLUSD and on testnet DSUSD, and they resume after recovery, with nothing converted at par meanwhile, while what arrives is still booked; the guard pauses the scheme when its reading is stale.

### Kopje across Dassie

The brand page holds Dassie's visual identity, Kopje ([visual-identity](decisions/visual-identity.md)), and dassie.land draws in it; the wallet and the desktop app do not yet.

- **Parts that can be built side by side:** the wallet, in its light theme and dusk; the desktop app's icons and tray.
- **Done when:** every screen of the wallet and the desktop app draws in Kopje's palette, type and mark, light and dark.

## Not built yet

Gaps between what the spec's contract or decisions ask for and what `main` does. Each can be picked up on its own.

- **ILP-over-HTTP** (RFC 35, with its synchronous mode): the node serves no `/ilp` route ([Connector](areas/connector.md#ilp-over-http)). **Done when** an RFC 35 client, synchronous mode included, pays through the node over `/ilp`.
- **Hybrid post-quantum TLS.** Confidentiality against "harvest now, decrypt later" belongs to TLS with the hybrid key exchange X25519MLKEM768 ([peer-protocol-v1](decisions/peer-protocol-v1.md)). The node's TLS uses rustls with ring as its only crypto provider, which offers no ML-KEM, so its handshakes use classical key exchange only ([HTTP](areas/http.md#certificates)). **Done when** the node's listeners and its clients (peer client, ACME) offer and prefer X25519MLKEM768, with a crypto provider that still builds the static musl binaries.
- **Per-ledger credit line and largest packet** ([owner-ledger](decisions/owner-ledger.md)). Both, and with the largest packet every threshold counted in it, are one value for the whole node ([Connector](areas/connector.md#the-largest-packet), [Settlement](areas/settlement.md#credit)), so the same thresholds count for more in XRP than in dollars. The credit line goes with direct settlement ([Hubs only](#hubs-only)) as credit; what remains of it is the size one request to settle funds a hold by, over a hub ([Settlement hubs](areas/hubs.md#in-the-node)). **Done when** each ledger is configured with its own largest packet and size of a hold's funding, and the thresholds follow the ledger's.
- **Draining in-flight packets on shutdown.** On SIGTERM the engine stops at once; it does not wait for the packets in flight to be answered or to expire ([Node](areas/node.md#shutdown)). Their pending transfers were never on disk, so after the restart the books stand as if they had been rejected, and a Fulfill that came back is lost, as in a crash; the cap on money in flight with a peer bounds what that costs ([Connector](areas/connector.md#money-in-flight-with-a-peer)). **Done when** the node stops taking new Prepares on SIGTERM and waits, up to the latest expiry among them, for the ones in flight before it closes the database.
- **The accounts of closed relationships.** A settlement channel closes once idle, to disk, and frees its place ([closing-channels](decisions/closing-channels.md)), but a peer's two ledger accounts stay in memory for good, since the journal refers to them. The ledger's account bound (`max_accounts`), less the room kept for the node's own accounts, so still caps the nodes a node ever peers with, at about three and a half thousand, half of them core nodes and half downlinks ([account-budgets](decisions/account-budgets.md)); past its kind's half, a peering is refused for capacity ([Settlement](areas/settlement.md#settlement-channels)). The design is [resident-peers](decisions/resident-peers.md): account numbers from a counter, never given again; only the accounts in memory, with each ledger's totals of those on disk; one way to a peer's accounts, from memory, disk or new; a core peer's accounts closing with its channel; and a migration that marks the accounts of closed channels. A downlink's accounts leaving memory when it disconnects come with [the downlink table](#periphery). **Done when** the accounts of a node whose channel closed leave memory and its bound, and come back with what they showed when it peers again, or when a late settlement comes over its channel; and in the simulation, nodes that peer and leave, more than the account bound holds, are settled exactly once, with every ledger balanced.
- **Issuer transfer fees.** An issuer that charges a fee on transfers between holders makes the RLUSD scheme `Unavailable` ([XRPL](areas/xrpl.md#rlusd)): a Payment without `SendMax` cannot pay the fee. The codec signs a Payment with `SendMax` already ([XRPL](areas/xrpl.md#tokens)); the scheme does not use it, and the fake ledger takes no transfer fee. **Done when** the scheme pays with `SendMax`, books what it sent above what the peer received as a fee, and settles while the issuer charges one.
- **Registration fees:** see [Registration fees](#registration-fees).
- **The rest of [random-seed-simulations](decisions/random-seed-simulations.md):** see [Continuous simulation](#continuous-simulation), including the release check.

## Real money

The `live` realm is refused, so the XRP Ledger mainnet's scheme, `xrpl`, runs only by a dev switch no release build honours ([real-money](decisions/real-money.md), [XRP Ledger](areas/xrpl.md#real-money)). `live` is enabled only when all of these hold:

- the peer protocol's sender authentication is in place ([Peer protocol](areas/peer-protocol.md));
- the ledger is durable: journal and reconciliation ([Ledger](areas/ledger.md));
- XRP Ledger reserve and result handling is complete: the reserve is never booked, an account's creation is a deposit, whole drops with fees booked, results checked with a failure's fee booked and the sender checked ([XRP Ledger](areas/xrpl.md));
- releases are signed ([Release](areas/release.md));
- the node checks the XRP Ledger's validators itself, on both networks ([Checking XRP Ledger validators](#checking-xrp-ledger-validators));
- a public proof server runs for each network, named in the node's defaults ([Proof servers](#proof-servers));
- mainnet schemes in XRP and RLUSD, with memo types of their own and the realm gate, and the dollar token's peg guard ([Mainnet schemes](#mainnet-schemes));
- settlement hubs on mainnet: `hub-xrpl-xrp` and `hub-xrpl-rlusd`, a default mainnet hub for each, and validators that observe the escrow through proofs ([Mainnet hubs](#mainnet-hubs));
- there has been an external review;
- a mainnet bootstrap node list exists.

Names are not among these: the live realm runs without them until registrations pay fees ([Names](#names)).

The first four are on `main`, and so is each network's proof server, named in the node's defaults. Until the rest are, the testnet account stops unless its server reports the testnet's network ID, so a mainnet server never gets XRP booked or signed for with a testnet wallet; and the account proves what it reads against ledgers its UNL validated, from the project's proof servers by default ([Proof servers](#proof-servers)). The issuer's conditions for a dollar token are decided ([issued-assets](decisions/issued-assets.md)): the issuer is the owner's counterparty, its freezes and clawbacks are booked as built, and the scheme pauses while its pool prices it off its peg.

## Smaller open points

- **A later peer-protocol version** drops the `credit` field of `peeringRequest` and of its response (sent as 0 and ignored), and the one-byte body of the reply to a Fulfill (always `accepted`, kept for older nodes) ([own-fulfills](decisions/own-fulfills.md)).
- **A post-quantum peer-protocol scheme.** The authentication choice reserves a tag for a scheme on the X-Wing hybrid KEM, and a link-state extension type for its key; neither is built ([Peer protocol](areas/peer-protocol.md#versions), [peer-protocol-v1](decisions/peer-protocol-v1.md)). Signatures and node IDs stay Ed25519.
- **The live realm's default payment names.** A front door reads one network's registrars, and a passphrase used in both realms gives one label in both, so the live realm needs a front door of its own, at a domain of its own or a part of `dassiepay.net`, chosen before the live realm opens; the testnet's may move with it, since its names may be reset ([public-domains](decisions/public-domains.md)).
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
- [names-by-earliest-commitment](decisions/names-by-earliest-commitment.md): names, sealed and held by a quorum of the bootstrap nodes' lists.
- [owner-ledger](decisions/owner-ledger.md): per-ledger credit line and largest packet.
- [real-money](decisions/real-money.md): the conditions for real money.
- [verified-ledgers](decisions/verified-ledgers.md): checking the XRP Ledger's validators against proofs from patched rippled servers.
- [mainnet-schemes](decisions/mainnet-schemes.md): mainnet's schemes, and a ledger only in its node's realm.
- [secrets-at-rest](decisions/secrets-at-rest.md): frontend signing stays future work.
- [release-integrity](decisions/release-integrity.md): a release is built once until a second build can check it.
- [peering-lifecycle](decisions/peering-lifecycle.md): the channel limit.
- [core-and-periphery](decisions/core-and-periphery.md): the periphery.
- [own-fulfills](decisions/own-fulfills.md): the fields a later protocol version drops.
- [settlement-hubs](decisions/settlement-hubs.md): settlement hubs, on mainnet too.
- [hubs-only](decisions/hubs-only.md): peers settle only over hubs, and direct settlement leaves the code.
- [token-hubs](decisions/token-hubs.md): hubs for DSUSD and RLUSD, the issuer's freeze and clawback, and the escrow's gas.
- [hub-scheme-names](decisions/hub-scheme-names.md): hub schemes named for their ledger and asset.
- [random-seed-simulations](decisions/random-seed-simulations.md): the simulation campaign.
- [ledger-resets](decisions/ledger-resets.md): a lagging server is handled; a reset counts once the testnet's UNL validates it.
- [default-relay](decisions/default-relay.md): the default relay, until an onboarding asks.
- [issued-assets](decisions/issued-assets.md): the peg guard; transfer fees.
- [settlement-methods](decisions/settlement-methods.md): settlement methods within a peering, and packets that keep their ledger.
- [weighted-next-hops](decisions/weighted-next-hops.md): random next hops weighted by fees.
