# D27. Simulations run continuously, from random seeds

**Status:** active, decided 2026-10-19, after a look at how TigerBeetle runs its VOPR and its fuzzing fleet. Partly built: [Testing](../areas/testing.md) states what exists, the [roadmap](../roadmap.md) the rest.
**Areas:** [Testing](../areas/testing.md), [Release](../areas/release.md)

## Facts

- The node, peer, connector and STREAM simulations ran in `cargo test`, each on a small fixed range of seeds.
- Longer runs, such as those behind [D18](D18-one-grace-limit-per-peer.md) and [D25](D25-a-links-thresholds-are-counted-in-its-largest-packet.md), were started by hand, and nothing kept their failures.
- The node simulation ran one script with a few random parameters: it drove the real engine core, but only paid, crashed and settled in the ways that script chose.

## Decision

The simulations run continuously, outside CI, from random seeds. Each seed draws its own configuration, and a failure becomes an issue that names its seed.

- **One runner,** `dassie-sim`: a seed in, an outcome out (pass, safety failure, liveness failure, unsupported). `cargo test` and the runner run the same harnesses.
- **Swarm testing:** each seed draws the whole configuration (nodes, network faults, crashes, clock skew, ledger faults, lying peers, workload), and some seeds switch a fault off entirely, so rare combinations get their turn.
- **Two phases:** chaos with every invariant checked, then healing, after which the network must converge within a time derived from the production timings. A run that cannot converge whatever the code does is unsupported.
- **Faults stay within what the node relies on:** the store commits with `synchronous = FULL`, so the only store fault is a commit that fails.
- **Coverage counts,** with minimums asserted in CI at a fixed seed, so a generator cannot quietly stop reaching a case.
- **A campaign on spot VMs** in the testnet's GCP project runs `main`, and pull requests labelled `sim`. Since that runs unreviewed code in the project that holds the bootstrap nodes' phrases, the VMs run as a service account with no role, which can read only the GitHub token.
- **Failures become issues,** one per signature, labelled `sim-failure`; a failure on a pull request is a comment there instead. A `tripwire` simulation fails on purpose once in a thousand seeds, so a silent campaign shows. A fix comes with a targeted test, because a seed reproduces a failure only at the commit it ran on.
- **CI is unchanged:** fixed seeds plus the coverage minimums.
- **Releases wait for it:** the release workflow refuses a tagged commit with an open `sim-failure` issue, or one the campaign has not yet run on for long enough.

## Accepted

- A failure surfaces after the merge that caused it, sometimes hours later.
- Spot VMs cost money; their number follows the throughput measured once the runner exists.
- Each VM holds a GitHub token, scoped to this repository's issues.

## Rejected

- *A random seed per commit in CI:* a pull request would fail on a bug it did not cause.
- *Fuzzing in CI* in any form, including a nightly workflow.
- *Keeping failing seeds as a regression corpus:* a seed means nothing after the code changes.
- *Results in a file that someone has to look at:* agents pick up issues; nobody watches a file.

## Not yet decided

Coverage-guided fuzzing (cargo-fuzz) of the decoders that face untrusted input, real binaries behind a proxy that injects faults, and shrinking failing runs.
