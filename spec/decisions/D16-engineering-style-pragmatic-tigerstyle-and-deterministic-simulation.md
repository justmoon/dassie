# D16. Engineering style: pragmatic TigerStyle and deterministic simulation

**Status:** active, confirmed 2026-10-12.
**Areas:** [Engineering](../engineering.md), [Testing](../areas/testing.md), every crate

## Facts

Dassie is built almost entirely by agents, and it must still reach a very high level of confidence in its security and reliability. That confidence comes from testing, simulating and exercising the code far more than review could. Deterministic simulation testing only works if the code is built for it from the first crate: retrofitting determinism means rewriting.

## Decision

Follow a pragmatic version of [TigerStyle](https://github.com/tigerbeetle/tigerbeetle/blob/main/docs/TIGER_STYLE.md): every crate is deterministic, bounded, asserted, instrumented, efficient and simple in shape. "Pragmatic" means that dynamic allocation inside the standard library or a robust dependency is accepted, and that a robust crate is used rather than rewritten, notably the cryptographic primitives. [Engineering](../engineering.md) states the requirements.

## Rejected

- *Conventional style, with determinism added when simulation starts:* clocks, RNGs, unordered maps and unbounded queues spread into every crate, and removing them afterwards costs more than avoiding them.
