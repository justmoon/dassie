# Dassie (rewrite)

> A multi-currency, zero-config, peer-to-peer, Interledger-enabled payment network

This branch holds the new implementation of Dassie in Rust, Deno and Solid 2. It has no history in common with `main`, which holds the original TypeScript implementation.

The work starts from the specification in [`spec/`](spec/README.md):

- the original's functionality and operation, in chapters 1–6;
- the plan for the rewrite, in [chapter 7](spec/07-rewrite.md);
- the decisions taken for the rewrite, in [chapter 8](spec/08-decisions.md).

## Layout

| Path | What |
|---|---|
| `crates/dassie-oer` | Canonical X.696 OER, the subset the protocols use |
| `crates/dassie-identity` | BIP-39 phrases, seed paths, node keys, node IDs, X25519 |
| `crates/dassie-ilp` | ILPv4 packets, addresses, timestamps, ILDCP, BTP 2.0 |
| `crates/dassie-stream` | STREAM as sans-IO connection and server state machines |
| `crates/dassie-ledger` | The internal double-entry ledger and its transfer journal |
| `crates/dassie-settlement` | The settlement scheme interface, the settlement policy, activation reconciliation, the stub scheme |
| `crates/dassie-connector` | The ILP connector: endpoints, peers and the route table, ILDCP, amounts, spending caps, the packet trace |
| `crates/dassie-peer` | The peer protocol v1: wire format, authentication, node table, link state, membership, registration, peering and its lifecycle, settlement delivery, routing |
| `crates/dassie-node-core` | The node's decisions without IO: config, setup, sessions, origins, rate limits, ACME and TLS policy |
| `crates/dassie-node` | The daemon: database, HTTP(S) and IPC listeners, TLS, ACME, owner login, RPC with its live streams, the ledger's engine with the peer protocol, `/peer` and the peer client, the BTP server, the STREAM receiver, `/stats`, the log ring |
| `crates/dassie-cli` | The `dassie` binary: `daemon`, `init`, `update`, `verify-install` |
| `crates/dassie-release` | What a release is and how to trust one: versions and channels, the mirror's layout, `SHA256SUMS` and its minisign signature |
| `nix/release.nix` | The release builds: the wallet, static binaries for x64 and arm64, their bundles, the mirror tree, and the install tests' releases |
| `resources` | What a release ships beside the binary: the systemd units and `install.sh`, and the release key (`minisign.pub`) |
| `scripts/release.ts` | Signs a built release for the mirror and checks it with `dassie-release verify`, as the release workflow does |
| `proto/dassie/ui/v1` | The RPC contract between the node and its owner's clients |
| `proto/dassie/dev/v1` | The dev environment's control channel, between the dev server and its dashboard |
| `apps/wallet` | The wallet (Solid 2), embedded in the binary: every page of the original but Send and payment status (M6), with live data |
| `apps/dev` | `deno task dev`: the scenarios on real binaries, certificates, vanity identities, the dev server with its SNI proxy and relay; also the scenario library the end-to-end tests use |
| `apps/dev-dashboard` | The dev dashboard (Solid 2): the network, every node's log, the scenarios, each node's debug pages |
| `packages/ui` | The Solid components the wallet and the dev dashboard share: amounts, the log viewer, the network graph, the debug pages, the router, live data, the theme |
| `packages/identity-wasm` | The core crates for Deno and browsers, with a STREAM driver |
| `packages/rpc` | The client for the RPC contract, over WebSocket (browsers) or the IPC socket (Deno) |
| `tests/interop` | Interop with `ilp-protocol-stream`, WASM vectors, browser test |
| `tests/e2e` | A fresh install through `dassie init` and browser setup, with Pebble as the ACME CA; a BTP client paying the node's STREAM receiver; the `two-nodes` and `six-nodes` scenarios; the wallet's pages in a browser against two nodes; `deno task dev` and its dashboard; the installer and the updater on Debian, Ubuntu, Fedora and CentOS, in containers |
| `testdata/original` | Generator for the golden vectors, run against the original |
| `testdata/pebble` | Configuration and API certificate for Pebble, Let's Encrypt's test server, also served by the nodes in the BTP and peer tests |
| `testdata/release` | The release test key and what minisign signs with it |

## Building and testing

