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

Dassie works with test money only: the `live` realm is refused until the conditions in the [roadmap](roadmap.md#real-money) are met ([D8](decisions/D08-real-money.md)).

## Concepts

| Term | Meaning |
|---|---|
| **Node** | One running Dassie daemon, identified by an Ed25519 key ([Identity](areas/identity.md)). |
| **Node ID** | A hash of the node's public key; the node's network identity. |
| **Passphrase** | A BIP-39 mnemonic. Everything secret derives from it: the node key, the login secret, and each settlement scheme's ledger keys. The browser derives the node key; only the node key reaches the node. The same passphrase recovers the same node ID and the same on-ledger accounts. |
| **Owner** | The person who set the node up with their passphrase and logs in to its wallet. |
| **Realm** | `test` or `live`. It picks the ILP allocation scheme (`test` or `g`) and which settlement schemes may run. |
| **ILP address** | Every node is `<scheme>.das.<nodeId>`. The node's own endpoints and clients live beneath it. |
| **Bootstrap node list (BNL)** | The configured bootstrap nodes, which serve node lists and take registrations ([Network](areas/network.md)). |
| **Known node list (KNL)** | The list a bootstrap node serves: its unexpired registrations and itself. |
| **Majority** | Nodes listed by strictly more than half of the *configured* bootstrap nodes. They are the network's members. |
| **Registration** | A node asking a bootstrap node to list it, renewed automatically. It is free for now ([roadmap](roadmap.md#registration-fees)). |
| **Link state** | A node's signed self-description (URL, key, neighbors, settlement schemes), flooded through the network. Routing runs on link states. |
| **Peer** | A node we have a peering with: a shared settlement scheme, ledger accounts, a credit line and a settlement channel. Packets travel only between peers. |
| **Junior, senior** | The junior is the node whose peering request was accepted, the senior the node that accepted it. The junior extends credit at first, and the senior none ([D20](decisions/D20-trust-follows-what-a-peer-earned-us.md)). |
| **Credit** | What a node lets a peer owe it. A node knows only the credit it extends, never the credit it is extended. Credit grows with the fees a peer earned the node ([Settlement](areas/settlement.md)). |
| **Largest packet** | The most one packet may carry on a ledger. A link's thresholds (in flight, the trigger to ask for settlement, the least settlement) are counted in it, never as a share of credit ([D25](decisions/D25-a-links-thresholds-are-counted-in-its-largest-packet.md)). |
| **Settlement scheme** | A pluggable module that moves value on an underlying ledger ([Settlement](areas/settlement.md), [XRP Ledger](areas/xrpl.md)). |
| **Settlement channel** | What a node keeps per node it ever peered with to deliver settlements exactly once: sequences sent and applied, and the message not yet acknowledged. |
| **Internal ledger** | The node's double-entry ledger, with accounts named `<ledgerId>:<path>`, and an append-only transfer journal ([Ledger](areas/ledger.md)). |
| **Owner's funds** | Per ledger, the pool `equity/owner` and its group: the owner's sub-accounts and what the node gains or loses on that ledger. Everything the node holds beyond what it owes peers. |
| **Endpoint** | Anything the connector exchanges packets with: a peer, a BTP client, or a local endpoint such as the STREAM receiver or payer ([Connector](areas/connector.md)). |

## System context

```
  Owner's browser ── wallet, RPC over WebSocket ──┐
  `dassie` CLI ───── RPC over the IPC socket ─────┤
  Other Dassie nodes ── peer protocol (HTTPS) ────┤        ┌──▶ underlying ledgers (XRP Ledger)
  ILP clients ───── BTP ──────────────────────────┼─▶ node ┼──▶ exchange-rate sources
  Open Payments clients and wallets ──────────────┤        └──▶ other nodes' Open Payments servers
  ACME CA ───────── http-01 ──────────────────────┘
```

- **One Rust binary**, `dassie`, is the daemon and the CLI, with the wallet's built assets embedded. Under systemd it is socket-activated; standalone, it binds its own ports ([Node](areas/node.md)).
- Everything outside faces one HTTPS listener ([HTTP](areas/http.md)); the CLI talks to the daemon over a local socket that only members of a group may use.
- The node keeps its state in one SQLite database.

## Life of a node

1. **Install.** On a Linux server with systemd, `curl … | sh` fetches the newest release, verifies its signature before unpacking anything, installs it and starts the systemd units ([Release](areas/release.md)).
2. **Configure.** `dassie init` asks for the hostname and an email address. The daemon obtains a certificate through ACME, and `dassie init` prints a one-time **setup URL** ([HTTP](areas/http.md)).
3. **Set up the wallet.** In the browser, the owner creates or enters a passphrase. The browser derives the node key with the same code the node uses (WASM), and posts it with a proof of the login secret. The node's identity-dependent services start: the ledger, the connector, Open Payments, the peer protocol and routing.
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

The node is Rust, the browser is Solid 2, and the tooling is Node ([D1](decisions/D01-what-runs-in-rust-and-what-in-typescript.md), [D21](decisions/D21-tooling-on-node-and-pnpm-not-deno.md)).

- **Rust:** the daemon, the CLI and every protocol, in one static binary per platform, with no bundled interpreter or native add-on.
- **WASM:** the cryptographic and protocol cores are also built to WASM, so the wallet derives keys with exactly the code the node uses, and the website's STREAM toy runs the real STREAM implementation.
- **Solid 2:** the wallet, which the node serves, and the dev dashboard.
- **Node:** everything that orchestrates or builds: the dev environment, the Vite builds, the website, the release scripts, and the interop and end-to-end tests. No release contains Node.

[spec/README.md](README.md) maps each path to the area doc that covers it.

## Frozen surfaces

Most of Dassie may change freely: the UI ↔ node RPC and the HTTP API ship with both their ends in one binary, and the database migrates forward. These surfaces must never change, because something outside the release depends on them:

- **The derivation chain:** passphrase → seed → node key → its public key → node ID, and every seed path hanging off the node key (the login secret, the settlement schemes' ledger keys, the STREAM receivers' secrets, the Open Payments client key). The passphrase is the owner's only backup: a changed step loses the owner their node ID, or a key to funds on a ledger. Pinned by golden vectors generated from the original implementation's code (`crates/dassie-identity/tests/data/`), run natively and through the WASM build, so the wallet derives what the node derives ([Identity](areas/identity.md#derivation-chain)). The STREAM receiver and Open Payments client paths are pinned only by their constants in `dassie-node`.
- **The XRP Ledger wallet derivation.** The same passphrase must find the same account and its funds. Pinned by the XRPL wallets in `crates/dassie-identity/tests/data/chain.json`, which `crates/dassie-xrpl/tests/vectors.rs` derives ([XRP Ledger](areas/xrpl.md)).
- **Ed25519 verification by ZIP-215.** Every node must accept and refuse the same signatures, or edge-case signatures split the network ([Identity](areas/identity.md)).
- **The Interledger wire formats:** ILPv4 packets and errors, ILDCP and BTP 2.0 byte for byte, STREAM per RFC 29, Open Payments per its specs. Other Interledger software must interoperate with a node. Pinned by golden vectors and by interop tests against `ilp-protocol-stream`, `ilp-plugin-btp` and `@interledger/open-payments` ([Testing](areas/testing.md#interop)).
- **The peer protocol, and what a settlement on a ledger means** (the XRPL memo type and its semantics), within one network version. Both sides of a peering must agree; a change is a new protocol version that older nodes refuse cleanly ([Peer protocol](areas/peer-protocol.md), [XRP Ledger](areas/xrpl.md)).
- **The release mirror's layout and the install layout.** Installed nodes' updaters and the `curl | sh` installer fetch from those paths and switch between those directories. Pinned by `dassie-release`'s layout tests and the install tests ([Release](areas/release.md)).

Not frozen, on purpose: a node does not talk to the original implementation's nodes or read its databases; an owner of one recovers by entering their passphrase in a new node ([D2](decisions/D02-peer-protocol-v0-or-v1.md), [D3](decisions/D03-database-continuity.md)).

## Principles

- **Zero configuration.** A hostname and a passphrase are all a node needs; discovery, peering, addressing and settlement are automatic.
- **Ledger-neutral, currency-neutral.** Settlement is a plug-in interface, and membership does not depend on any ledger.
- **Secrets stay with the owner.** The passphrase never leaves the browser, and every key derives from it.
- **HTTPS everywhere,** in development too.
- **Self-contained releases** that install and update themselves.

## Decisions

- [D1](decisions/D01-what-runs-in-rust-and-what-in-typescript.md): the node is Rust, the browser Solid, the tooling TypeScript.
- [D2](decisions/D02-peer-protocol-v0-or-v1.md), [D3](decisions/D03-database-continuity.md): no v0 peer protocol, no v0 databases.
- [D8](decisions/D08-real-money.md): no real money yet.
- [D15](decisions/D15-bootstrap-network-for-v1.md): the testnet's bootstrap nodes.
- [D19](decisions/D19-each-node-keeps-its-own-books.md): each node keeps its own books.
- [D21](decisions/D21-tooling-on-node-and-pnpm-not-deno.md): tooling on Node and pnpm.
