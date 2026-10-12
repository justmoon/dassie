# 8. Decisions and gap register

Chapters 2–6 record 124 numbered gaps in the original, each with a prefix for the chapter it is in:

| Prefix | Chapter | Count |
|---|---|---|
| RT | [2 §20](02-node-runtime.md) | 28 |
| NET | [3 §19](03-peer-network.md) | 36 |
| OER-q | [3 §19](03-peer-network.md) | 3 |
| ILP | [4 §14](04-interledger.md) | 14 |
| ACC | [5 §10](05-accounting-and-settlement.md) | 26 |
| UI | [6 §9](06-user-interfaces-and-tooling.md) | 17 |

Many more are marked inline as "Deviation" or "Open question / known gap". This chapter says what the rewrite does about all of them.

## 1. Default disposition

**Every gap is fixed** unless it is listed in §2 (kept on purpose) or depends on a decision in §3. "Fixed" means:

- implement what the original's design notes or code comments say was intended; or, where there is no stated intent,
- implement the governing standard (X.696, ILPv4, RFC 29 STREAM, RFC 31 ILDCP, RFC 23 BTP, RFC 35 ILP-over-HTTP, SPSP, Open Payments) or the obvious safe behaviour.

Every fix gets a test that would have failed against the original's behaviour.

## 2. Original behaviour kept on purpose

| What | Where | Why |
|---|---|---|
| Node ID algorithm, including stripping trailing `A`s | [3 §2.2](03-peer-network.md) | Node identity continuity. |
| BIP-39 seed → `HMAC-SHA256` seed-path chain, `node`, `node/login`, `node/settlement-module/<scheme>/<path>` | [3 §2.1](03-peer-network.md), [5 §4.5](05-accounting-and-settlement.md) | The same passphrase recovers the same node and the same ledger accounts. |
| XRPL wallet: first 16 bytes of entropy as an ed25519 family seed | [5 §6.3.1](05-accounting-and-settlement.md) | Funds continuity. |
| Ed25519 verification accepts ZIP-215 | [3 §2.1](03-peer-network.md) | Harmless. Being stricter could split the network on edge-case signatures. Kept for v1 as well (confirmed with D2): use `verify`, not `verify_strict`. |
| Majority rule: strictly more than N/2 of *configured* bootstrap nodes | [3 §11.4](03-peer-network.md) (NET-16) | The code is right and the doc's "N/2 + 1" is the outlier. Fix the doc, not the code. |
| Gossip parameters: suppress after 3 copies, 1–500 ms forward jitter, 5–20 s heartbeats, 3 h staleness, 1 h re-issue | [3 §17](03-peer-network.md) | Sound defaults. Make them configurable for tests. |
| Settlement trigger: every 4 s, threshold = credit midpoint × 1.1, settle down to the midpoint | [5 §4.6](05-accounting-and-settlement.md) | Keep the policy. Fix the credit asymmetry (ACC-24) with D6. |
| Default trust line 1.1 units per peer, `MAX_PACKET_AMOUNT` 0.1 units | [5 §2.7](05-accounting-and-settlement.md), [4 §4.3](04-interledger.md) | Test-network values. Become per-ledger config under D6. |
| STREAM extensions: AIMD concurrency, redistributing over-allocation, bigint amounts | [4 §11.13](04-interledger.md) | Deliberate improvements, compatible on the wire. |
| `__Host-` cookie with HttpOnly, Secure, SameSite=Strict | [2 §8](02-node-runtime.md) | Correct as is. Add expiry (RT-11). |
| One-time in-memory setup token, routes that exist only in the right state | [2 §8.2](02-node-runtime.md) | Simple and safe. Also fix the TLS-less setup URL (RT-2). |
| `curl \| sh` installer UX, `/opt/dassie/<ver>` + `current` symlink, socket activation, nightly update timer | [6 §5](06-user-interfaces-and-tooling.md) | Operational continuity. Add signatures (D11). |

## 3. Decisions

All decisions below were confirmed on 2026-10-11. Each states the **decision** and, where one was considered, the **rejected** alternative and why.

### D1. What runs in Rust and what in Deno

