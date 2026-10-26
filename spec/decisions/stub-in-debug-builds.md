# The stub scheme runs only in builds with debug assertions

**Areas:** [Settlement](../areas/settlement.md#the-stub-scheme), [Node](../areas/node.md), [Wallet](../areas/wallet.md), [Testing](../areas/testing.md).

## Facts

- The stub believes a peer's settlement as stated: anyone peered over it mints what it says it settled ([Settlement](../areas/settlement.md#the-stub-scheme)).
- Its ledger, `stub+usd`, counts US dollars, and the connector converts dollars at par between the dollar ledgers, so a node's stub dollars buy its DSUSD, or any token a peer holds at a dollar ([issued-assets](issued-assets.md)).
- The realm gate keeps the stub off live nodes, but every test-realm node offered it: a stub peer of a testnet node could take the node's DSUSD, or a market maker's inventory, for dollars it minted.
- Dev, the end-to-end tests, the tests and the simulations run on the stub, and are built with debug assertions: dev and the end-to-end binary in the `dev` and `e2e` profiles, tests and simulations in `test` and its heirs. A release is built without them, and the live realm's dev switch already relies on that ([mainnet-schemes](mainnet-schemes.md)).

## Decision

- **Only a build with debug assertions runs the stub** (`STUB_RUNS`). A release build neither offers it nor activates it: adding it is refused, saying why, and one stored in the database, from a build that ran it, is left as a disabled scheme is, its books kept, and its peerings ended as for any scheme that does not start.
- **Nothing else changes for the builds that run it,** so dev, the end-to-end tests and the simulations need no new feature flag.

## Accepted

- **The stub's code ships in release binaries,** unreachable: a runtime gate keeps both branches compiled and tested in every build, where a cargo feature would leave the release branch untested.
- **A testnet node set up on a release can no longer try Dassie on the stub;** it starts on the XRP Ledger testnet, whose faucet funds it.

## Rejected

- *A cargo feature (`testing`) for the stub, built into the end-to-end and dev binaries:* every build that runs the stub already has debug assertions, and a feature would be one more way for a binary to differ from what ships.
- *An asset code of its own for the stub's ledger,* so that it converts into no real dollar token: it would still mint by assertion on testnet nodes, and every test that converts between the stub and a token would need a rate for an asset no one prices.
