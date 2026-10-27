# Database continuity

**Areas:** [Node](../areas/node.md)

## Decision

The database schema is new, and v0 databases are not migrated. A database with the v0 application ID is refused with a message that says so.

## Why

The original was a development prototype that nobody ran, on the testnet or with real money, so there is no v0 database to carry. What one would hold is not worth carrying anyway: it never persisted peerings, its ledger totals were inflated on every restart, and BTP tokens are cheap to create again.

