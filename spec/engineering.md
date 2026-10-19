# Engineering

How Dassie's code is written, built and checked, in every crate and package. Every agent reads this, together with the [overview](overview.md).

The rewrite is built mostly by agents, and it must still reach a very high level of confidence in its security and reliability. That confidence comes from testing, simulating and exercising the code far more than review could, so the code is built for deterministic simulation from the first crate ([D16](decisions/D16-engineering-style-pragmatic-tigerstyle-and-deterministic-simulation.md)). How it is tested is in [Testing](areas/testing.md).

## Pragmatic TigerStyle

The code follows a pragmatic version of [TigerStyle](https://github.com/tigerbeetle/tigerbeetle/blob/main/docs/TIGER_STYLE.md). Pragmatic means:

- dynamic allocation inside the Rust standard library, or inside a robust dependency, is accepted;
- a robust third-party crate is used rather than rewritten, notably the cryptographic primitives.

Allocation and dependencies are still code smells, avoided where that makes sense.

### Requirements on every crate

- **Deterministic.** The protocol and state-machine crates never read the wall clock, the OS RNG, environment variables or threads.
  - Time is an input (`now`), and timers are deadlines the caller polls. Randomness comes from an injected RNG.
  - Iteration order is always defined: no randomly seeded hash maps.
  - State holds no floating-point values.
  - The same inputs and seed give the same outputs, byte for byte.
- **Bounded.** Every queue, table, buffer and loop has an explicit limit, set by configuration and named with its unit. Exceeding a limit is an error value, never unbounded growth. State machines allocate when they are created, not on the steady-state packet path.
- **Asserted.**
  - Assertions check preconditions, postconditions and invariants, and stay enabled in release builds. A broken invariant crashes rather than corrupting balances.
  - Each state machine has an invariant check that tests and simulations run after every transition.
  - The node checks its state before each commit, so a broken invariant never reaches the disk: what stops the core leaves the state that last held for its successor. Cheap assertions run at every change. The full sweep runs before every commit in tests and simulations, and in release at most once a second of engine time, since with full tables it costs about as much as a busy commit.
  - Untrusted input never reaches an assertion: it gets an error value.
  - Integer overflow checks are on in every profile.
- **Instrumented.** State machines report everything observable as events and counters through their API, so a simulation can record, compare and replay a run. Core crates have no logging dependency; the node maps events to `tracing`.
- **Efficient.** Simulations run many nodes for long simulated periods, so the hot paths stay cheap, and costly parts such as crypto sit behind narrow interfaces a simulation can substitute.
- **Simple shape.**
  - No recursion.
  - Functions of at most 70 lines.
  - No panicking indexing or `unwrap` outside tests.
  - No lossy integer casts.
  - Explicit units in names (`_ms`, `_bytes`, `_ppm`).

### How the rules are enforced

- **Workspace lints** (`Cargo.toml`), in every crate and target: `unsafe_code` forbidden; `missing_docs`, `indexing_slicing`, `unwrap_used`, `expect_used`, `panic`, `unreachable`, the lossy `cast_*` lints, `float_arithmetic` and `too_many_lines` denied. Tests opt out where a panic is the failed test.
- **`clippy.toml`** at the root: functions of at most 70 lines; `HashMap`, `HashSet`, `SystemTime`, `Instant` and the OS RNG disallowed; `std::env::var`, `var_os` and `thread::spawn` disallowed; `debug_assert!` and its variants disallowed, so assertions are `assert!` and stay on in release. A crate's own `clippy.toml` replaces the root's: `dassie-node`, `dassie-cli` and `dassie-sim`, which do IO, repeat what they keep of it.
- **`no_std`**: the protocol and state-machine crates (`dassie-oer`, `-identity`, `-ilp`, `-stream`, `-ledger`, `-settlement`, `-xrpl`, `-peer`, `-connector`, `-payments`, `-node-core`, `-release`) are `no_std`, which keeps clocks, the OS RNG, environment variables, threads and hash maps out of their code. The lints catch them in their tests, which link std.
- **Overflow checks** are on in every Cargo profile (`dev`, `test`, `release`, and those that inherit them).

### Where the rules bend

- **Open Payments follows the same split.** `dassie-payments` is sans-IO: the JSON shapes, HTTP message signatures, GNAP rules, intervals and quote math, with golden vectors and never-panics properties. The node's engine owns the money side (receivers, payers, quotes, grant spend) and is simulated. The HTTP and GNAP services around it are tested over real HTTPS and end to end.
- **Tooling is not simulated.** The dev environment (`apps/dev`) and the dev dashboard are tools around the node, not part of it. Their pure parts (the SNI parser, the vanity table, the reducers) get unit tests, and the rest is tested end to end against real binaries. They are still bounded: rings, frames and queues have limits.
- **The simulation runner** (`dassie-sim`) reads its arguments, draws a seed from the OS and times runs on the real clock; the simulations it runs are still bound by the rules in their own crates.

## Sans-IO protocol crates

`dassie-stream`, `dassie-peer` (message handling, link-state processing, routing calculation), `dassie-connector` (outcome calculation) and `dassie-ledger` are pure state machines. They take inputs plus `now`, and return outputs: packets to send, timers to arm, ledger operations. `dassie-node` wires them to sockets and the database. So timing rules are unit-testable, and the same `dassie-stream` code runs in WASM for the website.

- They are deterministic functions of their inputs, `now` and an injected RNG.
- Every queue, table and buffer has a configured limit, and memory is allocated when a state machine is created, not per packet.
- Invariants are asserted after every transition.
- Everything observable is reported as events and counters.

The node's decisions live in `dassie-node-core` the same way: setup and login checks, session lifetimes, rate limits, the same-origin rule and when to renew a certificate are pure functions of their inputs, `now` and an injected RNG. `dassie-node` only carries them out.

The node's services are the only place that touches real clocks, sockets and entropy. A simulation replaces exactly that layer.

## Each node keeps its own books

Dassie is not a consensus system ([D19](decisions/D19-each-node-keeps-its-own-books.md)). Each node books what it saw, and two peers' books can drift apart: a crash with packets in flight, a Fulfill that comes late, less than the least unit a ledger can move, or a peer that lies. A node cannot tell these apart, and does not need to. Each node keeps its own books and enforces its own limits from them; agreement between nodes' books is a non-goal.

- **What a node decides,** from its own books and limits alone: whether it takes a Prepare, whether it takes a settlement, when it settles and how much, and when it ends a peering. What a peer says (a settlement, a request to settle) is an input to these decisions, never trusted past the limits.
- **Drift and malice are one case.** However a peer behaves, and however far its books are from ours, it costs us at most what our limits allow: the credit we extend, plus what it has in flight. That bound is the design's safety property, and the simulation holds every adversary it models to it.
- **What a node keeps** is its own books, and the protocol state it needs to deliver messages: settlement sequences, whether a termination was acknowledged. It does not model the peer's books or decisions, and does not measure, store or show drift. It does not know the credit the peer extends to it: the peer asks when it should settle.
- **What the simulation checks** is each node against its own books and against what really happened, and each node's decisions against the liquidity its own books show. No check compares two nodes' books, or adds them up ([Testing](areas/testing.md)).

## Simulation time follows production

Once a simulated run heals, payments must complete, settlements arrive, routes converge and no node owe an honest peer past that peer's trigger, within a time derived from the production timings ([D27](decisions/D27-simulations-run-continuously-from-random-seeds.md)). A deadline is derived from the rules the node runs by, not chosen so that a run passes.

## Repository conventions

- **Cargo workspace** in `crates/`: edition 2024, the toolchain pinned in `rust-toolchain.toml` (a channel only; the Nix flake adds clippy, rustfmt and the wasm32 target), `Cargo.lock` committed. All crates are `publish = false`, Apache-2.0.
- **pnpm workspace** in `apps/`, `packages/` and `tests/` (`pnpm-workspace.yaml`): a `package.json` per package, `pnpm-lock.yaml` committed, packages linked with `workspace:*`, pre-release npm packages pinned exactly. Solid is `solid-js@2.0.0-rc.x` with `@solidjs/web` and Vite through `@solidjs/vite-plugin`.
- **Packages meet only through their exports.** For example, `packages/ui` imports nothing of the wallet or the dashboard: only itself, Solid, the RPC packages and `uqr` (a test checks it).
- **Normative contracts live in `proto/`,** and both sides are generated from them: `proto/dassie/ui/v1` (the RPC contract, generated into Rust and `packages/rpc`) and `proto/dassie/dev/v1`. After changing `proto/`, run `node --run gen` in `packages/rpc`; a test fails until the generated code matches.

## Tooling

The TypeScript side runs on Node 26 and pnpm 11, the versions the flake pins ([D21](decisions/D21-tooling-on-node-and-pnpm-not-deno.md)).

- **Packages:** JSR modules such as `@std/assert` come through JSR's npm registry. A release must be a day old before pnpm installs it (`minimumReleaseAge: 1440`).
- **Running:** Node runs the sources as they are, by stripping their types. So they use only syntax that strips (no enums, no parameter properties: `erasableSyntaxOnly`), import with `.ts` extensions, and nothing is emitted. Scripts are `node --run <name>`.
- **Checks:** dprint formats as `deno fmt` did (its TypeScript and JSON plugins in Deno's style), oxlint runs the rules of Deno's recommended set, and TypeScript 7 (`tsc`) checks the types: `node --run check`.
- **Tests:** `node --test`, with `@std/assert` for assertions. Test files are `*_test.ts`.
- **WASM:** one `web` build per module, which browsers and Node both load (`node --run build:wasm`, `scripts/build-wasm.ts`).
- **Rust:** `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings` (and for the WASM crates on `wasm32-unknown-unknown`), `cargo test`, `cargo deny` for licenses and sources.

## Build

Nix flake for CI and release artifacts, plain `cargo` and `node --run` locally ([D14](decisions/D14-build-system.md)).

- `nix develop` gives a shell with every tool at its pinned version; `nix flake check` runs what CI runs. CI builds the flake's checks on the Vorstand Nix cloud, and runs the Node steps (`node --run ci`) in the flake's shell.
- The release binaries are cross-compiled from x86_64-linux with nixpkgs' musl toolchains, so one builder makes both platforms. The wallet is built in the sandbox too, from its npm packages fetched one by one with the hashes `pnpm-lock.yaml` records, so a change to the lockfile needs no hash updated by hand ([Release](areas/release.md)).
- **Cargo profiles:** `test` builds optimized (`opt-level = 3`) with line tables, since crypto, property tests and simulations are slow otherwise; `ci` and `sim` inherit it, so CI and `dassie-sim` build what a local `cargo test` builds; `release` uses fat LTO; `e2e` (the end-to-end binary) inherits `dev`; `wasm` optimizes for size; `install-test` is release without fat LTO.

## Decisions

- [D14](decisions/D14-build-system.md): the Nix flake for CI and releases.
- [D16](decisions/D16-engineering-style-pragmatic-tigerstyle-and-deterministic-simulation.md): pragmatic TigerStyle and deterministic simulation, and why determinism comes first.
- [D19](decisions/D19-each-node-keeps-its-own-books.md): each node keeps its own books; why not reconcile.
- [D21](decisions/D21-tooling-on-node-and-pnpm-not-deno.md): Node and pnpm rather than Deno or Bun.
- [D27](decisions/D27-simulations-run-continuously-from-random-seeds.md): simulations from random seeds.

## Original

- [1 §1.6](original/01-overview.md): the original's design principles.
