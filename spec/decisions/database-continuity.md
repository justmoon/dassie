# Database continuity

**Areas:** [Node](../areas/node.md)

## Decision

The database schema is new, and v0 databases are not migrated. A v0 owner sets the node up again with their passphrase, which yields the same node ID and the same on-ledger accounts. A database with the v0 application ID is refused with a message that says so.

## Why

What a v0 database holds is not worth carrying: it never persisted peerings, its ledger totals were inflated on every restart, and BTP tokens are cheap to create again. Identity and funds come back from the passphrase anyway.

## Not now

An import of v0's configuration and BTP tokens. It is cheap to add if anyone asks.