- **Decision:** the node (daemon, CLI, all protocols) is Rust. The wallet and dev dashboard are Solid 2, built with Deno and Vite. The dev environment, website, build and release scripts, and interop tests are Deno. The core crates also ship as WASM ([7 §3](07-rewrite.md#3-how-the-original-maps-onto-the-stack)).
- **Rejected:** the node is a Deno app (as Vorstand's servers are), calling Rust cores through WASM or FFI.
- **Why not:**
  - The packet path, the ledger and the long-running network services benefit from Rust's performance and type system.
  - A static binary is what the install and update story wants.
  - A Deno node would bring back a bundled runtime, the problem the original had with Node.
- **Affects:** everything.

### D2. Peer protocol: v0 or v1

- **Decision:** v1, with envelope `version = 1`. It keeps the v0 message set and OER layouts ([3 §5](03-peer-network.md)) and changes:
  - **Sender authentication** (NET-8): the MAC key comes from a static-static X25519 between the sender's and the recipient's node keys, mixed with the ephemeral key. The MAC also covers `version` and `sender`. (Rejected: an Ed25519 signature by the sender over `version ‖ sender ‖ content ‖ timestamp`, which costs a signature per message.)
  - **Replay protection:** a timestamp plus a per-sender nonce window.
  - **Peering requests** carry the full signed node info, which is verified, and must match the sender (NET-3, NET-4).
  - **The HTTP status is meaningful:** non-200 responses are errors (NET-11).
  - **Peering termination** messages exist (NET-13; see D12).
  - **Settlement-module versions** are exchanged during peering (NET-26).
- **Cost:** v1 nodes cannot talk to the two v0 test nodes. Both are operated by the project and can be replaced.
- **Rejected:** stay v0-compatible and add v1 later. This means shipping a known sender-forgery hole (forged `interledgerPacket` and `settlement` messages from any peered node).

### D3. Database continuity

- **Decision:** start a new schema. Don't migrate v0 databases. A v0 owner re-runs setup with their passphrase, which yields the same node ID and the same XRPL account. The things that do not carry over:
  - peerings (v0 never persisted them anyway);
  - ledger totals (v0's are inflated on every restart, ACC-5 and ACC-6);
  - BTP tokens (the owner re-creates them).
- **Not now:** an import command for `config.*` scalars and `btp_tokens`. It is cheap to add later if anyone asks.

### D4. Network membership economics

- The design calls for:
  - a paid KNL registration whose fee grows with list size;
  - paying the cheapest 75% of bootstrap nodes;
  - removing nodes that lose majority or go silent ([3 §18](03-peer-network.md), NET-17, NET-18).

  The original implements none of it.
- **Decision:**
  - Implement **removal and expiry** now: majority loss, a staleness timeout, and registration expiry without renewal.
  - Implement **automatic registration and renewal** now: register with every BNL node, and re-register before expiry.
  - Implement the **fee quote and payment** after M6, once outgoing payments work, as a v1.x protocol addition: a quote message plus payment by ILP with a signed quote condition (the "naive" token payment variant, [5 §9](05-accounting-and-settlement.md)).
- **Fee schedule:** set when the fee quote and payment are implemented after M6. Registration is free until then.
- **Payment target:** the cheapest 75% of bootstrap nodes, as the design doc says. The UI's 80% changes to match.

### D5. UI and CLI RPC contract

- **Decision:** a normative contract in `proto/dassie/ui/v1`, following Vorstand's `proto/` convention. It is generated into Rust and TypeScript (`packages/rpc`) and carried over one WebSocket per client:
  - **Shape:** unary calls and server streams. Streams are either *value* streams or *snapshot + delta* streams.
  - **Messages:** protobuf binary frames.
  - **Amounts:** fixed-width integers with explicit scale.
  - **Errors:** typed, never a success response that carries a failure (fixes RT-5).

  The same contract serves the IPC socket for the CLI.
- **Procedure set:** that of [2 §10.3](02-node-runtime.md), extended by the missing setters (RT-4), XRPL ledger management, payment status, session management, and the service/cell inspector.
- **Rejected:** JSON over WebSocket with a JSON-Schema contract. It is easier to debug, but the codegen story is weaker on the Rust side.

### D6. Owner ledger and multi-currency accounts

- **Facts:**
  - The owner ledger is hard-coded to `stub+usd`, even on XRPL-only nodes (ACC-9).
  - Owner equity has no limit, so the owner can overspend.
  - The UI shows only USD (UI-2).
  - ILDCP always answers XRP/9 (ILP-7).
  - Incoming payments always say USD/2 (ILP-10).
- **Decision:**
  - An owner equity account per active ledger.
  - The owner's spendable balance per ledger is enforced: owner packets debit an account with a limit.
  - Local, BTP and HTTP endpoints get a ledger (the account's asset) chosen at creation.
  - ILDCP answers that ledger's asset.
  - Every amount shown in the UI carries its currency.
  - A preferred display currency, with the total converted at current rates (as `TotalOwnerBalance` does now).
  - Per-ledger config for trust line, max packet amount and settlement threshold ratio.
  - Learn the credit the peer extends to us, for the settlement midpoint (ACC-24).

### D7. Exchange rates, fees and spread

- **Facts:** rates are fetched once from Coinbase. There is no refresh, no spread and no fees. The `revenue/fees` and `expenses/fees` accounts are unused (ACC-3, ILP-6).
- **Decision:**
  - Refresh periodically (for example every 60 s) with retry and backoff.
  - Persist the last good snapshot. Reject cross-currency packets with T00 once rates are older than a staleness bound (for example 1 h).
  - Use exact integer math throughout.
  - Add a configurable connector fee and FX spread (default 0, as now), booked to `revenue/fees` and `revenue/fx` as the internal-ledger design note describes.
- **Rate source:** the median of a configurable list of sources, so that one source failing or being manipulated cannot move the rate. (Rejected: Coinbase alone, a single point of failure; an ordered fallback list, which still trusts whichever source answers first.)

### D8. Real money

- **Facts:** realm `live` is refused ("Livenet is not yet supported"). The XRPL mainnet module is unregistered.
- **Decision:** keep `live` disabled through M8. Enable it only when all of these are true:
  - the D2 authentication is in place;
  - the ledger is durable (journal, reconciliation);
  - XRPL reserve and result handling is complete (ACC-19 to ACC-22);
  - the release signing of D11 is in place;
  - there has been an external review.

  A mainnet bootstrap node list must exist before `live` makes sense.

### D9. Open Payments and SPSP scope

- **Facts:**
  - The node serves an Open-Payments-style wallet address and unauthenticated incoming payments, with hard-coded amounts.
  - There is no SPSP, no GNAP auth server, no quotes and no outgoing-payment resources (ILP-10).
- **Decision:**
  - **Serve SPSP** (`application/spsp4+json` on the payment-pointer URL), so the node is payable by any ILP wallet.
  - **Serve Open Payments incoming payments with correct state:** received amount, completion, expiry, `GET /incoming-payments/{id}`, pagination, and the correct asset.
  - **Client side:** pay SPSP and Open Payments receivers, quoting through STREAM rate probing, and track payment status.
  - **Defer** GNAP and the outgoing-payment and quote resources (the third-party-client use case).
- **Creating incoming payments** stays open to anyone, as Open Payments allows for receivers. It is rate-limited, and unused incoming payments expire. (Rejected: requiring a token, which would stop some third-party senders from paying without extra setup.)

### D10. Secrets at rest

- **Facts:** the node key, TLS key, ACME key, sessions and BTP tokens are stored in plaintext in SQLite with the default umask (RT-25).
- **Decision:**
  - Data directory 0700 and DB file 0600.
  - Session and BTP tokens stored only as hashes.
  - Node key and settlement entropy stay plaintext. The node must sign and settle unattended, and encrypting them with a key stored next to them would only be theatre.
  - The design notes' frontend-signing ideas stay future work ([2 §8.8](02-node-runtime.md)).

### D11. Release integrity

- **Facts:** bundles are unsigned. `meta/latest` is plain text. The updater does not check HTTP status (RT-18, UI-11). Releases are uploaded by hand (UI-12).
- **Decision:**
  - Each release publishes `SHA256SUMS` and a minisign signature. The public key is compiled into the binary and printed in the install script. (Rejected: Sigstore, whose verification needs the network or a bundled trust root and is heavier to embed.)
  - The installer and the updater verify before extracting.
  - The symlink swap is atomic (`rename`).
  - CI builds and publishes releases, reproducibly through the Nix flake of D14.
  - Canary gets its own channel file (`meta/canary`).

### D12. Peering lifecycle and liveness

- **Facts:**
  - Peers are never dropped.
  - Heartbeats don't update `lastSeen`.
  - There is no termination, advertise flag or banning (NET-12, NET-13).
  - Auto-peering stops at 2 peers and has no backoff (NET-22).
- **Decision:** implement the peer-state design ([3 §12](03-peer-network.md), and the `peer-state.md` content quoted in [3 §18](03-peer-network.md)):
  - states `request-peering`, `peered`, `terminating` and `banned`;
  - the `advertise` flag based on heartbeat consistency, liquidity and failures, with flapping damping;
  - termination with settle-on-terminate;
  - candidate scoring and backoff;
  - support for child (non-majority) peers routed beneath our address (NET-21).
- **Numbers:** the peer target stays 2. The missed-heartbeat threshold and the damping are set from the deterministic network simulation in M4 ([7 §7](07-rewrite.md#7-testing-strategy)) and are configurable.

### D13. Connector timing and packet limits

- **Facts:** the internal timeout is 5 s, independent of the packet expiry (up to 30 s), so fulfills that arrive late are lost (ILP-1). Invalid fulfills are dropped (ILP-2).
- **Decision:**
  - Time out at the outgoing packet's expiry.
  - Reject invalid fulfillments with F05 at once, and stop waiting.
  - F08 carries amount data.
  - Malformed packets get F01, unroutable ones F02. Nothing throws on the packet path (ILP-3).
  - Prefix matching is segment-aware (NET-33).
  - Request IDs are unique per endpoint.

  None of this is a real choice: it is the standard behaviour, recorded here because it changes observable timing.

### D14. Build system

- **Decision:** follow Vorstand: Nix flake for CI builds and release artifacts, plain `cargo` and `deno task` locally.
- **Rejected:** GitHub Actions with `cargo-zigbuild` or `cross` for musl x64/arm64, which gives no reproducible builds by default.

### D15. Bootstrap network for v1

- The default BNL is `test1.dassie.xyz` and `test2.dassie.xyz` ([3 §11.1](03-peer-network.md)).
- **Decision:** the project stands up and operates v1 bootstrap nodes (at least three, so that one failure doesn't block majority) under new hostnames in `dassie.xyz` (for example `boot1`–`boot3.dassie.xyz`), on separate providers or regions. Ship them as the default BNL, with their node IDs checked against their public keys at startup (NET-19).

## 4. Gaps by decision

Gaps not listed here follow the default disposition (§1).

| Decision | Gaps |
|---|---|
| D2 | NET-3, NET-4, NET-8, NET-9, NET-10, NET-11, NET-26, NET-27, NET-28 |
| D3 | RT-25 (storage format), ACC-5 and ACC-6 (no carried-over totals) |
| D4 | NET-15, NET-16 (doc fix), NET-17, NET-18, NET-19, NET-29 |
| D5 | RT-5, RT-6, RT-26, RT-27, UI-7 |
| D6 | ACC-1, ACC-9, ACC-24, ILP-7, ILP-10 (asset), UI-2, UI-3 |
| D7 | ACC-3, ILP-6, ILP-14 |
| D8 | ACC-13, ACC-19, ACC-20, ACC-21, ACC-22 |
| D9 | ILP-9, ILP-10 |
| D10 | RT-10, RT-11, RT-25 |
| D11 | RT-18, RT-19, UI-10, UI-11, UI-12, UI-13 |
| D12 | NET-12, NET-13, NET-21, NET-22, NET-23, NET-35 |
| D13 | ILP-1, ILP-2, ILP-3, ILP-4, ILP-5, ILP-8, NET-33, NET-34 |
| D15 | NET-19 |
