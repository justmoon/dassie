# 7. The rewrite: target stack, architecture and plan

Chapters 1–6 describe what the original Dassie does. This chapter describes what the new implementation is: which behaviour it keeps, how it maps onto the Vorstand stack (Rust, Deno, Solid 2), and in what order to build it. The choices that were open are recorded, with their reasons, in [chapter 8](08-decisions.md).

## 1. Goals and non-goals

**Goals**

- Everything the original does that works, with the same user-visible behaviour: install, `dassie init`, browser setup and login, ledgers, discovery, auto-peering, routing, forwarding, settlement, sending and receiving, the debug pages, the multi-node dev environment, and the website's STREAM simulator.
- The behaviour the original *intends* but does not deliver. That covers every gap in chapters 2–6 that is a plain bug or TODO (default disposition: fix, see [8 §1](08-decisions.md)).
- One self-contained release artifact per platform, with no bundled interpreter or native add-on, built for x64 and arm64.
- Tests at every level. The original has none for the app, the GUI or the tooling (UI-17).
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
| Open Payments wallet address and incoming payments, SPSP ([4 §9](04-interledger.md)) | Per the specs | Other wallets resolve `$host`. Add SPSP ([D9](08-decisions.md#d9-open-payments-and-spsp-scope)). |
| XRPL settlement memo type and the `settlement` message semantics | Yes within one network version | Both sides of a peering must agree; it changes only with the peer protocol version. |
| Dassie peer protocol ([3 §5–§8](03-peer-network.md)) | **No**: v1 keeps the OER message set and fixes authentication | v0 does not authenticate senders (NET-8). The network is a two-node test network. [D2](08-decisions.md#d2-peer-protocol-v0-or-v1). |
| OIDs, the `application/dassie-message` media type, `POST /peer` | Yes, with the envelope `version` byte set to 1 | Lets a v1 node recognise and refuse a v0 message cleanly (400 "unsupported version"). |
| UI ↔ node RPC (`dassie-rpc-01`, superjson) ([2 §10](02-node-runtime.md)) | **No** | Both ends are rewritten. A typed contract replaces it ([D5](08-decisions.md#d5-ui-and-cli-rpc-contract)). |
| `/api/setup`, `/api/login`, `/api/logout`, `__Host-Dassie-Session` cookie | Semantics yes, bodies may change | Same flow and security properties. |
| SQLite schema, `application_id 0x1DA53B81`, scalar names ([2 §5](02-node-runtime.md)) | **No** | A new schema designed for what must survive a restart (§5). It may use a new application ID. |
| Install layout, systemd units, socket names, `dassie` CLI verbs, env var names ([2 §3, §12](02-node-runtime.md)) | **Yes**, where sensible | Operators' muscle memory and docs. Drop variables that no longer mean anything (`NODE_ENV`). |
| Mirror layout `get.dassie.land/meta/latest`, `/<ver>/dassie-<ver>-linux-<arch>.tar.*` | Yes, plus signatures | So the installer and updater URLs keep working ([D11](08-decisions.md#d11-release-integrity)). |

## 3. How the original maps onto the stack

Vorstand's conventions apply throughout: a Cargo workspace in `crates/` (edition 2024, `rust-toolchain.toml` pinned, `Cargo.lock` committed); a Deno workspace in `apps/` and `packages/` (root `deno.json`, `deno.lock` committed, pre-release npm packages pinned exactly, `solid-js@2.0.0-rc.x` with `@solidjs/web` and Vite through `@solidjs/vite-plugin`); and normative contracts in a top-level directory that both sides are generated from. Packages meet only through their exports.

**The split ([D1](08-decisions.md#d1-what-runs-in-rust-and-what-in-deno)): the node is Rust, the browser is Solid, and the tooling is Deno.**

- The daemon, the CLI and every protocol are Rust. The result is one static binary (`dassie`) with the wallet's built assets embedded. It replaces the bundled Node.js, the esbuild bundle and the `better-sqlite3` prebuilds, along with the stale-hash build problem (UI-10) and the x64-only restriction.
- The cryptographic and protocol cores (`dassie-identity`, `dassie-oer`, `dassie-ilp`, `dassie-stream`) are also built to WASM. This follows the same pattern as Vorstand's `vault-core`:
  - the wallet derives keys in the browser with exactly the code the node uses;
  - the website's STREAM simulator runs the real STREAM implementation.
- Deno runs everything that orchestrates or builds: the multi-node dev environment and its dashboard server, the Vite builds of the Solid apps, the website, release scripts, and interop tests against the original.

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
  dassie-payments/      Open Payments + SPSP server and client, payment pointer resolution
  dassie-node-core/     the node's decisions without IO: config, setup, sessions, rate limits, ACME/TLS policy
  dassie-node/          daemon composition: config, DB, HTTP(S)/IPC listeners, TLS, ACME, auth, RPC
  dassie-cli/           the `dassie` binary: daemon | init | update | verify-install
proto/
  dassie/ui/v1/         UI and CLI RPC contract (normative; Rust and TS generated from it)
  dassie/dev/v1/        dev-environment control contract
apps/
  wallet/               Solid 2 SPA served by the node (chapter 6 §1)
  dev/                  Deno: multi-node dev environment, SNI proxy, certs, scenarios (6 §6)
  dev-dashboard/        Solid 2 dev dashboard (6 §2)
  website/              dassie.land static site + STREAM toy (6 §3)
packages/
  rpc/                  TS client for proto/dassie/ui/v1 (generated + transport)
  ui/                   shared Solid components: amount, log viewer, network graph, forms
  identity-wasm/        wasm-bindgen output of dassie-identity / dassie-stream for Deno + browser
resources/
  systemd/  installer/  launcher-less (the binary is the entry point)
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
| `app-dev` | `apps/dev` (Deno) | Spawns real `dassie` binaries instead of Vite SSR module runners. |
| `app-build` | Nix flake ([D14](08-decisions.md#d14-build-system)) and Deno release scripts; `cargo build --release` for musl x64/arm64 | Sign artifacts ([D11](08-decisions.md#d11-release-integrity)). |
| `app-website` | `apps/website` | Content from [6 §3](06-user-interfaces-and-tooling.md). The STREAM toy uses `dassie-stream` WASM. |
| `app-cli` (npm placeholder) | dropped | Keep the npm name reserved. |
| `meta-*` | `deno fmt/lint/check`, `cargo fmt/clippy/test` | |

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
| BTP tokens | plaintext | hashed, plus a label and created time; shown once at creation |
| Node table: nodes, link states (signed bytes), registrations, peers with scheme state | written only by dev prefill | written on every change (NET-1, NET-2, ACC-18) |
| Own link-state sequence | memory | persisted high-water mark (NET-7) |
| Settlement channels: per node ever peered with, the highest settlement sequence applied and sent, and the settlement message not yet acknowledged | none | in the batch that changes them; kept after the peering ends ([§4.7](#47-peer-protocol-v1)) |
| Bans | none | a table of node IDs |
| Replay state of peer messages | none (no replay protection) | per peer, the newest accepted message timestamp, written with the batch it took effect in ([§4.7](#47-peer-protocol-v1)) |
| Ledger | cumulative posted totals | append-only **transfer journal** (id, debit, credit, amount, state, timestamps, reference to packet or settlement), with account balances cached in the same transaction. Only transfers that reach posted balances are journaled, plus pending settlements; a voided packet transfer never touched a posted balance and leaves no row |
| Pending ILP transfers | memory | memory only: in-flight packets cannot survive a restart, so a restarted ledger has none |
| Pending outgoing settlements | memory | journal rows with module-specific reference (tx hash), reconciled on startup (ACC-16) |
| Settlement-module balance checkpoints | none (full balance re-deposited at every start) | last reconciled on-ledger balance per module; startup books only the difference, through `equity/suspense` (ACC-6, ACC-2) |
| Peer trust lines | re-posted at every start | posted once when the peer account is created; changes are explicit transfers (ACC-5) |
| Incoming and outgoing payments | rows without state | full Open Payments state: received/sent amounts, completion, expiry, STREAM credentials tied to the payment (ILP-10) |
| Exchange rates | memory | last good snapshot with timestamp; refreshed periodically ([D7](08-decisions.md#d7-exchange-rates-fees-and-spread)) |

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

**The owner's accounts ([D6](08-decisions.md#d6-owner-ledger-and-multi-currency-accounts)).** Each ledger `L` has one pool of owner funds, `L:equity/owner`. Every endpoint that spends for the owner gets a sub-account under it, for example `L:equity/owner/btp/<token>` for a BTP token. A sub-account holds no allocation and may go negative on its own; together with the pool it forms a **group** whose total carries the overspending limit (debits ≤ credits). So:

- a BTP client paying the node's own STREAM receiver books Dr `equity/owner/btp/<token>`, Cr `equity/owner`: the group total is unchanged, and the journal still has a transfer per packet (ACC-8);
- a transfer within the group never needs free funds, since its effect on the total is zero;
- each token's spending can be read from the journal, which is what the per-token spending caps of M5 (an amount per window, refilling continuously) are computed from. Caps are limits, not allocations, so they may add up to more than the funds.

**What the ledger refuses and what it records.** Packets and settlements are refused when they would break a limit (T04). Reconciliations, deposits and withdrawals report what happened on the underlying ledger and are recorded even when they take the group below zero; spending is then refused until it recovers. Settlement modules report their on-ledger balance as a checkpoint, and activation books only the difference to the last checkpoint, through `equity/suspense` (ACC-2, ACC-6). The checkpoint is written in the same transaction as the transfers it explains.

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
  - Both have a 1 MiB message limit, and per-connection limits on concurrent calls and open streams.
  - The node caps the number of open WebSockets. A browser's socket with no call in flight closes after 10 minutes without a message; the client reconnects on its next call.
  - A client that sends faster than it reads is disconnected, rather than allowed to queue answers without bound.
  - Every way a session ends (logout, revocation, expiry, eviction for a newer one) closes its sockets with code 4001, after the calls already answered have their answers.
- **The browser client** gives every call a timeout (RT-27).

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
  3. Once no packet is in flight with the peer and no response to it is waiting to be sent again, the peering ends. Its accounts stay, with their balances: they are the record of what is owed, and a later peering with the node uses them again without posting trust a second time.
  4. If the exchange of step 2 completed, each side then settles everything it owes. That is kept on the settlement channel, across restarts, until it is made; a cancelled or failed attempt is tried again. An unresponsive peer is not settled with; the debt stays on the books.
  5. The node does not ask the peer again for 24 h after an owner's termination, or after its usual backoff otherwise. A request from a node we are terminating with, or whose peering an owner ended in the last 24 h, is refused with reason 9; that cooldown does not survive a restart, and a ban is the lasting way.
- **Bans** are a list the owner keeps. A banned node is never asked, is refused with reason 8, and its messages do not confirm it as a peer. Its link states still flood.
- **One-sided peerings.** A node that holds no peering with a sender, and is not asking for one, answers an authenticated heartbeat that lists it as a neighbor with a `peeringTermination` of reason 3, at most once per backoff period. The other side then ends its peering.

**Settlement** ([5 §4.6](05-accounting-and-settlement.md), kept with the credit the peer extends; ACC-24):

```
balance    = IL.credits_posted − IL.debits_posted − IL.debits_pending     -- IL = L:assets/interledger/<peer>
credit_out = TR.debits_posted − TR.credits_posted                          -- TR = L:contra/trust/<peer>
midpoint   = (credit_in + credit_out) / 2                                  -- credit_in as the peer announced it
settle balance − midpoint when balance ≥ midpoint · 1.1; on termination, settle balance − credit_out
```

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

**Packets.** A Prepare that surely did not reach its peer (a refused connection, or 400, 401, 409 or 503 in reply) is rejected at once. One that may have reached it is left to expire. A Fulfill or Reject for a peer is sent again, every second, until it arrives or 30 s have passed: the peer's books depend on it, and a copy that arrives twice is dropped by its connector.

**Outbound requests** go only to `https` URLs. Requests to private, loopback, link-local or similar addresses are refused, after DNS resolution, unless the operator allows them (for dev and tests). The node never follows redirects.

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
- **The debug pages stay** (logs, nodes, ledger, routing, database). The "State" page that introspected `lib-reactive` becomes a service and cell inspector fed by the supervisor (§4.1).
- **The dev dashboard** reuses the wallet's debug components (`packages/ui`). It talks to each node with a per-node dev token over the same contract.

## 6. Developer environment (Deno)

`apps/dev` reproduces [6 §6](06-user-interfaces-and-tooling.md):

- the same hostnames (`dN.localhost`) and ports (SNI proxy on 443, dashboard 10100, control 10101, nodes `5000+N`);
- mkcert certificates, vanity node identities, the database prefill and auto-login links;
- the four scenarios, plus "add node" and file-watch restart.

The differences:

- **Nodes are real `dassie` binaries** built by `cargo build`, restarted when the Rust sources change. The wallet is served by Vite with HMR from the node's HTTPS port, by proxying non-API paths in dev, as the original did.
- **Prefill goes through the node's own CLI and RPC** (for example `dassie dev seed --config <json>` behind a dev feature flag), not by writing the database directly. That keeps the schema private to the node.
- **Logs, peer traffic and peering state reach the dashboard** through the dev contract (`proto/dassie/dev/v1`), not by patching the logger.
- **The vanity identity table is regenerated** with `dassie-identity` (same derivation, so the existing table stays valid).

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
   - the XRPL wallet derivation for a fixed passphrase.

   The vectors are generated from the original code by `testdata/original/` (this includes the independently computed ones of [3 §5.5](03-peer-network.md), now confirmed).
2. **Property tests** for `dassie-oer` (round trip, canonical-only parse), the ledger (no transfer breaks a limit; balances equal the journal), and STREAM allocation ([4 §11.11](04-interledger.md)).
3. **Interop.** A Deno test harness runs:
   - `dassie-stream` (native and WASM) against the npm `ilp-protocol-stream`, both directions;
   - the BTP server against `ilp-plugin-btp`;
   - Open Payments and SPSP against the reference clients.
4. **Deterministic network simulation.** N nodes are wired in-process through the sans-IO crates with a mock clock and seeded RNG. They cover discovery, majority computation, flooding suppression, peering, routing convergence, settlement thresholds, and restart recovery.
   - The simulated network injects latency, loss, reordering, duplication, partitions, crashes and restarts.
   - It checks every state machine's invariants after each step, and global properties such as conservation of money across all ledgers.
   - A run is reproducible from its seed, and two runs with the same seed must produce identical event traces.
   - Simulations start small in M1 (STREAM connections over a simulated connector) and grow with each milestone. In CI they run with fixed seeds, and longer randomized campaigns run outside CI.
   - Every decoder also has a "never panics on arbitrary input" property test.
5. **End to end.** The dev scenarios run headless in CI: start N nodes, set up through the HTTP API, add ledgers, peer, pay, settle, restart, and check that the balances are unchanged.
6. **Installer and updater.** Containers with systemd, as the original's `test-in-docker:*` scripts did ([6 §5.5](06-user-interfaces-and-tooling.md)), including signature-verification failure cases.

## 8. Milestones

| # | Milestone | Done when |
|---|---|---|
| M1 | Core crates: `dassie-oer`, `dassie-identity`, `dassie-ilp`, `dassie-stream` | All golden vectors pass. STREAM interoperates with `ilp-protocol-stream`. WASM builds load in Deno and the browser. |
| M2 | Node skeleton: config, DB, listeners (plain and socket-activated), TLS, ACME, setup/login/logout, RPC contract, `dassie init`/`daemon`/`verify-install` | A fresh install reaches "setup complete" in a browser with a Let's Encrypt staging cert. |
| M3 | Ledger journal, connector, local endpoints, ILDCP, BTP, stub settlement module. Exchange-rate math and its T00 rule; the rate fetcher comes in M7 with the first second currency | A BTP client pays the node's STREAM receiver. The journal matches the packet trace. |
| M4 | Peer protocol v1, node table, membership, registration, auto-peering, link state, routing, settlement loop. Delivered in two PRs: **M4a** (v1, membership, peering, routing, packets over peers) and **M4b** (settlement loop, the D12 lifecycle: termination, bans, the advertise flag) | The `two-nodes` and `six-nodes` scenarios converge and route payments across hops. Restarts keep peerings and balances. M4a stops at payments within the credit peers extend, with restarts that keep peerings; M4b pays several times the credit across hops, so every hop must settle. Child peers move to the milestone that adds registration fees ([D12](08-decisions.md#d12-peering-lifecycle-and-liveness)). |
| M5 | Wallet (Solid 2) and dev environment (Deno) with dashboard, including BTP token spending caps | All pages of [6 §1.6](06-user-interfaces-and-tooling.md) work against M4. `deno task dev` brings up any scenario. |
| M6 | Open Payments + SPSP, payment pointers, outgoing payments with quotes and status | Pays to and from another Open Payments/SPSP wallet. |
| M7 | XRPL testnet module with reserve and fee accounting; the exchange-rate fetcher of D7, fees and spread | The `xrpl-settlement` and `cross-currency` scenarios settle and reconcile across restarts. |
| M8 | Release: static binaries (x64, arm64), signed artifacts, installer, updater, systemd units | Installer and updater tests pass on Debian, Ubuntu, Fedora and CentOS images. A tampered bundle is refused. |
| M9 | Website and STREAM toy | `dassie.land` builds from `apps/website`. The toy runs on `dassie-stream` WASM. |
