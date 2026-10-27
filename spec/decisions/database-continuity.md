# Database continuity

**Areas:** [Node](../areas/node.md)

## Decision

The database schema is new, and v0 databases are not migrated. A v0 owner keeps their keys through the original's release, at tag `original`, and sets a new node up with a new phrase: a phrase of the new format is a different node ([phrase-format](phrase-format.md)). A database with the v0 application ID is refused with a message that says so.

## Why

What a v0 database holds is not worth carrying: it never persisted peerings, its ledger totals were inflated on every restart, and BTP tokens are cheap to create again. The original's release still derives its owners' keys from their passphrase.

## Not now

An import of v0's configuration and BTP tokens. It is cheap to add if anyone asks.
