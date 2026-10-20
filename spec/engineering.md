# Engineering

The rules Dassie's code follows, in every crate and package, and why. Every agent reads this, together with the [overview](overview.md).

Dassie is built mostly by agents, and it must still reach a very high level of confidence in its security and reliability. That confidence comes from testing, simulating and exercising the code far more than review could, so the code is built for deterministic simulation from the first crate ([D16](decisions/D16-engineering-style-pragmatic-tigerstyle-and-deterministic-simulation.md)). How it is tested is in [Testing](areas/testing.md).

## Pragmatic TigerStyle

The code follows a pragmatic version of [TigerStyle](https://github.com/tigerbeetle/tigerbeetle/blob/main/docs/TIGER_STYLE.md): dynamic allocation inside the standard library or a robust dependency is accepted, and robust third-party crates, notably the cryptographic primitives, are used rather than rewritten. Allocation and dependencies are still code smells, avoided where that makes sense.

### Requirements on every crate

- **Deterministic.** The protocol and state-machine crates never read the wall clock, the OS RNG, environment variables or threads. Time is an input (`now`), and timers are deadlines the caller polls; randomness comes from an injected RNG; iteration order is always defined; state holds no floating-point values. The same inputs and seed give the same outputs, byte for byte, which is what lets a simulation replay a failure from its seed.
- **Bounded.** Every queue, table, buffer and loop has an explicit limit, named with its unit. Exceeding a limit is an error value, never unbounded growth: what a peer can make a node hold is part of what it can cost the node. State machines allocate when they are created, not on the steady-state packet path.
- **Asserted.**
  - Assertions check preconditions, postconditions and invariants, and stay enabled in release builds: a broken invariant crashes rather than corrupting balances.
  - Each state machine has an invariant check that tests and simulations run after every transition.
  - The node checks its state before each commit, so a broken invariant never reaches the disk: a crash leaves the state that last held for the restart. The full sweep runs before every commit in tests and simulations; in release it runs at most once a second of engine time, since with full tables it costs about as much as a busy commit.
  - Untrusted input never reaches an assertion: it gets an error value. Expected failures are values everywhere, in TypeScript too, never panics or exceptions.
  - Integer overflow checks are on in every profile.
- **Instrumented.** State machines report everything observable as events and counters through their API, so a simulation can record, compare and replay a run. Core crates have no logging dependency.
- **Simulated at production timings.** A simulation's deadlines derive from the production rules, never tuned so a run passes ([Testing](areas/testing.md#simulation-time-follows-production)).
- **Efficient.** Simulations run many nodes for long simulated periods, so the hot paths stay cheap, and costly parts such as crypto sit behind narrow interfaces a simulation can substitute.
- **Simple shape.** No recursion, short functions, no panicking indexing or `unwrap` outside tests, no lossy integer casts, explicit units in names (`_ms`, `_bytes`, `_ppm`).

The workspace lints, `clippy.toml` and `no_std` on the protocol and state-machine crates enforce these; the IO crates (`dassie-node`, `dassie-cli`, `dassie-sim`) have their own `clippy.toml` that keeps what applies to them.

### Where the rules bend

- **Tooling is not simulated.** The dev environment and the dev dashboard are tools around the node. Their pure parts get unit tests, and the rest is tested end to end against real binaries. They are still bounded.
- **The simulation runner** (`dassie-sim`) reads its arguments, draws a seed from the OS and times runs on the real clock; the simulations it runs are still bound by the rules of their own crates.

## Sans-IO protocol crates

The protocol and state-machine crates are pure state machines: they take inputs plus `now`, and return outputs (packets to send, timers to arm, ledger operations). The node's decisions live in `dassie-node-core` the same way: setup and login checks, session lifetimes, rate limits, the same-origin rule and when to renew a certificate. `dassie-node` wires them to sockets, clocks, entropy and the database, and only carries their decisions out.

So timing rules are unit-testable, the same `dassie-stream` code runs in WASM for the website, and a simulation replaces exactly the node's IO layer. Open Payments follows the same split: `dassie-payments` holds the protocol without IO, the node's engine owns the money side and is simulated, and the HTTP and GNAP services around it are tested over real HTTPS.

## Each node keeps its own books

Dassie is not a consensus system ([D19](decisions/D19-each-node-keeps-its-own-books.md)). Each node books what it saw, and two peers' books can drift apart: a crash with packets in flight, a Fulfill that comes late, less than the least unit a ledger can move, or a peer that lies. A node cannot tell these apart, and does not need to. Each node keeps its own books and enforces its own limits from them; agreement between nodes' books is a non-goal.

- **What a node decides,** from its own books and limits alone: whether it takes a Prepare, whether it takes a settlement, when it settles and how much, and when it ends a peering. What a peer says (a settlement, a request to settle) is an input to these decisions, never trusted past the limits.
- **Drift and malice are one case.** However a peer behaves, and however far its books are from ours, it costs us at most what our limits allow: the credit we extend, plus what it has in flight. That bound is the design's safety property, and the simulation holds every adversary it models to it.
- **What a node keeps** is its own books, and the protocol state it needs to deliver messages: settlement sequences, whether a termination was acknowledged. It does not model the peer's books or decisions, and does not measure, store or show drift. It does not know the credit the peer extends to it: the peer asks when it should settle.
- **What the simulation checks** is each node against its own books and against what really happened, and each node's decisions against the liquidity its own books show. No check compares two nodes' books, or adds them up ([Testing](areas/testing.md#each-node-judged-by-its-own-books)).

## No fuzzing or random seeds in CI

CI runs tests and simulations on fixed seeds only, property tests included ([Testing](areas/testing.md#property-tests)). A random seed per commit would fail pull requests on bugs they did not touch. Random-seed campaigns and any fuzzing run outside CI, and never in it, nightly included ([D27](decisions/D27-simulations-run-continuously-from-random-seeds.md)).

## TypeScript and packages

- **Type-strippable TypeScript only.** Node runs the sources as they are, by stripping their types, and nothing is emitted. So the code uses only syntax that strips (no enums, no parameter properties: `erasableSyntaxOnly`) and imports with `.ts` extensions.
- **Packages meet only through their exports.** For example, `packages/ui` imports nothing of the wallet or the dashboard, which a test checks, so each can change its internals without breaking another.
- **Normative contracts live in `proto/`,** and both sides are generated from them; a test fails until the generated code matches.

## Decisions

- [D14](decisions/D14-build-system.md): the Nix flake for CI and releases.
- [D16](decisions/D16-engineering-style-pragmatic-tigerstyle-and-deterministic-simulation.md): pragmatic TigerStyle and deterministic simulation, and why determinism comes first.
- [D19](decisions/D19-each-node-keeps-its-own-books.md): each node keeps its own books; why not reconcile.
- [D21](decisions/D21-tooling-on-node-and-pnpm-not-deno.md): Node and pnpm rather than Deno or Bun.
- [D27](decisions/D27-simulations-run-continuously-from-random-seeds.md): simulations from random seeds, and why CI keeps fixed seeds.
