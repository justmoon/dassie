# Decisions

Each file records one choice between real alternatives: the facts that forced it, what was decided, what was accepted, and what was rejected and why. The rules themselves live in the [area docs](../README.md#areas) and the code; a decision says why they are what they are. Read one before changing the rule it explains.

| Decision | Title | Areas |
|---|---|---|
| [bootstrap-network](bootstrap-network.md) | Bootstrap network for v1 | network |
| [build-system](build-system.md) | Build system | engineering, release |
| [connector-timing](connector-timing.md) | Connector timing and packet limits | connector, ilp |
| [core-and-periphery](core-and-periphery.md) | Core and periphery (design, partly built) | designs/periphery |
| [database-continuity](database-continuity.md) | Database continuity | node |
| [default-relay](default-relay.md) | A node without a hostname keeps the default front door as its relay | node, http, payments, wallet |
| [earned-trust](earned-trust.md) | Trust follows what a peer earned us | settlement, network |
| [ended-for-room](ended-for-room.md) | A peering ended for room stays apart, and earns no credit back | network, settlement |
| [ending-a-peering](ending-a-peering.md) | A node that ends a peering collects first, and pays nothing | network, settlement, peer-protocol, connector |
| [exchange-rates](exchange-rates.md) | Exchange rates, fees and spread | connector |
| [external-assets](external-assets.md) | External assets apart from settlement schemes (RLUSD not built) | settlement, xrpl, ledger, designs/external-assets |
| [full-or-stopped-short](full-or-stopped-short.md) | A receiver says whether it is full or stopped short | payments, stream |
| [issued-assets](issued-assets.md) | An issued asset's issuer is the owner's counterparty (design, not built) | designs/external-assets, settlement, connector |
| [ledger-resets](ledger-resets.md) | A ledger below the newest seen is a reset only on the testnet | xrpl |
| [membership-economics](membership-economics.md) | Network membership economics (fee not built) | network |
| [named-decisions](named-decisions.md) | Decisions are named, not numbered | engineering |
| [nix-flake-on-the-mirror](nix-flake-on-the-mirror.md) | NixOS takes a release from a flake on the mirror | release |
| [node-and-pnpm](node-and-pnpm.md) | Tooling on Node and pnpm, not Deno | engineering |
| [open-payments](open-payments.md) | Open Payments, fully, and no SPSP | payments, http |
| [own-books](own-books.md) | Each node keeps its own books | engineering, settlement, testing |
| [own-fulfills](own-fulfills.md) | Each node books its own Fulfills | settlement, connector, peer-protocol |
| [owner-ledger](owner-ledger.md) | Owner ledger and multi-currency accounts | ledger, connector |
| [owners-share](owners-share.md) | Strangers never take the owner's share of a pool | payments, stream |
| [packets-in-flight](packets-in-flight.md) | Money in flight with a peer stays within a few packets | connector, settlement |
| [peer-protocol-v1](peer-protocol-v1.md) | Peer protocol: v0 or v1 | peer-protocol, identity |
| [peering-lifecycle](peering-lifecycle.md) | Peering lifecycle and liveness | network |
| [places-by-proved-key](places-by-proved-key.md) | A connection holds a place by the key it proved | peer-protocol, network, designs/periphery |
| [promotion-to-core](promotion-to-core.md) | A node becomes core by registering and being promoted (not built) | network, peer-protocol, designs/periphery |
| [random-seed-simulations](random-seed-simulations.md) | Simulations run continuously, from random seeds (partly built) | testing |
| [ready-after-first-step](ready-after-first-step.md) | A node is up once its engine has committed a step | node, release |
| [real-money](real-money.md) | Real money | xrpl, roadmap |
| [release-integrity](release-integrity.md) | Release integrity | release |
| [rpc-contract](rpc-contract.md) | UI and CLI RPC contract | rpc, wallet |
| [rust-and-typescript](rust-and-typescript.md) | What runs in Rust and what in TypeScript | overview, engineering |
| [secrets-at-rest](secrets-at-rest.md) | Secrets at rest | node, http |
| [settlement-hubs](settlement-hubs.md) | Settlement hubs (design, not built) | designs/settlement-hubs |
| [synced-before-peering](synced-before-peering.md) | A node peers on a scheme only once it has seen its ledger | settlement, network, xrpl |
| [thresholds-in-packets](thresholds-in-packets.md) | A link's thresholds are counted in its largest packet | settlement, connector |
| [tigerstyle](tigerstyle.md) | Engineering style: pragmatic TigerStyle and deterministic simulation | engineering, testing |
| [unreachable-peers](unreachable-peers.md) | A node takes no Prepare from a peer it cannot reach | connector, peer-protocol, settlement |
| [unusable-limits](unusable-limits.md) | A limit no payment can use is caught before approval | payments, wallet |
| [versions-0-1-and-0-2](versions-0-1-and-0-2.md) | Nodes of 0.1 and 0.2 do not settle with each other | peer-protocol, settlement, release |
| [website-hosting](website-hosting.md) | dassie.land is a bucket behind the mirror's load balancer | website, release |

## Adding or changing a decision

- A new decision gets a short name of its own, in lowercase words joined by hyphens, as `own-fulfills.md`: what was decided, in a few words. Its title, the file's heading, says it in full. A name is never given to another decision, so `spec/decisions/own-fulfills.md` is a valid citation in code ([named-decisions](named-decisions.md)).
- Add a row to the table above, where its name falls in alphabetical order.
- Use the shape of the files here: areas, facts, decision, accepted, rejected. No dates or history: git has those. Past experience belongs in only as an argument ("the simulation showed …").
- Keep it to the reasons. A number belongs in a decision only when the argument is about it.
- When a decision replaces another, fold what still argues for the current design (usually the rejected option and why) into the new one, and delete the old file.

## Former numbers

Decisions were once numbered, and commits, pull requests and issues still cite them so. This table only translates those numbers; nothing is added to it. A missing number belonged to a decision that was replaced.

| Number | Decision |
|---|---|
| D1 | [rust-and-typescript](rust-and-typescript.md) |
| D2 | [peer-protocol-v1](peer-protocol-v1.md) |
| D3 | [database-continuity](database-continuity.md) |
| D4 | [membership-economics](membership-economics.md) |
| D5 | [rpc-contract](rpc-contract.md) |
| D6 | [owner-ledger](owner-ledger.md) |
| D7 | [exchange-rates](exchange-rates.md) |
| D8 | [real-money](real-money.md) |
| D9 | [open-payments](open-payments.md) |
| D10 | [secrets-at-rest](secrets-at-rest.md) |
| D11 | [release-integrity](release-integrity.md) |
| D12 | [peering-lifecycle](peering-lifecycle.md) |
| D13 | [connector-timing](connector-timing.md) |
| D14 | [build-system](build-system.md) |
| D15 | [bootstrap-network](bootstrap-network.md) |
| D16 | [tigerstyle](tigerstyle.md) |
| D19 | [own-books](own-books.md) |
| D20 | [earned-trust](earned-trust.md) |
| D21 | [node-and-pnpm](node-and-pnpm.md) |
| D22 | [settlement-hubs](settlement-hubs.md) |
| D23 | [own-fulfills](own-fulfills.md) |
| D24 | [packets-in-flight](packets-in-flight.md) |
| D25 | [thresholds-in-packets](thresholds-in-packets.md) |
| D26 | [core-and-periphery](core-and-periphery.md) |
| D27 | [random-seed-simulations](random-seed-simulations.md) |
| D28 | [ending-a-peering](ending-a-peering.md) |
| D29 | [unusable-limits](unusable-limits.md) |
| D30 | [ready-after-first-step](ready-after-first-step.md) |
| D31 | [synced-before-peering](synced-before-peering.md) |
| D32 | [ledger-resets](ledger-resets.md) |
| D33 | [ended-for-room](ended-for-room.md) |
| D34 | [owners-share](owners-share.md) |
| D35 | [website-hosting](website-hosting.md) |
| D36 | [promotion-to-core](promotion-to-core.md) |
| D37 | [versions-0-1-and-0-2](versions-0-1-and-0-2.md) |
| D38 | [full-or-stopped-short](full-or-stopped-short.md) |
| D39 | [places-by-proved-key](places-by-proved-key.md) |
| D40 | [default-relay](default-relay.md) |
| D41 | [unreachable-peers](unreachable-peers.md) |
