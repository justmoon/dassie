# Decisions

Each file records one choice between real alternatives: the facts that forced it, what was decided, what was accepted, and what was rejected and why. The rules themselves live in the [area docs](../README.md#areas) and the code; a decision says why they are what they are. Read one before changing the rule it explains.

| Decision | Title | Areas |
|---|---|---|
| [bootstrap-network](bootstrap-network.md) | Bootstrap network for v1 | network |
| [build-system](build-system.md) | Build system | engineering, release |
| [close-rather-than-lower](close-rather-than-lower.md) | A receiver never lowers a maximum it told: it closes a connection that no longer fits | stream, payments |
| [closing-channels](closing-channels.md) | A settlement channel left idle closes to disk, whatever the balance | settlement, network, xrpl |
| [connector-timing](connector-timing.md) | Connector timing and packet limits | connector, ilp |
| [core-and-periphery](core-and-periphery.md) | Core and periphery (design, partly built) | designs/periphery |
| [crash-drift](crash-drift.md) | The simulation rules out a link that crashes drifted past what it survives | testing, settlement |
| [database-continuity](database-continuity.md) | Database continuity | node |
| [default-relay](default-relay.md) | A node without a hostname keeps the default front door as its relay | node, http, payments, wallet |
| [desktop-app](desktop-app.md) | The desktop app runs the node in its process and shows the wallet from the node's origin | desktop, node, http, overview |
| [device-pairing](device-pairing.md) | Only the owner on the node's machine pairs a device, with a secret kept in memory | rpc, peer-protocol, node, designs/periphery |
| [dsusd](dsusd.md) | A testnet dollar of our own, bought only with XRP through a pool we keep at the market | dsusd-keeper, xrpl, settlement |
| [earned-trust](earned-trust.md) | Trust follows what a peer earned us | settlement, network |
| [ended-for-room](ended-for-room.md) | A peering ended for room stays apart, and earns no credit back | network, settlement |
| [ending-a-peering](ending-a-peering.md) | A node that ends a peering collects first, and pays nothing | network, settlement, peer-protocol, connector |
| [epoch-handover](epoch-handover.md) | Each validator set runs an epoch of its own, and a repeat finalizes an epoch's last block | hubs, designs/settlement-hubs |
| [exchange-rates](exchange-rates.md) | Exchange rates, fees and spread | connector |
| [external-assets](external-assets.md) | External assets apart from settlement schemes | settlement, xrpl, ledger, wallet |
| [finality-after-any-outage](finality-after-any-outage.md) | A hub finalizes again after any outage, in bounded memory | hubs |
| [full-or-stopped-short](full-or-stopped-short.md) | A receiver says whether it is full or stopped short | payments, stream |
| [headless-setup](headless-setup.md) | An owner who keeps the passphrase on the node's host sets the node up with `dassie setup`, through the node's own setup route | node, http, identity, overview |
| [hub-founding](hub-founding.md) | The sponsor founds a hub over a stream of its own, and a quorum's signatures of the genesis found it | hubs, peer-protocol, designs/settlement-hubs |
| [issued-assets](issued-assets.md) | An issued asset's issuer is the owner's counterparty, and a dollar token pauses off its peg (transfer fees, the peg guard and disabling test RLUSD not built) | xrpl, settlement, connector |
| [ledger-resets](ledger-resets.md) | A ledger below the newest seen is a reset only on the testnet, and only once its UNL validates it (the UNL's part, the network's constant as the gate and a proven absence not built) | xrpl, designs/xrpl-validation |
| [mainnet-schemes](mainnet-schemes.md) | Mainnet has schemes of its own, and a ledger runs only in its node's realm (not built) | xrpl, settlement, ledger, node, identity |
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
| [paid-within-a-tolerance](paid-within-a-tolerance.md) | A payment within a tolerance of its amount is paid in full | payments, stream |
| [passive-rebalancing](passive-rebalancing.md) | Nodes rebalance passively: inventory pricing, a rate feed and market makers | connector, rate-feed, settlement |
| [peer-protocol-v1](peer-protocol-v1.md) | Peer protocol: v0 or v1 | peer-protocol, identity |
| [peering-lifecycle](peering-lifecycle.md) | Peering lifecycle and liveness | network |
| [periphery-lists-no-neighbors](periphery-lists-no-neighbors.md) | A periphery node's link state lists no neighbors, so it may keep many uplinks | network, peer-protocol, connector |
| [places-by-proved-key](places-by-proved-key.md) | A connection holds a place by the key it proved | peer-protocol, network, designs/periphery |
| [promotion-to-core](promotion-to-core.md) | A node becomes core by registering and being promoted (not built) | network, peer-protocol, designs/periphery |
| [quic-only](quic-only.md) | The peer protocol and the hubs' messages travel over QUIC only | peer-protocol, node, network, designs/periphery, release |
| [random-seed-simulations](random-seed-simulations.md) | Simulations run continuously, from random seeds (partly built) | testing |
| [ready-after-first-step](ready-after-first-step.md) | A node is up once its engine has committed a step | node, release |
| [real-money](real-money.md) | Real money | xrpl, node, designs/settlement-hubs, roadmap |
| [receive-limit-alone](receive-limit-alone.md) | A receive limit alone bounds nothing: it is refused, and grants kept with one are revoked | payments |
| [receive-limit-caps-delivery](receive-limit-caps-delivery.md) | A receive limit caps what payments deliver, not only what they reserve | payments, stream |
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
| [verified-ledgers](verified-ledgers.md) | The node books only what ledgers its own UNL validated prove (design, not built) | designs/xrpl-validation, xrpl, designs/settlement-hubs |
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
