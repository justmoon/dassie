# Testing

How Dassie is tested: what each level of tests is for, what the deterministic simulations judge and why, and how to reproduce a failure.

**Paths:** `crates/*/tests/`, `crates/*/src/sim/`, `crates/dassie-sim/`, `crates/dassie-node-core/src/testing.rs`, `tests/e2e/`, `tests/interop/`, `testdata/`, `**/*_test.ts`
**Read with:** [Engineering](../engineering.md), [Release](release.md), [Website](website.md), [Dev environment](dev.md)

## Levels

Deterministic simulation is the main source of confidence ([D16](../decisions/D16-engineering-style-pragmatic-tigerstyle-and-deterministic-simulation.md)), and the other levels cover what a simulation cannot judge:

- golden vectors pin the wire formats and derivations that must never change;
- interop tests pin the standards against their reference implementations;
- end-to-end tests check the real IO layer that a simulation replaces.

Rules for every test:

- **Every decoder has a "never panics on arbitrary input" property test.**
- **Every simulation or property-test failure prints the seed that reproduces it.**
- **CI runs fixed seeds only, and no fuzzing** ([Engineering](../engineering.md#no-fuzzing-or-random-seeds-in-ci)).
- **The end-to-end tests skip what they lack outside CI, and fail on it in CI** (`CI=true`), so a missing tool never passes CI silently.

The install tests and the static-binary check run only in the release workflow, before anything is signed ([Release](release.md)).

## Golden vectors

The frozen surfaces of the [overview](../overview.md#frozen-surfaces) are pinned by vectors generated from the original implementation's code (`testdata/original/`, see its README), into `crates/<crate>/tests/data/`. Each file records its source commit, and regenerating must not change a committed vector. Where Dassie deliberately differs from the original (a bug fixed), the Rust test carries its own hand-written vector.

Other vectors come from their own reference: XRPL Payments signed as xrpl.js signs them (`testdata/xrpl/`), Open Payments from the standards' own examples, releases from minisign with a test key (`testdata/release`). The WASM builds run the same vectors, and must produce the bytes the native tests check, so the wallet derives exactly what the node derives.

## Property tests

Properties pin what no example can: every decoder never panics, only canonical encodings parse, no transfer breaks a ledger limit, balances equal the journal. Their regression files (`*.proptest-regressions`) are committed beside the tests.

## Interop

Dassie runs against the reference implementations, which are the oracle for the standards: STREAM against `ilp-protocol-stream` in both directions, natively and in WASM; the BTP server against `ilp-plugin-btp`; Open Payments against `@interledger/open-payments`, which validates every response against the specs, with the node both as server and as a client of another node. The WASM build is also run in a headless browser, as the wallet and the website load it.

## Deterministic simulation

N nodes, or a crate's state machines, are wired in-process through the sans-IO crates, with a virtual clock and a seeded RNG (`dassie_node_core::testing::SeededRng`, reproducible, not cryptographic). Every simulation is library code in its crate's `src/sim/`, so `cargo test` (on fixed seeds) and the runner (on any seed) run the same code.

- Between them, the simulations inject latency, loss, reordering, duplication, partitions, clock skew, crashes and restarts, and lying peers. What the `node` simulation does not inject yet is in the [roadmap](../roadmap.md#continuous-simulation).
- Every state machine's invariants are checked after each step.
- A run is reproducible from its seed: two runs with the same seed produce identical event traces, and a test checks it for each simulation.

### Simulation time follows production

The simulations run the production timings, not the dev ones: on the virtual clock a simulated day costs seconds, and the ledgers they settle on, like the fake XRP Ledger, run at their real speed. Dev timings, scaled down, are only for what runs on a real clock: the dev environment and the end-to-end tests.

Once a run heals, payments must complete, settlements arrive, routes converge and no node owe an honest peer past that peer's trigger, within a time derived from the production rules: for the `node` simulation, as long as a network takes to be ready plus the longest a pending settlement's backoff may wait. A deadline is derived from the rules the node runs by, never chosen so that a run passes ([D27](../decisions/D27-simulations-run-continuously-from-random-seeds.md)).

### Each node judged by its own books

Beyond the invariants, a simulation judges each node on its own, by its own books, and never compares two nodes' books or adds them up: each node keeps its own, and they may drift ([Engineering](../engineering.md#each-node-keeps-its-own-books), [D19](../decisions/D19-each-node-keeps-its-own-books.md)).

- **Against what really happened:** a node's books hold what its scheme holds; its owner's funds moved by what its receivers took and its payers sent, and each token's by what its client sent. The underlying ledgers neither make nor lose money, each asset apart in runs across currencies.
- **Its decisions, against the liquidity its books show:** it refuses a Prepare for liquidity only by the limits its books set; it takes every settlement an honest peer sends; once the network is calm, it owes no peer past its own threshold for long; once it knows a peering is ending, it starts no settlement to that peer; and when it ends one, what the peer last said it had on its way is booked before the peer is told.
- **Against adversaries:** whatever a lying peer does (never settle, spend the credit it was given and vanish), an honest node loses no more than the credit it extends ([D23](../decisions/D23-each-node-books-its-own-fulfills.md)); one that vanishes loses its peering.
- **Against replays:** a copy of a request, sent again by someone who saw it arrive, is refused if its original was answered 200 or is still under way. Only a copy of a request that had no effect, refused or lost in a crash, may be taken once ([Peer protocol](peer-protocol.md#replay-protection)).
- **Liveness:** payments complete, and settlements arrive.

The peer protocol's own simulation checks what the protocol must make agree, which is not money: link states and routes converge, peerings are two-sided, and each settlement is applied exactly once.

### The simulations

Each simulation lives in its crate's `src/sim/`, and `dassie-sim list` names them all. The `peer` simulation runs the peer protocol alone; `connector` drives the connector with misbehaving clients; `stream` runs a payment over a lossy path; `node` runs real engine cores with their own databases, settling on the stub or the fake XRP Ledger, through a seeded stretch of chaos and a heal; the scripted node runs set up one situation each (crashes mid-payment, adversaries, runs across currencies). Seeds that draw their configuration print it before the run starts.

The `node` simulation counts what it reached (crashes mid-payment, cuts, settlements, withdrawals, …), and a test asserts that its fixed seeds between them reach every kind of fault and payment, so that a generator cannot quietly stop reaching a case.

### Outcomes

A run ends as `pass`, `safety` (something that must never happen did: an assertion, an invariant or a check of what the run did), `liveness` (something that must happen did not, once the network was calm), or `unsupported` (the run drew a situation no code could recover from, such as a majority of bootstrap nodes cut off; it neither passes nor fails) (`dassie_node_core::testing::Outcome`). Each of the node simulation's checks says which of the two failures it found. A run that broke both says safety: one that did not settle down is first checked for what must not happen even with money in flight, and one that did runs every check and reports a safety failure before any liveness failure.

### The runner: `dassie-sim`

`dassie-sim` runs any simulation on any seed, to replay a failure, search a range for one, or run a commit's own seed. It is a developer tool, never released.

**Reproducing a failure.** A test or run that fails names its simulation and seed.

- `dassie-sim run <sim> <seed>` replays it, and `dassie-sim explain <sim> <seed>` prints its trace, optionally only the lines that match.
- Build the runner with `--profile sim`, which is the test profile with assertions and overflow checks on, so a seed replays exactly as the test of the same seed.
- A seed reproduces a failure only at the commit it ran on, so a fix comes with a targeted test that sets up the situation directly.
- A panic is a safety failure that loses its trace; replay the seed to see what led to it.
- A git commit's full SHA is also a seed, so that a commit names its own run.

## End to end

The dev environment's scenarios run headless in CI on real binaries with the wallet built in: start several nodes, set them up through the HTTP API, add ledgers, peer, pay, settle, restart, and check that the balances are unchanged. They also cover what only real IO shows: a fresh install, socket-activated, getting its certificate from an ACME test server; reference clients paying through BTP and Open Payments; the wallet in a headless browser; settling on the fake XRP Ledger, which a test can switch to the public testnet by hand.

## Installer and updater

The install tests run the installer and the updater in containers with systemd as PID 1, against a mirror of signed test releases; [Release](release.md#tests) states what they check. Two rules for changing them:

- **Between them, the images must take every path the installer has:** both verifiers, both downloaders and both compressions. Each Dockerfile (`tests/e2e/install/`) says which path it takes; a change to an image must keep every path covered.
- **The installer runs under `umask 077`,** as on a hardened host, so what it installs must still be readable by the node's own user.

## Decisions

- [D16](../decisions/D16-engineering-style-pragmatic-tigerstyle-and-deterministic-simulation.md): why simulation is the main tool, and why the code is built for it from the start.
- [D19](../decisions/D19-each-node-keeps-its-own-books.md): why a simulation judges each node by its own books.
- [D23](../decisions/D23-each-node-books-its-own-fulfills.md): the bound an adversary is held to.
- [D27](../decisions/D27-simulations-run-continuously-from-random-seeds.md): one runner, simulations from random seeds, and why CI keeps fixed seeds.
