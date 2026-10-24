# Mainnet has schemes of its own, and a ledger runs only in its node's realm

**Areas:** [XRPL](../areas/xrpl.md), [Settlement](../areas/settlement.md#schemes-and-ledgers), [Ledger](../areas/ledger.md), [Node](../areas/node.md), [Identity](../areas/identity.md). Built: the account by network, `xrpl` with its memo type on mainnet's account, the realm gate, `live` refused at three points, the dev switch in the node, and the fake ledger's mainnet without RLUSD. Not built yet ([roadmap](../roadmap.md#mainnet-schemes)): `xrpl-rlusd`, test RLUSD disabled, the peg guard, a live node without the test network's bootstrap nodes and relay, the live realm in dev and the end-to-end tests, and the wallet's mainnet pages.

## Facts

- The XRPL account is the testnet's in every constant: its network ID, its asset kind, its seed path, its server and faucet, and the scheme whose books hold its XRP. A scheme's ledger slug is its ID, and a ledger names its realm; `xrpl+xrp` exists in the live realm, and no scheme serves it.
- A peering is on one scheme, and each XRPL scheme's memo type is a contract between peers, versioned with the peer protocol ([overview](../overview.md#frozen-surfaces)).
- Transactions on the XRP Ledger's mainnet and testnet name no network: the same signed bytes apply on either, to an account of the same address. Only the UNL binds a ledger to a network ([verified-ledgers](verified-ledgers.md)).
- The original derived a mainnet wallet at `node/settlement-module/xrpl/seed-mainnet`, with mainnet's own servers. The identity golden vectors pin the entropy at that path, from which that wallet derives, for every entry.
- The realm sets a node's ILP addresses: `test.` in the test realm, `g.` in the live one. The node's realm is set by its owner, and `live` is refused only where it is set.
- Mainnet has no faucet. RLUSD is Ripple's dollar token, on mainnet and the testnet; DSUSD is a testnet dollar of Dassie's own ([dsusd](dsusd.md)).

## Decision

- **One account type, by network.** The account takes its network as a constant: its network ID, asset kind, seed path, default proof server, faucet and realm, and, under [verified-ledgers](verified-ledgers.md), its list publishers. The testnet's values are what they are today, so a testnet node changes nothing.
- **Mainnet schemes in XRP and RLUSD,** `xrpl` on `xrpl+xrp` and `xrpl-rlusd` on a dollar ledger of its own, both on one mainnet account, with the same code as the testnet's schemes and every guarantee those give. **No DSUSD on mainnet:** a dollar of the project's own would make the project the issuer of real money.
- **Memo types of their own,** new for each mainnet scheme and versioned with the peer protocol. A Payment with a memo type of the other network is never a settlement; it is a deposit, as any Payment of our asset with another scheme's memo is. A test holds every XRPL scheme's memo type, ledger and network pairwise distinct.
- **The mainnet wallet is the original's,** at `node/settlement-module/xrpl/seed-mainnet`, so the same passphrase finds the same mainnet address the original derived. The two networks' accounts have different keys, so a Payment signed for one never applies on the other.
- **No faucet on mainnet.** The account waits for the owner's deposit, and the config refuses a faucet for a network without one.
- **A proof server by default.** Without a server named in its config, the account connects to the project's proof server for its network, a rippled patched to serve proofs ([verified-ledgers](verified-ledgers.md)); the owner can name another. Every answer proves itself, so within the history bound a server that lags or lies only delays the account, and one server per network is enough to start; what a server can still do is in [verified-ledgers](verified-ledgers.md#accepted).
- **Test RLUSD is disabled.** The testnet's RLUSD scheme is no longer offered or activated, and DSUSD stands in for RLUSD on the testnet, guarded by its own pool as mainnet RLUSD is by its own ([issued-assets](issued-assets.md)). What an owner holds of test RLUSD stays on the line, the owner's.
- **A ledger runs only in its node's realm.** A scheme activates only when its ledger's realm equals the node's, and a node offers only its realm's schemes. Test and live nodes therefore never share a scheme and never peer, and their addresses differ. Realm-dependent defaults (bootstrap nodes, relay, default hub) are the test network's; a live node takes none until mainnet's exist.
- **`live` stays refused,** now at three points: where the realm is set, at start for a `live` realm stored in the database (which the realm gate would otherwise turn into an open door to mainnet), and in the realm gate itself.
- **Live in tests.** In-process tests and the `node` simulation set the live realm directly, against a mainnet mode of the fake ledger. Dev and the end-to-end tests run binaries in the live realm through a dev switch that only builds with debug assertions honour, and that a release binary refuses to start with, which the install tests check.

## Accepted

- **A dev switch that turns on the live realm,** guarded only by the build profile. Without it, the first time a binary ran a mainnet scheme would be with real money.
- **A live node starts alone** until a mainnet bootstrap node list exists, unless its owner names one.
- **A node whose realm changed** with schemes of the other realm active shows them failed. Changing the realm with schemes active stays out of scope while `live` cannot be set.
- **RLUSD's issuer is a counterparty** on mainnet as on the testnet ([issued-assets](issued-assets.md)), there with real money; the peg guard bounds what a de-peg costs at the boundaries.

## Rejected

- *One scheme ID for both networks, with the network in its config:* peers would tell networks apart from each other's configs, one memo type would mean two things, and one ledger would hold test and real money.
- *Test and live ledgers in one node:* one set of ILP addresses and one set of books for both, and a test peer's settlement that could be taken for a real one.
- *A mainnet wallet at a new path:* owners of the original would not find their address.
- *A dev switch that release builds honour:* a way to real money around every condition of [real-money](real-money.md).
- *No dev switch, live only in-process:* the wallet, the CLI and the binaries would meet a mainnet scheme first with real money.
- *DSUSD on mainnet:* the project would issue a dollar backed by its own keeper, and owe its holders real money.
