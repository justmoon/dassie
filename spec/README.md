# Dassie specification

This directory holds what the code cannot say about Dassie: what each part is for, how the parts fit together, the invariants and contracts that span them, what must never change, and why the design is what it is. The code is the only source for everything else: values, layouts, lists. The spec is arranged by area, so that you read only what concerns the parts you touch.

## Where to start

1. [Overview](overview.md): what Dassie is, its concepts, how the parts fit, and the surfaces that must never change.
2. [Engineering](engineering.md): the rules every crate and package follows.
3. The area docs for the paths you touch, from the index below. Each one names the other docs to read with it.

Read a [decision](decisions/README.md) when an area doc links to it and you need to know why a rule is what it is. What the original implementation did is in its code, at the tag `original`.

## Index

| Path | Read |
|---|---|
| `crates/dassie-oer` | [Encoding](areas/encoding.md) |
| `crates/dassie-base64url` | [Encoding](areas/encoding.md#base64url), [Identity](areas/identity.md), [STREAM](areas/stream.md) |
| `crates/dassie-identity` | [Identity](areas/identity.md), `src/test_keys.rs` [Testing](areas/testing.md#slh-dsa-keys-in-tests) |
| `crates/dassie-ilp` | [ILP](areas/ilp.md) |
| `crates/dassie-stream` | [STREAM](areas/stream.md) |
| `crates/dassie-ledger` | [Ledger](areas/ledger.md) |
| `crates/dassie-connector` | [Connector](areas/connector.md), [Ledger](areas/ledger.md) |
| `crates/dassie-settlement` | [Settlement](areas/settlement.md), [Ledger](areas/ledger.md) |
| `crates/dassie-hub` | [Settlement hubs](areas/hubs.md), [Settlement](areas/settlement.md), `escrow/` [XRPL](areas/xrpl.md), `sim/` [Testing](areas/testing.md#deterministic-simulation) |
| `crates/dassie-xrpl` | [XRPL](areas/xrpl.md), [Settlement](areas/settlement.md) |
| `crates/dassie-peer` | [Peer protocol](areas/peer-protocol.md), [Network](areas/network.md), [Settlement](areas/settlement.md) |
| `crates/dassie-payments` | [Payments](areas/payments.md) |
| `crates/dassie-feed` | [Rate feed](areas/rate-feed.md), [Identity](areas/identity.md), [Encoding](areas/encoding.md) |
| `crates/dassie-node-core` | [Node](areas/node.md), [HTTP](areas/http.md) |
| `crates/dassie-kit` | [Testing](areas/testing.md#deterministic-simulation), [Engineering](engineering.md#sans-io-protocol-crates) |
| `crates/dassie-node` | [Node](areas/node.md), and by module: `engine/` [Connector](areas/connector.md), [Settlement](areas/settlement.md), [Ledger](areas/ledger.md); `http/` [HTTP](areas/http.md), with `http/btp.rs` [Connector](areas/connector.md) and `http/open_payments/` [Payments](areas/payments.md); `open_payments/` [Payments](areas/payments.md); `rpc/` [RPC](areas/rpc.md); `event_log.rs` [the event log](areas/node.md#the-event-log), with [RPC](areas/rpc.md#live-streams); `store/` [Node](areas/node.md#persistence) and the area of the table; `rates.rs`, `node/rates.rs` [Connector](areas/connector.md#exchange-rates); `feed.rs`, `engine/core/own_snapshot.rs` [Rate feed](areas/rate-feed.md#the-node-side); `hub/` [Settlement hubs](areas/hubs.md#the-validator-in-the-node), with `hub/founding.rs` and `hub/sponsor/` [founding a hub](areas/hubs.md#founding-a-hub), `hub/payouts.rs` [payout bundles](areas/hubs.md#payout-bundles), `hub/transport.rs` and `quic/hub.rs` [the hub's stream](areas/peer-protocol.md#the-hubs-stream); `outbound.rs`, `peer_server.rs` [Peer protocol](areas/peer-protocol.md); `quic/` [Peer protocol](areas/peer-protocol.md#quic), with `quic/relay.rs` [relayed streams](areas/peer-protocol.md#relayed-streams); `acme.rs`, `tls.rs` [HTTP](areas/http.md); `front_door/` [front doors](areas/http.md#front-doors), with [relayed streams](areas/peer-protocol.md#relayed-streams), and `front_door/names.rs` [names](areas/network.md#names); `engine/core/names.rs` [names](areas/network.md#names), with [RPC](areas/rpc.md); `fake_ledger.rs` [XRPL](areas/xrpl.md); `sim/` [Testing](areas/testing.md#deterministic-simulation) |
| `crates/dassie-cli` | [Node](areas/node.md), [Release](areas/release.md) (`dassie update`), [HTTP](areas/http.md#setup) (`dassie setup`) |
| `crates/dassie-wallet-assets` | [Node](areas/node.md), [HTTP](areas/http.md) |
| `crates/dassie-cli/src/market_maker/` | [Rate feed](areas/rate-feed.md#market-maker), [Connector](areas/connector.md#the-swap-service), [XRPL](areas/xrpl.md#amm-pools), [Settlement hubs](areas/hubs.md#in-the-node), [Node](areas/node.md#the-dassie-command) |
| `crates/dassie-desktop`, `nix/desktop.nix`, `nix/desktop-libs.nix` | [Desktop](areas/desktop.md), [Node](areas/node.md), [HTTP](areas/http.md) |
| `crates/dassie-release` | [Release](areas/release.md) |
| `crates/dassie-sim` | [Testing](areas/testing.md) |
| `proto/dassie/owner/v1` | [RPC](areas/rpc.md) |
| `proto/dassie/owner/v1/hub_member.proto` | [Settlement hubs](areas/hubs.md#in-the-node), [RPC](areas/rpc.md) |
| `proto/dassie/owner/v1/hub_founding.proto` | [Settlement hubs](areas/hubs.md#founding-a-hub), [RPC](areas/rpc.md) |
| `proto/dassie/owner/v1/names.proto` | [Network](areas/network.md#names), [RPC](areas/rpc.md) |
| `proto/dassie/dev/v1` | [Dev](areas/dev.md) |
| `packages/rpc` | [RPC](areas/rpc.md) |
| `crates/dassie-wasm`, `packages/wasm` | [Identity](areas/identity.md#in-the-browser-and-in-scripts), [STREAM](areas/stream.md) |
| `packages/ui` | [Wallet](areas/wallet.md) |
| `apps/wallet` | [Wallet](areas/wallet.md), [RPC](areas/rpc.md) |
| `apps/dev` | [Dev](areas/dev.md) |
| `apps/dev-dashboard` | [Dev](areas/dev.md), [Wallet](areas/wallet.md) |
| `apps/dsusd-keeper` | [DSUSD keeper](areas/dsusd-keeper.md), [XRPL](areas/xrpl.md) |
| `apps/market-maker` | [Market maker VM](areas/market-maker-vm.md), [Rate feed](areas/rate-feed.md#market-maker), [Release](areas/release.md), [Settlement hubs](areas/hubs.md#in-the-node) |
| `apps/website`, `scripts/website/`, `.github/workflows/website.yml` | [Website](areas/website.md) |
| `tests/e2e` | [Testing](areas/testing.md), and the area under test |
| `tests/interop` | [Testing](areas/testing.md), [STREAM](areas/stream.md), [Payments](areas/payments.md) |
| `resources/`, `nix/`, `scripts/release.ts`, `scripts/flake-latest.ts`, `scripts/flake-latest/`, `.github/workflows/release.yml` | [Release](areas/release.md) |
| `resources/front-door/` | [Front doors](areas/http.md#front-doors), [Node](areas/node.md#a-chosen-relay) |
| `resources/proof-server/` | [Proof servers](areas/proof-servers.md), [Checking the validators](designs/xrpl-validation.md#3-the-proof-api) |
| `flake.nix`, `.github/workflows/ci.yml`, `package.json`, `Cargo.toml` | [Engineering](engineering.md) |
| `.config/nextest.toml` | [Testing](areas/testing.md#levels), [Engineering](engineering.md) |
| `testdata/` | [Testing](areas/testing.md), and the area the vectors pin |
| `scripts/sim-campaign.ts`, `scripts/sim-campaign/` | [Testing](areas/testing.md#the-campaign) |

## Areas

| Area | What it covers |
|---|---|
| [Identity](areas/identity.md) | The passphrase, the key tree, the node key and ID, the login secret, the XRP Ledger accounts' keys |
| [Encoding](areas/encoding.md) | The canonical OER subset every wire format is written in |
| [ILP](areas/ilp.md) | ILPv4 packets, timestamps, error codes, ILDCP and BTP codecs |
| [STREAM](areas/stream.md) | STREAM connections, streams, frames and crypto |
| [Ledger](areas/ledger.md) | The double-entry books: accounts, groups, the owner's funds, the journal |
| [Connector](areas/connector.md) | The packet path: endpoints, outcomes, timing, fees and spread, spending caps, money in flight, exchange rates, BTP |
| [Peer protocol](areas/peer-protocol.md) | The wire protocol between nodes: transport, versions, messages, authentication by the QUIC connection, link states |
| [Network](areas/network.md) | The node table, flooding, membership and registration, bootstrap nodes, peering and its lifecycle, routing |
| [Settlement](areas/settlement.md) | Settlement schemes, external assets, credit, thresholds, settling on request, settlement channels |
| [Settlement hubs](areas/hubs.md) | A hub's rules and its ledger: accounts, holds, what validators attest, and who gets the fees |
| [XRPL](areas/xrpl.md) | The XRP Ledger accounts on the testnet and mainnet, the XRP scheme and the token scheme (DSUSD, RLUSD) on them, and the fake ledger |
| [Payments](areas/payments.md) | Open Payments: wallet addresses, GNAP, incoming and outgoing payments, quotes, the owner's Send |
| [Rate feed](areas/rate-feed.md) | A core node's signed snapshot of the quotes and ladders it swaps at: wire format, numbers, signing, requests over UDP and their budget, reading a snapshot, when to make one, and how the node makes and serves it |
| [Node](areas/node.md) | The daemon: runtime, startup, configuration, environment, CLI, persistence, secrets, logging |
| [HTTP](areas/http.md) | Listeners, routes, security headers, ACME and TLS, setup, login and sessions, front doors |
| [RPC](areas/rpc.md) | The owner's RPC contract, its transports and live streams |
| [Release](areas/release.md) | Builds, versions and channels, the mirror, signing, the installer and the updater |
| [Wallet](areas/wallet.md) | The owner's web app and the shared UI package |
| [Website](areas/website.md) | dassie.land: the docs, the STREAM toy and the architecture page |
| [Desktop](areas/desktop.md) | The desktop app: the owner's node in its process, the wallet's window, the tray and starting at login |
| [Dev](areas/dev.md) | The local dev network and its dashboard |
| [DSUSD keeper](areas/dsusd-keeper.md) | The testnet dollar token of our own: its issuer, its pool, and the keeper that holds the pool at the market |
| [Market maker VM](areas/market-maker-vm.md) | The VM that runs a market maker and its node against testnet core nodes: secrets, releases and updates, settings, and the testnet hubs it waits for |
| [Proof servers](areas/proof-servers.md) | The patched XRP Ledger servers that answer the node's proofs: the patch, its build, and the public server per network |
| [Testing](areas/testing.md) | Test levels, golden vectors, simulations and the simulation runner |

Not built yet: the [roadmap](roadmap.md), and the designs in `designs/`: what is not built of [settlement hubs](designs/settlement-hubs.md), what is not built of the [periphery](designs/periphery.md), [checking the XRP Ledger's validators](designs/xrpl-validation.md), and what is not built of [names](designs/names.md). [Direct settlement](designs/direct-settlement.md) keeps how peers settled without a hub, which left the code, and what bringing it back would take.

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
- **Cite the spec by path and heading,** as `spec/areas/peer-protocol.md#authentication` in code and as relative links inside the spec. Cite a decision the same way, by its file (`spec/decisions/own-fulfills.md`), never by a number. `node --test scripts/spec_test.ts` checks every link and citation, and the index.
