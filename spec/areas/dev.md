# Dev environment

`node --run dev` runs a network of real `dassie` nodes on one machine, sets each up as its owner would, and serves a dashboard at `https://localhost` that shows every node at once and opens any node's wallet or debug pages. The end-to-end tests run the same scenarios.

**Paths:** `apps/dev/`, `apps/dev-dashboard/`, `proto/dassie/dev/v1/`
**Read with:** [Node](node.md), [Wallet](wallet.md), [RPC](rpc.md), [Testing](testing.md), [XRPL](xrpl.md)

## Running it

`apps/dev/main.ts` checks what it needs, builds the node, the wallet and the dashboard, serves the dashboard's origin and the SNI proxy, and runs the scenario chosen last; its flags are documented there. `--port-offset` and `--state-dir` let several environments run side by side; a second dev server on the same state directory is refused, since stopping its children would pull the first one's network from under it. At start, processes a crashed run left behind are killed, since a node left running would hold its ports and socket.

The nodes' IPC sockets live outside the state directory, under `$XDG_RUNTIME_DIR` and a tag of the state directory's path, because a socket path is short (`sun_path`); the prerequisite check fails early if the longest one would not fit.

## Everything stays on loopback

The dev nodes' keys are public: they are derived from the [table of dev identities](#dev-identities) in the repository. So every listener of the dev environment binds loopback only, and nodes run with `--listen 127.0.0.1`. A node's QUIC endpoint has its HTTPS port's number on UDP, so nodes reach each other over QUIC directly; the SNI proxy is for browsers. Each node's [rate feed](rate-feed.md#the-socket) has a UDP port of its own on loopback, `6000+N` (`--feed-listen`), since the default port takes one node per host. Nodes reach each other on loopback through `DASSIE_PEER_ALLOW_PRIVATE` ([Peer protocol](peer-protocol.md#outbound-requests)); browsers and the node's resolver map `*.localhost` to loopback, so `dN.localhost` needs no name resolution.

### SNI proxy

One port, 443, serves the dashboard and every node under its own name (`https://dN.localhost`), so each node's wallet has an origin of its own, as in production. The proxy reads the server name from the TLS ClientHello and passes the connection on still encrypted, so each node serves its own certificate and the proxy holds no key. Anything it cannot parse goes to the dashboard. It listens on loopback only, like every dev listener, which is also what lets a node trust that a browser it carries is on the machine when it starts a pairing ([HTTP](http.md#security-model)). When it cannot bind 443, the dev server goes on without it and prints how to reach the pages at their own ports.

### Certificates

mkcert's local CA makes every certificate, so a browser that trusts the CA (`mkcert -install`) trusts every dev page, and the nodes trust each other through `DASSIE_EXTRA_CA_FILE`. When the CA changes, every certificate is made anew. Tests pass a `CAROOT` of their own, so they never touch the developer's CA.

## Nodes

Each node is the real binary with its own data directory, socket and ports, and dev timings (`crates/dassie-node/src/env.rs`): the peer protocol's timers, and a dollar token's peg guard on the same rule in seconds ([XRPL](xrpl.md#the-peg-guard)). Dev nodes serve the wallet's watch build instead of the embedded one. A node that does not start, cannot be set up or exits on its own stops the scenario and is shown failed with the end of its output.

### Setting nodes up

Nodes are set up through their own RPC over IPC and their HTTPS setup route, the way an owner would, never by writing the database: the schema stays private to the node. The end-to-end tests use the same code (`apps/dev/src/scenario/node.ts`). A periphery node is set up the same way, but without a hostname ([below](#scenarios)). Setting up runs on every start and is idempotent: on a node already set up it checks that the data is this node's and adds only what is missing. The certificate and the rate source are set again each time, since the CA or the ports may have changed.

### Restarts and resets

A restart (a new node build, or asked for) keeps the nodes' data, and each node resumes, except in a scenario that starts from scratch on every run, a live one or a hub's ([below](#scenarios)). Choosing a scenario, starting over, or starting the dev server with another scenario wipes it. So does a scenario that now sets its nodes up otherwise than when it left the data: a node keeps each scheme with the config it was added with, so it could not take the scenario's new one, and the state directory records how the scenario set its nodes up beside its name. Commands from the dashboards and the keys run one at a time, in order (`apps/dev/src/environment.ts`).

## Scenarios

`apps/dev/src/scenario/scenarios.ts` defines them: how many nodes, which are bootstrap nodes and which periphery nodes, which settlement schemes they and added nodes use and with which configs, and which validate or join a settlement hub. The end-to-end tests run them through `Scenario.temporary`, in a temporary directory with a CA of their own.

- **No scenario that runs settles directly,** as peers settle only over hubs ([hubs-only](../decisions/hubs-only.md)). The scenarios whose subject is not settlement, `two-nodes` (the default), `six-nodes` and `one-node`, run the stub with unlimited trust, whose peers never run out of room and never settle ([Settlement](settlement.md#the-stub-scheme)). A scenario on the XRP Ledger runs its schemes as owner books, which peer with nobody ([Settlement](settlement.md#owner-books)), and its nodes peer over a settlement hub or on that stub. A scheme's config in a scenario is set up on every node that runs the scheme, added nodes too; the hub's scheme's is the founded hub's rules ([below](#scenarios)).
- **The XRP Ledger.** The scenarios on it need what only the node's own model of the XRP Ledger has (the `fake_ledger` example of `dassie-node`): an issuer that mints on demand, a pool, a mainnet or a hub's escrow off the public testnet. So they run only with `--xrpl fake`, and the dashboard says why otherwise. It is a new ledger on every run, which nodes treat as a testnet reset ([XRPL](xrpl.md)).
- `dsusd-owner-book` holds test DSUSD in its node's owner book. Only the fake ledger's issuer hands DSUSD out on demand: once the node shows its trust line, which its scheme sets up itself, the scenario has the issuer mint it the token, and does so again on every run, since the fake ledger is new each time. Tests ask the same issuer to freeze lines, claw back and move the pool (`Scenario.askIssuer`, [XRPL](xrpl.md#tokens-in-the-fake-ledger)). The issuer also makes an XRP/DSUSD pool at the start, priced at the dev rates, so DSUSD's peg guard reads it on its peg ([XRPL](xrpl.md#the-peg-guard)). No scenario holds test RLUSD, whose scheme is disabled ([XRPL](xrpl.md#rlusd)).
- **The live realm.** `live-owner-books` runs a node in the live realm that holds mainnet's XRP and RLUSD in owner books on its one account ([mainnet-schemes](../decisions/mainnet-schemes.md)). It runs only on the fake ledger's mainnet, never with real money: the node runs with the node's dev switch (`DASSIE_DEV_LIVE_REALM`), which only a build with debug assertions honors, and is set up in the live realm, which its setup request names, so its engine starts in it ([Node](node.md)). Mainnet has no faucet, so the scenario funds the node's account by a deposit, as an owner would, and RLUSD's issuer then mints it RLUSD beside an XRP/RLUSD pool at the dev rates. Mainnet never resets, and the fake ledger is a new chain on every run, so a live scenario's nodes start from scratch on every run, a restart too ([XRPL](xrpl.md#real-money)).
- **Across currencies.** `cross-currency` pays between dollars and XRP through a node that holds both and converts ([Connector](connector.md#inventory-pricing)): one node has dollars on the stub with unlimited trust, one has XRP in a settlement hub, and the third both. Four other nodes validate the hub and found it, as `hub`'s do, below. The two with XRP are its members, and the one with both runs the stub besides: so it peers with the first on the stub and with the second over the hub, and the first and the second share no scheme and never peer. Nodes added later join the hub.
- **A settlement hub.** `hub` founds a hub on the fake ledger and settles over it ([Settlement hubs](hubs.md#founding-a-hub)); it runs only with `--xrpl fake`, so that no run leaves an escrow on the public testnet. Four nodes validate it, each by a validator file the scenario writes into its data directory before the node starts (`DASSIE_VALIDATOR_CONFIG`), naming the sponsor, the fake ledger's server and the publisher of its validators, whose UNL the escrow's ledgers are proven under. The sponsor is one of them: once every node runs and its owner book of XRP shows the faucet's XRP, the scenario founds the hub through the sponsor's owner RPC with all four, and waits until it is founded. Two other nodes are its members, as are nodes added later: they run the owner book of XRP, which peers with nobody ([Settlement](settlement.md#owner-books)), and the hub's scheme, added with the founded hub's rules once it is founded, so they peer over the hub alone, and each funds its account in it from its book by itself ([Settlement hubs](hubs.md#in-the-node)). The members are its bootstrap nodes; the validators run no scheme to peer on. A node could validate and be a member at once, but members apart from the validators can restart, or fail, without taking the hub's quorum with them. The fake ledger is a new chain on every run, and nothing sets a hub's escrow up again on a new chain, so its members could neither deposit nor settle: the scenario starts from scratch on every run, a restart too, and founds a new hub, as a live scenario does. Each step is idempotent all the same: a founding the sponsor keeps is taken as it is, and a member's scheme as it was added. A scenario checks its hub before it starts: as many validators as a hub is founded with, a sponsor that runs the owner book, members that run the book as an owner book and the hub's scheme, and added nodes that run those and nothing else. A member may run another scheme too, as the converting node across currencies runs the stub, but no other member may run it: two members that share a scheme peer over it before their accounts in the hub exist, and that peering stays off the hub.
- **Two hubs.** `two-hubs` founds an XRP hub and a DSUSD hub as `hub` founds its one, with the same validators and sponsor, the DSUSD hub's terms naming the fake ledger's DSUSD, which its issuer mints the members beside its XRP/DSUSD pool, and has one member join each hub alone and two join both, each with the hub scheme of its asset and the owner books of XRP and of that asset, so that a payment crosses from one hub to the other through a member of both, which converts; `apps/dev/src/scenario/hubs.ts` checks a scenario's several hubs before it starts, one hub per asset ([token-hubs](../decisions/token-hubs.md)).
- **The market maker is parked** until it runs on hubs ([Roadmap](../roadmap.md#hubs-only)): its scenarios say so, and neither runs. `market-maker` is passive rebalancing on one machine ([Rate feed](rate-feed.md#market-maker)): a core node with XRP and DSUSD, most of its value in XRP, and two periphery nodes pinned to it, one with XRP and one with DSUSD, for `dassie market-maker` to trade with, beside an XRP/DSUSD pool the fake ledger's issuer makes at the scenario's start, priced at the dev rates, so the pool and the nodes agree until something trades ([XRPL](xrpl.md#pools-in-the-fake-ledger)). `market-maker-two-targets` is the same with two such core nodes, and the two periphery nodes each pinned to both, for a market maker that trades through several. Their nodes settle directly on the XRP Ledger, which is why they wait for the market maker on hubs.
- **Periphery nodes.** A scenario may set nodes up without a hostname, so they have no URL and are periphery nodes, each pinned to core nodes of the scenario through `DASSIE_UPLINKS` ([Network](network.md#pinned-uplinks)). A node list is served only by core nodes, so none is a bootstrap node; the scenario checks this before it starts. Without a hostname a node answers its owner only at `localhost` ([HTTP](http.md#same-origin-rule)), so the dashboard opens its wallet there, at the node's own port, rather than through the SNI proxy.
- Each scenario serves its own exchange rates, which the setup makes every node's only rate source, so no run depends on an outside rate service and tests can move the rates or make the source fail ([Exchange rates](connector.md#exchange-rates)).

## Dev identities

`apps/dev/vanity.json` holds each dev node's phrase secret and the node ID it gives in each realm, one entry for each node a dev environment may run (`d1` onward). A phrase names its realm and the realm keys the master seed, so a secret is one node in the test realm and another in the live realm, and a live scenario's nodes take their live identities ([phrase-format](../decisions/phrase-format.md)). Everything else, the phrase, node key, public key and login proof, is derived from the secret and the realm with `dassie-identity`, as the wallet derives it ([Identity](identity.md)), and checked against the table's ID for that realm. Every dev phrase records the first hour a phrase can, so no reader's clock refuses it as made in the future. A node ID cannot carry its node's short name, since base32 has no `0`, `1`, `8` or `9` ([Identity](identity.md#node-id)), so the dev server and the dashboard name a node by the table: they show each dev node's ID as its short name, in peerings, traffic and logs. `apps/dev/scripts/vanity.ts` checks the table, and writes its IDs again from the secrets when the derivation or the form of a node ID changes, which keeps every secret.

## Wallet links

The dashboard opens a node's wallet signed in with the node's login proof in the URL fragment, which never reaches a server. Only the wallet's development build reads it, and sends it as an ordinary login ([Wallet](wallet.md#dev-auto-login)); the node checks it like any other. Nothing in the node accepts a dev token.

## The dashboard's origin

The dashboard's page, its control channel and the relay to the nodes share one origin, so the dashboard keeps a CSP with `connect-src 'self'`, like the wallet's (without WebAssembly). The control channel and the relay act as the nodes' owner, so `apps/dev/src/server.ts` guards them:

- A request for any host but the dashboard's own names is refused, so a rebound DNS name cannot reach it.
- A WebSocket upgrade must come from the dashboard's own page (`Origin` exactly `https://<Host>`).

### Relay

The dashboard reaches a node only through the relay: a WebSocket from its page, passed message for message to the node's IPC socket, where the node takes every call as its owner's ([RPC](rpc.md)). The dashboard therefore talks to a node exactly as the wallet does, and the node exposes nothing for development.

### Control channel

`proto/dassie/dev/v1/control.proto` is the contract between the dev server and its dashboard: the dev server's state, the logs, peerings and traffic of every node, and the commands to choose a scenario, add a node, restart or reset. Nothing in the node accepts it.

### Logs, peerings and traffic

The dev server follows each running node once, over IPC, and passes what it gathers on to every open dashboard, so the number of dashboards open does not multiply the load on the nodes. Packets to a peer become hops on the dashboard's graph, each counted once.

## Dashboard

`apps/dev-dashboard` reuses the wallet's debug pages, log viewer, network graph, router and theme ([Shared components](wallet.md#shared-components-packagesui)), and shows a node's [debug pages](wallet.md#debug-pages) through the relay, under its own path. Its graph links two nodes only where each lists the other as its peer, as the wallet's network page does.

## Dev CLI

`node --run cli -- dN <args…>` (`apps/dev/cli.ts`) runs the `dassie` CLI against a dev node through its IPC socket.

## Builds

The node is rebuilt when `crates/` or `proto/` change, and a successful build restarts the nodes; after a failed build they keep running the binary built before. The wallet and the dashboard are Vite watch builds in development mode; a watch build does not empty its directory, so a reload during a rebuild still finds a page.

## Tooling is not simulated

The dev environment and the dashboard are tools around the node, not part of it, so they are not built for deterministic simulation: their pure parts have unit tests, and the rest is tested end to end against real binaries (`tests/e2e/dev_test.ts`). They are still bounded: rings, frames, queues and connections have limits.

## Decisions

- [rust-and-typescript](../decisions/rust-and-typescript.md): the dev environment is TypeScript; the nodes are the real Rust binary.
- [tigerstyle](../decisions/tigerstyle.md): tooling is not simulated, but bounded.
- [node-and-pnpm](../decisions/node-and-pnpm.md): Node and pnpm, `node --run`.
