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
- implement the governing standard (X.696, ILPv4, RFC 29 STREAM, RFC 31 ILDCP, RFC 23 BTP, RFC 35 ILP-over-HTTP, Open Payments) or the obvious safe behaviour.

Every fix gets a test that would have failed against the original's behaviour.

## 2. Original behaviour kept on purpose

| What | Where | Why |
|---|---|---|
| Node ID algorithm, including stripping trailing `A`s | [3 §2.2](03-peer-network.md) | Node identity continuity. |
| BIP-39 seed → `HMAC-SHA256` seed-path chain, `node`, `node/login`, `node/settlement-module/<scheme>/<path>` | [3 §2.1](03-peer-network.md), [5 §4.5](05-accounting-and-settlement.md) | The same passphrase recovers the same node and the same ledger accounts. |
| XRPL wallet: first 16 bytes of entropy as an ed25519 family seed | [5 §6.3.1](05-accounting-and-settlement.md) | Funds continuity. |
| Ed25519 verification accepts ZIP-215 | [3 §2.1](03-peer-network.md) | Harmless. Being stricter could split the network on edge-case signatures. Kept for v1 as well (confirmed with D2). ed25519-dalek's `verify` is *not* ZIP-215: it is cofactorless and checks R by its encoding. So v1 verifies with `ed25519-zebra`, the ZIP-215 reference implementation (decided 2026-10-14). Small-order public keys are refused for link states, since under ZIP-215 one signature verifies for every message under such a key. |
| Majority rule: strictly more than N/2 of *configured* bootstrap nodes | [3 §11.4](03-peer-network.md) (NET-16) | The code is right and the doc's "N/2 + 1" is the outlier. Fix the doc, not the code. |
| Gossip parameters: suppress after 3 copies, 1–500 ms forward jitter, 5–20 s heartbeats, 3 h staleness, 1 h re-issue | [3 §17](03-peer-network.md) | Sound defaults. Make them configurable for tests. |
| Settlement trigger: every 4 s, threshold = credit midpoint × 1.1, settle down to the midpoint | [5 §4.6](05-accounting-and-settlement.md) | Keep the policy. Fix the credit asymmetry (ACC-24): the credit the peer extends is announced when peering ([7 §4.7](07-rewrite.md#47-peer-protocol-v1)). |
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
  - **Sender authentication** (NET-8): the MAC key comes from a static-static X25519 between the sender's and the recipient's node keys. The MAC also covers `version`, `sender` and the recipient. (Rejected: an Ed25519 signature by the sender over `version ‖ sender ‖ content ‖ timestamp`, which costs a signature per message.)
  - **Replay protection:** a timestamp plus a per-sender nonce window.
  - **Amended 2026-10-14.** The exact scheme is in [7 §4.7](07-rewrite.md#47-peer-protocol-v1).
    - **No ephemeral key.** The first version of this decision mixed an ephemeral X25519 into the key. For a scheme that only MACs, that adds nothing: whoever holds the recipient's static key can forge with or without it, so there is neither forward secrecy nor protection against key-compromise impersonation. It also let anyone force an X25519 operation per request by sending fresh ephemerals.
    - **Direction labels.** Request and response keys are expanded with direction labels and the ordered pair of public keys, so a message cannot be reflected back to its sender.
    - **Sender key in the envelope.** The sender's Ed25519 key travels in the authentication field and must hash to the claimed node ID. A recipient can then check a message from a node that is not in its table yet, such as a registrant or a node polling a bootstrap node.
    - **Replay window.** "Nonce window" became a session ID plus a counter, checked against a sliding window. A per-peer floor is persisted, so a restart does not reopen a replay window.
    - **Authenticated responses.** Responses are MACed as well, so node lists, peering answers and registrations are authenticated end to end, not just over TLS.
  - **Post-quantum path (2026-10-14).** The authentication CHOICE reserves tag `[2]` for a scheme built on the X-Wing hybrid KEM, and link-state *extension* entries can carry its public key. X-Wing is a KEM: it can protect the key agreement, but signatures and node IDs stay Ed25519, since the passphrase-to-node-ID chain is fixed (7 §2). Confidentiality against "harvest now, decrypt later" belongs to TLS (hybrid X25519MLKEM768), which comes when the release's crypto provider is settled (M8).
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
- **Refinements (2026-10-14, from the M4 design review):**
  - **The served list is `unexpired registrations ∪ self`.** In the original it was `majority ∪ registered` (3 §11.5). Majority is computed from the bootstrap nodes' lists, so serving it back would keep a node listed forever once it had made it, whether or not it renews. A bootstrap node lists itself and registers with the others like any node.
  - **Membership freezes when lists go stale.** A node is removed for losing majority only while more than half of the configured lists are fresh. Otherwise two bootstrap nodes going offline would empty every table in the network.
  - **NET-15.** A downloaded list is accepted when its response is authenticated by the bootstrap node's configured key. Its hash is recomputed from the downloaded bytes. Comparing the list with the hash polled earlier proves nothing, because both answers come from the same server, and on a busy bootstrap node the list changes between the two requests.
  - **Free registration is throttled** until the fee arrives:
    - a global token bucket for new registrations;
    - one registration per URL host and port;
    - a cap per /24 or /48;
    - a reachability probe: the bootstrap node asks the registrant's URL for its link state, with authentication, before accepting.
  - **Timings:** registrations last 24 h and are renewed every 6 h. A node with no reason to stay in the table is removed after a 1 h grace.

### D5. UI and CLI RPC contract

- **Decision:** a normative contract in `proto/dassie/ui/v1`, following Vorstand's `proto/` convention. It is generated into Rust and TypeScript (`packages/rpc`) and carried over one WebSocket per client:
  - **Shape:** unary calls and server streams. Streams are either *value* streams or *snapshot + delta* streams.
  - **Messages:** protobuf binary frames.
  - **Amounts:** fixed-width integers with explicit scale.
  - **Errors:** typed, never a success response that carries a failure (fixes RT-5).

  The same contract serves the IPC socket for the CLI.
- **Procedure set:** that of [2 §10.3](02-node-runtime.md), extended by the missing setters (RT-4), XRPL ledger management, payment status, session management, and the service/cell inspector. The live data of the wallet's pages comes from `Subscribe*` streams ([7 §4.6](07-rewrite.md#46-the-rpc-contract-in-practice)), which replace the original's polling.
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
- **What the node gains or loses is the owner's (M7):** the owner's group takes in `equity/suspense`, the fee accounts and `expenses/losses`, and in M7c the exchange accounts, so the owner's balance is everything the node holds on the ledger beyond what it owes peers ([7 §4.5](07-rewrite.md#45-settlement-and-ledgers), [D18](#d18-one-grace-limit-per-peer)). The exchange accounts come with the rule that the owner's own endpoints never convert: the owner pays from the ledger a route starts on.
- **The display currency (M7c):** one of the currencies the node keeps rates for, or none; the total is shown only while every rate it needs is fresh.
- **Owner sub-accounts (confirmed 2026-10-13):** the owner's funds on a ledger stay in one pool. Each spending endpoint, such as a BTP token, gets a sub-account of the pool for attribution, and the limit applies to the group's total ([7 §4.5](07-rewrite.md#45-settlement-and-ledgers)). Per-token spending caps are policies computed from the journal, built in M5.
  - **Caps (2026-10-14):** a cap counts what a token moves out of the owner's funds: packets to peers, not to the node's own receiver or another token. Packets in flight count as they are prepared. The usage is checkpointed with the journal rows that change it, because a window of recent rows cannot reproduce it ([7 §4.5](07-rewrite.md#45-settlement-and-ledgers)).
  - **Rejected:** an account per token holding its own balance, as `ilp-connector` child accounts do. Refilling a token means scheduled top-ups, and funds can't be committed to several tokens at once (a fractional reserve), so ten tokens that may each spend $10 would lock up $100.

### D7. Exchange rates, fees and spread

- **Facts:** rates are fetched once from Coinbase. There is no refresh, no spread and no fees. The `revenue/fees` and `expenses/fees` accounts are unused (ACC-3, ILP-6).
- **Decision:**
  - Refresh periodically (for example every 60 s) with retry and backoff.
  - Persist the last good snapshot. Reject cross-currency packets with T00 once rates are older than a staleness bound (for example 1 h).
  - Use exact integer math throughout.
  - Add a configurable connector fee and FX spread (default 0, as now), booked to `revenue/fees` and `revenue/fx` as the internal-ledger design note describes.
- **Rate source:** the median of a configurable list of sources, so that one source failing or being manipulated cannot move the rate. (Rejected: Coinbase alone, a single point of failure; an ordered fallback list, which still trusts whichever source answers first.)
- **When:** the conversion math and the T00 rule come with the connector in M3. The fetcher, fees and spread come in M7, the first milestone with a second currency.
- **M7c (2026-10-15):**
  - **Sources:** the owner's list, at most 8: `coinbase` (its `exchange-rates` format, which another server may serve), `kraken` and `bitstamp` (XRP's price in dollars, inverted); all three by default. Each is asked every minute, at once, through the peer client (10 s, 64 KiB, HTTPS), for the currencies of the active ledgers and the display currency, and read in exact decimal arithmetic.
  - **The rate per currency** is the median of the sources that cover it and gave one in the cycle, once more than half of those that cover it did; with an even number, the two in the middle must agree within 1%. A currency only Coinbase's format covers (the currencies of display) relies on that source. Otherwise the last rate stays; each currency's rate ages on its own, and the connector stops converting with one an hour old (T00). The rates are kept, and restored at start.
  - **Fees and spread** are charged only on transit, from one peer to another: `fee = floor(in × fee_ppm / 10^6)`, `out = floor(convert(in − fee) × (10^6 − spread_ppm) / 10^6)`, each at most 10%. The fee rounds down, since rounded up it takes a payment's last packets whole. It is booked in a third linked transfer to `revenue/fees`; the spread's gain is the exchange accounts' net ([7 §4.5](07-rewrite.md#45-settlement-and-ledgers)).

### D8. Real money

- **Facts:** realm `live` is refused ("Livenet is not yet supported"). The XRPL mainnet module is unregistered.
- **Decision:** keep `live` disabled through M8. Enable it only when all of these are true:
  - the D2 authentication is in place;
  - the ledger is durable (journal, reconciliation);
  - XRPL reserve and result handling is complete (ACC-19 to ACC-22);
  - the release signing of D11 is in place;
  - there has been an external review.

  A mainnet bootstrap node list must exist before `live` makes sense.
- **M7b (2026-10-15):** the testnet scheme settles ACC-19 (the reserve is never booked), ACC-20 (an account's creation is a deposit), ACC-21 (whole drops, fees booked) and ACC-22 (results checked, a failure's fee booked, the sender checked). Mainnet still needs its own memo type (ACC-13) and the rest of this list.

### D9. Open Payments, fully, and no SPSP

Revised on 2026-10-14; it first served SPSP and deferred GNAP, quotes and outgoing payments.

- **Facts:**
  - The node serves an Open-Payments-style wallet address and unauthenticated incoming payments, with hard-coded amounts.
  - There is no SPSP, no GNAP auth server, no quotes and no outgoing-payment resources (ILP-10).
- **Decision:** implement [Open Payments](https://github.com/interledger/open-payments-specifications) v1.4.0 as the specs define it.
  - **Serve all three servers:**
    - the wallet address server, with its `jwks.json`;
    - the GNAP authorization server: grants, continuation, interaction and token management;
    - the resource server: incoming payments, quotes, outgoing payments and grant spend.
  - **One wallet address per ledger**, since a wallet address has one asset:
    - each ledger is at `https://host/pay/<scheme>`;
    - `$host` (`https://host/.well-known/pay`) names the default ledger, a setting.
  - **Authorization:** every resource-server and authorization-server call needs a GNAP token and an HTTP message signature (RFC 9421) by the client's key, as the specs require.
    - **Non-interactive:** incoming-payment and quote grants are approved without interaction.
    - **The owner's consent in the wallet** is needed for outgoing-payment, `subject` and `list-all` grants, and to read others' quotes. Reading any incoming payment needs none: a sender's wallet reads a payment it did not create to learn how to pay it.
    - **What a client reaches:** `read`, `list` and `complete` reach only the incoming payments the client created; `read-all` reaches any.
    - **Bounds:** the owner's incoming payments and clients' are bounded apart, so that clients never crowd out the owner. When clients' share is full, the oldest of their payments that received nothing is closed. Clients' payments last at most 7 days, and unpaid ones are forgotten a week after they close.
    - **Grant limits** (amounts per repeating interval) are reserved when an outgoing payment is created.
    - **Each access item keeps to its own limits.** A grant may have several outgoing-payment items for one wallet address, with different actions. The item that allows `create` sets what payments may spend, and only one item per wallet address may allow it: a request with two is refused, as which limits apply would be unclear. On an item that only reads or lists, a `receiver` limit narrows it to the payments to that receiver, and its amounts and interval limit nothing.
  - **The owner's consent** (M6b):
    - A grant that needs the owner is pending until decided. The client is sent to `GET /gnap/interact/{id}/{key}`, a random key per grant, which leads to the wallet's consent page.
    - The page shows who asks (the client's wallet address and public name: a client known only by its key is refused for such grants), the access, and its limits in plain words; payments without limits read as "of any amount".
    - Limits' intervals have windows of at least a minute, since each window spent in is kept. Approving or denying sends the client to its finish URI, with the interaction hash and `interact_ref`, or with `result=grant_rejected`.
    - The client continues with `interact_ref` to get its token, no sooner than the `wait` it was given (`too_fast`).
    - A pending grant lapses after 10 minutes. At most 256 are pending at once.
  - **Quotes and outgoing payments** (M6b):
    - Until M7 every ledger has one asset and the connector charges no fees, so a quote is same-asset at a rate of 1 and needs no probe. A receiver in another asset is refused. STREAM rate probing comes with M7's exchange rates; the quote math already takes a rate.
    - **From M7c,** a quote probes the route to its receiver, same asset or not, since intermediaries charge fees: the measured rate less STREAM's 1% slippage, one for one to the node's own receivers. It pays from the ledger the route starts on. The payment sends at no worse than its quote's rate. The owner's Send fixes either the amount the recipient gets or the amount sent; a fixed amount to send goes out whole, and the incoming payment it creates at a wallet address takes any amount.
    - A quote lasts 5 minutes, or less if its receiver expires sooner, and pays one outgoing payment.
    - An outgoing payment never debits more than its debit amount, across crashes too: its sent amount is what was posted (D17).
    - It completes once it has sent that, or once its receiver says it has all it asked for, which a hop that lost a packet may have paid for (D17): a completed incoming payment tells a sender what its connection delivered and that no more fits. A connection that closes early is opened again for the rest, up to 8 times, a while later each time.
    - **Each payment has a STREAM connection of its own.** Each read of an incoming payment gets credentials of its own, and the receiver tells each connection what it received, so two payments to one incoming payment never take each other's delivery for their own: the one that delivered less fails rather than completes. An outgoing payment never uses credentials another used, as a wallet that gives the same credentials to every read would make them share a connection.
    - Grant limits are reserved in the interval window the payment is created in; a failed payment releases what it did not send.
  - **Client side:**
    - To pay an incoming payment on another node, the node reads it with a non-interactive `incoming-payment` `read-all` grant from that node's authorization server, as its own `$host` wallet address and key (M6b). Tokens are cached by authorization server, origin and action.
    - A wallet address on another node is paid only if it names itself: its `id` is at the origin it was read from, and its servers are HTTPS URLs. A payment request speaks only for its own wallet: one on another node for a wallet whose resource server is where the request is, never for one of this node's. Origins compare without case and without the scheme's own port.
    - The node asks other wallets for what it needs only: `read-all` to pay an incoming payment, `create` to make one, each its own token.
  - **The owner's Send** (M6c), over RPC, needs no grant: the node quotes and pays as the owner, with the logic third-party clients go through.
    - It goes to a payment pointer or wallet address, here or on another node, or to a payment request: an incoming payment's URL, as Receive hands out.
    - For a wallet address, quoting first creates the incoming payment there, expecting what the owner sends, for 30 minutes (the quote's 5 and the payment's 10, with room to spare), with the owner's note as its description. A payment request is paid as it is: what it still accepts, or less.
    - The amount is what the recipient gets. Until M7 the debit equals it; choosing between the two comes with M7's rates.
    - The quote and the payment keep the wallet address paid, for the owner's pages.
    - At most 4 lookups of recipients run at once, since each may fetch from other wallets as the node; the rest wait up to 30 seconds. The owner's quotes have a cap of their own (256 live), apart from clients' (4096), so clients cannot crowd the owner out.
  - **Payment pointers** stay only as input syntax: `$host/path` means `https://host/path`, and a bare `$host` means `https://host/.well-known/pay`.
  - **Not supported:** card authorization (v1.4.0), since the node has no cards. STREAM receipts are not part of Open Payments and are not issued.
- **Rejected:**
  - **SPSP:** Open Payments has replaced it, and its specs no longer mention it. Wallets that speak only SPSP cannot pay the node.
  - **Creating incoming payments without a token,** which the first version of this decision chose: the specs require GNAP, and a non-interactive grant costs a client one request. Grant requests and incoming payments are rate-limited, and unused incoming payments expire.
  - **Deferring GNAP and outgoing payments:** without them no third-party client, and no other wallet's user, can pay from the node.

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
  - The signature's trusted comment is `dassie <version>`, and both are checked: the signed sums of an old release, served under a new version's path, are refused ([7 §4.8](07-rewrite.md#48-releases)).
  - The release key's secret half lives in the `Release` environment of the repository, which only the release workflow on the main branch or a `v*` tag may use. The workflow verifies what it signed with `resources/minisign.pub`, using `dassie-release`, before it uploads, so a release signed with another key never reaches the mirror.
  - `meta/<channel>` is not signed, so it is not trusted to say which way to move: the updater moves on its own only to a later version of the same channel.
  - The installer and the updater verify before extracting, and unpack only plain files and directories.
  - The symlink swap is atomic (`rename`). The updater keeps the version it came from, and goes back to it if the new one does not come up. The owner can go back too, with `dassie update --target-version`; to stay there, they stop `dassie-update.timer`, which would otherwise move the node forward again.
  - The installer verifies with minisign where there is minisign, and else with OpenSSL, which reads minisign's format by hand: no extra tool to install first.
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
- **Refinements (2026-10-14, from the M4 design review):**
  - **Only confirmed peers count toward the target.** A peer is confirmed by a reciprocal edge in the link states or by a recent authenticated heartbeat. A node whose stored peerings went stale therefore looks for new peers instead of staying isolated.
  - **A bridge rule joins islands.** While a majority node with a fresh link state stays unreachable, a node attempts one more peering beyond the target, so two separate triangles of peers join up.
  - **Inbound slots are capped** below the maximum, so requests alone cannot fill every slot.
  - **Duplicate and crossed requests:**
    - a peering request from a node that is already peered is accepted again idempotently, which covers an accept that got lost;
    - when two nodes ask each other at once, the request from the lower node ID wins.
  - **The responder probes the requester's URL before accepting,** as the design note intended.
- **Delivery:** M4a builds the `none`, `request-peering` and `peered` states with the rules above. M4b adds termination, banning and the advertise flag ([7 §4.7](07-rewrite.md#47-peer-protocol-v1)).
- **Refinements (2026-10-14, from the M4b design review):**
  - **The advertise flag follows liveness only:** a recent authenticated message, with a hold-down that doubles per flap. Liquidity and failed deliveries are left out, because withdrawing an edge removes it both ways for the whole network, turning a temporary T04 into a final F02, while the traffic coming back is what restores liquidity.
  - **Termination ends a peering, not a relationship.** The accounts stay, with what is owed, and the settlement sequence carries on, so a settlement that arrives after the peering ended is still credited once. Peering with the node again reuses both, without posting trust twice.
  - **`banned` is a list, not a peering state,** since a node can be banned while its peering is still terminating.
- **Child peers deferred (2026-10-14).** Child peers (NET-21, NET-23) need a node's ILP address to follow its uplink, which touches the connector, BTP and the STREAM receiver. They only matter once registration costs something (NET-17): today any node with a URL registers automatically and becomes a member within minutes. They move to the milestone that adds registration fees.
- **Known limit:** a node never closes a peer's accounts or settlement channel. It keeps at most 512 channels, one per node it has ever peered with; after that, peering with a new node is refused for capacity. Closing a relationship whose balance is settled can lift this later.

### D13. Connector timing and packet limits

- **Facts:** the internal timeout is 5 s, independent of the packet expiry (up to 30 s), so fulfills that arrive late are lost (ILP-1). Invalid fulfills are dropped (ILP-2).
- **Decision:**
  - Time out at the outgoing packet's expiry.
  - Reject invalid fulfillments with F05 at once, and stop waiting. From a peer, a fulfillment that does not match the packet in flight with that request ID is treated as a stale resend instead ([D17](#d17-the-upstream-decides-whether-a-fulfill-counted)): it is answered from the record of Fulfills taken, and the packet waits for its own response or its expiry.
  - F08 carries amount data.
  - Malformed packets get F01, unroutable ones F02. Nothing throws on the packet path (ILP-3).
  - Prefix matching is segment-aware (NET-33).
  - Request IDs are unique per endpoint.

  None of this is a real choice: it is the standard behaviour, recorded here because it changes observable timing.

### D14. Build system

- **Decision:** follow Vorstand: Nix flake for CI builds and release artifacts, plain `cargo` and `deno task` locally.
  - The binaries are cross-compiled from x86_64-linux with nixpkgs' musl toolchains, so one builder makes both platforms.
  - The wallet is built in the sandbox too, from a fixed-output derivation of its npm packages ([7 §4.8](07-rewrite.md#48-releases)).
- **Rejected:** GitHub Actions with `cargo-zigbuild` or `cross` for musl x64/arm64, which gives no reproducible builds by default.

### D15. Bootstrap network for v1

- The default BNL is `test1.dassie.xyz` and `test2.dassie.xyz` ([3 §11.1](03-peer-network.md)).
- **Decision:** the project stands up and operates v1 bootstrap nodes (at least three, so that one failure doesn't block majority) under new hostnames in `dassie.xyz` (for example `boot1`–`boot3.dassie.xyz`), on separate providers or regions. Ship them as the default BNL, with their node IDs checked against their public keys at startup (NET-19).
- **Until they exist (2026-10-14):** the default BNL is empty. The v0 nodes would refuse every v1 message. A node with no bootstrap nodes runs alone and warns. It neither polls nor registers until `DASSIE_BOOTSTRAP_NODES` is set.

### D16. Engineering style: pragmatic TigerStyle and deterministic simulation

Confirmed on 2026-10-12, after the others.

- **Why:** the rewrite will be built almost entirely by agents, and it must still reach a very high level of confidence in its security and reliability. That confidence comes from testing, simulating and exercising the code far more than review could. Deterministic simulation testing (DST) is the main tool in the later milestones. It only works if the code is built for it from the first crate, because retrofitting determinism means rewriting.
- **Decision:** follow a pragmatic version of [TigerStyle](https://github.com/tigerbeetle/tigerbeetle/blob/main/docs/TIGER_STYLE.md). "Pragmatic" means:
  - dynamic allocation inside the Rust standard library, or inside a robust dependency, is accepted;
  - a robust third-party crate is used rather than rewritten, notably the cryptographic primitives.

  Allocation and dependencies are still treated as code smells and avoided where that makes sense.
- **Requirements on every crate:**
  - **Deterministic.** The protocol and state-machine crates never read the wall clock, the OS RNG, environment variables or threads:
    - Time is an input (`now`), and timers are deadlines the caller polls. Randomness comes from an injected RNG.
    - Iteration order is always defined, so no randomly seeded hash maps.
    - State holds no floating-point values.
    - The same inputs and seed give the same outputs, byte for byte.
  - **Bounded.** Every queue, table, buffer and loop has an explicit limit, set by configuration and named with its unit. Exceeding a limit is an error value, never unbounded growth. State machines allocate when they are created, not on the steady-state packet path.
  - **Asserted.**
    - Assertions check preconditions, postconditions and invariants, and stay enabled in release builds. A broken invariant crashes rather than corrupting balances.
    - Each state machine has an invariant check that tests and simulations run after every transition.
    - Untrusted input never reaches an assertion: it gets an error value.
    - Integer overflow checks are on in every profile.
  - **Instrumented.** State machines report everything observable as events and counters through their API, so a simulation can record, compare and replay a run. Core crates have no logging dependency; the node maps events to `tracing`.
  - **Efficient.** Simulations should run many nodes for long simulated periods, so the hot paths stay cheap, and costly parts such as crypto sit behind narrow interfaces that a simulation can substitute.
  - **Simple shape.**
    - No recursion.
    - Functions of at most 70 lines.
    - No panicking indexing or `unwrap` outside tests.
    - No lossy integer casts.
    - Explicit units in names.
- **Open Payments follows the same split.** `dassie-payments` is sans-IO: it holds the JSON shapes, HTTP message signatures, GNAP rules, intervals and quote math, and gets golden vectors and never-panics properties. The node's engine owns the money side (receivers, payers, quotes, grant spend) and is simulated. The HTTP and GNAP services around it are tested over real HTTPS and end to end.
- **Tooling is not simulated.** The dev environment (`apps/dev`) and the dashboard are tools around the node, not part of it. Their pure parts (the SNI parser, the vanity table, the reducers) get unit tests, and the rest is tested end to end against real binaries. They are still bounded: rings, frames and queues have limits.
- **Testing consequences:** see [7 §7](07-rewrite.md#7-testing-strategy). Every decoder gets a "never panics on arbitrary input" property test. Every simulation failure reports the seed that reproduces it.
- **Rejected:** conventional style with determinism added later, when DST starts. Clocks, RNGs, unordered maps and unbounded queues spread into every crate, and removing them afterwards costs more than avoiding them.

### D17. The upstream decides whether a Fulfill counted

Decided on 2026-10-14, after M6a.

- **Facts:** each hop books one transfer per packet, pending in memory while the packet is in flight. Two peers' books part when a Fulfill one side posted never counts on the other:
  - the downstream crashes after posting, before its Fulfill reaches the upstream, which expires the packet;
  - the upstream crashes with the Prepare in flight, and drops the Fulfill for a packet it no longer knows;
  - the Fulfill arrives after the upstream's expiry, with no crash at all.

  Sending responses again for 30 s from memory narrowed the first case only. The node simulation found the others when a payer, then a receiver, crashed mid-payment.
- **Decision:** only the upstream can know whether a Fulfill came in time, so it decides, says so, and the downstream books what it says.
  - **The verdict.** The reply to an `interledgerPacket` carrying a Fulfill says `accepted` or `refused`. `accepted` means the upstream posted the packet's transfers; it is only said once they are on disk. A packet in flight with a matching fulfillment before its expiry is accepted. Anything else (a resend, a packet the upstream no longer knows) is answered from a durable record of the Fulfills it took from that peer, kept 30 days: found means `accepted`, missing means `refused`.
  - **The downstream posts at once**, as before, and keeps what it owes: a durable row with the Fulfill, sent until a verdict comes back, with a backoff from 1 s to 1 h, across restarts and after the peering has ended. Anything but a verdict (no answer, 401, 409, a timeout) is tried again.
  - **On `refused`** the downstream books one compensation, `Reference::Refusal`, which reports what happened and so is recorded even past a limit. If its own receiver took the packet, the transfers are reversed: the owner was not paid, and the incoming payment does not count it. If anyone else took it (a peer, or a BTP client, both told it was fulfilled), the downstream books `expenses/losses` against the upstream's account. Either way that account ends where the upstream's books have it.
  - **Who bears a loss** is ILP's rule: the hop whose Fulfill its upstream did not take. Each hop decides on its own and answers its downstream at once; verdicts do not wait on each other along the path.
  - **Incoming payments** count only what was confirmed, and tell their senders only that. What waits for a verdict still counts against what a payment may take. A refusal closes the STREAM connection that took it, which counted what it took back: its sender starts anew, and may send that part again.
  - **Rejects** stay best effort, as do Fulfills of packets that moved no money: a lost one has the effect of expiry, and both sides void.
  - **Ending a peering** waits for the verdicts it is owed for at most 10 minutes, and not at all when the peer stopped answering; they are still asked for after. A banned node is not asked.
  - **Bounds:** at most 1024 Fulfills owed by one peer, counting the packets in flight from it; a peer at its cap gets T03 on new Prepares until it answers. Every peer holds a settlement channel, so all that is owed is bounded by the channels. At most 16 verdicts asked of one peer at once. A Fulfill owed for longer than its expiry plus 29 days is booked as refused and logged as an error: from then on the upstream may have forgotten it. Fulfills taken are forgotten after 30 days, 10,000 at a time each minute.
- **Rejected:**
  - *The downstream decides*, with the upstream taking any valid Fulfill however late: the upstream would have to persist every Prepare, and the downstream would hold a free option on every packet.
  - *Holding the downstream's transfers pending until the verdict:* pending transfers would pile up in the ledger while a peer is silent, blocking everyone's packets, and each pair of books would differ while they wait.
  - *Persisting packets in flight on both sides:* a write per Prepare, and the late Fulfill still parts the books.
- **The verdict is the upstream's word.** A dishonest upstream could refuse Fulfills it forwarded and keep the difference. What a peer refused is in the journal, and counts against the peer's grace limit ([D18](#d18-one-grace-limit-per-peer)).

### D18. One grace limit per peer

Decided on 2026-10-14, with D17.

- **Facts:** a peer can cost us money in several ways: Fulfills it refuses (D17), debt it never settles, settlements it announces that never arrive. The credit we extend caps what it owes us at any moment, not what it takes over time.
- **Decision:** each peer has **one** grace limit, a share of the credit we extend to it (a setting). Everything a peer cost us in a rolling window counts against the same limit, not a budget per kind:
  - Fulfills it refused;
  - what it owes us past the point where it should have settled, once its settlement is due and the peering's grace has passed;
  - any later kind of loss attributable to a peer, which joins the same sum.

  Each kind is measured from the books, journal queries over the window plus the debt now unpaid, never kept as separate counters, so the sum is exact across restarts. A peer past its limit loses its peering as if banned ([D12](#d12-peering-lifecycle-and-liveness)), and the owner is told. Honest peers cost us only through crashes and late Fulfills, which stay below it.
- **When:** D17 makes refusals measurable and shows them per peer. The limit itself is enforced before real money, in M7a ([7 §8](07-rewrite.md#8-milestones)).
- **Rejected:** a budget per kind of loss, which lets a peer spread what it takes across kinds.
- **The numbers (M7a, 2026-10-15):**
  - **Refused** is what the losses of a peer's refusals booked (Dr `expenses/losses`, Cr its account) in the last **30 days**, or since the owner last lifted a ban on it. A refusal our own receiver took back cost nothing and does not count.
  - **Overdue** is what a peer owes us, posted (packets in flight and our own settlements on their way left out), counted as a whole once it has stayed **past the point where it should have settled** for longer than **the grace for settling, 1 hour**. That point is the peer's own threshold ([5 §4.6](05-accounting-and-settlement.md), with the credits as it sees them) at the default ratio, or ours if higher, and never below half the credit we extend: with uneven credit, a peer settles well past half. The clock stops when a settlement from the peer brings it back under, starts no earlier than our own start, and stops while our scheme cannot see its ledger: a peer is not overdue while we could not have seen it settle. Only a peered peer can be overdue. One whose peering is being ended cannot settle until it ends, and one whose peering ended may never have heard so, which leaves it nothing to settle by; its debt stays on the books, and only its refusals count. A cancelled settlement of ours backs off at most 10 minutes, well within our peers' grace.
  - **The limit** is the owner's share of the credit we extend, **75% by default**, and at least a hundredth of a unit. One crash of an honest upstream with packets in flight can cost more than half the credit: over 300 simulated seeds the worst at the end of a run was 0.6 of 1.1 units (55%). A peer can owe us no more than all of its credit, so a share below 100% is what bans a peer that never settles; at the default, one that owes more than three quarters of its credit, past where it should have settled, for an hour. The share goes from a millionth to ten times the credit, so an owner can tolerate more.
  - **When it is checked:** every peer each minute, and a peer at once when one of its refusals is booked, after the step's books are on disk.
  - **The bound:** what a peer can cost us before its ban is its limit plus what it has in flight, at most the credit both ways.
  - **A ban over the limit** is kept with its cost; the Network page and the dashboard show it. A full ban list still ends the peering, once. Lifting the ban starts the peer's window of refusals anew; what it still owes counts again after the grace for settling.
  - **The owner's group takes in what the node gains or loses** (`equity/suspense`, `revenue/fees`, `expenses/fees`, `expenses/losses`), so a loss is the owner's to bear, not money the owner could still spend ([7 §4.5](07-rewrite.md#45-settlement-and-ledgers)).

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
