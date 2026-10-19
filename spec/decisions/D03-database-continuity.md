# D3. Database continuity

**Status:** active, confirmed 2026-10-11.
**Areas:** [Node](../areas/node.md)

## Decision

The rewrite starts a new schema and does not migrate v0 databases. A v0 owner sets the node up again with their passphrase, which yields the same node ID and the same XRPL account. A database with the v0 application ID is refused with a message that says so.

What does not carry over:

- peerings, which v0 never persisted anyway;
- ledger totals, which v0 inflates on every restart (ACC-5, ACC-6);
- BTP tokens, which the owner creates again.

## Not now

An import command for v0's `config.*` scalars and `btp_tokens`. It is cheap to add if anyone asks.
