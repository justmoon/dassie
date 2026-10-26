# Dassie

> A multi-currency, zero-config, peer-to-peer, Interledger-enabled payment network

Dassie is a payment node you run on your own server. It finds other Dassie nodes, peers with them on its own, and routes [Interledger](https://interledger.org) payments through them. Peers keep track of what they owe each other and settle it on a real ledger, today the XRP Ledger testnet. You run your node from a web wallet that it serves itself, and apps can pay through it with [Open Payments](https://openpayments.dev) or BTP.

**Status:** Dassie is early and works with test money only. A new node joins the public testnet through its four bootstrap nodes, `boot1`–`boot4.dassie.xyz` ([bootstrap-network](spec/decisions/bootstrap-network.md)).

This is the second implementation of Dassie, written in Rust, Node and Solid 2. It replaced the original TypeScript implementation, which you can still find at the tag `original`. The [specification](spec/README.md) describes Dassie as it is on `main`; start with its README, which says which docs to read for the code you touch. It has one doc per area, such as the [wallet](spec/areas/wallet.md) or the [connector](spec/areas/connector.md), which states that area's rules; one file per [decision](spec/decisions/README.md), which records why a rule is what it is; and [the original's chapters](spec/original/README.md), which describe how the original works, for reference.

## Installing a node

You need a Linux server with systemd, on x86-64 or arm64, with a public IP address, a domain name that points to it, TCP ports 80 and 443 open, and UDP port 443 open for QUIC, over which nodes talk to each other. The installer is tested on Debian 12, Ubuntu 24.04, Fedora 42 and CentOS Stream 9. On the server, run:

```sh
curl --proto =https --tlsv1.2 -sSf https://sh.dassie.land | sh
dassie verify-install
sudo dassie init
```

The installer fetches the newest release from `get.dassie.land`, checks its signature before unpacking anything, installs it in `/opt/dassie` and sets up the systemd units. It also adds you to the group `dassie-users`, whose members may talk to the node; log in again for that to take effect.

`dassie init` asks for your domain name and an email address, gets a certificate from Let's Encrypt, and prints a URL where you finish the setup in your browser. If you miss it, `sudo dassie setup-url` prints it again. For a node nobody sets up in a browser, `sudo dassie setup --passphrase-file <file>` sets it up from a passphrase you keep in a file on the server, and `--scheme <id>` adds a settlement scheme; the passphrase stays on the server, and only the node's key reaches the node.

Dassie updates itself: once a day, a systemd timer checks for a newer release, verifies it and switches to it, and goes back if the new version does not start. `sudo dassie update` does the same right away. To uninstall, run the installer with `sh -s -- uninstall`. The node's data stays in `/var/lib/private/dassie` in case you change your mind.

