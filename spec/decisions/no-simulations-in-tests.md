# No test runs a simulation: a case one finds becomes a targeted test

**Areas:** [Testing](../areas/testing.md), [Engineering](../engineering.md#no-fuzzing-or-random-seeds-in-ci)

## Facts

- A simulation run spends nearly all of its time on what its check is not about: forming a network, joining hubs, running payments and chaos, waiting for the network to calm. The degenerate case that a failure turns on is a small part of it.
- Runs of the simulations on fixed seeds dominated CI. The node crate's check takes as long as its slowest test, and that was a fixed-seed run over a hub of about four minutes, with another of three and a half beside it, on every pull request and merge that touched the node.
- Nearly every claim those runs made was already checked by a targeted unit or core test, one that sets up the case directly and finishes in a fraction of the time.
- A fixed seed reaches its case only at the commit it was picked on ([random-seed-simulations](random-seed-simulations.md)). A change elsewhere moves the run, so a fixed-seed test needs checks of what it reached, which fail on changes that did not touch what it is about, or it quietly stops checking what it was written for.
- When such a test fails, its failure is a whole run to replay and explain before anyone knows which code broke.

## Decision

Simulations are a development tool, and the campaign's. A developer runs one with `dassie-sim` while building a feature, to find real failure cases quickly; the campaign runs them from random seeds, outside CI. No test runs one.

- **No CI test and no local test suite runs a simulation,** a run that `dassie-sim` names: not on one fixed seed, not one of its scripted runs, and not a check that a seed passes or replays the same run. The runner's own tests are the one exception, since what they test is the runner: they run the tripwire, which simulates nothing, to check how the runner makes runs and records them, and one starts a campaign of `node` runs and kills it as soon as a run is under way, to see the runs die with it; it checks nothing of the runs.
- **A failure a simulation finds becomes a targeted test** before it goes into CI: one that sets up the degenerate case directly and runs the specific code in it, a unit test, a test of a crate's state machines or of engine cores on an in-memory network, leaving out the rest of the run, which is noise and not part of the repro. The fix comes with it.
- **The simulations stay library code** in their crates' `src/sim/`, which `dassie-sim` runs. A targeted test may build on what they are built from (the in-memory network and its harness, the simulations' checks, the fake XRP Ledger, the seeded RNG), as long as it sets up its case directly and runs no more than the case needs.
- **What a simulation judges is unchanged.** Its checks, its outcomes and how a failure is reproduced stay as they are ([Testing](../areas/testing.md#deterministic-simulation)); only the role of a test that runs it is gone.

## Accepted

- A change that breaks a simulation, stops one from reaching a case, or breaks its determinism fails no check. It shows in the campaign, in a failure that does not replay, or in the runner's counts, after the merge.
- A guarantee that only a simulation checked end to end, across many parts at once, is checked before a merge only in the targeted tests of its parts, until someone runs the simulation.
- Turning a failure into a targeted test takes work the seed alone does not: finding which few steps of the run matter.

## Rejected

- *Fixed-seed scenarios in CI,* one situation set up on one fixed seed, or the few seeds that between them reach what it checks, and a check that a short seed passes and replays: they made every pull request that touched the node wait minutes, mostly on setup and chaos unrelated to their checks; they kept checking their case only while their seeds still reached it; and what they claimed, targeted tests already checked.
- *A time budget per simulation test:* it caps what a run costs, not what it wastes, since a short run is still mostly noise to its check. A budget measured on a shared CI machine makes a test's outcome depend on how fast the machine is ([Testing](../areas/testing.md#levels)), and a run cut short to fit it reaches less.
- *Simulations in the local test suite only, outside CI:* a local `cargo test` would no longer run what CI runs, and a suite that takes minutes is run less often, so it would find less than the campaign does and slow everyone who runs it.
