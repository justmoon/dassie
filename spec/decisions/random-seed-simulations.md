# Simulations run continuously, from random seeds

**Areas:** [Testing](../areas/testing.md). Partly built ([roadmap](../roadmap.md#continuous-simulation)).

## Facts

Simulations on a small fixed range of seeds, each running one script, reach only the situations their authors thought of. Longer runs started by hand found real bugs, but nothing kept their failures.

Sweeps of fixed seeds in CI, with minimums on what they reached, became CI's slowest tests: up to a minute and a half of wall time, and many minutes of CPU, on every pull request and every merge, running the same code hundreds of thousands of times.

## Decision

The simulations run continuously, outside CI, from random seeds; each seed draws its own configuration, and a failure becomes an issue that names its seed. The design follows TigerBeetle's VOPR and fuzzing fleet.

- **One runner,** `dassie-sim`, so that a developer and the campaign run the same harnesses, and any failure replays from its seed.
- **Swarm testing:** each seed draws the whole configuration, and some seeds switch a fault off entirely, so rare combinations get their turn.
- **Two phases:** chaos with every invariant checked, then healing, after which the network must converge within a time derived from the production rules. A run that cannot converge whatever the code does is unsupported, neither passed nor failed.
- **Faults stay within what the node relies on:** the store commits durably, so the only store fault is a commit that fails.
- **Coverage counts:** each run counts what it reached, and the runner adds the counts up over a range of seeds, so whoever changes a generator can see whether it still reaches a case.
- **A campaign on spot VMs** runs `main`, and pull requests that ask for it. It runs unreviewed code in the project that holds the bootstrap nodes' phrases, so its VMs run as a service account that can read only its GitHub credentials, and the code they build runs in a sandbox that cannot reach those ([Testing](../areas/testing.md#isolating-pull-requests)).
- **Failures become issues,** one per signature; a failure on a pull request is a comment there instead. A tripwire simulation fails on purpose now and then, so a silent campaign shows. A fix comes with a targeted test, because a seed reproduces a failure only at the commit it ran on.
- **No test runs a simulation,** a run that `dassie-sim` names, in CI or in a local test suite, not even on one fixed seed ([no-simulations-in-tests](no-simulations-in-tests.md)). Sweeps over seeds run in the campaign alone, and nothing asserts minimums on what they reached.
- **A failure the campaign finds becomes a targeted test** of the case it found, set up directly, so that the bug cannot come back unnoticed ([no-simulations-in-tests](no-simulations-in-tests.md)).
- **The campaign is not a check.** It posts no commit status, so nothing it finds shows next to CI's checks or blocks anything; its issues are what someone acts on.
- **Releases do not wait for the campaign yet.** While Dassie changes quickly and no live network runs on its releases, a release held until the campaign has cleared its commit would slow every release for little gain. Releases wait for it once a live network depends on them ([roadmap](../roadmap.md#continuous-simulation)).

## Accepted

- A failure surfaces after the merge that caused it, sometimes hours later.
- Until releases wait for the campaign, a release can carry a failure the campaign has already found; its issue says so, and the owner decides.
- Spot VMs cost money, and each holds credentials that write this repository's issues and comments.
- A change that stops a generator from reaching a case fails no check; the runner's counts show it to whoever looks.
- A bug that a sweep of fixed seeds would have caught before its merge is found after it, by the campaign.

## Rejected

- *A random seed per commit in CI:* a pull request would fail on a bug it did not cause.
- *Sweeps of fixed seeds in CI, with minimums on what they reach:* they made every pull request and merge wait for minutes of simulation that tests the same code over and over, and find what the campaign finds anyway, a while later.
- *Fuzzing in CI,* in any form, including a nightly workflow.
- *A commit status that says whether the campaign has cleared a commit:* it sits among CI's checks and reads as one, though it judges a commit hours after its merge and gates nothing. A release check that needs such a verdict keeps its own record.
- *Keeping failing seeds as a regression corpus:* a seed means nothing after the code changes.
- *Results in a file that someone has to look at:* agents pick up issues; nobody watches a file.

## Not yet decided

Coverage-guided fuzzing of the decoders that face untrusted input, real binaries behind a proxy that injects faults, and shrinking failing runs.
