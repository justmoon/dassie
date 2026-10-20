# Decisions

Each file records one choice between real alternatives: the facts that forced it, what was decided, what was accepted, and what was rejected and why. The rules themselves live in the [area docs](../README.md#areas) and the code; a decision says why they are what they are. Read one before changing the rule it explains.

| # | Decision | Areas |
|---|---|---|
| [D1](D01-what-runs-in-rust-and-what-in-typescript.md) | What runs in Rust and what in TypeScript | overview, engineering |
| [D2](D02-peer-protocol-v0-or-v1.md) | Peer protocol: v0 or v1 | peer-protocol, identity |
| [D3](D03-database-continuity.md) | Database continuity | node |
| [D4](D04-network-membership-economics.md) | Network membership economics (fee not built) | network |
| [D5](D05-ui-and-cli-rpc-contract.md) | UI and CLI RPC contract | rpc, wallet |
| [D6](D06-owner-ledger-and-multi-currency-accounts.md) | Owner ledger and multi-currency accounts | ledger, connector |
| [D7](D07-exchange-rates-fees-and-spread.md) | Exchange rates, fees and spread | connector |
| [D8](D08-real-money.md) | Real money | xrpl, roadmap |
| [D9](D09-open-payments-fully-and-no-spsp.md) | Open Payments, fully, and no SPSP | payments, http |
| [D10](D10-secrets-at-rest.md) | Secrets at rest | node, http |
| [D11](D11-release-integrity.md) | Release integrity | release |
| [D12](D12-peering-lifecycle-and-liveness.md) | Peering lifecycle and liveness (child peers not built) | network |
| [D13](D13-connector-timing-and-packet-limits.md) | Connector timing and packet limits | connector, ilp |
| [D14](D14-build-system.md) | Build system | engineering, release |
| [D15](D15-bootstrap-network-for-v1.md) | Bootstrap network for v1 | network |
| [D16](D16-engineering-style-pragmatic-tigerstyle-and-deterministic-simulation.md) | Engineering style: pragmatic TigerStyle and deterministic simulation | engineering, testing |
| [D19](D19-each-node-keeps-its-own-books.md) | Each node keeps its own books | engineering, settlement, testing |
| [D20](D20-trust-follows-what-a-peer-earned-us.md) | Trust follows what a peer earned us | settlement, network |
| [D21](D21-tooling-on-node-and-pnpm-not-deno.md) | Tooling on Node and pnpm, not Deno | engineering |
| [D22](D22-settlement-hubs.md) | Settlement hubs (design, not built) | designs/settlement-hubs |
| [D23](D23-each-node-books-its-own-fulfills.md) | Each node books its own Fulfills | settlement, connector, peer-protocol |
| [D24](D24-money-in-flight-with-a-peer-stays-within-a-few-packets.md) | Money in flight with a peer stays within a few packets | connector, settlement |
| [D25](D25-a-links-thresholds-are-counted-in-its-largest-packet.md) | A link's thresholds are counted in its largest packet | settlement, connector |
| [D26](D26-core-and-periphery.md) | Core and periphery (design, not built) | designs/periphery |
| [D27](D27-simulations-run-continuously-from-random-seeds.md) | Simulations run continuously, from random seeds (partly built) | testing |
| [D28](D28-a-node-that-ends-a-peering-collects-first-and-pays-nothing.md) | A node that ends a peering collects first, and pays nothing | network, settlement, peer-protocol, connector |
| [D29](D29-a-grants-debit-limit-is-in-its-wallets-asset.md) | A grant's debit limit is in its wallet's asset | payments |
| [D30](D30-a-node-is-up-once-its-engine-has-committed-a-step.md) | A node is up once its engine has committed a step | node, release |
| [D31](D31-a-node-peers-on-a-scheme-only-once-it-has-seen-its-ledger.md) | A node peers on a scheme only once it has seen its ledger | settlement, network, xrpl |
| [D32](D32-a-ledger-below-the-newest-seen-is-a-reset-only-on-the-testnet.md) | A ledger below the newest seen is a reset only on the testnet | xrpl |
| [D33](D33-a-peering-ended-for-room-stays-apart-and-earns-no-credit-back.md) | A peering ended for room stays apart, and earns no credit back | network, settlement |
| [D34](D34-strangers-never-take-the-owners-share-of-a-pool.md) | Strangers never take the owner's share of a pool | payments, stream |

## Adding or changing a decision

- A new decision gets the next free number, `Dnn-<slug>.md`. Numbers are never reused, so `D23` is a valid citation in code; a missing number belongs to a decision that was replaced or is under review.
- Add a row to the table above.
- Use the shape of the files here: areas, facts, decision, accepted, rejected. No dates or history: git has those. Past experience belongs in only as an argument ("the simulation showed …").
- Keep it to the reasons. A number belongs in a decision only when the argument is about it.
- When a decision replaces another, fold what still argues for the current design (usually the rejected option and why) into the new one, and delete the old file.
