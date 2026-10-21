# Each node keeps its own books

**Areas:** [Engineering](../engineering.md), [Settlement](../areas/settlement.md), [Ledger](../areas/ledger.md), [Testing](../areas/testing.md)

## Facts

Dassie is not a consensus system. Each node books what it saw, and two peers' books can drift apart: a crash with packets in flight, a Fulfill that comes late, less than the least unit a ledger can move, or a peer that lies. A node cannot tell these apart, and does not need to.

## Decision

Each node keeps its own books and enforces its own limits from them. Agreement between nodes' books is a non-goal.

- **What a node decides** (whether it takes a Prepare or a settlement, when it settles and how much, when it ends a peering) follows from its own books and limits alone. What a peer says is an input, never trusted past the limits.
- **Drift and malice are one case.** However a peer behaves, it costs us at most what our limits allow. That bound is the design's safety property, and the simulation holds every adversary it models to it.
- **A node keeps its own books** and the protocol state it needs to deliver messages. It does not model the peer's books or decisions, and does not measure, store or show drift.
- **The simulation judges each node by its own books** and by what really happened, never by comparing two nodes' books.

## Rejected

- *Reconciling peers' books* (mirrored balances, a reconciliation message, drift metrics): it adds state and messages for nothing the limits do not already bound, and a liar cannot be made to agree.
