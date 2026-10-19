# D27. Simulations run continuously, from random seeds

**Areas:** [Testing](../areas/testing.md). Partly built ([roadmap](../roadmap.md#continuous-simulation)).

## Facts

Simulations on a small fixed range of seeds, each running one script, reach only the situations their authors thought of. Longer runs started by hand found real bugs, but nothing kept their failures.

## Decision

The simulations run continuously, outside CI, from random seeds; each seed draws its own configuration, and a failure becomes an issue that names its seed. The design follows TigerBeetle's VOPR and fuzzing fleet.

- **One runner,** `dassie-sim`, so that `cargo test` and the campaign run the same harnesses, and any failure replays from its seed.
- **Swarm testing:** each seed draws the whole configuration, and some seeds switch a fault off entirely, so rare combinations get their turn.
- **Two phases:** chaos with every invariant checked, then healing, after which the network must converge within a time derived from the production rules. A run that cannot converge whatever the code does is unsupported, neither passed nor failed.
- **Faults stay within what the node relies on:** the store commits durably, so the only store fault is a commit that fails.
- **Coverage counts,** with minimums asserted in CI, so a generator cannot quietly stop reaching a case.
- **A campaign on spot VMs** runs `main`, and pull requests that ask for it. It runs unreviewed code in the project that holds the bootstrap nodes' phrases, so its VMs run as a service account that can read only the GitHub token.
- **Failures become issues,** one per signature; a failure on a pull request is a comment there instead. A tripwire simulation fails on purpose now and then, so a silent campaign shows. A fix comes with a targeted test, because a seed reproduces a failure only at the commit it ran on.
- **CI runs fixed seeds only,** plus the coverage minimums.
- **Releases wait for the campaign** to have run on the tagged commit without an open failure.

## Accepted

- A failure surfaces after the merge that caused it, sometimes hours later.
- Spot VMs cost money, and each holds a token scoped to this repository's issues.

## Rejected

- *A random seed per commit in CI:* a pull request would fail on a bug it did not cause.
- *Fuzzing in CI,* in any form, including a nightly workflow.
- *Keeping failing seeds as a regression corpus:* a seed means nothing after the code changes.
- *Results in a file that someone has to look at:* agents pick up issues; nobody watches a file.

## Not yet decided

Coverage-guided fuzzing of the decoders that face untrusted input, real binaries behind a proxy that injects faults, and shrinking failing runs.
