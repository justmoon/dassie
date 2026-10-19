# Decisions

Each file records one choice that had alternatives: the facts that led to it, what was decided, what was accepted, and what was rejected and why. The rules themselves live in the [area docs](../README.md#areas). A decision says why they are what they are.

Read a decision when its area doc links to it and you need the reason, for example before changing the rule.

| # | Decision | Status | Areas |
|---|---|---|---|
| [D1](D01-what-runs-in-rust-and-what-in-typescript.md) | What runs in Rust and what in TypeScript | active | overview, engineering |
| [D2](D02-peer-protocol-v0-or-v1.md) | Peer protocol: v0 or v1 | active | peer-protocol, identity |
| [D3](D03-database-continuity.md) | Database continuity | active | node |
| [D4](D04-network-membership-economics.md) | Network membership economics | active; fee not built | network, peer-protocol |
| [D5](D05-ui-and-cli-rpc-contract.md) | UI and CLI RPC contract | active | rpc, wallet |
| [D6](D06-owner-ledger-and-multi-currency-accounts.md) | Owner ledger and multi-currency accounts | active | ledger, connector, wallet |
| [D7](D07-exchange-rates-fees-and-spread.md) | Exchange rates, fees and spread | active | connector |
| [D8](D08-real-money.md) | Real money | active | xrpl, node, roadmap |
| [D9](D09-open-payments-fully-and-no-spsp.md) | Open Payments, fully, and no SPSP | active | payments, http, wallet |
| [D10](D10-secrets-at-rest.md) | Secrets at rest | active | node, http |
| [D11](D11-release-integrity.md) | Release integrity | active | release |
| [D12](D12-peering-lifecycle-and-liveness.md) | Peering lifecycle and liveness | active; child peers not built | network, peer-protocol |
| [D13](D13-connector-timing-and-packet-limits.md) | Connector timing and packet limits | active | connector, ilp, network |
| [D14](D14-build-system.md) | Build system | active | engineering, release |
| [D15](D15-bootstrap-network-for-v1.md) | Bootstrap network for v1 | active | network |
| [D16](D16-engineering-style-pragmatic-tigerstyle-and-deterministic-simulation.md) | Engineering style: pragmatic TigerStyle and deterministic simulation | active | engineering, testing, all crates |
| [D17](D17-the-upstream-decides-whether-a-fulfill-counted.md) | The upstream decides whether a Fulfill counted | superseded by D23 | — |
| [D18](D18-one-grace-limit-per-peer.md) | One grace limit per peer | superseded by D23 | — |
| [D19](D19-each-node-keeps-its-own-books.md) | Each node keeps its own books | active | engineering, settlement, ledger, testing |
| [D20](D20-trust-follows-what-a-peer-earned-us.md) | Trust follows what a peer earned us | active | settlement, network |
| [D21](D21-tooling-on-node-and-pnpm-not-deno.md) | Tooling on Node and pnpm, not Deno | active | engineering, release, dev |
| [D22](D22-settlement-hubs.md) | Settlement hubs | active; design, not built | designs/settlement-hubs |
| [D23](D23-each-node-books-its-own-fulfills.md) | Each node books its own Fulfills | active | settlement, connector, peer-protocol, network, payments |
| [D24](D24-money-in-flight-with-a-peer-stays-within-a-few-packets.md) | Money in flight with a peer stays within a few packets | active | connector, settlement |
| [D25](D25-a-links-thresholds-are-counted-in-its-largest-packet.md) | A link's thresholds are counted in its largest packet | active | settlement, connector |
| [D27](D27-simulations-run-continuously-from-random-seeds.md) | Simulations run continuously, from random seeds | active; partly built | testing, release |

D26 is reserved for the periphery design, which is under review.

## Adding a decision

A choice between real alternatives gets a decision file:

- Name it `Dnn-<slug>.md`, with the next free number. Numbers never change and are never reused, so `D23` is a valid citation in code.
- Use the shape of the files here: status and date, areas, facts, decision, accepted, rejected.
- Keep it to the reasons. State the rule, with its numbers, in the area doc, in the same PR.
- Add a row to the table above.

When a later decision replaces one, mark the old one superseded and shrink it to what it decided, why it was replaced, and what remains of it. A refinement that keeps the decision's direction updates the area doc, and if the reason changed, the decision's text. The history is in git.
