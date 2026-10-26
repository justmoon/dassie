# Overview

What Dassie is, the words the rest of the spec uses, how a node fits into the world, and which surfaces must never change. Every agent reads this, together with [engineering](engineering.md).

## What Dassie is

Dassie is a peer-to-peer payment network built on [Interledger](https://interledger.org). Each participant runs a **Dassie node** on their own server. A node:

- holds money on one or more underlying ledgers, through **settlement schemes**;
- finds other nodes through bootstrap nodes, peers with some of them on its own, and keeps its own books of what each peer owes it and what it owes each peer;
- forwards Interledger packets between peers, its own local endpoints and ILP clients, converting between currencies;
- asks a peer to settle when the peer comes close to the credit it extends, and settles on the shared ledger when asked;
- serves its owner a web wallet, and serves [Open Payments](https://openpayments.dev) to other wallets and apps.

It is not a blockchain: nodes keep bilateral balances and settle them on whichever ledgers they share. It is not a consensus system either. Each node keeps its own books and enforces its own limits from them, so whatever a peer does, what it can cost the node is bounded; agreement between nodes' books is not a goal ([engineering](engineering.md#each-node-keeps-its-own-books)).

Dassie works with test money only: the `live` realm is refused until the conditions in the [roadmap](roadmap.md#real-money) are met ([real-money](decisions/real-money.md)).

## Concepts

| Term | Meaning |
|---|---|
| **Node** | One running Dassie daemon, identified by an Ed25519 key ([Identity](areas/identity.md)). |
| **Node ID** | A hash of the node's public key; the node's network identity. |
| **Passphrase** | A BIP-39 mnemonic. Everything secret derives from it: the node key, the login secret, and the keys of the node's accounts on each ledger. The browser derives the node key, or `dassie setup` on the node's host; only the node key reaches the node. The same passphrase recovers the same node ID and the same on-ledger accounts. |
| **Owner** | The person who set the node up with their passphrase and logs in to its wallet. |
| **Realm** | `test` or `live`. It picks the ILP allocation scheme (`test` or `g`) and which settlement schemes may run. |
| **ILP address** | Every node is `<scheme>.das.<nodeId>`. The node's own endpoints and clients live beneath it. |
| **Bootstrap node list (BNL)** | The configured bootstrap nodes, which serve node lists and take registrations ([Network](areas/network.md)). |
| **Known node list (KNL)** | The list a bootstrap node serves: its unexpired registrations and itself. |
| **Majority** | Nodes listed by strictly more than half of the *configured* bootstrap nodes. They are the network's members. |
| **Registration** | A node asking a bootstrap node to list it, renewed automatically. It is free for now ([roadmap](roadmap.md#registration-fees)). |
| **Link state** | A node's signed self-description (URL, key, neighbors, settlement schemes), flooded through the network. Routing runs on link states. |
| **Peer** | A node we have a peering with: a shared settlement scheme, ledger accounts, a credit line and a settlement channel. Packets travel only between peers. |
| **Junior, senior** | The junior is the node whose peering request was accepted, the senior the node that accepted it. The junior extends credit at first, and the senior none ([earned-trust](decisions/earned-trust.md)). |
| **Credit** | What a node lets a peer owe it. A node knows only the credit it extends, never the credit it is extended. Credit grows with the fees a peer earned the node ([Settlement](areas/settlement.md)). |
| **Largest packet** | The most one packet may carry on a ledger. A link's thresholds (in flight, the trigger to ask for settlement, the least settlement) are counted in it, never as a share of credit ([thresholds-in-packets](decisions/thresholds-in-packets.md)). |
| **Settlement scheme** | A pluggable module that moves value on an underlying ledger ([Settlement](areas/settlement.md), [XRP Ledger](areas/xrpl.md)). |
| **External asset** | Something the node holds outside its books, such as its account on the XRP Ledger testnet, which schemes settle through. The ledger, not the books, says what it holds ([Settlement](areas/settlement.md#external-assets)). |
| **Settlement channel** | What a node keeps per node it ever peered with to deliver settlements exactly once: sequences sent and applied, and the message not yet acknowledged. Once idle, it closes to disk. |
| **Internal ledger** | The node's double-entry ledger, with accounts named `<ledgerId>:<path>`, and an append-only transfer journal ([Ledger](areas/ledger.md)). |
| **Owner's funds** | Per ledger, the pool `equity/owner` and its group: the owner's sub-accounts and what the node gains or loses on that ledger. Everything the node holds beyond what it owes peers. |
| **Endpoint** | Anything the connector exchanges packets with: a peer, a BTP client, or a local endpoint such as the STREAM receiver or payer ([Connector](areas/connector.md)). |

## System context

```
  Owner's browser ── wallet, RPC over WebSocket ──┐
  `dassie` CLI ───── RPC over the IPC socket ─────┤
  Other Dassie nodes ── peer protocol (QUIC) ─────┤        ┌──▶ underlying ledgers (XRP Ledger)
  ILP clients ───── BTP ──────────────────────────┼─▶ node ┼──▶ exchange-rate sources
  Open Payments clients and wallets ──────────────┤        └──▶ other nodes' Open Payments servers
  ACME CA ───────── http-01 ──────────────────────┘
```

- **One Rust binary**, `dassie`, is the daemon and the CLI, with the wallet's built assets embedded. Under systemd it is socket-activated; standalone, it binds its own ports ([Node](areas/node.md)).
- Everything outside faces one HTTPS listener ([HTTP](areas/http.md)) and, for other nodes, one QUIC endpoint on UDP at the same port ([Peer protocol](areas/peer-protocol.md#quic)); the CLI talks to the daemon over a local socket that only members of a group may use.
- The node keeps its state in one SQLite database.

## Life of a node

1. **Install.** On a Linux server with systemd, `curl … | sh` fetches the newest release, verifies its signature before unpacking anything, installs it and starts the systemd units ([Release](areas/release.md)). On a Linux desktop, the desktop app runs the node in its own process as a periphery node, with no hostname, and opens the setup page in its window instead of steps 2 and 3's browser ([Desktop](areas/desktop.md)).
2. **Configure.** `dassie init` asks for the hostname and an email address. The daemon obtains a certificate through ACME, and `dassie init` prints a one-time **setup URL** ([HTTP](areas/http.md)).
3. **Set up the wallet.** In the browser, the owner creates or enters a passphrase. The browser derives the node key with the same code the node uses (WASM), and posts it with a proof of the login secret. The node's identity-dependent services start: the ledger, the connector, Open Payments, the peer protocol and routing. An owner who keeps the phrase on the node's host, for a node nobody logs in to, runs `dassie setup` there instead, which also does step 4 ([Node](areas/node.md#the-dassie-command)).
4. **Choose a ledger.** The wallet asks for a first settlement scheme.
5. **Join the network.** The node polls the bootstrap nodes for their lists, registers with each, keeps the majority nodes and fetches their link states. It peers on its own with nodes that share a settlement scheme ([Network](areas/network.md)).
6. **Transact.**
   - *Receive:* other wallets pay the node's wallet addresses through Open Payments; the STREAM receiver takes the money into the owner's funds.
   - *Send:* the owner pays a payment pointer, wallet address or payment request ([Payments](areas/payments.md)).
   - *Forward:* packets between peers follow the shortest routes. Each hop books pending transfers, posted on Fulfill and voided on Reject or expiry ([Connector](areas/connector.md)).
   - *Clients:* BTP clients and third-party Open Payments clients spend within the limits the owner set.
7. **Settle.** A node asks each peer whose position has fallen to its trigger to settle; the debtor decides how much, and settles on the shared ledger. Settlements are delivered exactly once ([Settlement](areas/settlement.md)).
8. **Operate.** A daily timer runs `dassie update`, which verifies the next release, switches to it and rolls back if the node does not come up ([Release](areas/release.md)).

## How the code is split

The node is Rust, the browser is Solid 2, and the tooling is Node ([rust-and-typescript](decisions/rust-and-typescript.md), [node-and-pnpm](decisions/node-and-pnpm.md)).

- **Rust:** the daemon, the CLI and every protocol, in one static binary per platform, with no bundled interpreter or native add-on. The desktop app is the one exception: a window needs the system's GTK and WebKitGTK, so it links them and glibc ([desktop-app](decisions/desktop-app.md)).
- **WASM:** the cryptographic and protocol cores are also built to WASM, so the wallet derives keys with exactly the code the node uses, and the website's STREAM toy runs the real STREAM implementation.
- **Solid 2:** the wallet, which the node serves, and the dev dashboard.
- **Node:** everything that orchestrates or builds: the dev environment, the Vite builds, the website, the release scripts, and the interop and end-to-end tests. No release contains Node.

[spec/README.md](README.md) maps each path to the area doc that covers it.

## Frozen surfaces

Most of Dassie may change freely: the UI ↔ node RPC and the HTTP API ship with both their ends in one binary, and the database migrates forward. These surfaces must never change, because something outside the release depends on them:

- **The derivation chain:** passphrase → seed → node key → its public key → node ID (24 digest bytes, written as their lowercase base32), and every frozen seed path hanging off the node key (`NodeSecret::is_frozen`: the login secret, the XRP Ledger accounts' keys, a hub's keys, the STREAM receivers' secrets, the Open Payments client key). The passphrase is the owner's only backup: a changed step loses the owner their node ID, or a key to funds on a ledger or in a hub. Pinned by golden vectors that a generator of our own computes with libraries independent of Dassie's code (`testdata/identity/`, into `crates/dassie-identity/tests/data/`), run natively and through the WASM build, so the wallet derives what the node derives ([Identity](areas/identity.md#derivation-chain)).
- **The XRP Ledger wallet derivation.** The same passphrase must find the same account and its funds. Pinned by the XRPL wallets in `crates/dassie-identity/tests/data/chain.json`, which xrpl.js derives and `crates/dassie-xrpl/tests/vectors.rs` derives again ([XRP Ledger](areas/xrpl.md#the-wallet)).
- **Ed25519 verification by ZIP-215.** Every node must accept and refuse the same signatures, or edge-case signatures split the network ([Identity](areas/identity.md)).
- **The Interledger wire formats:** ILPv4 packets and errors, ILDCP and BTP 2.0 byte for byte, STREAM per RFC 29, Open Payments per its specs. Other Interledger software must interoperate with a node. Pinned by golden vectors and by interop tests against `ilp-protocol-stream`, `ilp-plugin-btp` and `@interledger/open-payments` ([Testing](areas/testing.md#interop)).
- **The peer protocol, and what a settlement on a ledger means** (each XRPL scheme's memo type, such as RLUSD's and DSUSD's, and its semantics), within one network version. Both sides of a peering must agree; a change is a new protocol version that older nodes refuse cleanly ([Peer protocol](areas/peer-protocol.md), [XRP Ledger](areas/xrpl.md)).
- **A settlement hub's protocol, within its version:** its encodings and digests, what its ledger does with a block, and its consensus rules. A hub's validators run different releases, and must execute every block alike; members check what they sign. The hub ID commits to the version, and a change is a new version, under which hubs are founded again ([hub-protocol-version](decisions/hub-protocol-version.md)). So is the deposit that opens an account, which wallets outside Dassie write: a Payment to the escrow with tag 0 and one memo of the hub's type naming the account's key; a change to it is a new memo type. Pinned by `testdata/hub/vectors.json` ([Settlement hubs](areas/hubs.md#golden-vectors)).
- **The rate feed's wire format and signature, within its version.** Market makers and other subscribers run software of their own, outside the node's release, and must read every node's feed. Pinned by `testdata/feed/` ([Rate feed](areas/rate-feed.md#wire-format)). A change is a new version, which older subscribers refuse cleanly.
- **The release mirror's layout and the install layout.** Installed nodes' updaters fetch `meta/latest`, a version's sums and signature, and their platform's `.tar.gz` bundle by name, and switch between the directories of `/opt/dassie`; the `curl | sh` installer does the same. Pinned by `dassie-release`'s layout tests and the install tests ([Release](areas/release.md#the-mirror)).
- **The public domains that leave the binary:** `dassiepay.net` for nodes' payment names, which other wallets store, and `dassie.land` for the memo types on the ledgers and the mirror the updaters fetch from. The bootstrap nodes', the default relay's and the proof servers' names are the release's, and may move in any release ([public-domains](decisions/public-domains.md)).

Not frozen, on purpose: a node does not talk to the original implementation's nodes or read its databases; an owner of one recovers by entering their passphrase in a new node ([peer-protocol-v1](decisions/peer-protocol-v1.md), [database-continuity](decisions/database-continuity.md)).

## Principles

- **Zero configuration.** A hostname and a passphrase are all a node needs; discovery, peering, addressing and settlement are automatic.
- **Ledger-neutral, currency-neutral.** Settlement is a plug-in interface, and membership does not depend on any ledger.
- **Secrets stay with the owner.** The passphrase never leaves the browser, and every key derives from it. The exception is an owner who keeps the phrase on the node's own host, as a headless node or a market maker must: `dassie setup` sets the node up from it there, and still only the node key reaches the node ([headless-setup](decisions/headless-setup.md)).
- **HTTPS everywhere,** in development too.
- **Self-contained releases** that install and update themselves.

## Decisions

- [rust-and-typescript](decisions/rust-and-typescript.md): the node is Rust, the browser Solid, the tooling TypeScript.
- [peer-protocol-v1](decisions/peer-protocol-v1.md), [database-continuity](decisions/database-continuity.md): no v0 peer protocol, no v0 databases.
- [real-money](decisions/real-money.md): no real money yet.
- [bootstrap-network](decisions/bootstrap-network.md): the testnet's bootstrap nodes.
- [public-domains](decisions/public-domains.md): the domains of payment names, memo types and the mirror.
- [own-books](decisions/own-books.md): each node keeps its own books.
- [headless-setup](decisions/headless-setup.md): `dassie setup`, the exception to a passphrase that never leaves the browser.
- [node-and-pnpm](decisions/node-and-pnpm.md): tooling on Node and pnpm.
