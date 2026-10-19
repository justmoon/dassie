# Overview

What Dassie is, the words the rest of the spec uses, how a node fits into the world, and how the repository is laid out. Every agent reads this, together with [engineering](engineering.md).

## What Dassie is

Dassie is a peer-to-peer payment network built on [Interledger](https://interledger.org). Each participant runs a **Dassie node** on their own server. A node:

- holds money on one or more underlying ledgers, through **settlement schemes**: `stub`, which settles nothing and is for testing, and `xrpl-testnet`, the XRP Ledger testnet;
- finds other nodes through bootstrap nodes, peers with some of them on its own, and keeps its own books of what each peer owes it and what it owes each peer;
- forwards Interledger packets between peers, its own local endpoints and ILP clients, converting between currencies at rates it fetches;
- asks a peer to settle when the peer comes close to the credit it extends, and settles on the shared ledger when asked;
- serves its owner a web wallet, from which the owner sends and receives payments by payment pointer, wallet address or payment request, and serves [Open Payments](https://openpayments.dev) to other wallets and apps.

It is "not a blockchain": nodes keep bilateral balances and settle them on whichever ledgers they share. It is not a consensus system either. Each node keeps its own books and enforces its own limits from them, so whatever a peer does, what it can cost the node is bounded; agreement between nodes' books is not a goal ([engineering](engineering.md#each-node-keeps-its-own-books), [D19](decisions/D19-each-node-keeps-its-own-books.md)).

Dassie works with test money only. The `live` realm is refused until the conditions in the [roadmap](roadmap.md#real-money) are met ([D8](decisions/D08-real-money.md)). A new node joins the public testnet through its three bootstrap nodes, `boot1`–`boot3.dassie.xyz` ([D15](decisions/D15-bootstrap-network-for-v1.md)).

This is the second implementation. It replaced the original TypeScript proof of concept (tag `original`), whose behaviour [spec/original/](original/01-overview.md) records as a quick reference. The rewrite does everything the original does that works, with the same user-visible behaviour: install, `dassie init`, browser setup and login, ledgers, discovery, auto-peering, routing, forwarding, settlement, sending and receiving, the debug pages, the multi-node dev environment and the website's STREAM simulator. It also does what the original intended but did not deliver (see [Compatibility](#compatibility-with-the-original)), and is tested at every level ([Testing](areas/testing.md)).

## Concepts

| Term | Meaning |
|---|---|
| **Node** | One running Dassie daemon, identified by an Ed25519 key ([Identity](areas/identity.md)). |
| **Node ID** | `d` + a base64url prefix of SHA-256 over the node's public key, e.g. `dNKbwOMCw0DTToPM4tyyVASWxM4NP3MKW`. The node's network identity. |
| **Passphrase** | A 12-word BIP-39 mnemonic. Everything secret derives from it: the node key, the login secret, and each settlement scheme's ledger keys. The browser derives the node key; only the node key reaches the node. The same passphrase recovers the same node ID and the same on-ledger accounts. |
| **Owner** | The person who set the node up with their passphrase and logs in to its wallet. |
| **Realm** | `test` or `live`. It picks the ILP allocation scheme (`test` or `g`) and which settlement schemes may run. Only `test` is enabled. |
| **ILP address** | Every node is `<scheme>.das.<nodeId>`, e.g. `test.das.dNKb…`. The node's own endpoints and clients live beneath it. |
| **Bootstrap node list (BNL)** | The configured bootstrap nodes, which serve node lists and take registrations. By default `boot1`–`boot3.dassie.xyz`; `DASSIE_BOOTSTRAP_NODES` replaces it ([Network](areas/network.md)). |
| **Known node list (KNL)** | The list a bootstrap node serves: its unexpired registrations and itself. |
| **Majority** | Nodes listed by strictly more than half of the *configured* bootstrap nodes. They are the network's members. |
| **Registration** | A node asking a bootstrap node to list it. It lasts 24 h and is renewed every 6 h, automatically. It is free for now ([roadmap](roadmap.md#registration-fees-and-child-peers)). |
| **Link state** | A node's signed self-description (URL, alias, public key, neighbors, settlement schemes), flooded through the network. Routing runs on link states. |
| **Peer** | A node we have a peering with: a shared settlement scheme, ledger accounts, a credit line and a settlement channel. Packets travel only between peers. |
| **Junior, senior** | The junior is the node whose peering request was accepted, the senior the node that accepted it. The junior extends credit at first, and the senior none ([Network](areas/network.md), [D20](decisions/D20-trust-follows-what-a-peer-earned-us.md)). |
| **Credit** | What a node lets a peer owe it. A node knows only the credit it extends, never the credit it is extended. Credit grows with the fees a peer earned the node ([Settlement](areas/settlement.md)). |
| **Largest packet** | The most one packet may carry on a ledger (0.1 units by default). A link's thresholds are counted in it: the most in flight each way (2.5), the trigger to ask for settlement (1.5) and the least settlement (1) ([D25](decisions/D25-a-links-thresholds-are-counted-in-its-largest-packet.md)). |
| **Settlement scheme** | A pluggable module that moves value on an underlying ledger: `stub` or `xrpl-testnet` ([Settlement](areas/settlement.md), [XRP Ledger](areas/xrpl.md)). |
| **Settlement channel** | What a node keeps per node it ever peered with to deliver settlements exactly once: sequences sent and applied, and the message not yet acknowledged. |
| **Internal ledger** | The node's double-entry ledger, with accounts named `<ledgerId>:<path>`, e.g. `xrpl-testnet+xrp:assets/interledger/<peer>`, and an append-only transfer journal ([Ledger](areas/ledger.md)). |
| **Owner's funds** | Per ledger, the pool `equity/owner` and its group: the owner's sub-accounts and what the node gains or loses on that ledger. Everything the node holds beyond what it owes peers. |
| **Endpoint** | Anything the connector exchanges packets with: a peer, a BTP client, or a local endpoint such as the STREAM receiver or payer ([Connector](areas/connector.md)). |
| **Wallet address** | An Open Payments wallet address: `https://<host>/pay/<scheme>` per ledger; `https://<host>/.well-known/pay` names the default one ([Payments](areas/payments.md)). |
| **Payment pointer** | Input syntax only: `$host/path` means `https://host/path`, and a bare `$host` means `https://host/.well-known/pay`. |

## System context

```
                         ┌──────────────────────────────────────────────────────────┐
  Owner's browser ──────▶│ HTTPS :443                                               │
  (wallet, Solid SPA)    │  /                 the wallet (static assets)            │
   wss /rpc ◀───────────▶│  /rpc              RPC over WebSocket (proto/dassie/ui)  │
   POST /api/{setup,     │  /api/*            setup, login, logout                  │
        login,logout}    │  /stats            version and advertised peers (public) │
                         │                                                          │
  Other Dassie nodes ◀──▶│  /peer             peer protocol v1 (OER, POST)          │
  ILP clients ──────────▶│  /btp              BTP 2.0 over WebSocket                │
  (e.g. ilp-plugin-btp)  │                                                          │
  Open Payments ────────▶│  /.well-known/pay, /pay/<scheme>   wallet addresses      │
  clients and wallets    │  /gnap…            GNAP authorization server             │
                         │  /incoming-payments, /quotes, /outgoing-payments,        │
                         │  /outgoing-payment-grant           resource server       │
                         │                                                          │
  ACME CA ──────────────▶│ HTTP :80   ACME http-01, else redirect to HTTPS          │
                         │                                                          │
  `dassie` CLI ─────────▶│ IPC socket /run/dassie.sock   RPC, length-prefixed       │
                         │                                                          │
                         │ SQLite  <state>/dassie.sqlite3                           │
                         └──────┬──────────────────┬───────────────────┬────────────┘
                                │ wss              │ https             │ https
                                ▼                  ▼                   ▼
                         XRP Ledger testnet   Rate sources       Other nodes' Open
                         (server, faucet)     (Coinbase, Kraken, Payments servers
                                              Bitstamp)          (the node as client)
```

- **One Rust binary**, `dassie`, is the daemon and the CLI, with the wallet's built assets embedded. Under systemd it is socket-activated on ports 80 and 443 and on the IPC socket; standalone, it binds the configured ports and `$XDG_RUNTIME_DIR/dassie.sock` ([Node](areas/node.md)).
- The HTTPS route table, the routes that exist only in some states (`/api/setup` only before setup, `/api/login` and `/api/logout` only after), security headers and Origin/Host checks are in [HTTP](areas/http.md). The wallet's consent page for Open Payments grants, `/consent/{id}`, is a wallet route.
- ILP-over-HTTP (RFC 35) is part of the compatibility contract below, but the node serves no `/ilp` route ([roadmap](roadmap.md#not-built-yet)).

## Life of a node

1. **Install.** On a Linux server with systemd, `curl --proto =https --tlsv1.2 -sSf https://sh.dassie.land | sh` fetches the newest release from `get.dassie.land`, verifies its signature before unpacking anything, installs it to `/opt/dassie/<version>` with `/opt/dassie/current` pointing at it, sets up and starts the systemd units, and adds the user to the group `dassie-users`, whose members may use the IPC socket. `dassie verify-install` checks the result ([Release](areas/release.md)).
2. **Configure.** `sudo dassie init`, over the IPC socket, asks for the hostname and an email address. The daemon obtains a Let's Encrypt certificate through ACME http-01, streaming its progress to the CLI. `dassie init` then prints a one-time **setup URL**, `https://<host>/setup/<token>`; `dassie setup-url` prints it again, and it survives restarts until setup is done ([HTTP](areas/http.md)).
3. **Set up the wallet.** In the browser, the owner creates a new passphrase or enters an existing one. The browser derives the node key with the same code the node uses (WASM), proves it knows the login secret, and posts both with the setup token to `/api/setup`. The node stores the key and opens a session. Its identity-dependent services start: the ledger, the connector, Open Payments, the peer protocol and routing.
4. **Choose a ledger.** The wallet asks for a first settlement scheme: `stub`, which gives 100 test USD, or `xrpl-testnet`, whose account the testnet faucet funds.
5. **Join the network.** The node polls the bootstrap nodes for their lists, registers with each, keeps the majority nodes in its node table and fetches their link states. It peers on its own with nodes that share a settlement scheme, until it has two confirmed peers ([Network](areas/network.md)).
6. **Transact.**
   - *Receive:* other wallets pay the node's wallet addresses through Open Payments; the STREAM receiver takes the money into the owner's funds.
   - *Send:* the owner's Send quotes and pays a payment pointer, wallet address or payment request, here or on another node ([Payments](areas/payments.md)).
   - *Forward:* packets between peers follow the shortest routes. Each hop books pending transfers, posted on Fulfill and voided on Reject or expiry, and may charge the owner's fee and spread ([Connector](areas/connector.md)).
   - *Clients:* BTP clients and third-party Open Payments clients spend within the limits the owner set.
7. **Settle.** Every 4 s, a node asks each peer whose position has fallen to its trigger to settle (`settlementRequest`). Asked, the debtor settles half the way to its own trigger, on the shared ledger. Settlements are delivered exactly once ([Settlement](areas/settlement.md)).
8. **Operate.** Logs go to the journal and to a ring the wallet's debug pages read. Once a day `dassie-update.timer` runs `dassie update`, which verifies the next release of the node's channel, switches to it and rolls back if the node does not come up ([Release](areas/release.md)).

## How the code is split

The node is Rust, the browser is Solid 2, and the tooling is Node ([D1](decisions/D01-what-runs-in-rust-and-what-in-typescript.md), [D21](decisions/D21-tooling-on-node-and-pnpm-not-deno.md)).

- **Rust:** the daemon, the CLI and every protocol. The result is one static binary per platform (x64 and arm64, musl), with no bundled interpreter or native add-on.
- **WASM:** the cryptographic and protocol cores (`dassie-identity`, `dassie-oer`, `dassie-ilp`, `dassie-stream`) are also built to WASM, one `web` build for browsers and Node. The wallet derives keys with exactly the code the node uses, and the website's STREAM toy runs the real STREAM implementation.
- **Solid 2:** the wallet, which the node serves, and the dev dashboard.
- **Node:** everything that orchestrates or builds: the multi-node dev environment and its dashboard server, the Vite builds, the website, the release scripts, and the interop and end-to-end tests. No release contains Node.

## Workspace layout

```
crates/
  dassie-oer/          X.696 OER, the subset the protocols use                       (+ wasm)
  dassie-identity/     BIP-39, seed paths, Ed25519/X25519, node ID, login secret       (+ wasm)
  dassie-ilp/          ILPv4 packets, addresses, timestamps, errors, ILDCP, BTP codecs (+ wasm)
  dassie-stream/       STREAM: sans-IO connection and server state machines, crypto   (+ wasm)
  dassie-ledger/       internal double-entry ledger, transfer journal
  dassie-settlement/   settlement scheme trait, host effects, settlement policy, stub scheme
  dassie-xrpl/         XRP Ledger: wallets, Payment codec, rippled API, xrpl-testnet scheme, fake ledger
  dassie-peer/         peer protocol v1: wire, authentication, node table, link state,
                       membership, registration, peering lifecycle, settlement delivery, routing
  dassie-connector/    ILP connector: endpoints, route table, ILDCP, FX, fees and spread,
                       spending caps, money in flight, packet trace
  dassie-payments/     Open Payments without IO: resources, GNAP, HTTP message signatures,
                       payment pointers, intervals, quote math
  dassie-node-core/    the node's decisions without IO: config, setup, sessions, origins,
                       rate limits, ACME and TLS policy; the seeded RNG and sim outcomes
  dassie-node/         the daemon: database, HTTP(S) and IPC listeners, TLS, ACME, login,
                       RPC, the engine, peer client, BTP server, rate fetcher, log ring
  dassie-cli/          the `dassie` binary: daemon | init | status | setup-url | update | verify-install
  dassie-release/      versions, channels, the mirror's layout, SHA256SUMS and its minisign signature
  dassie-sim/          runs the simulations from any seed; a developer tool, never released
proto/
  dassie/ui/v1/        UI and CLI RPC contract (normative; Rust and TypeScript generated from it)
  dassie/dev/v1/       the dev environment's control channel
apps/
  wallet/              the wallet (Solid 2), embedded in the binary
  dev/                 `node --run dev`: scenarios on real binaries, SNI proxy, certs, rates server
  dev-dashboard/       the dev dashboard (Solid 2)
  website/             dassie.land, the Quick Start, the STREAM toy, the architecture page
packages/
  rpc/                 TypeScript client for proto/dassie/ui/v1, over WebSocket or IPC
  ui/                  Solid components shared by the wallet, the dashboard and the STREAM toy
  identity-wasm/       the WASM builds of the core crates, with a STREAM driver
tests/
  e2e/                 end-to-end tests on real binaries, install tests, website test
  interop/             STREAM against ilp-protocol-stream, WASM vectors, browser test
testdata/              golden-vector generators (original/, xrpl/), Pebble config, release test key
resources/             systemd units, install.sh, minisign.pub (the release key)
nix/                   release.nix (release builds), pnpm.nix (wallet packages), registry mirror
scripts/               build-wasm.ts, release.ts, nix-cloud-build.sh, spec_test.ts (checks the spec's links)
```

Which area doc covers which path is in [spec/README.md](README.md). Where the original's packages went:

| Original | Now |
|---|---|
| `lib-oer` | `dassie-oer`, correct per X.696 |
| `lib-protocol-ilp`, `-ildcp`, `-utils` (BTP) | `dassie-ilp` |
| `lib-protocol-stream` | `dassie-stream`; also replaces `ilp-protocol-stream` for Open Payments |
| `lib-reactive`, `-reactive-io`, `-reactive-rpc` | supervised tokio tasks in `dassie-node`; Solid 2 signals in the browser |
| `lib-rpc` | `proto/dassie/ui/v1` and generated code |
| `lib-http-server` | hyper and rustls in `dassie-node` |
| `lib-sqlite` | rusqlite with STRICT tables, migrations by `user_version` |
| `lib-logger` | `tracing`, with the same `das:node:*` targets and `DEBUG` patterns |
| `lib-x509` | `rcgen` and `pkcs8`/`ed25519-dalek` |
| `app-dassie` | `dassie-node`, `dassie-cli` and the protocol crates |
| `gui-dassie` | `apps/wallet` and `packages/ui` |
| `gui-dev`, `app-dev` | `apps/dev-dashboard`, `apps/dev` (spawns real `dassie` binaries) |
| `app-build` | the Nix flake and `scripts/release.ts` ([D14](decisions/D14-build-system.md)) |
| `app-website` | `apps/website` |
| `app-cli` (npm placeholder) | dropped; the npm name stays reserved |
| `meta-*` | dprint, oxlint, tsc, `cargo fmt`/`clippy`/`test` |

## Compatibility with the original

**Every gap of the original is fixed**, unless it is kept on purpose (below) or a decision says otherwise. Fixed means: what the original's design notes or code comments say was intended; or, where there is no stated intent, the governing standard (X.696, ILPv4, RFC 29 STREAM, RFC 31 ILDCP, RFC 23 BTP, RFC 35 ILP-over-HTTP, Open Payments) or the obvious safe behaviour.

What stays compatible with the original, and why:

| Surface | Compatible? | Why | Stated in |
|---|---|---|---|
| Passphrase → seed → node key → login secret → node ID, including stripping trailing `A`s from the ID | **Byte for byte** | Owners recover their node with the same passphrase; the node ID is the network identity. | [Identity](areas/identity.md) |
| Seed-path chain (`node`, `node/login`, `node/settlement-module/<scheme>/<path>`) and the XRPL wallet (first 16 bytes of entropy as an ed25519 family seed) | **Byte for byte** | The same passphrase finds the same XRPL account and its funds. The rewrite's own paths, `node/stream-server/<ledger slug>` (the STREAM receivers' secrets) and `node/open-payments-client` (the key the node signs Open Payments requests with), hang off the same chain, so they too follow from the passphrase and never change. | [Identity](areas/identity.md#derivation-chain), [XRP Ledger](areas/xrpl.md) |
| Ed25519 verification accepts ZIP-215 | Kept | Being stricter could split the network on edge-case signatures. Small-order keys are refused for link states. | [Identity](areas/identity.md), [Peer protocol](areas/peer-protocol.md) |
| ILPv4 packets, error codes, timestamps | **Byte for byte** | Interledger standard; the parse quirks are fixed. | [ILP](areas/ilp.md) |
| ILDCP | Format | Standard; answers each endpoint's real asset. | [ILP](areas/ilp.md), [Connector](areas/connector.md) |
| STREAM | Per RFC 29 | Interoperates with `ilp-protocol-stream`; unknown frames are skipped, not refused. The original's AIMD concurrency and redistributed over-allocation stay: compatible on the wire. Amounts are `u64`, not the original's bigints: larger wire values saturate at `2^64 − 1`, as RFC 29 asks. | [STREAM](areas/stream.md) |
| BTP 2.0 | **Byte for byte** | `ilp-plugin-btp` connects with `btp+wss://:<token>@host/btp`. | [ILP](areas/ilp.md), [Connector](areas/connector.md) |
| ILP-over-HTTP | Per RFC 35, with synchronous mode | Standard; the original's was a stub. Not served: the node has no `/ilp` route ([roadmap](roadmap.md#not-built-yet)). | [Connector](areas/connector.md) |
| Open Payments | Per the specs, v1.4.0; SPSP is dropped | Other wallets pay `$host`; third-party clients act with the owner's consent. | [Payments](areas/payments.md) |
| XRPL memo type and `settlement` semantics | Within one network version | Both sides of a peering must agree. | [XRP Ledger](areas/xrpl.md), [Settlement](areas/settlement.md) |
| Peer protocol | **No**: v1 keeps the OER message set and fixes authentication | v0 did not authenticate senders. | [Peer protocol](areas/peer-protocol.md) |
| OIDs, `application/dassie-message`, `POST /peer` | Yes, envelope `version` 1 | A v1 node refuses a v0 message cleanly (400). | [Peer protocol](areas/peer-protocol.md) |
| Majority: strictly more than half of the configured bootstrap nodes | Kept | What the original's code does; its doc's "N/2 + 1" is the outlier. | [Network](areas/network.md) |
| Gossip defaults: suppress after 3 copies, 1–500 ms forward jitter, 5–20 s heartbeats, 3 h staleness, 1 h re-issue | Kept, configurable | Sound defaults. | [Network](areas/network.md) |
| Default credit line 1.1 units, largest packet 0.1 units | Kept, one value for the whole node | Test-network values. Only the junior extends the credit line at first. Per-ledger values are not built ([roadmap](roadmap.md#not-built-yet)). | [Settlement](areas/settlement.md), [Connector](areas/connector.md#bounds) |
| UI ↔ node RPC | **No** | Both ends were rewritten; a typed contract replaces `dassie-rpc-01`. | [RPC](areas/rpc.md) |
| `/api/setup`, `/api/login`, `/api/logout`, the `__Host-Dassie-Session` cookie (HttpOnly, Secure, SameSite=Strict), the one-time setup token, routes only in the right state | Semantics, not bodies | Same flow and security properties. Unlike the original's, sessions expire and the setup token survives restarts. | [HTTP](areas/http.md) |
| SQLite schema | **No** | A new schema with application ID `0x7DA5E2C9` (the original's was `0x1DA53B81`); a v0 database is refused. | [Node](areas/node.md) |
| Install layout, systemd units and socket activation, socket names, `dassie` CLI verbs, env var names, `curl \| sh` installer, nightly update timer | Yes, where sensible | Operators' muscle memory and docs; meaningless variables (`NODE_ENV`) are dropped. Releases are signed. | [Node](areas/node.md), [Release](areas/release.md) |
| Mirror layout `get.dassie.land/meta/latest`, `/<ver>/dassie-<ver>-linux-<arch>.tar.*` | Yes, plus signatures | Installer and updater URLs keep working. | [Release](areas/release.md) |

Not compatible on purpose: v1 nodes do not talk to v0 nodes ([D2](decisions/D02-peer-protocol-v0-or-v1.md)), and the node does not read v0 databases. A v0 owner recovers by entering their passphrase in a new node: identity, node ID and on-ledger accounts all derive from it; peerings, ledger totals and BTP tokens do not carry over ([D3](decisions/D03-database-continuity.md)).

## Principles kept from the original

- **Zero configuration.** A hostname and a passphrase are all a node needs; discovery, peering, addressing and settlement are automatic.
- **Ledger-neutral, currency-neutral.** Settlement is a plug-in interface, and membership does not depend on any ledger.
- **Secrets stay with the owner.** The passphrase never leaves the browser, and every key derives from it.
- **HTTPS everywhere,** in development too (mkcert, `*.localhost`).
- **Self-contained releases** that install and update themselves: a static binary with no runtime inside.
- **Expected failures are values** (`Result`), never panics or exceptions.

## Decisions

- [D1](decisions/D01-what-runs-in-rust-and-what-in-typescript.md): the node is Rust, the browser Solid, the tooling TypeScript.
- [D2](decisions/D02-peer-protocol-v0-or-v1.md), [D3](decisions/D03-database-continuity.md): no v0 peer protocol, no v0 databases.
- [D8](decisions/D08-real-money.md): no real money yet.
- [D15](decisions/D15-bootstrap-network-for-v1.md): the testnet's bootstrap nodes.
- [D19](decisions/D19-each-node-keeps-its-own-books.md): each node keeps its own books.
- [D21](decisions/D21-tooling-on-node-and-pnpm-not-deno.md): tooling on Node and pnpm.

## Original

- [1](original/01-overview.md): the original's overview, concepts, system context and feature inventory.
