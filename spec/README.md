# Dassie specification

This directory holds what the code cannot say about Dassie: what each part is for, how the parts fit together, the invariants and contracts that span them, what must never change, and why the design is what it is. The code is the only source for everything else: values, layouts, lists. The spec is arranged by area, so that you read only what concerns the parts you touch.

## Where to start

1. [Overview](overview.md): what Dassie is, its concepts, how the parts fit, and the surfaces that must never change.
2. [Engineering](engineering.md): the rules every crate and package follows.
3. The area docs for the paths you touch, from the index below. Each one names the other docs to read with it.

Read a [decision](decisions/README.md) when an area doc links to it and you need to know why a rule is what it is. Read [the original's chapters](original/README.md) only when you need to know what the original did.

## Index

| Path | Read |
|---|---|
| `crates/dassie-oer` | [Encoding](areas/encoding.md) |
| `crates/dassie-identity` | [Identity](areas/identity.md) |
| `crates/dassie-ilp` | [ILP](areas/ilp.md) |
| `crates/dassie-stream` | [STREAM](areas/stream.md) |
| `crates/dassie-ledger` | [Ledger](areas/ledger.md) |
| `crates/dassie-connector` | [Connector](areas/connector.md), [Ledger](areas/ledger.md) |
| `crates/dassie-settlement` | [Settlement](areas/settlement.md), [Ledger](areas/ledger.md) |
| `crates/dassie-xrpl` | [XRPL](areas/xrpl.md), [Settlement](areas/settlement.md) |
| `crates/dassie-peer` | [Peer protocol](areas/peer-protocol.md), [Network](areas/network.md), [Settlement](areas/settlement.md) |
| `crates/dassie-payments` | [Payments](areas/payments.md) |
| `crates/dassie-node-core` | [Node](areas/node.md), [HTTP](areas/http.md) |
| `crates/dassie-node` | [Node](areas/node.md), and by module: `engine/` [Connector](areas/connector.md), [Settlement](areas/settlement.md), [Ledger](areas/ledger.md); `http/` [HTTP](areas/http.md), with `http/peer.rs` [Peer protocol](areas/peer-protocol.md), `http/btp.rs` [Connector](areas/connector.md) and `http/open_payments/` [Payments](areas/payments.md); `open_payments/` [Payments](areas/payments.md); `rpc/` [RPC](areas/rpc.md); `store/` [Node](areas/node.md#persistence) and the area of the table; `rates.rs`, `node/rates.rs` [Connector](areas/connector.md#exchange-rates); `peer_client.rs` [Peer protocol](areas/peer-protocol.md); `acme.rs`, `tls.rs` [HTTP](areas/http.md); `fake_ledger.rs` [XRPL](areas/xrpl.md); `sim/` [Testing](areas/testing.md#deterministic-simulation) |
| `crates/dassie-cli` | [Node](areas/node.md), [Release](areas/release.md) (`dassie update`) |
| `crates/dassie-release` | [Release](areas/release.md) |
| `crates/dassie-sim` | [Testing](areas/testing.md) |
| `proto/dassie/ui/v1` | [RPC](areas/rpc.md) |
| `proto/dassie/dev/v1` | [Dev](areas/dev.md) |
| `packages/rpc` | [RPC](areas/rpc.md) |
| `packages/identity-wasm` | [Identity](areas/identity.md), [STREAM](areas/stream.md) |
| `packages/ui` | [Wallet](areas/wallet.md) |
| `apps/wallet` | [Wallet](areas/wallet.md), [RPC](areas/rpc.md) |
| `apps/dev` | [Dev](areas/dev.md) |
| `apps/dev-dashboard` | [Dev](areas/dev.md), [Wallet](areas/wallet.md) |
| `apps/website` | [Website](areas/website.md) |
| `tests/e2e` | [Testing](areas/testing.md), and the area under test |
| `tests/interop` | [Testing](areas/testing.md), [STREAM](areas/stream.md), [Payments](areas/payments.md) |
| `resources/`, `nix/`, `scripts/release.ts`, `.github/workflows/release.yml` | [Release](areas/release.md) |
| `flake.nix`, `.github/workflows/ci.yml`, `package.json`, `Cargo.toml` | [Engineering](engineering.md) |
| `testdata/` | [Testing](areas/testing.md), and the area the vectors pin |

## Areas

| Area | What it covers |
|---|---|
| [Identity](areas/identity.md) | The passphrase, the key tree, the node key and ID, the login secret, settlement entropy |
| [Encoding](areas/encoding.md) | The canonical OER subset every wire format is written in |
| [ILP](areas/ilp.md) | ILPv4 packets, timestamps, error codes, ILDCP and BTP codecs |
| [STREAM](areas/stream.md) | STREAM connections, streams, frames and crypto |
| [Ledger](areas/ledger.md) | The double-entry books: accounts, groups, the owner's funds, the journal |
| [Connector](areas/connector.md) | The packet path: endpoints, outcomes, timing, fees and spread, spending caps, money in flight, exchange rates, BTP |
| [Peer protocol](areas/peer-protocol.md) | The wire protocol between nodes: transport, envelope, messages, authentication, replay, link states |
| [Network](areas/network.md) | The node table, flooding, membership and registration, bootstrap nodes, peering and its lifecycle, routing |
| [Settlement](areas/settlement.md) | Settlement schemes, credit and its growth, thresholds, settling on request, settlement channels |
| [XRPL](areas/xrpl.md) | The XRP Ledger testnet scheme and its fake ledger |
| [Payments](areas/payments.md) | Open Payments: wallet addresses, GNAP, incoming and outgoing payments, quotes, the owner's Send |
| [Node](areas/node.md) | The daemon: runtime, startup, configuration, environment, CLI, persistence, secrets, logging |
| [HTTP](areas/http.md) | Listeners, routes, security headers, ACME and TLS, setup, login and sessions |
| [RPC](areas/rpc.md) | The owner's RPC contract, its transports and live streams |
| [Release](areas/release.md) | Builds, versions and channels, the mirror, signing, the installer and the updater |
| [Wallet](areas/wallet.md) | The owner's web app and the shared UI package |
| [Website](areas/website.md) | dassie.land, the STREAM toy and the architecture page |
| [Dev](areas/dev.md) | The local dev network and its dashboard |
| [Testing](areas/testing.md) | Test levels, golden vectors, simulations and the simulation runner |

Not built yet: the [roadmap](roadmap.md), and the designs in `designs/`: [settlement hubs](designs/settlement-hubs.md) and the [periphery](designs/periphery.md).

## What belongs here

- **What the code cannot state:** purpose, how parts fit across modules, invariants, contracts between components and versions, frozen surfaces and what pins them, and the reasons for choices a competent engineer would not guess. A number belongs here only when the argument is about it, and then the spec names the constant that holds it.
- **Never a copy of the code.** Values, limits, timeouts, layouts, field and route lists, error strings, flags and environment variables live only in the code, where they cannot drift. Point to the module instead.
- **Nothing a competent engineer already knows,** or that a linked standard defines. State only Dassie's choices and deviations.
- **No history.** No dates, milestones, PR numbers or "used to": version control has the timeline. Past experience belongs here only as an argument ("the simulation showed …").

## Keeping it current

- **A change to what a part guarantees, or why, updates its area doc in the same PR.** A changed value or list does not touch the spec.
- **A choice between real alternatives gets a decision** ([how](decisions/README.md#adding-or-changing-a-decision)). The decision keeps the reasons.
- **New work that is not built yet** is an open area of the [roadmap](roadmap.md), never a numbered milestone. A large design gets a file in `designs/`.
- **A new crate, app, package, test suite or contract** gets a row in the index above, and an area doc, or a section in an existing one.
- **Cite the spec by path and heading,** as `spec/areas/peer-protocol.md#replay-protection` in code and as relative links inside the spec. Decisions may be cited by number, as `D23`. `node --test scripts/spec_test.ts` checks every link and citation, and the index.
