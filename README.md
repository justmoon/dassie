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
| `crates/dassie-node-core` | The node's decisions without IO: config, setup, sessions, origins, rate limits, ACME and TLS policy |
| `crates/dassie-node` | The daemon: database, HTTP(S) and IPC listeners, TLS, ACME, owner login, RPC |
| `crates/dassie-cli` | The `dassie` binary: `daemon`, `init`, `verify-install` |
| `proto/dassie/ui/v1` | The RPC contract between the node and its owner's clients |
| `apps/wallet` | The wallet (Solid 2), embedded in the binary; so far setup and login |
| `packages/identity-wasm` | The core crates for Deno and browsers, with a STREAM driver |
| `packages/rpc` | The browser's client for the RPC contract |
| `tests/interop` | Interop with `ilp-protocol-stream`, WASM vectors, browser test |
| `tests/e2e` | A fresh install through `dassie init` and browser setup, with Pebble as the ACME CA |
| `testdata/original` | Generator for the golden vectors, run against the original |
| `testdata/pebble` | Configuration and API certificate for Pebble, Let's Encrypt's test server |

## Building and testing

With [Nix](https://nixos.org), `nix develop` provides every tool at the pinned versions, and `nix flake check` runs what CI runs in the sandbox. Without it, install the Rust toolchain named in `rust-toolchain.toml` with the `wasm32-unknown-unknown` target, the `wasm-bindgen` CLI at the version in `Cargo.lock`, Deno, and Chrome or Chromium; the end-to-end tests also need `pebble`, `pebble-challtestsrv` and `systemd-socket-activate`, and are skipped without them outside CI.

```sh
cargo fmt --check && cargo clippy --all-targets -- -D warnings && cargo test
deno task build:wasm   # WASM modules and bindings into packages/identity-wasm/gen
deno task build:wallet # the wallet into apps/wallet/dist
deno task build:e2e    # target/e2e/dassie, with the wallet built in
deno task check        # fmt, lint, type check
deno task test         # WASM vectors, STREAM interop, browser, end to end
```

`deno task ci` runs the Deno steps in order, building what they need first. After changing `proto/`, run `deno task gen` in `packages/rpc`; a test fails until the generated code matches.

To try a node without root, run it standalone on high ports, then configure it from another terminal:

```sh
DASSIE_STATE_DIRECTORY=/tmp/dassie DASSIE_IPC_SOCKET_PATH=/tmp/dassie.sock \
  target/e2e/dassie daemon --http-port 8080 --https-port 8443
DASSIE_IPC_SOCKET_PATH=/tmp/dassie.sock target/e2e/dassie init --hostname localhost --skip-acme
```

and open the setup URL it prints. The certificate is self-signed until `dassie init` obtains one (it needs a public hostname with port 80 reachable).

The code follows a pragmatic TigerStyle and is built for deterministic simulation testing ([D16](spec/08-decisions.md#d16-engineering-style-pragmatic-tigerstyle-and-deterministic-simulation)): the core crates never read a clock or the OS RNG, bound every table and queue, and check their invariants after every step of their simulations.
