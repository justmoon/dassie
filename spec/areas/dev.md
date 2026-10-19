# Dev environment

`node --run dev` runs a network of real `dassie` nodes on one machine, sets each up as its owner would, and serves a dashboard at `https://localhost` that shows every node at once and opens any node's wallet or debug pages. The end-to-end tests run the same scenarios.

**Paths:** `apps/dev/`, `apps/dev-dashboard/`, `proto/dassie/dev/v1/`
**Read with:** [Node](node.md), [Wallet](wallet.md), [RPC](rpc.md), [Testing](testing.md), [XRPL](xrpl.md), [Exchange rates](connector.md#exchange-rates)

## Running it

`node --run dev` (`node apps/dev/main.ts`) on Node, in order:

1. Checks the [prerequisites](#prerequisites), takes the [lock](#state-directory) on its state directory, and stops processes a crashed run left behind.
2. Makes sure the mkcert CA and the dashboard's certificate exist ([Certificates](#certificates)).
3. Starts the [SNI proxy](#sni-proxy) on 443. If it cannot have the port, it prints a hint and goes on without it.
4. Builds the node (`cargo build --locked -p dassie-cli`, to `target/debug/dassie`), unless `--dassie` names a binary; with `--xrpl fake`, also the [fake XRP Ledger](#fake-xrp-ledger). A node that does not build stops the start.
5. Builds the wallet and the dashboard with Vite in development mode, watching, into each app's `dist-dev/`, emptied first.
6. Serves [the dashboard's origin](#the-dashboards-origin).
7. Runs the scenario chosen last ([Scenarios](#scenarios)), and from then on [rebuilds the node and restarts the nodes](#builds) when the Rust sources or the contracts change.
8. Prints `Dashboard: https://localhost/  <- Start here` (with the proxy's port if not 443, or the dashboard's own port without the proxy) and, unless headless, a hint to run `mkcert -install` once if the browser warns.

### Flags

| Flag | Effect |
|---|---|
| `--scenario <id>` | Run this scenario instead of the one chosen last |
| `--dassie <path>` | Run this binary; do not build or watch the node |
| `--no-watch` | Build the wallet and the dashboard once; watch nothing |
| `--no-proxy` | No SNI proxy |
| `--headless` | No keys and no hints for a person |
| `--port-offset <n>` | Move every port by n, 0 to 40,000, so environments can run side by side |
| `--state-dir <dir>` | Keep the state there instead of `local/dev` (also `DASSIE_DEV_STATE_DIR`) |
| `--xrpl <testnet\|fake>` | Where the XRPL scenarios settle: the public testnet (default) or the fake ledger |

An unknown flag or a bad value exits with code 2.

### Prerequisites

- Linux or macOS; elsewhere: "use WSL, a container or a VM".
- `mkcert` runs (it is in `nix develop`).
- The IPC socket path of the 100th node fits ([State directory](#state-directory)).

Each missing one is printed and the dev server exits with code 1. Node.js versions, OpenSSL and name resolution of `dN.localhost` are not checked: browsers map `*.localhost` to loopback, and so does the node's peer client.

### Keys

On a terminal, in raw mode: **space** opens the dashboard in the default browser (`xdg-open`, `open` on macOS), **r** restarts the nodes, **q** quits, **ctrl+c** sends this process SIGINT.

### Stopping

SIGINT or SIGTERM stops, in order: the keys, the source watcher, the nodes (all at once), the Vite builds, the proxy and the server; then it removes the PID file and gives up the lock. A second signal quits at once (exit code 130).

### State directory

`local/dev/` in the repository (ignored by git), or the directory of `--state-dir`:

| Path | What |
|---|---|
| `tls/` | The certificates, kept across runs, and `ca.pem`, the CA that made them |
| `scenario.txt` | The scenario chosen last |
| `pids` | The PIDs of the nodes and Vite builds this dev server started, rewritten as they change |
| `dev.pid` | The PID of the dev server using the directory: the lock |
| `data/dN/` | Each node's data (`DASSIE_STATE_DIRECTORY` is `data/dN/state`) |

- **Sockets** live apart, since a socket path is short: `$XDG_RUNTIME_DIR/dassie-dev-<tag>/dN.sock` (or under `/tmp`), where the tag is the first 12 hex digits of the SHA-256 of the state directory's path, so runs on other state directories never meet. The directory is created with mode 0700. A path must leave room for the suffix the node adds while binding (`.1234567.d/s`) within 103 bytes (macOS's `sun_path` less its NUL); otherwise the prerequisite check says to set `XDG_RUNTIME_DIR` to a shorter directory.
- **Lock:** a dev server refuses to start, naming the other's PID, while another (`apps/dev/main.ts`) runs on the same state directory, since stopping its children would pull its network from under it.
- **Stale processes:** at start, each PID in `pids` that is still a `dassie daemon` or a Vite build gets SIGKILL; a PID reused by something else is left alone. A node left running would hold its ports and socket.

## Ports and hostnames

Every port moves by `--port-offset`. Everything listens on loopback only.

| What | Port | Name |
|---|---|---|
| SNI proxy | 443 | any; routes by name |
| Dashboard, its control channel, the relay | 10100, and 10101 answers the same | `localhost`, `dev-rpc.localhost` |
| Node N, HTTPS | 5000 + N | `dN.localhost` |
| Node N, HTTP | 4000 + N | `dN.localhost` |
| Fake XRP Ledger, WebSocket | 10200 | `localhost` |
| Fake XRP Ledger, faucet (HTTPS) | 10201 | `localhost` |
| Rates server (HTTPS) | 10210 | `localhost` |

- The proxy and the dashboard listen on 127.0.0.1 and on ::1 where there is one. Nodes run with `--listen 127.0.0.1`, since their keys are public in the vanity table. The fake ledger and the rates server listen on 127.0.0.1.
- If the proxy cannot bind 443: on `EACCES` the hint is `sudo sysctl -w net.ipv4.ip_unprivileged_port_start=0` (Linux), or to use the dashboard at its own port; on `EADDRINUSE`, that another dev environment may be running.

### SNI proxy

- It reads the server name from the client's TLS ClientHello and passes the connection on, still encrypted:

  | Name | Upstream |
  |---|---|
  | `dN.localhost`, N from 1 to 9999 and at most 100 | node N's HTTPS port |
  | anything else, or no name | the dashboard |

- Parsing: a TLS record of type 22 (handshake) holding a ClientHello (type 1); it skips the version, random, session ID, cipher suites and compression methods, walks the extensions to `server_name` (type 0) and takes the first `host_name` (type 0). A name must match `^[a-z0-9.-]{1,253}$` (case-insensitive) and is lowercased. It reads at most one record: 5 header bytes and up to 16,384 bytes. Input that is not a ClientHello, or a ClientHello spread over several records, goes to the dashboard.
- The client has 5 s to send its ClientHello, or the connection is closed. At most 256 connections are passed on at once; more are closed.
- What was read is replayed to the upstream, and the two are piped both ways, half-open. Either side closing closes both: a browser asking for a node that is down sees the connection close.

### Certificates

- Made by mkcert from its local CA, so a browser that trusts the CA (`mkcert -install`) trusts every dev page, and the nodes trust each other through `DASSIE_PEER_CA_FILE`, the CA's `rootCA.pem`. mkcert makes its CA on first use.
- ECDSA certificates in `tls/`, kept across runs and made again when either file is missing:

  | Certificate | Names |
  |---|---|
  | `localhost` | `localhost`, `dev-rpc.localhost`, `127.0.0.1`, `::1` |
  | `dN.localhost` | `dN.localhost`, `localhost`, `127.0.0.1` |
  | `rates` | `localhost`, `127.0.0.1` |
  | `fake-ledger` | `localhost`, `127.0.0.1` |

- When the CA changes (another `CAROOT`, or a CA made anew), every certificate in `tls/` is dropped and made again.
- Tests pass a `CAROOT` of their own, so they never touch the developer's CA.

## Nodes

- Each node is the real binary, started as `dassie daemon --http-port <4000+N> --https-port <5000+N> --listen 127.0.0.1`, with:

  | Variable | Value |
  |---|---|
  | `DASSIE_STATE_DIRECTORY` | `<state>/data/dN/state` |
  | `DASSIE_IPC_SOCKET_PATH` | its socket |
  | `DASSIE_PEER_CA_FILE` | the mkcert CA |
  | `DASSIE_PEER_ALLOW_PRIVATE` | `1`, so dev nodes reach each other on loopback and by `dN.localhost` ([Peer protocol](peer-protocol.md#outbound-requests)) |
  | `DASSIE_DEV_TIMINGS` | `1` |
  | `DASSIE_DEV_WALLET_DIR` | `apps/wallet/dist-dev`, served instead of the embedded wallet |
  | `DASSIE_LOG_LEVEL` | `warn` |
  | `DEBUG` | the dev server's `DEBUG`, or `das:*` |
  | `DASSIE_BOOTSTRAP_NODES` | the scenario's bootstrap nodes |

  What each variable does is in [Node](node.md#environment-variables).
- `DASSIE_BOOTSTRAP_NODES` is a JSON array with one `{ id, url, publicKey }` per bootstrap node: its vanity ID, `https://dN.localhost:<5000+N>`, and its Ed25519 public key in base64url without padding.
- **Start:** the data and socket directories are created and a socket left by a killed node removed; the node has 30 s to listen on its HTTPS port and its IPC socket. The dev server then talks to it as its owner over IPC.
- **Output:** every line a node prints goes to the terminal as `[dN] <line>` (warnings, errors and panics, given `DASSIE_LOG_LEVEL=warn`). Output is read line by line as it comes, each line cut at 4096 bytes, and the last 200 lines are kept for reports.
- **Stop:** SIGTERM, and SIGKILL after 5 s.
- **Failure:** a node that does not start or cannot be set up is stopped and dropped, the scenario is stopped, and its state becomes failed with the last 4 KiB of its output. A node that exits on its own is marked failed the same way, and logged.

### Setting nodes up

Nodes are set up through their own RPC over IPC and their HTTPS API, the way an owner would, never by writing the database: the schema stays private to the node. The end-to-end tests use the same code. On every start:

1. `SetCertificate` with the node's mkcert certificate and key (again each time, since the CA may have been made anew), and `SetHostname` `dN.localhost`.
2. If `GetBasicState` says `UNINITIALIZED`: `GetSetupUrl` gives the setup token; `POST https://localhost:<5000+N>/api/setup` with `Origin` set to that origin, trusting the CA, and the body `{ setupAuthorizationToken, rawDassieKeyHex, loginAuthorizationSignature }` from the vanity identity; anything but 200 fails. Then it waits for the engine (`ListLedgers` answers, polled every 100 ms for up to 15 s) and calls `SetAlias` `dN`.
3. If the node is set up already: its node ID must be the vanity one ("<dir> holds another node's data"), and it waits for the engine.
4. `SetRateSources` with the scenario's rates server as the only source, of kind `coinbase` (again each time, since the port may have moved).
5. `AddSettlementScheme` for each of the node's schemes not active yet, with the scheme's config (empty, or the fake ledger's for `xrpl-testnet`).

Running it again on a node that is set up leaves it as it is and adds only what is missing.

### Restarts and resets

| Event | The nodes' data |
|---|---|
| Restart: a new node build, **r**, or "Restart the nodes" | Kept: each node resumes |
| Start of the dev server, same scenario as last time | Kept; added nodes' data is wiped first |
| Start of the dev server, another scenario | Wiped |
| Choosing a scenario, or "Start over" | Wiped |

- **Adding a node** starts the next index with the scenario's schemes for added nodes, joining its network; at most 100 nodes in all. Added nodes do not outlive the dev server; across a restart they start again after the scenario's own, 500 ms apart.
- Commands from the dashboards and the keys run one at a time, in order; at most 4 wait behind the one running, and more are refused with a warning.

## Scenarios

| ID | Name | Nodes and schemes | Bootstrap | Added nodes |
|---|---|---|---|---|
| `two-nodes` (default) | Two Simple Nodes | d1, d2: stub | d1, d2 | stub |
| `six-nodes` | Six Simple Nodes | d1 … d6: stub | d1, d2 | stub |
| `xrpl-settlement` | XRPL Testnet Settlement | d1, d2: `xrpl-testnet` | d1, d2 | `xrpl-testnet` |
| `cross-currency` | Cross-currency Settlement | d1: stub; d2: stub and `xrpl-testnet`; d3: `xrpl-testnet` | d1, d2 | stub |
| `one-node` | One Node | d1: stub, no peers | none | stub |

- The scenario chosen is kept in `scenario.txt` and restored at start, unless `--scenario` names another; an unknown one falls back to `two-nodes`.
- `xrpl-settlement` and `cross-currency` settle on the public XRP Ledger testnet, or, with `--xrpl fake`, on the [fake XRP Ledger](#fake-xrp-ledger).
- For the tests, `Scenario.temporary` runs a scenario in a new temporary directory with a CA of its own and deletes it when stopped; a scenario can leave chosen nodes without a ledger. "Converged" means every node has a route to every other and at least `min(2, N − 1)` peers.

### Fake XRP Ledger

- The node's own model of the XRP Ledger (`cargo build --locked -p dassie-node --features testing --example fake_ledger`), served over WebSocket on 10200 with a faucet over HTTPS on 10201, under the `fake-ledger` certificate. It closes a ledger every second.
- It runs only for a scenario that uses `xrpl-testnet` and only with `--xrpl fake`. The `xrpl-testnet` scheme is set up with `{"server": <its WebSocket URL>, "faucet": <its faucet URL>}`.
- It is a new ledger on every run, which nodes treat as a testnet reset: their accounts are gone, and the faucet funds them again ([XRPL](xrpl.md)).
- It prints its URLs as one JSON line when it is ready (within 30 s); closing its stdin stops it, with a kill after 5 s.

### Exchange rates

Each scenario serves its own `rates.json` in Coinbase's format, `{"data": {"currency": "USD", "rates": {…}}}`, at `https://localhost:10210/rates.json` under the scenario's CA. The rates: USD 1, XRP 2 (a dollar buys two XRP), EUR 0.92. Tests change them, or make it answer 503. The setup makes it every node's only rate source ([Exchange rates](connector.md#exchange-rates)).

## Vanity identities

- `apps/dev/vanity.json` maps each index (0 is d1) to `[entropy, node ID]`: 16 bytes of BIP-39 entropy as hex, and a node ID brute-forced to start with `dN_`, for example `d1_UeCE9UMCwvVHSXdI9o0Tpz4rnidWvR`. It holds d1 to d1000 and the largest index ever found (the record).
- The phrase, node key, public key, node ID and login proof are derived from the entropy with `dassie-identity`, as the node derives them ([Identity](identity.md)), and the ID is checked against the table's; a mismatch throws. The table began as the original's, which the same derivation keeps valid.
- `node --run vanity < found.txt` (in `apps/dev`) adds a brute-forcer's finds, one `[0x]<32 hex>: <node ID>` a line (`#` starts a comment). Each is derived and refused if its entropy does not give its ID. An entry is kept if its index is new, or its ID has fewer `-` and `_` than the one there; indexes above 999 are dropped except the record. `node --run vanity -- --check` derives every entry.
- A short name is the part before the `_` (`d3_…` is `d3`).

## Wallet links

A node's wallet opens signed in from `https://dN.localhost/#dassie-dev-login=<login proof as hex>`: through the proxy, with its port if not 443; without the proxy, `https://dN.localhost:<5000+N>`. Only the wallet's development build reads the fragment and sends it as an ordinary login ([Wallet](wallet.md#dev-auto-login)); the node checks it like any other. Nothing in the node accepts a dev token.

## The dashboard's origin

One origin for the dashboard, its control channel and the relay to the nodes, so the dashboard keeps a strict CSP with `connect-src 'self'`. It is served on 10100 and 10101, at 127.0.0.1 and ::1 where there is one (IPv4 must work), under the `localhost` certificate.

- **Host:** a request whose host (without port, from `Host` or HTTP/2's `:authority`) is not `localhost` or `dev-rpc.localhost` gets 421 "misdirected", so no rebound name reaches it.
- **WebSocket upgrades** act as the nodes' owner, so the `Origin` must be exactly `https://<Host>`, the dashboard's own page; otherwise 403.
  - `/control`: the [control channel](#control-channel).
  - `/nodes/<dN>/rpc`: the [relay](#relay) to a running node's IPC socket; 404 if the node is not running, 400 unless the client offers the subprotocol `dassie.ui.v1`.
- **Files:** only `GET` and `HEAD` (else 405), from `apps/dev-dashboard/dist-dev`. A path without an extension is the page (`index.html`); a missing file with an extension is 404; a missing page is 503 "the dashboard is being built; reload in a moment"; a path with `..` is 400. Every response has `Cache-Control: no-cache`, `X-Content-Type-Options: nosniff` and the CSP:

  ```
  default-src 'self'; script-src 'self'; style-src 'self'; img-src 'self' data:; connect-src 'self';
  object-src 'none'; base-uri 'none'; form-action 'self'; frame-ancestors 'none'
  ```

  (the wallet's, without WebAssembly).
- **Closing** stops accepting, closes open sockets with 1001 "shutting down", and waits at most 1 s.

### Relay

- The dev server connects to the node's IPC socket (5 s to answer, or 502 "the node does not answer"), then completes the upgrade and passes messages both ways, one for one. On the IPC socket the node takes every call as its owner's ([RPC](rpc.md)).
- Messages are binary, at most 1 MiB each way (the contract's limit; a larger one closes with 1009); a text message closes with 1003.
- A page that stops reading, with more than 8 MiB queued, is closed with 1013. Either side closing closes the other; the node going away closes the page's socket with 1001.

### Control channel

`proto/dassie/dev/v1/control.proto`: one WebSocket at `/control`, each binary message one `ServerMessage` or `ClientMessage` of at most 1 MiB.

- **Server messages:**
  - `State`: the scenario running; every scenario (`id`, `name`, `description`, `available`, `unavailable`); the nodes (`name`, `id`, `url`, `state`, `login_proof_hex`, `last_error` cut at 4 KiB); the node build (`state`: idle, building or failed, `error_excerpt` cut at 8 KiB, `finished_at_unix_ms`); and whether the proxy runs. Node states: starting, setting up, running, stopped, failed.
  - `Logs`: `SourcedLogLine`s (`source` "host" or a node's short name, and the node's `LogLine`), oldest first; `reset` on the first message of a connection.
  - `Peerings`: each node's peers by short name, sent whole when they change.
  - `Traffic`: the hops (`from`, `to`, short names) of ILP packets between peers since the last message.
- **Client messages:** `SetScenario { id }` (stops every node, wipes their data, starts the scenario), `AddNode`, `RestartNodes` (keeps data), `Reset` (the scenario again from scratch). An unknown or unavailable scenario is refused with a warning in the log.
- **On connecting**, a dashboard gets `State`, then the logs the dev server holds in chunks of at most 256 KiB encoded (the first with `reset`), then `Peerings`; after that every change. `State` goes out whenever it changes.
- At most 32 dashboards at once (more: 1013). A dashboard with more than 8 MiB queued is dropped (1013 "not reading"). A text message closes with 1003, an undecodable one with 1007.

### Logs, peerings and traffic

The dev server follows each running node over IPC, once, and passes what it gathers on to every open dashboard, with its own log:

- `SubscribeLogs`: the node's lines, with source `dN`. After a resubscription only lines with an index above the last one seen pass.
- `SubscribeNodes`: the node's peers, those whose peering is `peered`, by short name.
- `SubscribePackets`: each packet whose destination is a peer (`peer:<id>`) is a hop from the node to that peer, counted once by its trace epoch and sequence (the last 4,096 are remembered). The peer protocol's own messages are not shown.
- The dev server's own log has source "host" and target `das:dev`; its messages are cut at 1024 bytes and also printed.
- Subscriptions are retried as the wallet's are ([Wallet](wallet.md#live-data)); one that fails for good is logged as a warning.
- Lines and hops go out every 100 ms. At most 1,000 hops wait for the next flush; more are dropped. The dev server keeps its last 10,000 lines from all sources.

## Dashboard

`apps/dev-dashboard`, "Dassie//dev": a Solid 2 page built with Vite, served by the dev server under the CSP above. It reuses `packages/ui`: the debug pages, the log viewer, the network graph, the router and the theme ([Wallet](wallet.md#shared-components-packagesui)).

- **Connection:** one WebSocket to `wss://<host>/control`, opened again whenever it closes, after a delay between 500 ms and a ceiling that doubles per attempt up to 10 s. While it is not open: "Connecting to the dev server…". While the node builds: "Building the node…"; when the build failed: "The node did not build", with the excerpt, and "The nodes keep running the binary built before."
- **Sidebar:** "Dassie//dev"; **Dashboard** (`/`), **Logs**, **Scenarios**; **Nodes** with a **+** that adds a node; a "host" entry and one per node, each with its seeded colour, its state, a link to its page that keeps the debug tab shown (`/nodes/dN/debug/<tab>`), and ↗, which opens its [wallet link](#wallet-links) in a new tab; the theme toggle.

| Route | Page |
|---|---|
| `/` | The network graph and every log line |
| `/logs` | Every log line, with a source column |
| `/scenarios` | A card per scenario: name, description, "running" on the current one. **Run** sends `SetScenario` and goes to `/`; the running one has **Start over** (confirmed: every node stops, loses its data and starts afresh; sends `Reset`) and **Restart the nodes** (`RestartNodes`). One not available yet says why. |
| `/nodes/host` | The dev server's own log |
| `/nodes/dN`, `/nodes/dN/*` | The node: "Node: dN", a **Wallet ↗** link, and the wallet's [debug pages](wallet.md#debug-pages) under `/nodes/dN/debug/…` |

- **Graph:** the nodes of the dev server's state; a link only where each of two nodes lists the other as its peer; a link lights up for 600 ms when a packet goes along it.
- **Log lines** have a source column linked to the node's page, and vanity IDs in messages become links named `dN`. The dashboard keeps the last 10,000 lines.
- **A node's page** opens an RPC client of its own to `wss://<host>/nodes/dN/rpc` (the relay) and asks `GetBasicState`, again after 1 s doubling to 10 s while the node is between restarts ("Cannot reach dN … Trying again…"). With an answer it follows the node's data and shows the debug pages under its own base path. A node that is not running shows its state and its last output.

## Dev CLI

`node --run cli -- dN <args…>` runs the `dassie` CLI against a dev node through its IPC socket (`DASSIE_IPC_SOCKET_PATH`), for example `node --run cli -- d1 status`. `dN` must match `^d[1-9]\d{0,3}$`. `DASSIE_DEV_BINARY` names another binary, and `DASSIE_DEV_STATE_DIR` another state directory. The exit code is the CLI's (128 plus the signal for a signal).

## Builds

- **The node:** `cargo build --locked -p dassie-cli` with `CARGO_TERM_COLOR=never`, one at a time: requests during a build make one more build after it. Only lines starting with `error` or `warning: unused` are printed. A failure's excerpt starts at its first `error` line and is cut at 8 KiB. The nodes keep running the binary built before.
- **Watching:** `crates/` and `proto/`, recursively, ignoring `target/`. Changes are gathered until the files have been still for 300 ms. A changed `.proto` regenerates `packages/rpc` first (`node scripts/gen.ts`; a failure is a warning to run `node --run gen` there). A successful build restarts the nodes.
- **The wallet and the dashboard:** `vite build --watch --mode development` each; the first build has 180 s. A change shows after a reload.

## Tooling is not simulated

The dev environment and the dashboard are tools around the node, not part of it. Their pure parts (the SNI parser, the vanity table, the log ring, the peer set and traffic filter, the control messages' chunking, the relay's checks, paths, stale-process handling, the dashboard's client and graph) get unit tests; the rest is tested end to end against real binaries (`tests/e2e/dev_test.ts`: the dashboard through the proxy, the relay refusing other pages, a node's debug pages, the wallet link, adding a node, switching and resuming scenarios, a second dev server on the same state). They are still bounded: rings, frames, queues and connections have the limits above.

## Decisions

- [D1](../decisions/D01-what-runs-in-rust-and-what-in-typescript.md): the dev environment is TypeScript; the nodes are the real Rust binary.
- [D16](../decisions/D16-engineering-style-pragmatic-tigerstyle-and-deterministic-simulation.md): tooling is not simulated, but bounded.
- [D21](../decisions/D21-tooling-on-node-and-pnpm-not-deno.md): Node and pnpm, `node --run`.

## Original

- [6 §6](../original/06-user-interfaces-and-tooling.md): the original dev environment (`app-dev`): launch, prerequisites, ports, the vanity table, the database prefill, runner processes, scenarios, the CLI wrapper.
- [6 §2](../original/06-user-interfaces-and-tooling.md): the original dashboard (`gui-dev`).
