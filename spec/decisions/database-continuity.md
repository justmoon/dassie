# Database continuity

**Areas:** [Node](../areas/node.md)

## Facts

- The original was a development prototype that nobody ran, on the testnet or with real money, so there is no v0 database to carry. What one would hold is not worth carrying anyway: it never persisted peerings, its ledger totals were inflated on every restart, and BTP tokens are cheap to create again.
- The early testnet's nodes built their databases through a long chain of migrations: tables made and dropped for features since gone, the journal rebuilt several times, schemes and wallet addresses renamed. Each migration, and each path in the code for data only an old migration left, stays code to read and test for as long as such a database may come.
- The testnet is reset with the [phrase format](phrase-format.md): its phrases give new node keys, so every node is set up again and starts from a new database.

## Decision

- **The schema starts from one migration,** which creates the whole of it, with the rows a new node starts with (`store/schema.rs`). Migrations after it follow the [usual rule](../areas/node.md#database).
- **It has an application ID of its own.** A database of the early testnet, with the ID before it, is refused with a message to set the node up again, and left as it is.
- **No other Dassie database is read.** A v0 database is not migrated: its ID is refused as another application's.
- **The code holds no path for data that only the early testnet's migrations could leave,** such as credentials issued before STREAM connections had keys.

## Accepted

- An early-testnet node is set up again, with no books or peerings carried over. Its node key changes with the phrase format anyway.

## Rejected

- *Keep the chain, and give new databases a baseline:* two ways to one schema, kept equal for as long as an old database may come, and the old one's paths kept with them.
- *The baseline under the same application ID:* a database of the early testnet, at a later version than the baseline's, would read as one from a newer release, which the updater's rollback takes as a reason to put back a copy ([Release](../areas/release.md#rolling-back)). A new ID tells the two apart from the file's header alone.
