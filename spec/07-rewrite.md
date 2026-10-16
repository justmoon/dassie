# 7. The rewrite: target stack, architecture and plan

Chapters 1–6 describe what the original Dassie does. This chapter describes what the new implementation is: which behaviour it keeps, how it maps onto the Vorstand stack (Rust, Node, Solid 2), and in what order to build it. The choices that were open are recorded, with their reasons, in [chapter 8](08-decisions.md).

## 1. Goals and non-goals

**Goals**

- Everything the original does that works, with the same user-visible behaviour: install, `dassie init`, browser setup and login, ledgers, discovery, auto-peering, routing, forwarding, settlement, sending and receiving, the debug pages, the multi-node dev environment, and the website's STREAM simulator.
- The behaviour the original *intends* but does not deliver. That covers every gap in chapters 2–6 that is a plain bug or TODO (default disposition: fix, see [8 §1](08-decisions.md)).
- One self-contained release artifact per platform, with no bundled interpreter or native add-on, built for x64 and arm64.
- Tests at every level. The original has none for the app, the GUI or the tooling (UI-17).
- Each node protects itself. It keeps its own books and enforces its own limits from them, so whatever a peer does, what it can cost the node is bounded. Agreement between nodes' books is not a goal: the network is not a consensus system ([D19](08-decisions.md#d19-each-node-keeps-its-own-books)).
- Code built for deterministic simulation testing from the first crate: deterministic, bounded, asserted, instrumented and cheap to run, in a pragmatic TigerStyle ([D16](08-decisions.md#d16-engineering-style-pragmatic-tigerstyle-and-deterministic-simulation)). The rewrite will be built mostly by agents, and its confidence comes from exercising the code at scale.

**Non-goals for the first release**

- Interoperating with running v0 nodes on the peer protocol ([D2](08-decisions.md#d2-peer-protocol-v0-or-v1)).
- Reading v0 databases ([D3](08-decisions.md#d3-database-continuity)). A v0 owner recovers by entering their passphrase in the new node: identity, node ID and on-ledger accounts all derive from it (§2).
- The design-only features: the token payment protocol, the private UDP transport, and signed or independent frontends. They stay future work, and the architecture should not preclude them.

## 2. Compatibility contract

| Surface | Keep byte-for-byte? | Why |
|---|---|---|
| Passphrase → seed → node key → login secret → node ID ([3 §2](03-peer-network.md), [2 §8](02-node-runtime.md)) | **Yes** | Owners recover their node with the same passphrase. The node ID is the node's network identity. |
| Settlement-module entropy paths and XRPL wallet derivation ([5 §4.5, §6.3.1](05-accounting-and-settlement.md)) | **Yes** | The same passphrase must find the same XRPL account and its funds. |
| ILPv4 packets, error codes, Interledger timestamps ([4 §1](04-interledger.md)) | **Yes** | Interledger standard. Fix the parse quirks (`triggeredBy` may be empty, ILP-3). |
| ILDCP ([4 §6](04-interledger.md)) | **Yes**, format | Standard. Answer each endpoint's real asset (ILP-7). |
| STREAM ([4 §11](04-interledger.md)) | **Yes**, per RFC 29 | Must interoperate with `ilp-protocol-stream` and other STREAM implementations. Skip unknown frames instead of rejecting them (ILP-11). |
| BTP 2.0 ([3 §14](03-peer-network.md), [4 §10](04-interledger.md)) | **Yes** | Existing ILP clients (`ilp-plugin-btp`) connect with `btp+wss://:<token>@host/btp`. |
| ILP-over-HTTP ([4 §8](04-interledger.md)) | Per RFC 35, adding synchronous mode | Standard. The original's version is a stub. |
| Open Payments ([4 §9](04-interledger.md)) | Per the specs, v1.4.0: wallet address, GNAP authorization server, resource server, HTTP message signatures | Other wallets resolve `$host` and pay it, and third-party clients use the node with the owner's consent. SPSP is dropped ([D9](08-decisions.md#d9-open-payments-fully-and-no-spsp)). |
| XRPL settlement memo type and the `settlement` message semantics | Yes within one network version | Both sides of a peering must agree; it changes only with the peer protocol version. |
| Dassie peer protocol ([3 §5–§8](03-peer-network.md)) | **No**: v1 keeps the OER message set and fixes authentication | v0 does not authenticate senders (NET-8). The network is a two-node test network. [D2](08-decisions.md#d2-peer-protocol-v0-or-v1). |
| OIDs, the `application/dassie-message` media type, `POST /peer` | Yes, with the envelope `version` byte set to 1 | Lets a v1 node recognise and refuse a v0 message cleanly (400 "unsupported version"). |
| UI ↔ node RPC (`dassie-rpc-01`, superjson) ([2 §10](02-node-runtime.md)) | **No** | Both ends are rewritten. A typed contract replaces it ([D5](08-decisions.md#d5-ui-and-cli-rpc-contract)). |
| `/api/setup`, `/api/login`, `/api/logout`, `__Host-Dassie-Session` cookie | Semantics yes, bodies may change | Same flow and security properties. |
| SQLite schema, `application_id 0x1DA53B81`, scalar names ([2 §5](02-node-runtime.md)) | **No** | A new schema designed for what must survive a restart (§5). It may use a new application ID. |
| Install layout, systemd units, socket names, `dassie` CLI verbs, env var names ([2 §3, §12](02-node-runtime.md)) | **Yes**, where sensible | Operators' muscle memory and docs. Drop variables that no longer mean anything (`NODE_ENV`). |
| Mirror layout `get.dassie.land/meta/latest`, `/<ver>/dassie-<ver>-linux-<arch>.tar.*` | Yes, plus signatures | So the installer and updater URLs keep working ([D11](08-decisions.md#d11-release-integrity)). |

## 3. How the original maps onto the stack

Vorstand's conventions apply throughout: a Cargo workspace in `crates/` (edition 2024, `rust-toolchain.toml` pinned, `Cargo.lock` committed); a pnpm workspace in `apps/`, `packages/` and `tests/` (`pnpm-workspace.yaml`, a `package.json` per package, `pnpm-lock.yaml` committed, pre-release npm packages pinned exactly, `solid-js@2.0.0-rc.x` with `@solidjs/web` and Vite through `@solidjs/vite-plugin`); and normative contracts in a top-level directory that both sides are generated from. Packages meet only through their exports.

**The split ([D1](08-decisions.md#d1-what-runs-in-rust-and-what-in-typescript)): the node is Rust, the browser is Solid, and the tooling is Node ([D21](08-decisions.md#d21-tooling-on-node-and-pnpm-not-deno)).**

- The daemon, the CLI and every protocol are Rust. The result is one static binary (`dassie`) with the wallet's built assets embedded. It replaces the bundled Node.js, the esbuild bundle and the `better-sqlite3` prebuilds, along with the stale-hash build problem (UI-10) and the x64-only restriction.
- The cryptographic and protocol cores (`dassie-identity`, `dassie-oer`, `dassie-ilp`, `dassie-stream`) are also built to WASM. This follows the same pattern as Vorstand's `vault-core`:
  - the wallet derives keys in the browser with exactly the code the node uses;
  - the website's STREAM simulator runs the real STREAM implementation.
- Node runs everything that orchestrates or builds: the multi-node dev environment and its dashboard server, the Vite builds of the Solid apps, the website, release scripts, and interop tests against the original.

### 3.1 Workspace layout

```
crates/
  dassie-oer/           X.696 OER subset used by Dassie, correct per spec (fixes ILP-12, OER-q1..3)
  dassie-identity/      BIP-39, seed paths, Ed25519/X25519, node ID, login secret      (+ wasm feature)
  dassie-ilp/           ILPv4 packets, timestamps, error codes, F08, ILDCP, BTP codecs  (+ wasm)
  dassie-stream/        STREAM: sans-IO connection/stream state machines, crypto        (+ wasm)
  dassie-ledger/        internal double-entry ledger, transfer journal
  dassie-settlement/    SettlementScheme trait, host interface, stub module
  dassie-xrpl/          XRPL module: binary codec (Payment+Memos), signing, WebSocket client
  dassie-peer/          peer protocol v1, node table, link state, membership, peering, routing
  dassie-connector/     ILP connector pipeline, endpoints (peer/BTP/local/HTTP/ILDCP), FX
  dassie-payments/      Open Payments, sans-IO: resources, GNAP, HTTP message signatures, intervals, quotes
  dassie-node-core/     the node's decisions without IO: config, setup, sessions, rate limits, ACME/TLS policy
  dassie-node/          daemon composition: config, DB, HTTP(S)/IPC listeners, TLS, ACME, auth, RPC
  dassie-cli/           the `dassie` binary: daemon | init | status | setup-url | update | verify-install
  dassie-release/       versions, channels, the mirror's layout, SHA256SUMS and its minisign signature (sans-IO)
proto/
  dassie/ui/v1/         UI and CLI RPC contract (normative; Rust and TS generated from it)
  dassie/dev/v1/        dev-environment control contract
apps/
  wallet/               Solid 2 SPA served by the node (chapter 6 §1)
  dev/                  Node: multi-node dev environment, SNI proxy, certs, scenarios (6 §6)
  dev-dashboard/        Solid 2 dev dashboard (6 §2)
  website/              dassie.land static site + STREAM toy (6 §3)
packages/
  rpc/                  TS client for proto/dassie/ui/v1 (generated + transport)
  ui/                   shared Solid components: amount, log viewer, network graph, debug pages, router, live data
  identity-wasm/        wasm-bindgen output of dassie-identity / dassie-stream, one web build for browser + Node
resources/
  systemd/  installer/  launcher-less (the binary is the entry point)
  minisign.pub          the release key: releases must verify with it, and from M8b the binaries and the installer trust it (D11)
nix/release.nix         the release builds: wallet, static binaries, bundles, the mirror tree (§4.8)
scripts/release.ts      signs a built release for the mirror
```

### 3.2 Original package → new home

| Original | New | Notes |
|---|---|---|
| `lib-oer` | `crates/dassie-oer` | Implement X.696 correctly. Golden vectors come from the original's fixtures ([4 §1.9, §6.1, §11.2](04-interledger.md)). |
| `lib-protocol-ilp`, `-ildcp`, `-utils` (BTP) | `crates/dassie-ilp` | |
| `lib-protocol-stream` | `crates/dassie-stream` | Keep the original's API ideas: AIMD concurrency, bigint amounts, failures as values, explicit `measureExchangeRate`. Close the RFC gaps (ILP-11). Also replaces the upstream `ilp-protocol-stream` that Open Payments used. |
| `lib-reactive`, `-reactive-io`, `-reactive-rpc` | tokio task tree in `dassie-node` (§4), and Solid 2 signals in the browser | Solid 2's reactive primitives are the natural home for the UI half of `lib-reactive`. |
| `lib-rpc` | `proto/dassie/ui/v1` + generated code | |
| `lib-http-server` | `hyper`/`axum` + `rustls` in `dassie-node` | Keep scope-bound route registration semantics (§4.3). |
| `lib-sqlite` | `rusqlite` with STRICT tables, migrations by `user_version` | Keep the application-ID and schema-check ideas. |
| `lib-logger` | `tracing` with the same `das:node:*` targets and `DEBUG` patterns | `DASSIE_LOG_LEVEL` keeps working. |
| `lib-x509` | `rcgen` (self-signed) + `pkcs8`/`ed25519-dalek` (key PEM) | Fixes the serial and time encoding bugs (RT-14). |
| `lib-terminal-graphics` | Rust TUI helpers in `dassie-cli` (e.g. `cliclack`/`inquire`-style prompts) | Same prompts and notes as `dassie init` ([2 §4.5](02-node-runtime.md)). |
| `app-dassie` | `crates/dassie-node`, `dassie-cli` and the protocol crates | |
| `gui-dassie` | `apps/wallet` (Solid 2) + `packages/ui` | No React, wouter, TanStack Query or Radix. Use Solid 2 primitives and the Vorstand UI conventions where they fit. |
| `gui-dev` | `apps/dev-dashboard` | |
| `app-dev` | `apps/dev` (Node) | Spawns real `dassie` binaries instead of Vite SSR module runners. |
| `app-build` | Nix flake ([D14](08-decisions.md#d14-build-system)) and Node release scripts; `cargo build --release` for musl x64/arm64 | Sign artifacts ([D11](08-decisions.md#d11-release-integrity)). |
| `app-website` | `apps/website` | Content from [6 §3](06-user-interfaces-and-tooling.md). The STREAM toy uses `dassie-stream` WASM. |
| `app-cli` (npm placeholder) | dropped | Keep the npm name reserved. |
| `meta-*` | `dprint`, `oxlint`, `tsc`, `cargo fmt/clippy/test` | |

## 4. Node architecture (Rust)

### 4.1 Runtime model

The original is a tree of reactive actors that re-run when signals they read change ([2 §18.1](02-node-runtime.md)). The port keeps the **observable behaviour** of that model without reproducing the framework:

- **Services are supervised tokio tasks in a tree.** Each one owns a `CancellationToken` and a LIFO cleanup stack. Unlike the original, where an error stops an actor permanently (RT-24), a failed service is restarted with backoff and reported. If a service cannot start, the daemon exits non-zero instead of running half-started.
- **Shared state lives in versioned cells (`tokio::sync::watch`).** Covers the config, node table, peers set, active settlement schemes, routing table, exchange rates and ledger. A service that the original restarted on a signal change instead `select!`s on `changed()` and reconfigures itself.
- **Stores that the UI replicates also broadcast change events.** These are the node table and the log buffer. The UI gets a snapshot followed by deltas ([D5](08-decisions.md#d5-ui-and-cli-rpc-contract)).
- **Timers use an injectable `Clock`, and randomness an injectable RNG.** As in the original (`lib-reactive-io`, mock clock and deterministic crypto), this lets tests simulate multi-node networks deterministically.
- **The start order of [2 §2.4](02-node-runtime.md) is kept**, but nothing blocks on the network:
  - The listeners come up first, as in the original's systemd runtime (fixes RT-8).
  - Identity-dependent services start when an identity exists.
  - The exchange-rate fetch, XRPL connection and bootstrap polls become readiness conditions with retries, rather than `await`s that can wedge startup (ILP-14, ACC-26, NET-14).
- **Graceful shutdown on SIGTERM**: stop accepting, drain in-flight packets up to their expiry, close the DB, remove the socket (RT-7).
- **Two runtimes, chosen automatically.** `dassie daemon --runtime standalone|systemd` replaces `nodejs|nodejs-systemd`, which name a runtime that no longer exists. Without the flag, the daemon uses `systemd` when `LISTEN_FDS` is set for its PID.
  - `standalone` binds the ports in the config, and the IPC socket at `$XDG_RUNTIME_DIR/dassie.sock` unless `DASSIE_IPC_SOCKET_PATH` says otherwise.
  - `systemd` takes the sockets by name and checks `LISTEN_PID` (RT-20). It reports `READY=1` and `STOPPING=1` natively.
  - Either way, URLs the node prints or redirects to use the ports it actually listens on.
- **The node's decisions live in `dassie-node-core`.** Setup and login checks, session lifetimes, rate limits, the same-origin rule, and when to renew a certificate are pure functions of their inputs, `now` and an injected RNG, as [D16](08-decisions.md#d16-engineering-style-pragmatic-tigerstyle-and-deterministic-simulation) asks. `dassie-node` only carries them out.

### 4.2 Protocol crates are sans-IO

`dassie-stream`, `dassie-peer` (message handling, link-state processing, routing calculation), `dassie-connector` (outcome calculation) and `dassie-ledger` are pure state machines. They take inputs plus `now`, and they return outputs: packets to send, timers to arm, ledger operations. `dassie-node` wires them to sockets and the database. This makes the original's implicit timing rules ([3 §17](03-peer-network.md), [4 §12](04-interledger.md), [5 §7](05-accounting-and-settlement.md)) unit-testable, and it lets the same `dassie-stream` code run in WASM for the website.

These crates follow [D16](08-decisions.md#d16-engineering-style-pragmatic-tigerstyle-and-deterministic-simulation):

- They are deterministic functions of their inputs, `now` and an injected RNG.
- Every queue, table and buffer has a configured limit, and memory is allocated when a state machine is created, not per packet.
- Invariants are asserted after every transition.
- Everything observable is reported as events and counters.

The node's services are the only place that touches real clocks, sockets and entropy. A simulation replaces exactly that layer.

### 4.3 HTTP surface

The route table of [2 §6.3](02-node-runtime.md) is kept, including routes that exist only in certain states: `/api/setup` only while uninitialized, `/api/login` and `/api/logout` only after. The following is fixed:

- the redirect uses the configured hostname and keeps the query string (RT-1);
- `HEAD` is handled, and all asset types are served with correct MIME types, caching and compression (RT-15);
- unmatched WebSocket upgrades get a 404 (RT-16);
- strict security headers on every response: CSP with no inline script or style, HSTS once a real certificate is installed, `X-Content-Type-Options`, `Referrer-Policy`, `frame-ancestors 'none'`;
- an `Origin` check on `/rpc` and on the `/api/*` POSTs (RT-6): `Origin` must be `https://` plus the request's `Host`. `Host` must be the configured hostname, `localhost` or an IP literal, which stops DNS rebinding;
- WebSocket sessions are re-validated on every call, so logout takes effect immediately. Logging out also closes that session's open sockets.
- each listener holds at most 1024 connections, and at most 64 from one client (an IPv4 address or an IPv6 /56), so that a few addresses cannot fill it. Loopback is not counted per client where private peers are allowed, in dev environments and tests.

**Open Payments routes** ([D9](08-decisions.md#d9-open-payments-fully-and-no-spsp)) come before the wallet's fallback to `index.html`:
- **Wallet addresses:** `GET /.well-known/pay` and `GET /pay/<scheme>`, each with `/jwks.json`.
- **The authorization server** at `/gnap`: `POST /gnap`, `POST|DELETE /gnap/continue/{id}`, `POST|DELETE /gnap/token/{id}`, and `GET /gnap/interact/{id}/{key}`, which leads the owner to the wallet's consent page, `/consent/{id}`.
- **The resource server:**
  - `/incoming-payments[/{id}[/complete]]`;
  - `/quotes[/{id}]`;
  - `/outgoing-payments[/{id}]`;
  - `/outgoing-payment-grant`.

They differ from the rest of the surface in four ways:
- **No `Origin` check.** Clients authenticate by GNAP token and signature, not by cookie.
- **CORS.** They answer `Access-Control-Allow-Origin: *` without credentials, and a preflight `OPTIONS`.
- **Errors** follow the Open Payments shape, `{"error": {"code", "description"}}`. A 401 carries `WWW-Authenticate: GNAP as_uri=…`.
- **Bounds.** Bodies are at most 16 KiB. Every route is rate-limited per IP, and grant requests and new incoming payments are also limited per client.

**The daemon drives ACME**, for the first certificate as for renewals. In the original, `dassie init` ran its own ACME client and asked the daemon over RPC to publish challenge tokens. That meant two ACME implementations, with token RPCs that could fail on duplicates (RT-13). Now `dassie init` calls `acme.ObtainCertificate` and shows the progress the daemon streams back.
- Challenge tokens live in memory, in a bounded table whose expiry is enforced.
- Only one order runs at a time.
- A failed renewal backs off from 1 hour to 24 hours and honours `Retry-After`.
- Certificate keys are ECDSA P-256.
- Changing the hostname while a CA-issued certificate is installed falls back to the self-signed certificate until a new one is issued.
- The self-signed certificate is generated once and kept (RT-14).

### 4.4 Persistence (what must survive a restart)

Most of the original's integrity gaps come from state it keeps only in memory, or reconstructs incorrectly at startup (ACC-5, ACC-6, ACC-16, ACC-18, NET-1, NET-2). The new schema persists:

| Data | Original | New |
|---|---|---|
| Config (realm, hostname, ports, alias, exchange-rate URL), TLS cert/key, ACME account | scalars | same, with RPC setters for all (RT-4) |
| Node key | PEM in DB | the raw 32-byte seed; file mode 0600 on the DB and 0700 on the data dir ([D10](08-decisions.md#d10-secrets-at-rest)) |
| Sessions | token only, never expire | SHA-256 of the token, created and last used. A session ends after 30 days unused or 1 year in total, whichever comes first, or when revoked. At most 64 are kept; the least recently used goes first (RT-11) |
| ACME challenge tokens | table | memory, since the daemon that serves them also runs the order |
| BTP tokens | plaintext | hashed, plus a label and created time; shown once at creation. A token's spending cap and how much of it is used, written with the journal rows that use it ([§4.5](#45-settlement-and-ledgers)) |
| Node table: nodes, link states (signed bytes), registrations, peers with scheme state | written only by dev prefill | written on every change (NET-1, NET-2, ACC-18) |
| Own link-state sequence | memory | persisted high-water mark (NET-7) |
| Settlement channels: per node ever peered with, the highest settlement sequence applied and sent, and the settlement message not yet acknowledged | none | in the batch that changes them; kept after the peering ends ([§4.7](#47-peer-protocol-v1)) |
| Bans | none | a table of node IDs, and a record of each ban: by the owner or for what a peer cost ([D18](08-decisions.md#d18-one-grace-limit-per-peer)), with that cost, when, and when it was lifted. A record outlives its ban; at most 2048 are kept, the longest lifted going first |
| Peer settings | none | the owner's grace share ([D18](08-decisions.md#d18-one-grace-limit-per-peer)), fee and spread ([D7](08-decisions.md#d7-exchange-rates-fees-and-spread)), in a single-row table |
| Replay state of peer messages | none (no replay protection) | per peer, the newest accepted message timestamp, written with the batch it took effect in ([§4.7](#47-peer-protocol-v1)) |
| Ledger | cumulative posted totals | append-only **transfer journal** (id, debit, credit, amount, state, timestamps, reference to packet or settlement), with account balances cached in the same transaction. Only transfers that reach posted balances are journaled, plus pending settlements; a voided packet transfer never touched a posted balance and leaves no row |
| Pending ILP transfers | memory | memory only: in-flight packets cannot survive a restart, so a restarted ledger has none |
| Fulfills owed a verdict, and Fulfills taken | none | the downstream keeps each Fulfill it sent a peer, with the packet's transfers as posted (three at most: the packet's two legs and our fee), until the peer's verdict comes back, written with the journal rows that post the packet; the upstream records each Fulfill it took from a peer for 30 days, so it answers a resend the same way ([D17](08-decisions.md#d17-the-upstream-decides-whether-a-fulfill-counted)) |
| Pending outgoing settlements | memory | journal rows with module-specific reference (tx hash), reconciled on startup (ACC-16) |
| Settlement-module balance checkpoints | none (full balance re-deposited at every start) | last reconciled on-ledger balance per module, with the module's state (for XRPL: its cursor, sequence, transactions in flight and peers' addresses), written in the batch they explain; startup books only the difference, through `equity/suspense` (ACC-6, ACC-2) |
| Ledger fees | never booked | journal rows with their transaction (`fee`), Dr `expenses/fees` |
| Peer trust lines | re-posted at every start | posted once when the peer account is created; changes are explicit transfers (ACC-5) |
| Incoming and outgoing payments, quotes | rows without state | full Open Payments state, written with the journal rows that move the money: received/sent amounts, completion, expiry, the STREAM token tied to the incoming payment (ILP-10), and what each of an open incoming payment's STREAM connections received. An outgoing payment's sent amount is what was posted: a packet in flight at a crash was never posted, and its Fulfill is refused ([D17](08-decisions.md#d17-the-upstream-decides-whether-a-fulfill-counted)), so a restart sends only the rest. Each grant's spend in its current interval window, written with the payment that spends it. The owner's quotes and payments keep the wallet address they pay, and each quote and payment whether it fixes the amount to send or to deliver ([D9](08-decisions.md#d9-open-payments-fully-and-no-spsp)) |
| GNAP grants and access tokens | none | grants with their access, client key and interaction state; tokens as SHA-256 hashes ([D10](08-decisions.md#d10-secrets-at-rest)). The amount of each grant spent in its current interval goes with the payments |
| Exchange rates | memory | the owner's sources (at most 8), and each currency's last rate with when it was fetched, restored at start; fetched every minute ([D7](08-decisions.md#d7-exchange-rates-fees-and-spread)). The currency the owner sees totals in ([D6](08-decisions.md#d6-owner-ledger-and-multi-currency-accounts)) |

The database has a new application ID, `0x7DA5E2C9` (like the original's, a random positive int32 with nibbles 2–4 set to `DA5`). A file with the v0 ID `0x1DA53B81` is refused with a message telling the owner to set the node up again with their passphrase ([D3](08-decisions.md#d3-database-continuity)). Configuration is typed columns with CHECK constraints in single-row tables, not a key/value table, so that SQLite itself refuses an invalid port or realm.

### 4.5 Settlement and ledgers

The settlement interface of [5 §4.3–§4.4](05-accounting-and-settlement.md) is a good shape, and the port keeps it as a Rust trait with these changes:

- explicit transfer handles;
- `cancel` actually called on failure;
- a durable pending-settlement record;
- `supportedVersions` negotiated during peering (ACC-14, NET-26);
- module config passed through (ACC-12, RT-21).

The XRPL module needs:

- reserve awareness, so it never settles from the reserve (ACC-19);
- a `TransactionResult` check (ACC-22);
- activation (`CreatedNode`) handling (ACC-20);
- booked fees and drop rounding (ACC-21);
- sender-account checking;
- a memo type per network: testnet vs mainnet, versioned with the peer protocol (ACC-13).

Mainnet stays disabled until the [D8](08-decisions.md#d8-real-money) criteria are met.

**Schemes that follow a ledger (M7b).** The trait stays sans-IO. A scheme with a ledger to follow names a WebSocket URL (`connection`); it says what to send (`LedgerSend`) and the node feeds it what comes back, with `Connected`, `Disconnected`, and the faucet's answer (`Fund`, `Funded`). The driver runs one WebSocket per scheme, opened and closed when the core says, bound to an *epoch*, so a message from a connection the core has replaced is dropped; it reconnects backing off from 1 s to 60 s, with frames of at most 1 MiB. A scheme has timers, a *state* the node keeps with its checkpoint in the batch it explains (written whenever the state's version or the balance changed, not at every ledger), a status the wallet shows, the least unit its ledger can move (settlements are whole multiples; less stays owed, for good at the end of a peering), and can make its settlement message again from a settlement's ID. Two more effects: `Fee` (Dr `expenses/fees`, Cr `assets/settlement`, a report) and `Reconcile` (through `equity/suspense`). A scheme that cannot settle now says `Unavailable`, which the node asks again at the next check, without backoff, warning the owner at most every 10 minutes. A settlement the node prepared but could not book is discarded, never sent. Effects the books cannot take stop the scheme (shown as failed) rather than the node; its state is not written, so a restart reads the ledger again. The registry of schemes is the node's.

**The `xrpl-testnet` scheme (`dassie-xrpl`).**
- **Its wallet** is the original's, from the node's settlement entropy (7 §2): pinned by the 64 wallets of the identity chain. **Its Payments** are signed byte for byte as xrpl.js signs them (golden vectors from `testdata/xrpl`), with the original's memo type and our node ID as memo data.
- **It books every validated transaction that touches its account once, in ledger order,** from a cursor `(ledger, index)` kept in its state:
  - one of ours: posted (`tesSUCCESS`) or voided (`tec…`), and its fee booked either way;
  - a successful Payment of XRP to us with our memo, from the address a peer gave when peering, naming that peer: a settlement from it, by `delivered_amount`, its hash the journal reference;
  - anything else that adds XRP, the account's creation included: a deposit; anything that takes it: a withdrawal of the whole difference.

  After each, the books hold the balance less the reserve; any difference (a reserve change, a balance below the reserve) is reconciled through `equity/suspense`. The reserve is never booked, so never spent.
- **The settlement message** carries the transaction's hash; it books nothing, the ledger does.
- **Settling:** whole drops, up to 8 transactions in flight with consecutive sequences, a fee from the server's state capped at 1000 drops, `LastLedgerSequence` 20 ledgers on, and only from what is above the reserve. A transaction is sent only once the node executed its settlement (its pending transfer on disk). Only `tem…` on submit cancels at once. Any other answer waits: the transaction is submitted again after a reconnect or restart, and once past its last ledger looked for over its ledgers with `tx`; only when every one was searched and it is not there is it cancelled, and the sequence read again. One of ours is settled whenever the ledger shows it, even behind the cursor (what the balance booked through suspense meanwhile comes back out). A settlement the node still holds pending after a restart that the scheme no longer has in flight (its posting waited for an earlier announcement) is looked up by its hash, and posted or voided.
- **Restarts:** the state (cursor, sequence, transactions in flight, peers' addresses, settlements to look up) comes back with the checkpoint; the scheme subscribes, then reads its account (asking again while the server is behind the cursor), and catches up with `account_tx` from the cursor, 200 a page, holding what the stream sends meanwhile; a busy server is tried again on a new connection. A server whose history does not reach back leaves the balance to say what changed, through suspense; a transaction of ours that no history holds any more cannot be known to have applied, and its settlement stays pending, for the owner. A first activation starts from `account_info`. An account gone (a testnet reset) is a loss through suspense, cancels what was in flight, and the faucet is asked again. A connection quiet for 30 s, no ledger closing, is made anew.
- **Config:** empty for the testnet's server and faucet, or JSON with `server` (`wss://`, or `ws://` where private addresses are allowed) and `faucet` (HTTPS, or `null`).
- **The fake ledger** (feature `fake`): accounts, reserves, fees, the checks the ledger makes on a Payment with its result codes, metadata, streams, history and a faucet. The node simulation settles on it in-process; `fake_ledger` serves it over WebSocket for the node's tests, the end-to-end tests and `node --run dev -- --xrpl fake`.

**The owner's accounts ([D6](08-decisions.md#d6-owner-ledger-and-multi-currency-accounts)).** Each ledger `L` has one pool of owner funds, `L:equity/owner`. Every endpoint that spends for the owner gets a sub-account under it, for example `L:equity/owner/btp/<token>` for a BTP token. A sub-account holds no allocation and may go negative on its own; together with the pool it forms a **group** whose total carries the overspending limit (debits ≤ credits). The group also holds what the node gains or loses on the ledger: `equity/suspense`, `revenue/fees`, `expenses/fees` and `expenses/losses` (M7). So the owner's funds are everything the node holds on the ledger beyond what it owes peers: a loss, a network fee or a reconciliation that finds less money is the owner's, and the owner cannot spend money the node no longer has. `revenue/fx` and `expenses/fx` are in the group too (M7c), with the rule that makes them coherent: **the owner's own endpoints never convert**. So:

- a BTP client paying the node's own STREAM receiver books Dr `equity/owner/btp/<token>`, Cr `equity/owner`: the group total is unchanged, and the journal still has a transfer per packet (ACC-8);
- a transfer within the group never needs free funds, since its effect on the total is zero;
- a packet from a BTP token, a payer or a receiver goes out on its own ledger only: one whose first hop is on another ledger is refused (F02 "pay from the route's ledger"). The owner pays from the ledger the route starts on, and the first hop that charges for a conversion converts;
- a packet from a peer on another ledger into the node's own receiver is converted at the node's rates and lands in the asset it came in: the receiver's account shows what arrived, `expenses/fx` takes it back out on that ledger, and `revenue/fx` keeps it on the ledger it came from;
- a conversion between two peers pays out of the owner's funds on the outgoing ledger, so it needs them;
- each token's spending can be read from the journal, which is what the per-token spending caps of M5 (an amount per window, refilling continuously) are computed from. Caps are limits, not allocations, so they may add up to more than the funds.

**Open Payments on the books ([D9](08-decisions.md#d9-open-payments-fully-and-no-spsp)).** Each ledger has its own STREAM receiver and its own payer, two local endpoints.
- **The receiver** credits `L:equity/owner`. A packet is fulfilled only for an open incoming payment on that ledger, and only up to what that payment still expects. The payment's new received amount is written in the same transaction as the journal row.
  - **A STREAM connection per payment.** Each read of an incoming payment's methods gets credentials of its own: the token's tag is the payment's ID and a random connection key. Each payment that pays it therefore has a connection of its own, and the receiver tells each connection what it received, not what the payment did, so a sender never takes another's delivery for its own. What each connection received is written with the payment's. At most 32 connections pay one incoming payment; a new one past them is refused. The credentials kept with the payment are one connection like any other: the wallet shows them, to pay with STREAM alone. Those issued before connection keys carry the ID alone, and are one connection.
- **The payer** debits `L:equity/owner/local/pay`, a sub-account in the owner's group, so the group's limit is the outgoing payments' limit too. Grant limits are checked when an outgoing payment is created, not by the connector.
  - Each outgoing payment that is sending has one STREAM connection; at most 16 send at once, and the rest wait their turn. No two outgoing payments use the same credentials: a payment whose destination another payment used is refused, and its receiver must be read again.
  - A payment to the node's own receiver goes through the connector like any other, through a bounded queue drained at the end of the step, so nothing recurses.
  - Each packet must deliver at least the payment's rate, its receive amount over its debit amount: a worse path fails it.
  - A payment completes once it has sent its debit amount. One that fixes the amount to send also completes once it delivered what its quote said and its receiver takes no more (a request it filled). One that fixes the amount to deliver also completes once its receiver says it has all it asked for, or all of it but less than a unit sent is worth, which no packet can be sized to deliver as the hops round down. It fails when the receiver keeps refusing it, or 10 minutes after it was created. It ends only with none of its packets in flight. What it did not send is released from its grant's window.
- **Quotes** measure the rate of the route to their receiver with a probe: a STREAM connection from the payer that sends only packets the receiver cannot fulfill, as STREAM measures rates. At most 8 probes run at once, for at most 10 s each; they hold the owner's funds while in flight, as the payment would. A quote is the measured rate less STREAM's 1% slippage, or one for one to the node's own receivers, which convert and charge nothing; without a route, or a measured rate, there is none.

**Fees and spread ([D7](08-decisions.md#d7-exchange-rates-fees-and-spread)).** What passes from one peer to another is charged the owner's fee and spread, each 0 to 10% (default 0); the node's own endpoints are never charged.
- `fee = floor(in × fee_ppm / 10^6)`, rounded down: rounded up, it would take a payment's last packets whole, and strand it short. `out = floor(convert(in − fee) × (10^6 − spread_ppm) / 10^6)`, the spread only on a conversion. A packet whose output rounds to nothing is refused (F02).
- The fee is a third linked transfer, Dr the source's account, Cr `L1:revenue/fees`; the spread's gain is the FX accounts' net. The ledger has room for three pending transfers for every packet in flight.

**Spending caps.** A BTP token may carry a cap of `A` per window `W`, refilling continuously.
- **What counts** is a packet that debits the token's account and credits an account outside the owner's group: a peer (a token never converts). A payment to the node's own receiver, or to another token's client, moves no money out of the owner's funds and does not count.
- **The bucket**, in exact integers: `U` (amount·ms) as of time `τ`, and `R`, what packets in flight hold. `U` drains at `A` per ms: `U(t) = max(0, U − (t − τ)·A)`.
  - A Prepare of `a` is refused with F08 (maximum `A`) when `a > A`, and with T04 when `U(t) + (R + a)·W > A·W`. Otherwise `R` grows by `a`.
  - A fulfilled packet adds `a·W` to `U`; a rejected one only frees `R`.
  - So `U + R·W ≤ A·W` always, and what a token spends in any period `t` is at most `A + A·t/W`.
- **It is enforced in the connector**, per account, so all of a token's connections share one cap.
- **The usage is checkpointed** with the journal rows that change it. The journal alone cannot rebuild it from a recent window: two histories with the same recent rows can have used different amounts. A replay of the token's journal since the cap was set is the reference the tests check the checkpoint against.
- Setting a cap where there was none starts from nothing used; changing one converts what is used, rounding up; removing it, or revoking the token, drops it.

**Fulfills a peer refused ([D17](08-decisions.md#d17-the-upstream-decides-whether-a-fulfill-counted)).** A packet from a peer is posted when it is fulfilled. If the peer then refuses the Fulfill, one compensation is booked with the packet's trace:
- when the node's own receiver took the packet, its transfers are reversed, and the incoming payment does not count it;
- when anyone else took it (a peer or a BTP client, who keep what they were paid), Dr `L:expenses/losses`, Cr `L:assets/interledger/<peer>`, for the incoming amount: every leg that debited the peer, our fee with the packet.

Either way the peer owes only for the Fulfills it took, so an honest peer never looks overdue ([D19](08-decisions.md#d19-each-node-keeps-its-own-books)). What each peer refused is read from the journal, and its losses count against the peer's grace limit ([D18](08-decisions.md#d18-one-grace-limit-per-peer)); the reversals cost nothing and do not.

**What the ledger refuses and what it records.** Packets and settlements are refused when they would break a limit (T04). Reconciliations, deposits, withdrawals and refusals report what happened on the underlying ledger and are recorded even when they take the group below zero; spending is then refused until it recovers. Settlement modules report their on-ledger balance as a checkpoint, and activation books only the difference to the last checkpoint, through `equity/suspense` (ACC-2, ACC-6). The checkpoint is written in the same transaction as the transfers it explains.

### 4.6 The RPC contract in practice

[D5](08-decisions.md#d5-ui-and-cli-rpc-contract) fixes the shape. The details:

- **Envelopes.**
  - The client sends a `ClientMessage`: a call ID plus a request or a cancel.
  - The server answers with a `ServerMessage`: the call ID plus one of a response, a stream item, the end of a stream, or an error.
  - Requests and responses are `oneof`s of one typed message per method, not string paths.
- **Errors** carry a code (`UNAUTHENTICATED`, `INVALID_ARGUMENT`, `FAILED_PRECONDITION`, `NOT_FOUND`, `RESOURCE_EXHAUSTED`, `UNAVAILABLE`, `INTERNAL`) and a message for people.
- **Transports.**
  - WebSocket at `/rpc`: one message per binary frame.
  - The IPC socket: frames with a 4-byte big-endian length prefix.
  - Both have a 1 MiB message limit, and per-connection limits on concurrent calls (16) and open streams (12). A node serves at most 64 live streams in all.
  - The node caps the number of open WebSockets at 256. Sockets without a session get at most 128 of them, and 4 per client (an IPv4 address or an IPv6 /56), since the Origin check binds only browsers and any client can open one: sockets with a session always find room, and no single client can take every place.
  - A browser's socket with no call in flight closes after 10 minutes without a message. A socket without a session also closes 60 s after it opened, whatever it sends, after the calls in flight have their answers. Both close with code 1000; the client reconnects on its next call.
  - A client that sends faster than it reads is disconnected, rather than allowed to queue answers without bound.
  - Every way a session ends (logout, revocation, expiry, eviction for a newer one) closes its sockets with code 4001, after the calls already answered have their answers.
- **Live streams** (`Subscribe*`) carry either the whole value each time, or a snapshot followed by upserts and removals by key.
  - The first item of a stream, and of every snapshot split over several items, has `reset` set.
  - The node publishes a change only once the step that made it is on disk. It sends at most 4 items a second, coalescing changes in between, so a slow client never falls behind; it sends an item at least every 60 s, so a client can tell a quiet stream from a dead one.
  - A stream ends with `UNAVAILABLE` when the node's engine restarts, and the client subscribes again.
- **The browser client** gives every call a timeout (RT-27), and lets a caller cancel a call or stream.

### 4.7 Peer protocol v1

This section is normative for v1. It describes what changes from v0 ([3 §5–§13](03-peer-network.md)); anything not mentioned here keeps the v0 layout and meaning. The reasons are in [D2](08-decisions.md#d2-peer-protocol-v0-or-v1), [D4](08-decisions.md#d4-network-membership-economics) and [D12](08-decisions.md#d12-peering-lifecycle-and-liveness).

**Transport.** Requests stay `POST /peer` with `Accept` and `Content-Type` exactly `application/dassie-message`. Responses are `application/dassie-peer-response`. The HTTP status means something (NET-11):

| Status | Meaning |
|---|---|
| 200 | Handled. The body is a `PeerResponse`. |
| 400 | Malformed, a version other than 1, a sender equal to the recipient, or a link state that fails verification. |
| 401 | Authentication is missing where it is required, or fails, or is a replay. |
| 406 / 415 / 413 | Wrong `Accept`, wrong `Content-Type`, or a body over 64 KiB. |
| 409 | The message needs a peering that does not exist (`interledgerPacket`, `settlement`, `settlementMessage`). |
| 429 | Rate limited. |
| 503 | The node is busy, has a full table, or is not running its peer protocol yet. |

Every non-200 response has a short `text/plain` body for people. Senders treat any status other than 200 as a failure. Status 400, 401, 409 and 503 mean the message was not acted upon.

**Envelope.**

```
PeerMessage ::= SEQUENCE {
  version        UINT8,                 -- 1
  sender         NodeId,
  authentication Authentication,
  content        PeerMessageContent }   -- raw choice, runs to the end of the body

Authentication ::= CHOICE {
  none         [0] NULL,
  hmacSha256V1 [1] SEQUENCE {
    senderKey   OCTET STRING (SIZE(32)),   -- Ed25519 public key; NodeId(senderKey) MUST equal sender
    sessionId   OCTET STRING (SIZE(16)),
    counter     UINT64,                    -- 0, 1, 2, ... within the session
    timestampMs UINT64,
    mac         OCTET STRING (SIZE(32)) },
  reserved     [2] OCTET STRING }          -- for a future X-Wing scheme; answered 401

PeerResponse ::= SEQUENCE {
  authentication CHOICE { none [0] NULL, hmacSha256V1 [1] OCTET STRING (SIZE(32)) },
  body }                                   -- the v0 response for the request type, runs to the end
```

**Keys and MACs.** The sender is S and the recipient is R. `pub` means an Ed25519 public key, and `x25519` is the conversion of [3 §2.4](03-peer-network.md).

```
dh     = X25519(x25519(S), x25519(pub R))     -- = X25519(x25519(R), x25519(pub S)); all-zero is refused
K_pair = HKDF-SHA256-Extract(salt = "dassie peer v1", ikm = dh)
K_req  = HKDF-SHA256-Expand(K_pair, "request"  ‖ pub S ‖ pub R, 32)
K_resp = HKDF-SHA256-Expand(K_pair, "response" ‖ pub S ‖ pub R, 32)
mac    = HMAC-SHA256(K_req, 0x01 ‖ LEN ‖ sender ‖ LEN ‖ recipient ‖ sessionId ‖ counter ‖ timestampMs ‖ content)
respMac = HMAC-SHA256(K_resp, mac ‖ body)
```

- `content` is the exact bytes of `PeerMessageContent`, tag included. `LEN ‖ id` is the OER encoding of a node ID.
- S and R always name the request's sender and recipient, so a response is keyed by the same pair in the same order. The labels and the ordered keys make each MAC specific to one direction. Comparisons are constant-time (NET-10).
- A response is authenticated if and only if its request was.

**Which messages need authentication.**
- Always: `interledgerPacket`, `settlement`, `settlementMessage`, `peeringRequest`, `registration` and `peeringTermination`.
- The others may be authenticated. A v1 node authenticates every request it sends, since it only sends to nodes whose key it knows. An authenticated `linkStateUpdate` whose sender is the subject of the state counts as a heartbeat.
- For `peeringRequest` and `registration`, the embedded signed node info must verify and name the sender (NET-3, NET-4).

**Replay.** For each sender, the recipient accepts a message only if:
- `|timestampMs − now| ≤ 60 s`;
- `timestampMs` is above the sender's floor;
- its `(sessionId, counter)` pair is new.

A sender uses one session per destination and starts a new one every hour. The recipient keeps at most two sessions per sender. Each session holds the highest counter seen and a 1024-counter sliding window; counters below the window are refused. When a third session displaces one, the sender's floor rises to the newest timestamp of the displaced session.
- For peers, the newest accepted timestamp is persisted in the same transaction as the message's effects, and becomes the floor after a restart.
- For other senders, the floor after a restart is the start time.
- A message's effects only become visible after the transaction that records it, so a message lost in a crash can be accepted once more, but never twice.
- Keys are cached per sender, at most 1024 of them. An entry is added only after a MAC verifies (NET-9).
- Peers and bootstrap nodes always keep their replay state. Every other sender shares 1024 places, so a flood of new identities cannot crowd out the nodes that matter: once the places are full, new senders get 503 until one goes idle.

**Link state.**
- **New OID.** `PeerNodeInfo` uses OID `1.3.6.1.4.1.54392.5.1399.1.3`, so a node info signed under v0 is not a v1 link state. Verification is ZIP-215 ([8 §2](08-decisions.md#2-original-behaviour-kept-on-purpose)), and small-order public keys are refused.
- **New entry.** A third entry kind is added: `extension [2] SEQUENCE { type UINT16, value OCTET STRING (SIZE(0..2048)) }`. Unknown types are kept, since the signature covers them, and otherwise ignored. A future X-Wing key is type 1.
- **Limits:**
  - `url` is at most 256 bytes;
  - `alias` at most 64;
  - at most 64 entries;
  - the signed info at most 4 KiB.
- **URL syntax** is part of verification, so all nodes agree on it: `https://`, a lowercase hostname or IP literal, an optional port, and nothing else. There is no userinfo, path, query, fragment or trailing slash. `~` is allowed (NET-30).
- **Sequence:**
  - A state whose sequence is more than 10 min ahead of the receiver's clock is refused. Routing uses only states whose sequence is within 3 h before now and 10 min after.
  - An older or equal sequence is dropped before its signature is checked. A byte-identical copy counts toward the flooding suppression; other copies with the same sequence do not.
  - Each subject may issue at most 8 states in a burst, then one per 5 s; the excess is neither stored nor forwarded.
  - A node's own sequence is `max(now, last + 1)` with the last value persisted (NET-7). It is re-issued when its neighbors, schemes, URL or alias change (NET-6), and every hour (NET-5).
- **Anti-entropy:** when no link state is stale, a node asks a random peer (or a bootstrap node, if it has no peers) for the link states of the next 16 nodes in its table, round and round, every 10 s. Flooding alone loses an update for up to an hour when a message is lost, a node is down or the network splits. The simulation found that, and the sweep repairs it within one pass of the table.
- **Requests:** `linkStateRequest` carries at most 16 distinct node IDs. It is answered in the order asked, each ID once.
- **Scheme IDs:** `SettlementSchemeId` is at most 64 characters, down from 128. The alphabet is unchanged.
- **Public key:** the `publicKey` field is exactly 32 bytes.

**Changed messages.**

```
peeringRequest [0] SEQUENCE {
  settlementSchemeId   SettlementSchemeId,
  nodeInfo             OCTET STRING CONTAINING SignedPeerNodeInfo,
  settlementSchemeData OCTET STRING,
  versions             SEQUENCE (SIZE(1..16)) OF UINT16,   -- the scheme versions the sender supports (NET-26)
  credit               UINT64 }                            -- credit the sender extends, in the scheme ledger's units (ACC-24)

-- response body
PeeringResponse ::= CHOICE {
  accepted [0] SEQUENCE { version UINT16, credit UINT64, data OCTET STRING },
  refused  [1] SEQUENCE { reason UINT8 } }
  -- 0 unspecified, 1 unknown node, 2 unsupported scheme, 3 no common version, 4 capacity,
  -- 5 concurrent request, 6 refused by the scheme, 7 unreachable, 8 banned,
  -- 9 terminating (the responder is ending, or an owner recently ended, its peering with the
  -- requester); unknown values read as 0

registration [8] SEQUENCE { nodeInfo SignedPeerNodeInfo }  -- unchanged
-- response body, was empty
RegistrationResponse ::= SEQUENCE { expiresAtMs UINT64 }

settlement [4] SEQUENCE {
  settlementSchemeId   SettlementSchemeId,
  amount               UINT64,
  settlementSchemeData OCTET STRING,
  sequence             UINT64 }   -- per sender and recipient, increasing, never reset

peeringTermination [10] SEQUENCE { reason UINT8 }
  -- 0 unspecified, 1 by the owner, 2 unresponsive, 3 not peered (the sender holds no peering
  -- with the recipient); unknown values read as 0
-- both responses are empty
```

**Membership** ([D4](08-decisions.md#d4-network-membership-economics)):
- A bootstrap node serves `unexpired registrations ∪ itself`, sorted by bytes (NET-32).
- A downloaded list is accepted when its response is authenticated by the configured key, and is stored with the hash of the bytes received (NET-15).
- Majority is unchanged: strictly more than half of the *configured* bootstrap nodes.
- Nodes lose their majority place only while more than half of the lists have been fetched within the last 10 min.
- **Registration:**
  - it lasts 24 h, and clients renew every 6 h;
  - before accepting a new registration, or one whose URL changed, the bootstrap node asks that URL for the registrant's link state, with authentication, and must get the registered sequence back;
  - new registrations are throttled globally (a bucket of 20, refilled at 60 per hour);
  - there is one registration per URL host and port (443 counts as no port), and at most 8 per /24 (IPv4) or /48 (IPv6) of the address the registration came from;
  - only bootstrap nodes take registrations; others answer 409;
  - a renewal is a registration from the URL that was probed; one from another URL is probed again;
  - a probe of the same node or host is never made twice at once, and probes are rate-limited (a bucket of 32, refilled at one per second).
- **The node table** removes a node 1 h after its last reason to be there is gone: self, bootstrap, majority, registration or peering (NET-18).

**Peering.**
- **Who counts:** only confirmed peers count toward the target of 2. A peer is confirmed by a reciprocal edge or an authenticated heartbeat within the last 60 s.
- **Slot caps:**
  - inbound requests are accepted while there are fewer than 6 inbound peers;
  - a node starts at most target + 2 outbound peerings.
- **Bridge rule:** if a majority node with a fresh link state has been unreachable for 2 min, the node starts one more outbound peering, with that node if it can.
- **Before accepting,** the responder probes the requester's URL as for registration.
- **Repeats and crossings:**
  - a request from a node that is already peered on the same scheme is answered from the stored peering, which it does not change;
  - crossed requests resolve in favour of the lower node ID.
- **Backoff:** failed candidates back off exponentially, from 10 s to 1 h. "Unknown node" is retried after one list-poll period instead.
- **Durability:** a peering is persisted before the acceptance is sent and before the requester uses it.

**Lifecycle** ([D12](08-decisions.md#d12-peering-lifecycle-and-liveness)). A peering is `requesting`, `peered` or `terminating`.
- **Advertised.** A peer is listed as a neighbor in our link state while it is peered, has sent an authenticated message within the last 60 s, and is not held down. Each time it stops being advertised, it is held down for `30 s · 2^flaps`, at most 1 h; `flaps` resets after 1 h advertised without a break. After a restart, the flag starts from the persisted time the peer was last heard from, without a hold-down. Only advertised peers count toward the target. Liquidity does not withdraw a peer: withdrawing removes the edge both ways for the whole network and turns T04 into F02, while the traffic that comes back is what restores it.
- **Termination** starts when the owner asks, when the owner bans the node, or when the peer has not been heard from for 24 h (counted from the later of the last message and the node's start).
  1. The peer is no longer advertised, and takes no new Prepares to or from it: the connector skips it, and its Prepares get 409. Responses still flow.
  2. The side that started sends `peeringTermination`, again until a 200 or a 409 arrives or 2 min have passed. The other side answers at once and terminates too.
  3. Once no packet is in flight with the peer, no response to it is waiting to be sent again, and no Fulfill to it waits for its verdict, the peering ends. Verdicts are waited for at most 10 minutes, and not at all when the peer stopped answering, since they may never come; they are still asked for over the settlement channel. Its accounts stay, with their balances: they are the record of what is owed, and a later peering with the node uses them again without posting trust a second time.
  4. If the exchange of step 2 completed, each side then settles everything it owes. That is kept on the settlement channel, across restarts, until it is made; a cancelled or failed attempt is tried again. An unresponsive peer is not settled with; the debt stays on the books.
  5. The node does not ask the peer again for 24 h after an owner's termination, or after its usual backoff otherwise. A request from a node we are terminating with, or whose peering an owner ended in the last 24 h, is refused with reason 9; that cooldown does not survive a restart, and a ban is the lasting way.
- **Bans** are a list the owner keeps, and the node adds to: a peer that cost it more than its grace limit is banned ([D18](08-decisions.md#d18-one-grace-limit-per-peer)). A banned node is never asked, is refused with reason 8, and its messages do not confirm it as a peer. Its link states still flood. A ban ends a peering as the owner's termination does, with the same wire reason.
- **One-sided peerings.** A node that holds no peering with a sender, and is not asking for one, answers an authenticated heartbeat that lists it as a neighbor with a `peeringTermination` of reason 3, at most once per backoff period. The other side then ends its peering.

**Settlement** ([5 §4.6](05-accounting-and-settlement.md), kept with the credit the peer extends; ACC-24):

```
balance    = IL.credits_posted − IL.debits_posted − IL.debits_pending     -- IL = L:assets/interledger/<peer>
credit_out = TR.debits_posted − TR.credits_posted                          -- TR = L:contra/trust/<peer>
midpoint   = (credit_in + credit_out) / 2                                  -- credit_in as the peer announced it
settle balance − midpoint when balance ≥ midpoint · 1.1; on termination, settle balance − credit_out
```

- `credit_in` is a term the peer announced, and serves only our choice of when to settle ([D19](08-decisions.md#d19-each-node-keeps-its-own-books)): a stale one costs liquidity, not money. From M10 the credit each side extends depends on which asked to peer, grows with what the peering earned, and is announced again when it changes ([D20](08-decisions.md#d20-trust-follows-what-a-peer-earned-us)).
- Every 4 s, for each peer with nothing in flight: no pending settlement transfer and no undelivered `settlement` message (ACC-17).
- A settlement is prepared, booked as a durable pending transfer (Dr IL, Cr `assets/settlement`), and executed only once that transfer is on disk. A restart in between hands the scheme its pending settlements to reconcile (ACC-16).
- Only a settlement that executed is posted and announced to the peer. A cancelled one is voided, and the node backs off settling with that peer, from 1 s up to 1 h.
- **Delivery is exactly once.** The `settlement` message is sent again until a 200 arrives, with a backoff from 1 s up to 1 h, and is never dropped; it survives restarts. Its `sequence` belongs to the pair of nodes, not to the peering. The receiver keeps the highest sequence it applied from each sender, acknowledges anything at or below it without applying it again, applies the next one, and refuses any other with 400: the sender sends one at a time, in order.
- A `settlement` is accepted from any authenticated node we hold accounts for, peered or not, so a final settlement that arrives after the peering ended is still credited. `settlementMessage` needs a peering.
- The receiver checks that it can book the settlement before the scheme sees it, so the scheme's balance never moves without the books.

**Routing.** Routing runs a breadth-first search over edges that both ends advertise and whose link states are fresh.
- The route for `<scheme>.das.<id>` lists up to 4 first hops on shortest paths.
- There is no route for the node's own address (NET-34). Destinations under our own address go only to our own endpoints, or get F02.
- A packet is never forwarded back to the endpoint it came from.

**Packets.** A Prepare that surely did not reach its peer (a refused connection, or 400, 401, 409 or 503 in reply) is rejected at once. One that may have reached it is left to expire. A Reject for a peer, or a Fulfill of a packet that moved no money, is sent again, every second, until it arrives or 30 s have passed; one that never arrives has the effect of expiry. A Fulfill for a peer is sent until the peer's verdict comes back ([D17](08-decisions.md#d17-the-upstream-decides-whether-a-fulfill-counted)): `accepted` or `refused`, in the reply's body, which is what the books follow. A Fulfill or Reject is accepted from any node we hold a settlement channel with, peered or not, since a verdict may still be owed after the peering ended.

**Outbound requests** go only to `https` URLs. Requests to private, loopback, link-local or similar addresses are refused, after DNS resolution, unless the operator allows them (for dev and tests, `DASSIE_PEER_ALLOW_PRIVATE`). The node never follows redirects.

**Dev switches warn on a public node.** `DASSIE_PEER_ALLOW_PRIVATE`, and `DASSIE_DEV_TIMINGS` (the dev timers below), are plain environment variables. If either is set while a listener is bound beyond loopback, the daemon warns at startup and names it. An unspecified address (`0.0.0.0`, `::`) counts as beyond loopback. The address is the one actually bound, whether `--listen`, the standalone default or systemd's socket chose it. Dev and the tests listen on loopback, so they stay quiet.

**Timers and limits.** All of these are configurable; the values in brackets are dev settings.

| What | Value |
|---|---|
| Hash poll and list download | 60 s (1 s) |
| Peer message timeout | 30 s; 5 s for hash polls; a packet's own expiry if sooner |
| Heartbeat | 5–20 s, and once at once when a peering forms |
| Link-state re-issue | 1 h, debounced to one per 5 s (0.5 s) |
| Refresh | one stale node per 1 s, up to 16 IDs per request, staleness 3 h |
| Anti-entropy sweep | 16 nodes per 10 s (1 s) |
| Registration lifetime and renewal | 24 h and 6 h (2 min and 30 s) |
| Removal grace | 1 h (10 s) |
| Peering check, bridge check | 1 s, 60 s (2 s) |
| Peer confirmed by a message within | 60 s |
| Advertise hold-down | 30 s doubling to 1 h; reset after 1 h stable (5 s to 1 min; 5 min) |
| Peer unresponsive after | 24 h (2 min) |
| Termination message retries | 2 min (20 s) |
| No new peering after an owner's termination | 24 h (5 min) |
| Settlement check | 4 s (1 s) |
| Settlement message retries | 1 s doubling to 1 h, never dropped (1 s to 10 s) |
| Request body | 64 KiB |

### 4.8 Releases

A release is one static binary per platform, with the wallet built in ([D11](08-decisions.md#d11-release-integrity), [D14](08-decisions.md#d14-build-system)).

**The build** (`nix/release.nix`, on x86_64-linux):
- **The wallet** is built by Vite in the Nix sandbox, offline. Its npm packages and JSR modules are fetched one by one, each checked against the integrity `pnpm-lock.yaml` records for it (`nix/pnpm.nix`). A registry mirror on loopback serves them to `pnpm install --frozen-lockfile`. A change to the lockfile needs no hash updated by hand.
- **The binary** is `dassie-cli` in the `release` profile (fat LTO), statically linked against musl, for `x86_64-unknown-linux-musl` and, cross-compiled, `aarch64-unknown-linux-musl`. It embeds the wallet (`DASSIE_WALLET_DIST`) and its version (`DASSIE_VERSION`).
- **The check** `static-binaries` (`nix build .#static-binaries`, which the release workflow runs) confirms that both are static, name their version, and pass `verify-install`, the arm64 one under qemu.

**Versions and channels** (`dassie-release`):
- A release is `major.minor.patch`, dassie-cli's version, published on `latest`.
- A canary is `canary-YYYY-MM-DD-HHMMSS-<commit>`: when the commit it is built from was made, in UTC, and its short hash. It is published on `canary`.
- A version names a directory on the mirror and in `/opt/dassie`, so only these two shapes parse: digits, lowercase letters, dots and dashes, at most 80 bytes.
- **The updater moves on its own only to a later version of the same channel:** a higher release, or a canary of a later commit time (`Version::supersedes`). `meta/<channel>` is not signed, so a mirror cannot take a node back, or to the other channel; only the owner can. What `meta/<channel>` names must be of that channel, or the update fails: when the owner switches with `--channel`, the mirror chooses neither a version of the channel left nor an older one.

**The mirror** keeps the original's paths (§2), plus the signature:

```
meta/latest                                  the newest release, no newline
meta/canary                                  the newest canary
<version>/dassie-<version>-linux-x64.tar.gz  and .tar.xz, and the same for arm64
<version>/SHA256SUMS                         as sha256sum writes it
<version>/SHA256SUMS.minisig                 minisign, trusted comment "dassie <version>"
```

- **A bundle** holds `dassie/bin/dassie` and the systemd units in `dassie/share/systemd`; `dassie/` becomes `/opt/dassie/<version>`. Its archives are the same bytes for the same input: sorted, with fixed times and owners.
- **The signature** is minisign's default, Ed25519 over the BLAKE2b-512 of `SHA256SUMS`. Its trusted comment names the version, so the signed sums of one release cannot pass for another's. Legacy signatures (of the file itself) are refused.
- **`dassie-release` verifies it** with `ed25519-dalek`'s strict verification, and parses `SHA256SUMS` only once the signature holds. Every parser there is bounded: a version at most 80 bytes, a signature at most 1 KiB, `SHA256SUMS` at most 4 KiB and 16 lines of plain file names.

**Publishing** (`.github/workflows/release.yml`):
- Every tag `v<version>` builds a release, whose version must be the tag's. A canary is built and signed by hand; no workflow publishes one.
- The release checks run there, not in CI: the install tests, one job per distribution, and `static-binaries`. Only once they pass is anything signed.
- `scripts/release.ts` checks each bundle against `SHA256SUMS` and signs it with the release key. Then `dassie-release verify` checks the signed tree with `resources/minisign.pub`, using the code the updater trusts a release with, before anything is uploaded.
- The key and the bucket's credentials belong to the `Release` environment, which admits only `v*` tags. The job's actions are pinned to commits.
- The upload goes to the bucket behind `get.dassie.land`, through workload identity.
  - A version's files are uploaded once, the signature last, and cached for good. A file already there must be the same bytes, so a run again completes what an interrupted one left.
  - Only then does `meta/<channel>` name the version, uncached. It never moves to a version older than the one it names.
  - A release also publishes `install.sh`, with the release key in it, which `sh.dassie.land` serves.

**The install** keeps the original's layout ([2 §3.1](02-node-runtime.md)): `/opt/dassie/<version>`, `/opt/dassie/current` pointing at the one that runs, `/usr/local/bin/dassie`, the group `dassie-users`, and the systemd units of `resources/systemd`, which each bundle carries in `share/systemd`.
- **`dassie.service`** runs `dassie daemon --runtime=systemd`, which takes its sockets by name and reports readiness itself.
  - It runs as a dynamic user, with `/var/lib/dassie` its state directory (0700).
  - It is sandboxed: no capabilities, a read-only system, no home directories, private `/tmp` and devices, only Unix and IP sockets, no namespaces, no writable executable memory.
  - It has 5 minutes to report readiness, so a node that never comes up after an update fails, and the update is rolled back.
  - The socket units listen on ports 80 and 443 (IPv4 and IPv6) and on `/run/dassie.sock`, which root and `dassie-users` may use.
- **`dassie-update.timer`** starts `dassie-update.service` daily, at a random time in the hour after 03:20. The service runs `dassie update`. A drop-in setting `DASSIE_MIRROR_URL` points it at another mirror.

**`install.sh`** (`resources/installer`, POSIX sh, one function called on its last line):
- It needs Linux, systemd (running, or still starting, as when cloud-init runs it), curl or wget, tar, and minisign or OpenSSL 1.1.1 or later. It takes the xz bundle where there is `xz`, and the gzip one otherwise.
- It sets `umask 022`: on a hardened host, what it installs must still be readable by the node's own user.
- Run again, it installs the channel's newest version, but not one older than the one installed: `meta/<channel>` is not signed. `--target` installs any version.
- **Nothing is unpacked before it is verified.**
  - The signature of `SHA256SUMS` is checked with the release key, built into the script at release. With minisign that is `minisign -V`; with OpenSSL it is minisign's format by hand: the key ID, Ed25519 over BLAKE2b-512 of the file, and the global signature over the trusted comment.
  - The trusted comment must be `dassie <version>`, and the bundle must match its sum.
  - The unpacked bundle may hold only files and directories, and its binary must name the version and pass `verify-install`, before anything is installed.
- **It installs:**
  - the version (one installed again is renamed aside first), then `sync`, then `current` with one rename (`mv -T`);
  - the command's link and the group, adding the user who ran it, through sudo or with sudo for each step;
  - the units, enabled and started.
- **`uninstall`** stops and removes all six units, `/opt/dassie` and the command's link. It keeps the data and the group.
- Options: `--channel latest|canary`, `--target <version>`, `--mirror <url>`, as `DASSIE_CHANNEL`, `DASSIE_VERSION` and `DASSIE_MIRROR_URL` too. Errors keep the original's `E_*` codes.

**`dassie update`** (`dassie-cli`):
- **The install:** it finds it from its own path, which must be `<base>/<its version>/bin/dassie`, and holds `<base>/.update.lock` while it works.
- **What it moves to:** it reads `meta/<channel>` of its own channel and moves only to a version that supersedes its own. `--target-version`, `--channel` and `--force` let the owner go elsewhere.
- **Verifying:**
  - `SHA256SUMS` is verified with the release key built into the binary. The build takes it from `resources/minisign.pub`, or the file `DASSIE_RELEASE_KEY` names. A binary without one refuses to update, and `verify-install` says so.
  - Then the bundle for its platform must match its sum. The downloads are bounded: 64 MiB for the bundle, 10 minutes.
- **Unpacking:** only into a staging directory beside the versions. `dassie_release::bundle` admits only directories and regular files under `dassie/` with plain names, at most 64 entries and 256 MiB.
  - Modes are set explicitly (0755 for directories and executables, 0644 for the rest), whatever the umask.
  - Every file and directory is synced, so `current` never names a version a power cut left half written.
- **Checking the new binary:** it must name the version and pass `verify-install`, each within 60 s.
- **Switching:**
  - the version moves into place;
  - the units are installed, each only if it changed (`DASSIE_SYSTEMD_UNIT_DIR` for tests). The owner's changes belong in drop-ins, which the updater leaves alone;
  - `current` moves with one rename, synced;
  - systemd reloads if a unit changed, units new in the release are enabled, and the node restarts (unless `--no-restart`, or systemd is not running).
- **Rolling back:** if the node does not come up, `current` goes back to the version that ran, with its units. The node is stopped, and if the new version migrated the database, the copy from before the migration is put back. Then the old version starts. The update fails.
  - The daemon writes that copy itself, `dassie.sqlite3.pre-migration` beside the database, with `VACUUM INTO`, just before it migrates and before it serves anything; nothing the copy lacks has reached a peer.
  - The updater puts it back only if the old version could not open the database as it is (its schema is newer), and only if the copy is from this update, by its time. The database it replaces is kept as `dassie.sqlite3.rolled-back`, for the owner to look at. The updater finds the data directory at `/var/lib/dassie`, or where `DASSIE_STATE_DIRECTORY` says.
- **Recording:** each update writes how it went (up to date, updated, rolled back or failed, with the reason) to `<base>/last-update`, for `dassie status`.
- **Pruning:** it keeps the version it came from and the new one, and removes every other version and what an interrupted update or install left.
- **The mirror** must be `https://`, and may be on a private network: the owner names it. `DASSIE_MIRROR_CA_FILE` adds roots to trust it by.

**Tests:**
- `crates/dassie-cli/tests/update.rs` runs the binary against a mirror served over TLS from an install in a temporary directory. The binary is built with the `test-key` feature, which trusts `testdata/release`'s key and is never in a release; the Nix check enables it. The tests check:
  - installing, being up to date, and an older version on offer;
  - tampered bundles, sums signed for another version, a bad or missing signature, links and paths outside `dassie/`, and a binary that names another version;
  - one update at a time, and modes that let the node run under a strict umask;
  - the record each update leaves.
- The store's tests check the copy before a migration, and that a rollback puts it back only when it must and only a copy it can trust.
- `tests/e2e/install_test.ts` installs, updates and uninstalls on Debian 12, Ubuntu 24.04, Fedora 42 and CentOS Stream 9 (item 6 of §7), the installer under `umask 077`; a re-run installer does not go back to an older version the mirror offers.
- The installer's ShellCheck runs with `nix flake check`.

## 5. Wallet and dashboard (Solid 2)

- Same pages, flows and copy as [6 §1.5–§1.6](06-user-interfaces-and-tooling.md), with the listed UI gaps fixed (UI-1 to UI-8). In particular:
  - multi-currency amounts;
  - adding XRPL ledgers;
  - deposit address and QR code for XRPL;
  - a working payment status page.
- **Key derivation in the browser uses `dassie-identity` WASM.** The mnemonic is validated (UI-5). Setup sends only the node key and the login proof, as today.
- **The live data model.** Every `subscribe*` procedure delivers either:
  - a value stream (signals), or
  - a snapshot followed by typed deltas (stores).

  In the client, these feed Solid 2 signals and stores. This replaces `useRemoteSignal` and `useRemoteStore`, and it no longer needs the server's reducer code in the browser ([6 §0](06-user-interfaces-and-tooling.md)).
- The page CSP stays strict: no inline script (the original's dark-mode snippet moves into the bundle) and nothing loaded from other origins.
- **The debug pages stay** (logs, nodes, ledger, routing, database), and a packets page joins them. The "State" page that introspected `lib-reactive` becomes a service and cell inspector fed by the supervisor (§4.1).
  - The logs page reads a ring of the node's last 1000 lines (at most 1 MiB), kept whatever `DASSIE_LOG_LEVEL` says: info and above, and debug for the targets `DEBUG` names. Fields named like secrets are redacted.
  - The database page never reads a secret column: the node key, the TLS key, the ACME credentials, session IDs and token hashes are shown as redacted.
- **Payments come with M6:**
  - **M6a:** Receive, with the wallet addresses and incoming payments, and an activity list.
  - **M6b:** the consent page for third-party grants, and a grants page to revoke them.
  - **M6c:** Send, to a payment pointer, a wallet address or a payment request, here or on another node, and the payment status page, which follows one payment live (UI-1). Activity's outgoing payments lead to it.
- **The dev dashboard** reuses the wallet's debug components (`packages/ui`). It talks to each node over the same contract, through the node's IPC socket, which the dev server relays to it; nothing in the node accepts a dev token.

**The website (M9).** `apps/website` builds `dassie.land`: the pages of [6 §3](06-user-interfaces-and-tooling.md), with the Quick Start rewritten for the installer and updater of §4.8.

- **Static pages.** A Vite build with one HTML page each for the home page, the Quick Start and the STREAM toy. A build step adds the header and footer to each page. Only the toy runs a script; it is a Solid 2 island, styled by `packages/ui`, whose log viewer it shares.
- **The same strict CSP as the wallet**, set in `vercel.json`: no inline script or style, nothing loaded from elsewhere, and WebAssembly may compile.
- **The toy** runs the sender and the receiver on `dassie-stream`'s WASM build (`@dassie/identity-wasm/web/stream`), with the original's settings and defaults. Between them sits a connector, simulated event by event on a clock of its own:
  - It refuses a packet above its largest amount with F08, with the amounts the receiver of an F08 reads.
  - It refuses a packet beyond its packets in flight with T03.
  - It forwards the rest. The answer comes back after a round trip drawn from a log-normal distribution, whose median is the latency and whose σ is the jitter.
  - An answer that would come back at or after the Prepare's expiry never reaches the sender. The connector rejects the Prepare with R00 when it expires, as a real one does, and a Fulfill that comes later is its loss.
  - The draws are seeded, so the same settings and seed replay the same run, as in the simulations of §7.
  - Time dilation sets how fast the run plays, and the run can be paused, stepped one event at a time, or finished at once.
  - A run is bounded: at most 2000 packets, 10 minutes on its clock, and 10,000 log lines.
  - The packet log decrypts each STREAM packet with the connection's shared secret, through `decodeStreamPacket` of the same WASM build.
- **Deployment.** Vercel builds the site from `apps/website` with `vercel-build.sh`, so each branch gets a preview. Vercel's build image has neither the pinned Node and pnpm nor Rust, so the script installs Node, pnpm, rustup and `wasm-bindgen`, each pinned by version and its download by SHA-256. rustup then installs the toolchain `rust-toolchain.toml` names, which rustup checks against the channel's manifest. A test keeps those versions equal to the flake's Node and pnpm and to `Cargo.lock`. `vercel-ignore.sh` skips a deployment when nothing the site is built from changed since the commit Vercel last deployed on that branch: the site, the packages it imports, the crates of its WASM modules, and the lockfiles and toolchain pins. A test keeps that list complete. Anything it cannot compare, such as a first deployment or a force push, builds.

## 6. Developer environment (Node)

`apps/dev` reproduces [6 §6](06-user-interfaces-and-tooling.md):

- the same hostnames (`dN.localhost`) and ports (SNI proxy on 443, dashboard 10100, nodes `5000+N` for HTTPS and `4000+N` for HTTP);
- mkcert certificates, vanity node identities, the database prefill and auto-login links;
- the four scenarios, plus "add node" and file-watch restart. `xrpl-settlement` runs from M7b, on the public testnet or, with `--xrpl fake`, the fake ledger (on port 10200, a new ledger each run, which nodes treat as a testnet reset); `cross-currency` runs from M7c, with d1 on the stub, d3 on the XRP Ledger and d2 on both.

The differences:

- **Nodes are real `dassie` binaries** built by `cargo build`, restarted when the Rust sources change; a restart keeps their data, while changing scenario or "reset" starts from scratch. The wallet is rebuilt by `vite build --watch`, and in dev the node serves it from that directory (`DASSIE_DEV_WALLET_DIR`) instead of its embedded copy. A change shows after a reload, under the same CSP as in production; there is no HMR proxy in the node.
- **Prefill goes through the node's own RPC and HTTP API**, as the end-to-end tests set nodes up, not by writing the database directly. That keeps the schema private to the node. It can run again on a node that is set up already, which it then leaves as it is. The end-to-end tests use the same scenario code.
- **One origin for the dashboard.** The dashboard, its control channel (`proto/dassie/dev/v1`) and the relay to the nodes are served on one port (10100; 10101 answers the same, and the proxy sends `dev-rpc.localhost` there too), so the dashboard keeps the strict CSP with `connect-src 'self'`. Everything listens on loopback only, the nodes too (`dassie daemon --listen 127.0.0.1`), since their keys are public in the vanity table; and every WebSocket upgrade needs a `Host` of `localhost` or `dev-rpc.localhost` and an `Origin` of the same site, so that no other page and no rebound name can reach the nodes as their owner.
- **Logs, peer traffic and peering state come from the nodes' own RPC** (`SubscribeLogs`, `SubscribePackets`, `SubscribeNodes`), not by patching the logger. The dev server subscribes once per node over IPC and passes them on to every open dashboard, together with its own log. The node pages of the dashboard reach their node through the relay instead. Peer traffic is ILP packets between peers; the peer protocol's own messages are not shown.
- **Auto-login links** carry the node's login proof in the URL fragment (`#dassie-dev-login=…`), which only a wallet built in development mode reads; the node checks it like any other login. The production wallet has no such code.
- **`*.localhost` resolves to loopback** in the node's peer client (RFC 6761), when private addresses are allowed, so dev nodes reach each other by name on any system.
- **The vanity identity table is regenerated** with `dassie-identity` (same derivation, so the existing table stays valid). It keeps each node's BIP-39 entropy and ID.
- **Exchange rates.** Each scenario serves its own `rates.json`, in Coinbase's format (a dollar buys two XRP), over HTTPS under its CA on port 10210 plus its offset; the prefill makes it every node's only rate source.

## 7. Testing strategy

Deterministic simulation testing (item 4) is the main source of confidence, and the other levels feed it:

- golden vectors pin the wire formats a simulation cannot judge;
- interop pins the standards;
- end-to-end tests check the real IO layer that a simulation replaces.

Every crate is written so that a simulation can drive it ([D16](08-decisions.md#d16-engineering-style-pragmatic-tigerstyle-and-deterministic-simulation)). Every simulation or property-test failure prints the seed that reproduces it.

1. **Golden vectors.** The original's fixtures and computed vectors become test data:
   - ILP and ILDCP packets, every STREAM frame, STREAM crypto ([4 §1.9, §6.1, §11.2, §11.3](04-interledger.md));
   - node IDs of the bootstrap nodes ([3 §2.2](03-peer-network.md));
   - the BIP-39 → node key → login secret chain;
   - the XRPL wallet derivation for a fixed passphrase;
   - for Open Payments, the examples of the standards: RFC 9421 (Ed25519 signatures), RFC 9530 (`Content-Digest`), RFC 8941 (structured fields), the interaction hash of the Open Payments docs, and the repeating intervals of its specs.

   The vectors are generated from the original code by `testdata/original/` (this includes the independently computed ones of [3 §5.5](03-peer-network.md), now confirmed).
2. **Property tests** for `dassie-oer` (round trip, canonical-only parse), the ledger (no transfer breaks a limit; balances equal the journal), and STREAM allocation ([4 §11.11](04-interledger.md)).
3. **Interop.** A Node test harness (`node --test`) runs:
   - `dassie-stream` (native and WASM) against the npm `ilp-protocol-stream`, both directions;
   - the BTP server against `ilp-plugin-btp`;
   - Open Payments against the reference client `@interledger/open-payments`, as a third-party client of the node; and the node as a client of another node.
4. **Deterministic network simulation.** N nodes are wired in-process through the sans-IO crates with a mock clock and seeded RNG. They cover discovery, majority computation, flooding suppression, peering, routing convergence, settlement thresholds, and restart recovery.
   - The simulated network injects latency, loss, reordering, duplication, partitions, crashes and restarts.
   - It checks every state machine's invariants after each step. Beyond them it judges each node on its own, by its own books, and never compares two nodes' books or adds them up: each node keeps its own, and they may drift ([D19](08-decisions.md#d19-each-node-keeps-its-own-books)).
     - **Against what really happened:** a node's books hold what its scheme holds; its owner's funds moved by what its receivers took and its payers sent, and each token's by what its client sent. The underlying ledgers neither make nor lose money, each asset apart in runs across currencies (M7c).
     - **Its decisions, against the liquidity its books show:** it refuses a Prepare for liquidity only by the limits its books set (the ledger's property tests check those); it takes every settlement an honest peer sends; once the network is calm, it owes no peer past its own threshold for long; and when a peering ends, its books say it owes nothing.
     - **Against adversaries:** whatever a lying peer does (refuse the Fulfills it took, never settle, spend the credit it was given and vanish), an honest node loses no more than D18's bound; a liar that stays is banned, one that vanishes loses its peering, and no honest peer is banned.
     - **Liveness:** payments complete, settlements arrive, and every verdict owed is answered.
   - The peer protocol's own simulation checks what the protocol must make agree, which is not money: link states and routes converge, peerings are two-sided, and each settlement is applied exactly once.
   - A run is reproducible from its seed, and two runs with the same seed must produce identical event traces.
   - Simulations start small in M1 (STREAM connections over a simulated connector) and grow with each milestone. In CI they run with fixed seeds, and longer randomized campaigns run outside CI.
   - Every decoder also has a "never panics on arbitrary input" property test.
5. **End to end.** The dev scenarios run headless in CI: start N nodes, set up through the HTTP API, add ledgers, peer, pay, settle, restart, and check that the balances are unchanged.
6. **Installer and updater.** Containers with systemd, as the original's `test-in-docker:*` scripts did ([6 §5.5](06-user-interfaces-and-tooling.md)), including signature-verification failure cases.
   - `tests/e2e/install_test.ts` boots Debian, Ubuntu, Fedora and CentOS images (pinned by digest, with systemd as PID 1). Caddy serves a mirror on a Docker network of the test's own.
   - The releases come from `nix build .#install-test-mirror`: 1.0.0 and 1.0.1, built in a cheaper profile and signed with the test key.
   - The images between them take both verifiers (OpenSSL and minisign), both downloaders (curl and wget) and both compressions.
   - The release workflow runs one job per distribution, before it signs; CI does not run them.
7. **Website.** `tests/e2e/website_test.ts` serves the built site with the headers of its `vercel.json`, so that its CSP applies, and drives it in headless Chrome. The toy runs a payment to the end, and its packets decrypt. The toy's simulation is also tested in Node against the same `web` build of the WASM module that the browser loads.

## 8. Milestones

| # | Milestone | Done when |
|---|---|---|
| M1 | Core crates: `dassie-oer`, `dassie-identity`, `dassie-ilp`, `dassie-stream` | All golden vectors pass. STREAM interoperates with `ilp-protocol-stream`. WASM builds load in Node and the browser. |
| M2 | Node skeleton: config, DB, listeners (plain and socket-activated), TLS, ACME, setup/login/logout, RPC contract, `dassie init`/`daemon`/`verify-install` | A fresh install reaches "setup complete" in a browser with a Let's Encrypt staging cert. |
| M3 | Ledger journal, connector, local endpoints, ILDCP, BTP, stub settlement module. Exchange-rate math and its T00 rule; the rate fetcher comes in M7 with the first second currency | A BTP client pays the node's STREAM receiver. The journal matches the packet trace. |
| M4 | Peer protocol v1, node table, membership, registration, auto-peering, link state, routing, settlement loop. Delivered in two PRs: **M4a** (v1, membership, peering, routing, packets over peers) and **M4b** (settlement loop, the D12 lifecycle: termination, bans, the advertise flag) | The `two-nodes` and `six-nodes` scenarios converge and route payments across hops. Restarts keep peerings and balances. M4a stops at payments within the credit peers extend, with restarts that keep peerings; M4b pays several times the credit across hops, so every hop must settle. Child peers move to the milestone that adds registration fees ([D12](08-decisions.md#d12-peering-lifecycle-and-liveness)). |
| M5 | Wallet (Solid 2) and dev environment (Node) with dashboard, including BTP token spending caps. Delivered in two PRs: **M5a** (the node's live streams and inspectors, `GET /stats`, spending caps, every wallet page) and **M5b** (`node --run dev`, the dev dashboard, `packages/ui`) | All pages of [6 §1.6](06-user-interfaces-and-tooling.md) but Send and payment status work against M4. `node --run dev` brings up any scenario that M4 can run (the XRPL ones come with M7). |
| M6 | Open Payments ([D9](08-decisions.md#d9-open-payments-fully-and-no-spsp)): wallet addresses, GNAP, incoming payments, quotes, outgoing payments, and the wallet's Send, payment status and grant pages. Delivered in three PRs: **M6a** (`dassie-payments`, HTTP message signatures, non-interactive grants, wallet addresses, incoming payments, Receive and Activity), **M6b** (quotes, outgoing payments, grant limits, interactive grants with the owner's consent, token management, and reading incoming payments on other nodes to pay them) and **M6c** (creating incoming payments on other wallets, payment pointers as input, Send and payment status) | Two nodes pay each other through Open Payments, and the reference client runs the full third-party flow against a node. M6a stops at the reference client paying into an incoming payment, with state and books exact. M6b stops at the reference client's full third-party flow from one node to another, with consent given in the wallet. M6c closes it: two nodes pay each other from the wallet's Send, to a payment pointer and to a payment request. |
| M7 | XRPL testnet module with reserve and fee accounting; the exchange-rate fetcher of D7, fees and spread; each peer's grace limit ([D18](08-decisions.md#d18-one-grace-limit-per-peer)), before any real money. Delivered in three PRs: **M7a** (grace limits, automatic bans, and the owner's group taking in what the node gains or loses), **M7b** (the XRPL testnet scheme, with a fake ledger for CI and the simulation) and **M7c** (the rate fetcher, fees and spread, cross-asset quotes) | The `xrpl-settlement` and `cross-currency` scenarios settle and reconcile across restarts. A peer past its grace limit loses its peering. M7a stops at liars banned, and no honest peer banned, in the simulation; M7b at `xrpl-settlement`; M7c at `cross-currency`. |
| M8 | Release: static binaries (x64, arm64), signed artifacts, installer, updater, systemd units. Delivered in two PRs: **M8a** (the static binaries with the wallet, the bundles and the mirror tree, SHA256SUMS signed with minisign, the release workflow) and **M8b** (the systemd units, the installer, `dassie update`, and their tests in containers) | Installer and updater tests pass on Debian, Ubuntu, Fedora and CentOS images. A tampered bundle is refused. M8a stops at signed releases built and published by CI, verified by `dassie-release`. |
| M9 | Website and STREAM toy | `dassie.land` builds from `apps/website`. The toy runs on `dassie-stream` WASM. |
| M10 | Trust from earnings ([D20](08-decisions.md#d20-trust-follows-what-a-peer-earned-us)): junior and senior, the senior's credit starting at 0, credit that grows with what a peering earned, and its announcement | In the simulation, a junior sends through its senior only after settling in advance; credit grows only with fees, by at most k of them; a junior that spends its credit and vanishes costs its senior no more than that; and settlement keeps working when announcements are lost, repeated or reordered. |