With [Nix](https://nixos.org), `nix develop` provides every tool at the pinned versions, and `nix flake check` runs what CI runs in the sandbox (CI builds those checks on [nixbuild.net](https://nixbuild.net)). Without it, install the Rust toolchain named in `rust-toolchain.toml` with the `wasm32-unknown-unknown` target, the `wasm-bindgen` CLI at the version in `Cargo.lock`, Deno, and Chrome or Chromium; the end-to-end tests also need `pebble`, `pebble-challtestsrv`, `systemd-socket-activate` and `mkcert`, and are skipped without them outside CI.

```sh
cargo fmt --check && cargo clippy --all-targets -- -D warnings && cargo test
deno task build:wasm   # WASM modules and bindings into packages/identity-wasm/gen
deno task build:wallet # the wallet into apps/wallet/dist
deno task build:dashboard # the dev dashboard into apps/dev-dashboard/dist
deno task build:e2e    # target/e2e/dassie, with the wallet built in
deno task check        # fmt, lint, type check
deno task test         # WASM vectors, STREAM interop, browser, end to end
```

`deno task ci` runs the Deno steps in order, building what they need first. After changing `proto/`, run `deno task gen` in `packages/rpc`; a test fails until the generated code matches.

Releases are built by Nix on x86_64 Linux ([spec 7 §4.8](spec/07-rewrite.md#48-releases)): `nix build .#dassie-x64` or `.#dassie-arm64` makes a static binary with the wallet built in, and `nix build .#release` (or `.#canary`) the tree a release uploads to the mirror, but for its signature. After changing `deno.lock`, set the new hash of `denoDeps` in `nix/release.nix`, which the build reports. The release workflow signs and publishes: a canary for every push to the main branch, and a release for every tag `v<version>`.

The updater's tests need the test key built in: `cargo test -p dassie-cli --features test-key` (the Nix check does this). The install tests run the installer and the updater on four distributions in Docker; they need privileged containers and mkcert:

```sh
nix build .#install-test-mirror -o install-test-mirror
DASSIE_INSTALL_MIRROR=$PWD/install-test-mirror DASSIE_INSTALL_DISTROS=debian \
  deno test -A tests/e2e/install_test.ts
```

To try a node without root, run it standalone on high ports, then configure it from another terminal:

```sh
DASSIE_STATE_DIRECTORY=/tmp/dassie DASSIE_IPC_SOCKET_PATH=/tmp/dassie.sock \
  target/e2e/dassie daemon --http-port 8080 --https-port 8443
DASSIE_IPC_SOCKET_PATH=/tmp/dassie.sock target/e2e/dassie init --hostname localhost --skip-acme
```

and open the setup URL it prints. The certificate is self-signed until `dassie init` obtains one (it needs a public hostname with port 80 reachable).

Once set up, the wallet asks for a first ledger. The stub scheme gives the owner 100 test USD; a BTP token, made in the wallet's settings, lets an ILP client such as `ilp-plugin-btp` spend them through the node, at most its spending cap per window if it has one. The same works over RPC: `addSettlementScheme` with `stub`, then `createBtpToken`, whose answer holds the `btp+wss://` URL to connect with.

Nodes find each other through bootstrap nodes. There are none by default until the v1 network's exist ([D15](spec/08-decisions.md#d15-bootstrap-network-for-v1)), so a node runs alone until `DASSIE_BOOTSTRAP_NODES` names some, as JSON: `[{"id": "d…", "url": "https://host:port", "publicKey": "<base64url>"}]`. A node needs a hostname to take part. For a local network, `DASSIE_PEER_ALLOW_PRIVATE=1` lets nodes reach each other on `localhost`, `DASSIE_PEER_CA_FILE` names the CA their certificates come from, and `DASSIE_DEV_TIMINGS=1` makes the protocol poll every second; `deno task dev` sets up such networks (below). The RPC methods `listNodes`, `listPeers`, `listRoutes` and `getRegistrationStatus` show what a node knows.

Peers extend each other 1.1 units of credit, and a node settles with a peer once it owes it a tenth more than the middle of that range, every 4 seconds (every second with dev timings). `listSettlementChannels` shows the settlements on their way. `terminatePeering` ends a peering: it drains, ends on both sides, and each settles what it owes; `banNode`, `unbanNode` and `listBans` keep nodes away. The wallet's network page does the same. `GET /stats` tells anyone the node's version and its advertised peers.

## The dev environment

`deno task dev` (in `nix develop`, or with mkcert installed) runs a network of nodes on this machine and a dashboard to watch it:

- **Builds:** the node with cargo, again whenever `crates/` or `proto/` change, restarting the nodes with their data. The wallet and the dashboard with `vite build --watch` in development mode, into `dist-dev/`; the nodes serve that wallet (`DASSIE_DEV_WALLET_DIR`), and a reload shows a change.
- **The dashboard** is at <https://localhost> (through an SNI proxy on 443) or <https://localhost:10100>. It shows the network as it peers and pays, every node's log, the scenarios, and each node's debug pages; each node's ↗ opens its wallet, signed in.
- **The nodes** are `d1`, `d2`, … at `https://dN.localhost` (or `:500N` directly), with the original's vanity identities, mkcert certificates, and the stub ledger. Their data is in `local/dev/`; `deno task cli d1 status` runs the CLI against one.
- **The scenarios:** `two-nodes` (the default), `six-nodes` and `one-node`; `xrpl-settlement` and `cross-currency` come with the XRPL module (M7). The last one chosen runs again next time.
- **Keys:** space opens the dashboard, `r` restarts the nodes, `q` or ctrl+c quits.

Once, run `mkcert -install` so the browser trusts the certificates. Port 443 needs `sudo sysctl -w net.ipv4.ip_unprivileged_port_start=0` on Linux; without it the dev server says so and goes on without the proxy. `--port-offset`, `--state-dir`, `--no-proxy`, `--no-watch` and `--dassie <binary>` serve tests and special cases (see `apps/dev/main.ts`).

To work on the wallet alone, run `deno task dev` in `apps/wallet` and start a node with `DASSIE_DEV_WALLET_DIR` set to the absolute path of `apps/wallet/dist-dev`. `DASSIE_E2E_WALLET_DIR` does the same for the wallet's browser test, and `DASSIE_E2E_SCREENSHOTS` keeps a picture of each step of the wallet's and the dev environment's browser tests.

## Engineering style

The code follows a pragmatic TigerStyle and is built for deterministic simulation testing ([D16](spec/08-decisions.md#d16-engineering-style-pragmatic-tigerstyle-and-deterministic-simulation)): the core crates never read a clock or the OS RNG, bound every table and queue, and check their invariants after every step of their simulations.