The [Quick Start](https://dassie.land/guides/quick-start/) on dassie.land walks through the same steps.

On NixOS, each release publishes a flake instead, `https://get.dassie.land/<version>/flake.tar.gz`, and `https://get.dassie.land/flake/latest.tar.gz` is the newest one, which `nix flake update` follows. It holds a module that runs the release's signed binary under its own systemd units, without the updater. [Dassie on NixOS](https://dassie.land/guides/nixos/) shows how to use it.

## Using your node

Once set up, the wallet asks you for a first ledger:

- **Stub** settles nothing, but gives you 100 test USD to play with.
- **XRP Ledger Testnet** settles in test XRP. The testnet faucet funds the node's account. Over RPC, it is `addSettlementScheme` with `xrpl-testnet`, and an optional JSON config naming another `server` or `faucet`.
- **DSUSD on the XRP Ledger Testnet**, `xrpl-testnet-dsusd`, settles in DSUSD, Dassie's own testnet dollar, from the same account, which pays its fees in XRP; you buy DSUSD with test XRP on the testnet's XRP/DSUSD pool. It stands in on the testnet for Ripple's RLUSD, whose testnet scheme, `xrpl-testnet-rlusd`, is disabled: a node that ran it starts without it, and its test RLUSD stays on its line.

From there, the wallet sends and receives payments, shows their status and your activity, and manages everything below.

**Open Payments.** Each ledger has a wallet address at `https://<your host>/pay/<ledger>`, for example `/pay/stub`, and `$<your host>` names the default one. Other wallets and apps pay into it, and with a grant you approve in the wallet, they may also pay out of it, up to the limits you agreed to. The wallet's Send takes a payment pointer or a payment request, and its grants page lists who may do what.

A node without a hostname answers Open Payments at `https://<label>.dassiepay.net` instead, through the front door there, which reaches it over a connection the node keeps to it while it has no hostname. `DASSIE_RELAY` names another relay, as the entry its front door logs, or, set empty, none ([default-relay](spec/decisions/default-relay.md)).

**BTP.** A BTP token, made in the wallet's settings, lets an ILP client such as `ilp-plugin-btp` spend through the node, at most its spending cap per window if you give it one. Over RPC, `createBtpToken` answers with the `btp+wss://` URL to connect with.

**Exchange rates.** When your ledgers hold different currencies, the node converts between them at rates it fetches from Coinbase, Kraken, Bitstamp and the European Central Bank, taking the median of the three that cover each currency. You can change the sources and the currency the wallet shows totals in, in the wallet's settings or with `setRateSources` and `setDisplayCurrency`.

**Peers.** Nodes find each other through bootstrap nodes: by default the testnet's, `boot1`–`boot4.dassie.xyz`. A node needs a hostname to take part; it then registers with them by itself, and peers with nodes that share one of its settlement schemes. The bootstrap nodes run `xrpl-testnet` and `xrpl-testnet-dsusd`.

`DASSIE_BOOTSTRAP_NODES` replaces the list, as JSON: `[{"id": "d…", "url": "https://host:port", "publicKey": "<base64url>"}]`, or `[]` to run alone. Under systemd, set it in a drop-in, then `sudo systemctl daemon-reload && sudo systemctl restart dassie`; `dassie update` leaves drop-ins alone. Version 0.1.1 and earlier have no default list, so they join the testnet with this one:

```ini
# /etc/systemd/system/dassie.service.d/bootstrap.conf
[Service]
Environment='DASSIE_BOOTSTRAP_NODES=[{"id":"dDHq9ng9J1z7luQ64eN4MJ_4_w-KK4FPk","url":"https://boot1.dassie.xyz","publicKey":"w25V-7IreZNDCPvwCrpiIv9Li5uwomzPSvqHot9ffLI"},{"id":"dE0GE90-9QCQI78S2_h5QaG81oo_cVKz1","url":"https://boot2.dassie.xyz","publicKey":"s9xgWff_UZ26HQwHT_aPd0q0SwyKPGXS74fd2xQjh14"},{"id":"dtTay2e6AV4J940nFJ6_WPkVL-TyrVKGW","url":"https://boot3.dassie.xyz","publicKey":"--P-gYMJ_ZZrFHa4GiNfVQ0x2jSu9_dr7z0mI_bcngY"},{"id":"dxZ8EFGaoQLvgSQacinFm64zjWeA0BzV-","url":"https://boot4.dassie.xyz","publicKey":"qNW0ZwnIvaca6f54gIG-YFLQSefzC6JA4h3Oh1QJU6I"}]'
```

A node without a hostname still peers, as a periphery node: it asks core nodes to be its uplinks and is reached through them. `DASSIE_UPLINKS` pins them, as node IDs separated by commas (`d…,d…`): the node then asks only those, and looks for as many uplinks as it names, which is what a market maker's nodes need to trade through one core node. A node with a hostname ignores it, and says so when it starts.

`listNodes`, `listPeers`, `listRoutes` and `getRegistrationStatus` show what your node knows, and the wallet's network page shows the same.

The node that asks to peer extends the other 1.1 units of credit; the node that accepts extends nothing at first, so the newcomer pays in advance. Each side's credit then grows by a tenth of the fees the other earned it, up to 10 units. A node asks a peer to settle once its room for the peer is down to one and a half of the largest packets, checking every 4 seconds; asked, a node settles half the way to its own point of asking; `listSettlementChannels` shows the settlements on their way. Each node keeps its own books, Fulfills it sent included, whatever its peer counted ([own-fulfills](spec/decisions/own-fulfills.md)): a peer can cost your node at most the credit it extends, since packets past it are refused. The wallet's settings, or `getPeerSettings` and `setPeerSettings`, change the fee and the exchange spread your node charges for passing payments on, and how far its holdings move the rate it converts at, so that payments bring them back toward balance.

`terminatePeering` ends a peering. Your node first collects, without telling the peer: it takes nothing new from it, asks it to settle once more, and waits for that settlement; then it tells the peer, and the peering drains and ends on both sides. Nothing else is settled for its end ([ending-a-peering](spec/decisions/ending-a-peering.md)): what either side still owes stays on its books, for a later peering to settle. `banNode`, `unbanNode` and `listBans` keep nodes away. `GET /.well-known/dassie/node` tells anyone the node's ID, key and rate feed port.

**Logs.** `DASSIE_LOG` sets what the node logs: a level (`trace`, `debug`, `info`, `warn`, `error` or `off`; `info` by default), or directives per target in the form of `tracing`'s `EnvFilter`, such as `info,dassie::peer=debug`. The node's targets are `dassie::*`. The wallet's debug pages show the recent log as well.

## Layout

| Path | What |
|---|---|
| `crates/dassie-oer` | Canonical X.696 OER, the subset the protocols use |
| `crates/dassie-identity` | BIP-39 phrases, seed paths, node keys, node IDs, X25519 |
| `crates/dassie-ilp` | ILPv4 packets, addresses, timestamps, ILDCP, BTP 2.0 |
| `crates/dassie-stream` | STREAM as sans-IO connection and server state machines |
| `crates/dassie-ledger` | The internal double-entry ledger and its transfer journal |
| `crates/dassie-settlement` | The settlement scheme interface, the settlement policy, activation reconciliation, the stub scheme |
| `crates/dassie-xrpl` | The XRP Ledger: wallets, the Payment codec, the rippled API, the testnet settlement scheme, and a fake ledger for tests |
| `crates/dassie-connector` | The ILP connector: endpoints, peers and the route table, ILDCP, amounts, exchange, fees and spread, spending caps, the packet trace |
| `crates/dassie-peer` | Peer protocol v1: wire format, authentication, node table, link state, membership, registration, peering and its lifecycle, settlement delivery, routing |
| `crates/dassie-payments` | Open Payments without IO: resources, GNAP grants, HTTP message signatures, payment pointers, intervals, quote math |
| `crates/dassie-feed` | The public rate feed without IO: signed UDP datagrams, the subscriber table and its address cookies, when to publish, a subscriber's side; the node does not publish one yet |
| `crates/dassie-node-core` | The node's decisions without IO: config, setup, sessions, origins, rate limits, ACME and TLS policy |
| `crates/dassie-node` | The daemon: database, HTTP(S) and IPC listeners, TLS, ACME, owner login, RPC with its live streams, the engine (ledger, peer protocol, Open Payments, bans), the QUIC endpoint and the peer client, the BTP server, the STREAM receiver, the rate fetcher, the discovery document, the log ring, and a fake XRP Ledger for tests |
| `crates/dassie-cli` | The `dassie` binary: `daemon`, `init`, `update`, `verify-install` |
| `crates/dassie-release` | What a release is and how to trust one: versions and channels, the mirror's layout, `SHA256SUMS` and its minisign signature |
| `crates/dassie-sim` | Runs the simulations from any seed, to replay and explain failures: `dassie-sim run peer 0..1000`, `dassie-sim explain peer 42` ([random-seed-simulations](spec/decisions/random-seed-simulations.md)); a developer tool, never released |
| `nix/release.nix` | The release builds: the wallet, static binaries for x64 and arm64, their bundles, the mirror tree, and the install tests' releases |
| `resources` | What a release ships beside the binary: the systemd units (the node, its sockets and the daily updater) and `install.sh`, and the release key (`minisign.pub`) |
| `scripts/build-wasm.ts` | Builds the WASM modules and their bindings (`node --run build:wasm`) |
| `scripts/release.ts` | Signs a built release for the mirror and checks it with `dassie-release verify`, as the release workflow does |
| `proto/dassie/owner/v1` | The RPC contract between the node and its owner's clients |
| `proto/dassie/dev/v1` | The dev environment's control channel, between the dev server and its dashboard |
| `apps/wallet` | The wallet (Solid 2), embedded in the binary: every page of the original, with live data, plus grants and consent for Open Payments |
| `apps/dev` | `node --run dev`: the scenarios on real binaries, certificates, vanity identities, the dev server with its SNI proxy and relay, a rates server; also the scenario library the end-to-end tests use |
| `apps/dev-dashboard` | The dev dashboard (Solid 2): the network, every node's log, the scenarios, each node's debug pages |
| `apps/website` | dassie.land: the home page, the Quick Start, the NixOS guide, and the STREAM toy, which runs `dassie-stream` in WASM over a simulated network; `main` publishes it to GCP |
| `packages/ui` | The Solid components the wallet, the dev dashboard and the STREAM toy share: amounts, the log viewer, the network graph, the debug pages, the router, live data, the theme |
| `packages/wasm` | The core crates in WASM (bound by `crates/dassie-wasm`), one build for browsers and Node: identity, and STREAM with ILP's packets, as two modules |
| `packages/rpc` | The client for the RPC contract, over WebSocket (browsers) or the IPC socket (Node) |
| `tests/interop` | STREAM interop with `ilp-protocol-stream`, natively and in WASM; WASM vectors; the browser test |
| `tests/e2e` | End-to-end tests on real binaries: a fresh install through `dassie init` and browser setup, with Pebble as the ACME CA; a BTP client paying the node's STREAM receiver; the `two-nodes` and `six-nodes` scenarios; Open Payments, including a third party's full flow with consent in the wallet; XRPL settlement and cross-currency payments; the wallet's pages in a browser; `node --run dev` and its dashboard; the installer and the updater on Debian, Ubuntu, Fedora and CentOS, in containers; dassie.land and its STREAM toy in a browser, under the site's CSP |
| `testdata/original` | Generator for the golden vectors, run against the original |
| `testdata/feed` | The rate feed's golden vectors, from `cargo run -p dassie-feed --example feed_vectors` |
| `testdata/xrpl` | Generator for the XRPL golden vectors, run against xrpl.js |
| `testdata/tls` | The tests' TLS certificate for localhost and its CA, which Pebble (Let's Encrypt's test server) serves its API with, as do the nodes and mirrors of the tests; and Pebble's configuration |
| `testdata/release` | The release test key and what minisign signs with it |

## Building and testing

The easiest way to get every tool at the right version is [Nix](https://nixos.org): `nix develop` gives you a shell with all of them, and `nix flake check` runs what CI runs (CI builds those checks on the Vorstand Nix cloud), offline; CI also checks the lockfile against the npm and JSR registries, which `pnpm install` does too.

Without Nix, you need:

- the Rust toolchain named in `rust-toolchain.toml`, with the `wasm32-unknown-unknown` target;
- the `wasm-bindgen` CLI, at the version in `Cargo.lock`;
- Node 26 and pnpm 11, and Chrome or Chromium;
- for the end-to-end tests, also `pebble`, `pebble-challtestsrv`, `systemd-socket-activate` and `mkcert`. Without them, those tests are skipped outside CI.

```sh
cargo fmt --check && cargo clippy --all-targets -- -D warnings && cargo test
pnpm install                  # the npm and JSR packages, as pnpm-lock.yaml pins them
node --run build:wasm         # WASM modules and bindings into packages/wasm/gen
node --run build:wallet       # the wallet into apps/wallet/dist
node --run build:dashboard    # the dev dashboard into apps/dev-dashboard/dist
node --run build:website      # dassie.land into apps/website/dist
node --run build:e2e          # target/e2e/dassie with the wallet built in, the fake XRP Ledger and the release tool
node --run check              # fmt (dprint), lint (oxlint), type check (tsc)
node --run test               # the apps', packages' and scripts' tests
node --run test:e2e           # WASM vectors, STREAM interop, browser, end to end
```

`node --run ci` runs all the Node steps in order, building what they need first. If you change `proto/`, run `node --run gen` in `packages/rpc`; a test fails until the generated code matches.

The XRPL end-to-end tests run against the node's fake ledger. To run them against the real testnet, by hand, set `DASSIE_XRPL_TESTNET=1`.

The updater's tests need the test key built in: `cargo test -p dassie-cli --features test-key` (the Nix check does this). The install tests run the installer and the updater on four distributions in Docker. They need privileged containers and mkcert:

```sh
nix build .#install-test-mirror -o install-test-mirror
DASSIE_INSTALL_MIRROR=$PWD/install-test-mirror DASSIE_INSTALL_DISTROS=debian \
  node --test tests/e2e/install_test.ts
```

### Trying a node without root

You can run a node on high ports as yourself, and configure it from another terminal:

```sh
DASSIE_STATE_DIRECTORY=/tmp/dassie DASSIE_IPC_SOCKET_PATH=/tmp/dassie.sock \
  target/e2e/dassie daemon --http-port 8080 --https-port 8443
DASSIE_IPC_SOCKET_PATH=/tmp/dassie.sock target/e2e/dassie init --hostname localhost --skip-acme
```

Then open the setup URL it prints. The certificate stays self-signed until `dassie init` obtains a real one, which needs a public hostname with port 80 reachable.

To make a few such nodes find each other locally, set `DASSIE_PEER_ALLOW_PRIVATE=1` so they may reach each other on `localhost`, `DASSIE_EXTRA_CA_FILE` to the CA their certificates come from, and `DASSIE_DEV_TIMINGS=1` so the protocol polls and settles every second. Keep such nodes on loopback with `--listen 127.0.0.1`; the daemon warns about either flag when it listens anywhere else. `node --run dev`, below, does all of this for you.

## Releases

Releases are built by Nix on x86_64 Linux ([the build](spec/areas/release.md#the-build)):

- `nix build .#dassie-x64` (or just `nix build`) or `.#dassie-arm64` makes a static binary with the wallet built in;
- `nix build .#release` makes the tree a release uploads to the mirror, with its flake for NixOS, without its signature.

The wallet's build fetches its packages one by one with the hashes in `pnpm-lock.yaml`, so a change to the lockfile needs no hash updated by hand. For every tag `v<version>`, the release workflow runs the install tests on each distribution, boots the NixOS module in a VM (`nix build .#nixos-test`) and checks the static binaries (`nix build .#static-binaries`), then signs and publishes the release. CI on pull requests and `main` runs neither.

## The dev environment

`node --run dev` (in `nix develop`, or with mkcert installed) runs a small network of nodes on your machine, with a dashboard to watch it:

- **Builds:** it builds the node with cargo, and again whenever `crates/` or `proto/` change, restarting the nodes with their data. The wallet and the dashboard are built with `vite build --watch` in development mode, into `dist-dev/`. The nodes serve that wallet (`DASSIE_DEV_WALLET_DIR`), so a reload shows your change.
- **The dashboard** is at <https://localhost> (through an SNI proxy on 443) or <https://localhost:10100>. It shows the network as it peers and pays, every node's log, the scenarios, and each node's debug pages. Each node's ↗ opens its wallet, signed in.
- **The nodes** are `d1`, `d2`, … at `https://dN.localhost` (or `:500N` directly). They use the original's vanity identities, mkcert certificates, and fixed exchange rates from a local rates server. Their data is in `local/dev/`. `node --run cli -- dN <command>` runs the CLI against one, for example `node --run cli -- d1 init`.
- **The scenarios:** `two-nodes` (the default), `six-nodes` and `one-node` settle on the stub ledger; `xrpl-settlement` settles on the XRP Ledger, and `cross-currency` pays between the stub and XRP through a third node. The XRPL ones use the public testnet unless you pass `--xrpl fake`. The scenario you chose last runs again next time, or pick one with `--scenario <id>`.
- **Keys:** space opens the dashboard, `r` restarts the nodes, `q` or ctrl+c quits.

The first time, run `mkcert -install` so your browser trusts the certificates. On Linux, port 443 needs `sudo sysctl -w net.ipv4.ip_unprivileged_port_start=0`; without it, the dev server tells you and runs without the proxy. `--port-offset`, `--state-dir`, `--no-proxy`, `--no-watch`, `--headless` and `--dassie <binary>` are there for tests and special cases (see `apps/dev/main.ts`).

To work on the wallet alone, run `node --run dev` in `apps/wallet` and start a node with `DASSIE_DEV_WALLET_DIR` set to the absolute path of `apps/wallet/dist-dev`. `DASSIE_E2E_WALLET_DIR` does the same for the wallet's browser test, and `DASSIE_E2E_SCREENSHOTS` keeps a picture of each step of the wallet's and the dev environment's browser tests.

To work on the website, run `node --run dev` in `apps/website`, which rebuilds `dist/` on every change, and `node --run preview` next to it to serve it. A push to `main` that changes what the site is built from publishes it to a bucket on GCP ([deployment](spec/areas/website.md#deployment)). The headers every response gets, the CSP among them, are in its `headers.json`.

## Engineering style

The code follows a pragmatic TigerStyle and is built for deterministic simulation testing ([tigerstyle](spec/decisions/tigerstyle.md), [Engineering](spec/engineering.md#pragmatic-tigerstyle)): the core crates never read a clock or the OS RNG, bound every table and queue, and check their invariants after every step of their simulations.

## License

Apache-2.0, see [LICENSE](LICENSE).
