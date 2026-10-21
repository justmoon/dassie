# Engineering style: pragmatic TigerStyle and deterministic simulation

**Areas:** [Engineering](../engineering.md), [Testing](../areas/testing.md)

## Facts

Dassie is built almost entirely by agents, and it must still reach a very high level of confidence in its security and reliability. That confidence comes from testing, simulating and exercising the code far more than review could. Deterministic simulation only works if the code is built for it from the start: retrofitting determinism means rewriting.

## Decision

Follow a pragmatic version of [TigerStyle](https://github.com/tigerbeetle/tigerbeetle/blob/main/docs/TIGER_STYLE.md): code is deterministic, bounded, asserted, instrumented and simple in shape ([Engineering](../engineering.md) states the rules). Pragmatic means that allocation inside the standard library or a robust dependency is accepted, and that robust crates, notably the cryptographic primitives, are used rather than rewritten.

## Rejected

- *Conventional style, with determinism added when simulation starts:* clocks, RNGs, unordered maps and unbounded queues spread into every crate, and removing them afterwards costs more than avoiding them.
