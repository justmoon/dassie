# 1. Overview

## 1.1 What Dassie is

Dassie is a peer-to-peer payment network built on [Interledger](https://interledger.org). Each participant runs a **Dassie node**. A node:

- holds money on one or more underlying ledgers ("settlement schemes": a stub ledger for testing, the XRP Ledger testnet, and an XRP Ledger mainnet module that exists but is not wired up);
- discovers other nodes, peers with some of them, and keeps an account of what it owes and is owed by each peer;
- forwards Interledger packets between peers, local applications and ILP clients, converting between currencies along the way;
- settles net balances with each peer on the underlying ledger;
- serves a web wallet to its owner, from which the owner can send and receive payments addressed by payment pointer (`$hostname`).

The project's tagline is "a multi-currency, zero-config, peer-to-peer, Interledger-enabled payment network". The website describes it as "a peer-to-peer network which can transfer tiny amounts of money quickly and cheaply", and stresses that it is "not a blockchain": nodes keep bilateral balances and settle them on whichever ledgers they share.

The original is a TypeScript monorepo (Node.js, React, SQLite). It is a working prototype on a test network, not a production system: chapters 2–6 record 124 numbered gaps between what it does, what its design notes say, and what a real deployment needs.

## 1.2 Concepts

| Term | Meaning |
|---|---|
| **Node** | One running Dassie daemon, identified by an Ed25519 key. |
| **Node ID** | `d` + a base64url prefix of SHA-256 over the node's public key ([3 §2.2](03-peer-network.md)), e.g. `dNKbwOMCw0DTToPM4tyyVASWxM4NP3MKW`. |
| **Wallet passphrase** | A 12-word BIP-39 mnemonic. Everything secret derives from it: the node key, the login secret, and each settlement module's ledger keys. The browser derives the node key from it, and only the node key ever reaches the server. |
| **Realm** | `test` or `live`. It picks the ILP allocation scheme (`test.` or `g.`) and which settlement modules may run. |
| **ILP address** | Every node is `<scheme>.das.<nodeId>`. Clients of a node live beneath it: `<scheme>.das.<nodeId>.<suffix>`. |
| **Bootstrap Node List (BNL)** | The configured set of trusted-majority nodes (two test nodes by default). |
| **Known Node List (KNL)** | The node list a node serves to others: its majority list plus the nodes registered with it. |
| **Majority node list** | Nodes listed by a strict majority of the BNL nodes' KNLs. These are the network's top-level routable nodes. |
| **Registration** | A node asking a bootstrap node to add it to that node's KNL. The design calls for a fee; the original charges none. |
| **Link state** | A node's signed self-description: URL, alias, public key, neighbors and settlement schemes. Link states are flooded through the network and drive routing. |
| **Peer** | A node we have a bilateral relationship with: a settlement scheme, ledger accounts and a credit line. Packets travel only between peers. |
| **Settlement scheme** | A pluggable module (`stub`, `xrpl-testnet`, `xrpl`) that moves real or simulated value on an underlying ledger. |
| **Internal ledger** | A double-entry, TigerBeetle-style ledger with accounts named `<ledgerId>:<path>`, e.g. `xrpl-testnet+xrp:assets/interledger/<peer>`. |
| **Endpoint** | Anything the connector exchanges packets with: a peer, a BTP client, a local in-process endpoint, the ILDCP responder, or an ILP-over-HTTP client. |
| **Payment pointer** | `$host[/path]`, resolved to `https://host/.well-known/pay` (Open Payments style). |

## 1.3 System context

```
                         ┌────────────────────────────────────────────┐
  Owner's browser ──────▶│ HTTPS :443                                 │
  (web wallet, SPA)      │  /            SPA (wallet UI)              │
   wss /rpc  ◀──────────▶│  /rpc         WebSocket RPC (UI ↔ node)    │
   POST /api/{setup,     │  /api/*       setup / login / logout       │
        login,logout}    │                                            │
                         │  /peer        Dassie peer protocol (OER)   │◀──▶ other Dassie nodes
  ILP clients ──────────▶│  /btp         BTP 2.0 over WebSocket       │
  (e.g. ilp-plugin-btp)  │  /ilp         ILP-over-HTTP (async)        │
  Open Payments clients ▶│  /.well-known/pay, /incoming-payments      │
                         │                                            │
  ACME CA ──────────────▶│ HTTP :80      ACME http-01 + HTTPS redirect│
                         │                                            │
  `dassie` CLI ─────────▶│ /run/dassie.sock  local IPC RPC (root)     │
                         │                                            │
                         │ SQLite  <state>/dassie.sqlite3             │
                         └───────┬───────────────────────┬────────────┘
                                 │ wss (xrpl.js)         │ https
                                 ▼                       ▼
                         XRP Ledger (testnet)    Exchange-rate feed (Coinbase)
```

The node runs as one process, normally under systemd with socket activation on ports 80 and 443 and on the IPC socket. Releases are tarballs that bundle a Node.js runtime and the native SQLite binding. They are installed by a `curl | sh` installer to `/opt/dassie/<version>`, and a timer updates them nightly ([6 §5](06-user-interfaces-and-tooling.md)).

## 1.4 Life of a node

1. **Install.** `curl --tlsv1.2 -sSf https://sh.dassie.land | sh` downloads the bundle, installs the systemd units, starts the daemon and runs `dassie verify-install`.
2. **Configure.** `dassie init` (as a member of `dassie-users`, over the IPC socket) asks for a hostname and obtains a Let's Encrypt certificate through ACME http-01. It then prints a one-time **setup URL**: `https://<host>/setup/<token>`.
3. **Set up the wallet.** In the browser the owner generates a new 12-word passphrase or recovers an existing one. The browser derives the node key, proves it knows the login secret, and posts both with the setup token to `/api/setup`. The node stores the key and opens a session. From then on the node has an identity and starts its identity-dependent services: accounting, ILP, ledgers, Open Payments, the peer protocol and routing.
4. **Choose a ledger.** A node with no settlement scheme forces the owner to add one. The UI offers only `stub`; the XRPL testnet module can be added through the RPC.
5. **Join the network.** The node polls the bootstrap nodes for their node lists, adds majority nodes to its node table, and fetches their link states. It then peers automatically with random compatible nodes until it has two peers. To become globally routable, the owner registers the node with the bootstrap nodes from the Network page.
6. **Transact.**
   - *Receive*: others pay `$<hostname>`. The node's STREAM receiver accepts the money, and it is credited to the owner's equity.
   - *Send*: the owner enters a payment pointer and an amount. The node resolves the pointer, creates an incoming payment at the receiver, and streams the money over ILP.
   - *Forward*: packets between peers are routed by shortest hop count. Each hop places pending ledger transfers, which post on fulfill and void on reject or timeout.
7. **Settle.** Every 4 s, for each peer, the node checks whether its net debt exceeds the threshold. If it does, the node pays the excess on the shared ledger: an XRPL `Payment` with a memo, or a stub message. The receiving side credits the peer when it observes the payment.
8. **Operate.** Logs go to the journal. The web UI has debug pages for logs, the node table, ledger accounts, routes, reactive state and raw database tables. Updates arrive nightly through `dassie update`.

## 1.5 Feature inventory and maturity

| Area | Original status | Where specified |
|---|---|---|
| Node identity from BIP-39 mnemonic, browser-side derivation | Implemented | [2](02-node-runtime.md), [3 §2](03-peer-network.md), [6 §1.3](06-user-interfaces-and-tooling.md) |
| Setup, login, sessions, setup token | Implemented | [2](02-node-runtime.md) |
| HTTPS server, ACME (Let's Encrypt), HTTP→HTTPS redirect | Implemented | [2](02-node-runtime.md) |
| Local IPC RPC + `dassie` CLI (`init`, `daemon`, `update`, `verify-install`) | Implemented | [2](02-node-runtime.md) |
| WebSocket RPC for the UI | Implemented | [2](02-node-runtime.md) |
| Peer protocol over HTTPS (`POST /peer`, OER) | Implemented; sender authentication is weak | [3](03-peer-network.md) |
| Node discovery, BNL/KNL majority lists | Implemented; never removes nodes | [3 §11](03-peer-network.md) |
| Registration | Implemented, manual and free; the design calls for a fee | [3 §11.6](03-peer-network.md) |
| Auto-peering (minimum 2 peers) | Implemented; no termination | [3 §12](03-peer-network.md) |
| Link-state flooding and heartbeats | Implemented; no liveness detection | [3 §10](03-peer-network.md) |
| Shortest-hop routing | Implemented; prefix matching ignores segment boundaries | [3 §13](03-peer-network.md) |
| ILP connector (prepare/fulfill/reject, expiry, FX) | Implemented; 5 s internal timeout | [4 §4](04-interledger.md) |
| ILDCP | Implemented; always answers XRP/9 | [4 §6](04-interledger.md) |
| BTP server for ILP clients | Implemented; route-registration bug | [3 §14](03-peer-network.md), [4 §10](04-interledger.md) |
| ILP-over-HTTP | Inbound async only; outbound dead code | [4 §8](04-interledger.md) |
| STREAM (own library) | Implemented for money; no data streams or receipts | [4 §11](04-interledger.md) |
| Open Payments (wallet address, incoming payments) | Partial; no auth server, hard-coded amounts | [4 §9](04-interledger.md) |
| SPSP | Not implemented | [4 §9](04-interledger.md) |
| Exchange rates (Coinbase feed, no spread) | Implemented; loaded once | [4 §5](04-interledger.md) |
| Internal double-entry ledger | Implemented; persists only posted totals | [5 §2](05-accounting-and-settlement.md) |
| Settlement framework + stub module | Implemented | [5 §4, §6.1](05-accounting-and-settlement.md) |
| XRPL testnet settlement | Implemented; direct payments with no reserve handling | [5 §6.3](05-accounting-and-settlement.md) |
| XRPL mainnet settlement | Present but not registered | [5 §6.3](05-accounting-and-settlement.md) |
| Fees and spread | Not implemented | [5 §3](05-accounting-and-settlement.md) |
| Peering termination, banning, liveness | Design only | [3 §12](03-peer-network.md) |
| Token payment protocol | Design only | [5 §9](05-accounting-and-settlement.md) |
| Private UDP peer transport | Design only | [3 §18](03-peer-network.md) |
| Web wallet (React) | Implemented; USD-only display, stub-only ledger setup | [6 §1](06-user-interfaces-and-tooling.md) |
| Release build, installer, updater, systemd units | Implemented; unsigned artifacts, x64 only | [6 §5](06-user-interfaces-and-tooling.md) |
| Multi-node dev environment and dashboard | Implemented | [6 §2, §6](06-user-interfaces-and-tooling.md) |
| Website (Astro, STREAM simulator) | Implemented | [6 §3](06-user-interfaces-and-tooling.md) |

## 1.6 Design principles worth keeping

These come from the README, the design notes and the code's structure, and the rewrite should keep them:

- **Zero configuration.** One hostname and one passphrase are all a node needs; discovery, peering, addressing and settlement are automatic.
- **Ledger-neutral, currency-neutral.** Settlement is a plug-in interface. Anti-Sybil membership is deliberately independent of any ledger ([`network-membership.md`](03-peer-network.md#18-documentation-vs-implementation)).
- **Secrets stay with the owner.** The passphrase never leaves the browser. Every key is derived deterministically from it, so the same passphrase recovers the same node ID and the same on-ledger accounts.
- **HTTPS everywhere**, in development too (mkcert, `*.localhost`).
- **Self-contained releases.** One tarball carries its runtime and installs and updates itself. Releases bundle an exact runtime version on purpose.
- **Expected failures are values.** Library code returns `Failure` objects for expected errors rather than throwing. A Rust port maps this directly onto `Result`.
