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
| `packages/identity-wasm` | The core crates for Deno and browsers, with a STREAM driver |
| `tests/interop` | Interop with `ilp-protocol-stream`, WASM vectors, browser test |
| `testdata/original` | Generator for the golden vectors, run against the original |

## Building and testing

With [Nix](https://nixos.org), `nix develop` provides every tool at the pinned versions, and `nix flake check` runs what CI runs in the sandbox. Without it, install the Rust toolchain named in `rust-toolchain.toml` with the `wasm32-unknown-unknown` target, the `wasm-bindgen` CLI at the version in `Cargo.lock`, Deno, and Chrome or Chromium.

```sh
cargo fmt --check && cargo clippy --all-targets -- -D warnings && cargo test
deno task build:wasm   # WASM modules and bindings into packages/identity-wasm/gen
deno task check        # fmt, lint, type check
deno task test         # WASM vectors, STREAM interop, headless browser
```

The code follows a pragmatic TigerStyle and is built for deterministic simulation testing ([D16](spec/08-decisions.md#d16-engineering-style-pragmatic-tigerstyle-and-deterministic-simulation)): the core crates never read a clock or the OS RNG, bound every table and queue, and check their invariants after every step of their simulations.
