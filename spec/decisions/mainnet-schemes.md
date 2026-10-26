# Mainnet has schemes of its own, and a ledger runs only in its node's realm

**Areas:** [XRPL](../areas/xrpl.md), [Settlement](../areas/settlement.md#schemes-and-ledgers), [Ledger](../areas/ledger.md), [Node](../areas/node.md), [Identity](../areas/identity.md), [HTTP](../areas/http.md#setup). Built: the account by network, `xrpl-xrp` and `xrpl-rlusd` with their memo types on mainnet's account, the realm gate, the realm fixed at setup, `live` refused at three points, the dev switch in the node, test RLUSD disabled, with its in-process and simulated runs moved to mainnet RLUSD in the live realm and its dev and end-to-end runs to DSUSD, the fake ledger's mainnet with RLUSD's issuer and its pool, the peg guard on mainnet RLUSD and testnet DSUSD, the live realm in dev and the end-to-end tests through the dev switch, and the wallet's mainnet pages with the guard. Not built yet ([roadmap](../roadmap.md#mainnet-schemes)): a live node without the test network's bootstrap nodes and relay.

## Facts

- The XRPL account is the testnet's in every constant: its network ID, its asset kind, the path its keys derive at, its server and faucet, and the scheme whose books hold its XRP. A scheme's ledger slug is its ID, and a ledger names its realm; `xrpl-xrp+xrp` exists in the live realm, and no scheme serves it.
- A peering is on one scheme, and each XRPL scheme's memo type is a contract between peers, versioned with the peer protocol ([overview](../overview.md#frozen-surfaces)).
- Transactions on the XRP Ledger's mainnet and testnet name no network: the same signed bytes apply on either, to an account of the same address. Only the UNL binds a ledger to a network ([verified-ledgers](verified-ledgers.md)).
- The original had a mainnet module, at `node/settlement-module/xrpl/seed-mainnet`, but never registered it, and its node refused the `live` realm, so no owner holds funds at that path.
- The realm sets a node's ILP addresses: `test.` in the test realm, `g.` in the live one, and with them every peering, account and book the node keeps. The owner chooses it, and a check where it is chosen is the one place it can be refused before anything runs.
- Mainnet has no faucet. RLUSD is Ripple's dollar token, on mainnet and the testnet; DSUSD is a testnet dollar of Dassie's own ([dsusd](dsusd.md)).

## Decision

- **One account type, by network.** The account takes its network as a constant: its network ID, which the path of its keys names, asset kind, default proof server, faucet and realm, and, under [verified-ledgers](verified-ledgers.md), its list publishers. The testnet's values are what they are today, so a testnet node changes nothing.
- **Mainnet schemes in XRP and RLUSD,** `xrpl-xrp` on `xrpl-xrp+xrp` and `xrpl-rlusd` on a dollar ledger of its own, both on one mainnet account, with the same code as the testnet's schemes and every guarantee those give. **No DSUSD on mainnet:** a dollar of the project's own would make the project the issuer of real money.
- **Memo types of their own,** new for each mainnet scheme and versioned with the peer protocol. A Payment with a memo type of the other network is never a settlement; it is a deposit, as any Payment of our asset with another scheme's memo is. A test holds every XRPL scheme's memo type, ledger and network pairwise distinct.
- **Each network's accounts have keys of their own,** at `node/xrpl/<network ID>/account/<index>`, as the testnet's are ([XRPL](../areas/xrpl.md#the-wallet)). The two networks' accounts have different keys, so a Payment signed for one never applies on the other.
- **No faucet on mainnet.** The account waits for the owner's deposit, and the config refuses a faucet for a network without one.
- **A proof server by default.** Without a server named in its config, the account connects to the project's proof server for its network, a rippled patched to serve proofs ([verified-ledgers](verified-ledgers.md)); the owner can name another. Every answer proves itself, so within the history bound a server that lags or lies only delays the account, and one server per network is enough to start; what a server can still do is in [verified-ledgers](verified-ledgers.md#accepted).
- **Test RLUSD is disabled.** The testnet's RLUSD scheme is no longer offered or activated, and DSUSD stands in for RLUSD on the testnet, guarded by its own pool as mainnet RLUSD is by its own ([issued-assets](issued-assets.md)). What an owner holds of test RLUSD stays on the line, the owner's.
- **A ledger runs only in its node's realm.** A scheme activates only when its ledger's realm equals the node's, and a node offers only its realm's schemes. Test and live nodes therefore never share a scheme and never peer, and their addresses differ. Realm-dependent defaults (bootstrap nodes, relay, default hub) are the test network's; a live node takes none until mainnet's exist.
- **The realm is fixed at setup.** The setup request carries it, the test realm if it names none, and the node stores it with its key, in one write; nothing changes it after. So a node never holds books, peerings or schemes of the other realm.
- **`live` stays refused,** now at three points: at setup, at start for a `live` realm stored in the database (which the realm gate would otherwise turn into an open door to mainnet), and in the realm gate itself.
- **Live in tests.** In-process tests and the `node` simulation set the live realm directly, against a mainnet mode of the fake ledger. Dev and the end-to-end tests run binaries in the live realm through a dev switch that only builds with debug assertions honour, and that a release binary refuses to start with, which the install tests check.

## Accepted

- **A dev switch that turns on the live realm,** guarded only by the build profile. Without it, the first time a binary ran a mainnet scheme would be with real money.
- **A live node starts alone** until a mainnet bootstrap node list exists, unless its owner names one.
- **An owner who wants the other realm sets up a new node,** with the same passphrase if they like: the node ID is the same, and each realm's accounts on the ledgers are its own. A node keeps the rules for a scheme of the other realm stored in its database, which a realm fixed at setup no longer produces: it starts without it and tells the owner, and its peerings end ([Settlement](../areas/settlement.md#schemes-and-ledgers)).
- **RLUSD's issuer is a counterparty** on mainnet as on the testnet ([issued-assets](issued-assets.md)), there with real money; the peg guard bounds what a de-peg costs at the boundaries.

## Rejected

- *One scheme ID for both networks, with the network in its config:* peers would tell networks apart from each other's configs, one memo type would mean two things, and one ledger would hold test and real money.
- *Test and live ledgers in one node:* one set of ILP addresses and one set of books for both, and a test peer's settlement that could be taken for a real one.
- *The original's mainnet path, `node/settlement-module/xrpl/seed-mainnet`, so that owners of the original find their address:* the original never ran on mainnet, so there is no such owner, and the path names a scheme where every scheme on the account shares one wallet.
- *A realm the owner changes later:* every book, peering and scheme of the old realm would be left behind in a node of the new one, to run again if it changed back, and every part of the node would have to handle a realm that moves under it. Fixed at setup, none of that can happen, and the node's ILP address never changes.
- *A dev switch that release builds honour:* a way to real money around every condition of [real-money](real-money.md).
- *No dev switch, live only in-process:* the wallet, the CLI and the binaries would meet a mainnet scheme first with real money.
- *DSUSD on mainnet:* the project would issue a dollar backed by its own keeper, and owe its holders real money.
