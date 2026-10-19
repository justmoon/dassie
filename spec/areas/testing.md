# Testing

How Dassie is tested: the levels of tests, the deterministic simulations and their runner, the end-to-end, install and website tests, where each lives and how to run it.

**Paths:** `crates/*/tests/`, `crates/*/src/sim/`, `crates/dassie-sim/`, `crates/dassie-node-core/src/testing.rs`, `tests/e2e/`, `tests/interop/`, `testdata/`, `**/*_test.ts`
**Read with:** [Engineering](../engineering.md), [Release](release.md), [Website](website.md), [Dev environment](dev.md)

## Levels

Every level has tests, where the original had none for the app, the GUI or the tooling (UI-17). Deterministic simulation is the main source of confidence ([D16](../decisions/D16-engineering-style-pragmatic-tigerstyle-and-deterministic-simulation.md)), and the other levels feed it:

- golden vectors pin the wire formats a simulation cannot judge;
- interop tests pin the standards;
- end-to-end tests check the real IO layer that a simulation replaces.

Every crate is written so that a simulation can drive it ([Engineering](../engineering.md#pragmatic-tigerstyle)).

Rules for every test:

- **A fix gets a test that would have failed against the original's behaviour.** `dassie-identity`, `dassie-ilp` and `dassie-stream` keep these in `tests/fixes.rs`, named after the gap they fix; elsewhere such a test sits with the others of its module and names the gap (for example `NET-8`) in its name or comment.
- **Every decoder has a "never panics on arbitrary input" property test.**
- **Every simulation or property-test failure prints the seed that reproduces it.**
- **CI runs fixed seeds only.** A random seed per commit would fail pull requests on bugs they did not touch. No fuzzing runs in CI, in any form, nightly included ([D27](../decisions/D27-simulations-run-continuously-from-random-seeds.md)).

## Where tests live and how to run them

| What | Where | Run with |
|---|---|---|
| Unit tests | each crate's `src/`, in `#[cfg(test)]` modules | `cargo test` |
| Golden vectors, fixes, properties, scenario tests | `crates/<crate>/tests/` (`vectors.rs`, `fixes.rs`, `properties.rs`, …), vectors in `tests/data/`, shared helpers in `tests/support/` | `cargo test -p <crate>` |
| Simulations on fixed seeds | `crates/<crate>/src/sim/` behind the crate's `sim` feature, run from `tests/simulation.rs`; the node's in `crates/dassie-node/tests/network.rs` | `cargo test` |
| Simulations on any seed | `crates/dassie-sim` | `cargo run --profile sim -p dassie-sim -- run peer 0..1000` |
| The updater's tests | `crates/dassie-cli/tests/update.rs` | `cargo test -p dassie-cli --features test-key` |
| Apps', packages' and scripts' tests | `apps/**/*_test.ts`, `packages/**/*_test.ts`, `scripts/*_test.ts` | `node --run test` |
| WASM vectors, STREAM interop, browser, end to end | `tests/interop/`, `tests/e2e/` | `node --run test:e2e` (after `node --run build:wasm`, `build:wallet`, `build:website`, `build:e2e`) |
| Install tests | `tests/e2e/install_test.ts` | `nix build .#install-test-mirror -o install-test-mirror`, then `DASSIE_INSTALL_MIRROR=$PWD/install-test-mirror node --test tests/e2e/install_test.ts` |
| Everything on the TypeScript side | | `node --run ci` (builds what it needs first) |
| What CI runs | | `nix flake check`, and `node --run ci` in `nix develop` |

- Property tests use `proptest`. Its regression files (`*.proptest-regressions`) are committed beside the tests.
- The `test` profile is optimized, so `cargo test` runs crypto, properties and simulations at speed; `ci` and `sim` inherit it.
- The end-to-end tests need `pebble`, `pebble-challtestsrv`, `systemd-socket-activate`, `mkcert` and Chrome or Chromium. Without them they are skipped outside CI; with `CI=true`, as on GitHub, they fail instead.
- The XRPL end-to-end tests run against the node's fake ledger. `DASSIE_XRPL_TESTNET=1` runs them against the public testnet, by hand.
- `DASSIE_E2E_SCREENSHOTS` names a directory for a picture of each step of the wallet's and the dev environment's browser tests. `DASSIE_E2E_WALLET_DIR` serves another wallet build to the wallet's browser test.

### What CI runs

On every pull request and every push to `main`:

- **`nix`**: the flake's checks, built on the Vorstand Nix cloud: `cargo fmt`, `cargo clippy --all-targets -- -D warnings`, clippy for the WASM crates on `wasm32-unknown-unknown`, `cargo test` with `dassie-cli/test-key` and `systemd-socket-activate` available, `cargo deny` (licenses, sources), the WASM bindings, and the installer's ShellCheck.
- **`ts`**: `pnpm install --frozen-lockfile` and `node --run ci` in the flake's shell: the WASM, wallet, dashboard, website and end-to-end builds, `node --run check`, `node --run test` and `node --run test:e2e`.

The install tests and the static-binary check run only in the release workflow, for a tag `v<version>`, before anything is signed ([Release](release.md)).

## Golden vectors

The original's fixtures and computed vectors are test data:

- ILP and ILDCP packets, F08 data, Interledger timestamps and BTP messages;
- every STREAM frame and the STREAM crypto vectors;
- OER: lib-oer's sample tables (variable unsigned integers, object identifiers, UTF-8 strings, length determinants) and the peer-protocol structures of the original's 3 §5.5;
- the node IDs of the original's bootstrap nodes, `test1.dassie.xyz` and `test2.dassie.xyz`, from their public keys (`keys.json`). They pin the node-ID algorithm; they are not the default bootstrap nodes, which are `boot1`–`boot3.dassie.xyz` ([Network](network.md#bootstrap-nodes));
- Ed25519 keys from the seeds `01`×32 and `03`×32, with signatures, their X25519 conversions and the session shared secret of the original's 3 §5.5;
- the BIP-39 → node key → login secret chain, over a spread of the dev vanity table, with dev session tokens and settlement entropy;
- the XRPL wallet derivation for a fixed passphrase.

`testdata/original/` generates them from the original's code (see its README), into `crates/<crate>/tests/data/` (`dassie-identity`'s `chain.json` and `keys.json`, and `vectors.json` of `dassie-oer`, `dassie-ilp` and `dassie-stream`). Each file records its source commit. Regenerating must not change a committed vector. Where the rewrite deliberately differs from the original (a fixed gap), the Rust test carries its own hand-written vector, named after the gap.

Other vectors:

- **XRPL Payments**, signed byte for byte as xrpl.js signs them: `testdata/xrpl/generate-vectors.mjs`, run against xrpl.js.
- **Open Payments**, from the standards' own examples: RFC 9421 (Ed25519 signatures), RFC 9530 (`Content-Digest`), RFC 8941 (structured fields), the interaction hash of the Open Payments docs, and the repeating intervals of its specs (`crates/dassie-payments/tests/vectors.rs`).
- **Releases:** `testdata/release` holds the release test key and what minisign signs with it, legacy signature included.
- **WASM:** `tests/interop/identity_wasm_test.ts` runs the golden vectors through the WASM builds; the bytes must be the ones the native tests check, so the wallet derives exactly what the node derives.

## Property tests

- `dassie-oer`: round trip, and only canonical encodings parse.
- The ledger: no transfer breaks a limit, and balances equal the journal.
- STREAM: amount allocation among streams (original's 4 §11.11).
- Every decoder: never panics on arbitrary input.

## Interop

A Node test harness (`node --test`) runs Dassie against the reference implementations:

- `dassie-stream` against the npm `ilp-protocol-stream`, both directions: natively through the `interop_peer` example (`crates/dassie-stream/examples`, length-prefixed frames over stdio; `tests/interop/native_interop_test.ts`), and in WASM (`stream_interop_test.ts`).
- The BTP server against `ilp-plugin-btp`, with `ilp-protocol-stream` paying through it (`tests/e2e/btp_payment_test.ts`, `scenarios_test.ts`).
- Open Payments against the reference client `@interledger/open-payments`, as a third-party client of the node, which validates every response against the specs (`open_payments_test.ts`, `third_party_test.ts`); and the node as a client of another node (`third_party_test.ts`, `wallet_test.ts`).
- `tests/interop/browser_test.ts`: the `web` WASM build loads in headless Chrome, derives a node ID and runs a STREAM payment inside the page. `CHROME_BIN` chooses the browser.

## Deterministic simulation

N nodes, or a crate's state machines, are wired in-process through the sans-IO crates, with a virtual clock and a seeded RNG (`dassie_node_core::testing::SeededRng`, xoshiro256** seeded by SplitMix64: reproducible, not cryptographic). They cover discovery, majority computation, flooding suppression, peering, routing convergence, settlement thresholds and restart recovery.

- Between them, the simulations inject latency, loss, reordering, duplication, partitions, clock skew, crashes and restarts ([table below](#the-simulations) for which does what). The node simulation does not yet inject duplication or clock skew ([roadmap](../roadmap.md#continuous-simulation)).
- Every state machine's invariants are checked after each step.
- A run is reproducible from its seed: two runs with the same seed produce identical event traces, and a test checks it for each simulation.

### Each node judged by its own books

Beyond the invariants, a simulation judges each node on its own, by its own books, and never compares two nodes' books or adds them up: each node keeps its own, and they may drift ([Engineering](../engineering.md#each-node-keeps-its-own-books), [D19](../decisions/D19-each-node-keeps-its-own-books.md)).

- **Against what really happened:** a node's books hold what its scheme holds; its owner's funds moved by what its receivers took and its payers sent, and each token's by what its client sent. The underlying ledgers neither make nor lose money, each asset apart in runs across currencies.
- **Its decisions, against the liquidity its books show:** it refuses a Prepare for liquidity only by the limits its books set (the ledger's property tests check those); it takes every settlement an honest peer sends; once the network is calm, it owes no peer past its own threshold for long; and when a peering ends, its books say it owes nothing.
- **Against adversaries:** whatever a lying peer does (never settle, spend the credit it was given and vanish), an honest node loses no more than the credit it extends ([D23](../decisions/D23-each-node-books-its-own-fulfills.md)); one that vanishes loses its peering.
- **Liveness:** payments complete, and settlements arrive.

The peer protocol's own simulation checks what the protocol must make agree, which is not money: link states and routes converge, peerings are two-sided, and each settlement is applied exactly once.

### Outcomes

A run ends in one of four outcomes (`dassie_node_core::testing::Outcome`):

| Outcome | Meaning |
|---|---|
| `pass` | Every check held. |
| `safety` | Something that must never happen did: an assertion or invariant broke, or a check of what the run did failed. A panic is a safety failure, with the panic's message; its trace is lost. |
| `liveness` | Something that must happen did not, once the network was calm. |
| `unsupported` | The run drew a situation no code could recover from (for example, a majority of bootstrap nodes cut off). It neither passes nor fails. |

### The simulations

| Name | Where | What a seed draws | What it checks | Fixed seeds in `cargo test` |
|---|---|---|---|---|
| `peer` | `dassie_peer::sim::chaos` | 2–12 nodes, 1–3 of them bootstrap nodes, each node's clock skew (±10 s), and the chaos | 90 s of chaos on a rough network (latency 1–400 ms, 3% loss, 2% duplication): crashes while at least two thirds stay up, restarts, partitions between pairs, owners ending peerings, bans and unbans, settlements, peers with packets in flight. Then faults stop, every node restarts, bans are lifted, and 240 s of calm. Every settlement sent is applied exactly once (otherwise safety); the network converges (every node knows every other's latest link state and has its peers, the peer graph is connected, routes follow from it), every termination finishes, nothing is left to settle, and both sides hold and advertise every peering (otherwise liveness). | `0..12` |
| `connector` | `dassie_connector::sim::traffic` | 1,500 steps of five clients sending random packets, answering late, wrongly, with garbage or never, and disconnecting | The invariants after every step; every request gets exactly one answer; the journal matches the packet trace. | `0..24` |
| `stream` | `dassie_stream::sim::payment` | which packets are lost, on a fixed path (10% loss, largest packet 7,000, rate 3/2, 10 ms latency) | A client pays 300,000 and sends data to a server, then closes; both sides check their invariants at every step; the payment arrives and the connection closes. | `100..120` |
| node | `crates/dassie-node/tests/network.rs` | the network, its payments and crashes | Real engine cores, each with its own in-memory database, through a virtual network with latency and loss, crashed and restarted from their databases, paying over STREAM through BTP clients and with outgoing payments. Peerings and balances survive restarts; payments of several times the credit cross hops, so every hop settles; an owner's termination ends a peering settled; one token's spending cap holds against its journal; outgoing payments send exactly their debit amount, across a crash; a node that crashes mid-payment (sender, hop or receiver) keeps its books. Variants settle on the stub, on the XRP Ledger testnet scheme against an in-process fake ledger whose connection lags, loses requests and drops (books match each account less its reserve, no XRP made or lost), and across currencies at fixed rates with a fee and a spread (no ledger makes or loses an asset). Adversaries: a peer that never settles, and one that spends its credit and vanishes, cost no more than that credit. Juniors pay their seniors in advance, and credit grows with fees. | `0..6`; `DASSIE_SIM_SEEDS=from..to` runs others |

The `peer`, `connector` and `stream` simulations are library code behind their crate's `sim` feature, so `cargo test` and the runner run the same code. The node simulation is still a test only; the runner cannot run it ([roadmap](../roadmap.md#continuous-simulation)).

### The runner: `dassie-sim`

`dassie-sim` runs any simulation it knows on any seed, to replay a failure, search a range for one, or run a commit's own seed. It is a developer tool, never released.

```
dassie-sim list                                  the simulations, with what each simulates
dassie-sim run <sim> [<seeds>]                   runs seeds; one random seed by default
dassie-sim explain <sim> <seed> [--grep <text>]  runs one seed and prints its trace, or its lines holding <text>
```

- **Seeds** are a decimal number, as the runner prints them (`42`); a range, `0..1000`, which runs 0 to 999; or a git commit's full 40-digit SHA, whose first 16 hex digits are its seed, so that a commit names its own run. Only a full SHA is read as hex.
- **Output.** For each seed, `run` prints the simulation, the seed and what the seed drew, then the outcome and how long the run took; for more than one seed, a tally of passed, unsupported, safety and liveness.
- **Exit status:** 1 when a run broke safety; 2 when one broke liveness and none safety; 3 when the runner itself failed; 64 for a usage error; 0 otherwise. A reader that stops reading (such as `head`) is not a failure.
- **Build** it with `--profile sim`, which is the test profile: optimized, with assertions and overflow checks on, so every invariant is checked before every commit, as in tests, and a seed replays exactly as the test of the same seed.
- A test that fails on a seed names it; `dassie-sim run <sim> <seed>` replays it and `dassie-sim explain <sim> <seed>` prints its trace.

The continuous campaign from random seeds, swarm configurations drawn per seed, coverage counts and their minimums in CI, failure issues and the release check are not built yet ([roadmap](../roadmap.md#continuous-simulation)).

## End to end

The dev scenarios run headless in CI on real binaries (`target/e2e/dassie` with the wallet built in, from `node --run build:e2e`): start N nodes, set them up through the HTTP API, add ledgers, peer, pay, settle, restart, and check that the balances are unchanged. The scenario code is the dev environment's (`apps/dev`).

| Test | What it checks |
|---|---|
| `fresh_install_test.ts` | A fresh install, socket-activated as under systemd, reaches "setup complete" in a browser with a certificate from Pebble, Let's Encrypt's test server, through `dassie init`. |
| `btp_payment_test.ts` | `ilp-plugin-btp` and `ilp-protocol-stream` pay an incoming payment at the node's STREAM receiver through a BTP token; the journal matches the packet trace. |
| `scenarios_test.ts` | `two-nodes` and `six-nodes` converge and route payments of several times the credit across hops, so every hop settles; restarts keep peerings and balances. |
| `open_payments_test.ts` | The reference client gets a grant, creates an incoming payment, `ilp-protocol-stream` pays it, and state and books are exact. |
| `third_party_test.ts` | The reference client's full third-party flow from one node to another, with consent given in the wallet in Chrome; the owner then revokes the grant and its token stops working. |
| `wallet_test.ts` | Every wallet page against a `two-nodes` scenario in a headless browser, with live data arriving without a reload; two nodes pay each other from the wallet's Send. |
| `xrpl_test.ts` | `xrpl-settlement` on the fake ledger: the faucet funds both nodes, payments settle on the ledger, and after both restart their books match their accounts less the reserve, nothing booked twice. |
| `cross_currency_test.ts` | `cross-currency`: dollars on one node, XRP on another, a third converting with a fee and a spread; every node settles on each peer's ledger, and after restarts the books are as they were. |
| `dev_test.ts` | `node --run dev`, headless on other ports with a CA of its own, brings up a scenario, and its dashboard shows it through the SNI proxy. |

## Installer and updater

`tests/e2e/install_test.ts` runs the installer and the updater in containers with systemd as PID 1, as the original's `test-in-docker:*` scripts did, including signature-verification failures. [Release](release.md#tests) states what it checks, on which images, against which releases, and the updater's Rust tests (`crates/dassie-cli/tests/update.rs`, with the `test-key` feature), the store's tests of the pre-migration copy, and the installer's ShellCheck.

- **Images:** Debian 12, Ubuntu 24.04, Fedora 42 and CentOS Stream 9, pinned by digest, privileged (`tests/e2e/install/<distro>.Dockerfile`). `DASSIE_INSTALL_DISTROS` picks some; by default all four.
- **Between them, the images must take every path the installer has:** both verifiers (OpenSSL on Debian, Ubuntu and CentOS; minisign on Fedora), both downloaders (curl, and wget on Ubuntu, which has no curl), and both compressions (xz, and gzip on CentOS, which has no xz). Each Dockerfile says which path it takes; a change to an image must keep every path covered.
- **The installer runs under `umask 077`,** as on a hardened host, so what it installs must still be readable by the node's own user.
- **The mirror** is served by Caddy, pinned by digest, as `https://mirror.test` on a Docker network of the test's own.
- **Releases:** from `nix build .#install-test-mirror`, which `DASSIE_INSTALL_MIRROR` names. Without it the test is skipped, so `node --run test:e2e` passes over it.
- **In each container,** in order:
  1. `install.sh` refuses a tampered bundle, sums that are not what was signed, sums signed for another version, and a canary that `meta/latest` names (fresh or run again), and installs nothing;
  2. it installs 1.0.0: the node runs under systemd, socket-activated, and answers on port 443;
  3. the updater, started as its timer starts it, refuses 1.0.1 tampered or with the sums of 1.0.0, and the node runs on as it was;
  4. it updates to 1.0.1, restarts the node and keeps its data;
  5. it goes back from 1.0.2, whose node does not start, to 1.0.1, and to the database from before 1.0.2 migrated it;
  6. `install.sh` run again goes neither back nor to the other channel on the mirror's word; it moves to `canary` when asked and stays there when run again, and back to `latest` when asked;
  7. it uninstalls.
- **Needs** Docker with privileged containers, and mkcert. The release workflow runs one job per distribution before it signs; CI does not run it.

## Website

`tests/e2e/website_test.ts` serves the built site (`node --run build:website`) with the headers and trailing slashes of its `vercel.json`, so that its CSP applies, and drives it in headless Chrome: the architecture map draws and answers clicks, and the STREAM toy runs a payment to the end on the WASM STREAM implementation, its packets decrypting. The toy's simulation is also tested in Node (`apps/website/src/toy/simulation_test.ts`) against the same `web` build of the WASM module that the browser loads. `apps/website/src/architecture/map_test.ts` checks the architecture page's `map.json` ([Website](website.md)).

## Decisions

- [D16](../decisions/D16-engineering-style-pragmatic-tigerstyle-and-deterministic-simulation.md): why simulation is the main tool, and why the code is built for it from the start.
- [D19](../decisions/D19-each-node-keeps-its-own-books.md): why a simulation judges each node by its own books.
- [D23](../decisions/D23-each-node-books-its-own-fulfills.md): the bound an adversary is held to.
- [D27](../decisions/D27-simulations-run-continuously-from-random-seeds.md): one runner, simulations from random seeds, and why CI keeps fixed seeds.

## Original

- [6 §5.5](../original/06-user-interfaces-and-tooling.md): the original's `test-in-docker:*` install tests.
- [4 §1.9, §6.1, §11.2, §11.3](../original/04-interledger.md) and [3 §2.2, §5.5](../original/03-peer-network.md): the fixtures and computed vectors the golden vectors come from.
