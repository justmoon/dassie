# Database continuity

**Areas:** [Node](../areas/node.md)

## Facts

- The original was a development prototype that nobody ran, on the testnet or with real money, so there is no v0 database to carry. What one would hold is not worth carrying anyway: it never persisted peerings, its ledger totals were inflated on every restart, and BTP tokens are cheap to create again.
- The early testnet's nodes built their databases through a long chain of migrations: tables made and dropped for features since gone, the journal rebuilt several times, schemes and wallet addresses renamed. Each migration, and each path in the code for data only an old migration left, stays code to read and test for as long as such a database may come.
- The testnet is reset with the [phrase format](phrase-format.md): its phrases give new node keys, so every node is set up again and starts from a new database.
- The testnet's releases since its reset write their databases under this schema's application ID, and their nodes keep them, some still on the first of those releases, which updates only on its own schedule. The first built its schema through nine migrations, and each later release through more. The states they keep went through several versions of their encodings.

## Decision

- **The schema starts from one migration, the first release's schema.** The baseline creates the whole schema, with the rows a new node starts with, as the first release since the testnet's reset left it, and at that release's version (`BASELINE_VERSION` in `store/schema.rs`). The migrations later releases added stay on top of it as they shipped, so a database any release wrote opens, and migrates on from where it is, with the copy kept before migrating as always. Migrations after them follow the [usual rule](../areas/node.md#database).
- **The baseline is held to what that release wrote.** The release's migrations, as it shipped them, are test data (`store/testdata/`): a test builds a database from them and checks that the baseline builds the same, and that a database of each released version opens and migrates to the current one.
- **A database older than the baseline is refused,** and left as it is: no release wrote one.
- **It has an application ID of its own.** A database of the early testnet, with the ID before it, is refused with a message to set the node up again, and left as it is.
- **No other Dassie database is read.** A v0 database is not migrated: its ID is refused as another application's.
- **The code reads no data in a form that no database it opens can hold,** such as credentials issued before STREAM connections had keys, or a kept state in a version of its encoding that no release since the testnet's reset writes. A state in a version that such a release wrote is still read, whichever release wrote it.

## Accepted

- An early-testnet node is set up again, with no books or peerings carried over. Its node key changes with the phrase format anyway.

## Rejected

- *Keep the chain, and give new databases a baseline:* two ways to one schema, kept equal for as long as an old database may come, and the old one's paths kept with them.
- *The baseline under the same application ID as the early testnet's:* a database of the early testnet, at a later version than the baseline's, would read as one from a newer release, which the updater's rollback takes as a reason to put back a copy ([Release](../areas/release.md#rolling-back)). A new ID tells the two apart from the file's header alone.
- *A new application ID for the folded schema, with another reset of the testnet:* every node on the testnet would refuse its database on update and be set up again, with no books or peerings carried over. Folding into the released schema needs neither.
- *The latest release's schema as the baseline:* a node still on an earlier release would refuse its database on update, and its updater would roll back and skip the release. A node that updates only on its own schedule, with no other way in, would then need setting up again.
