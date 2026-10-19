# D8. Real money

**Status:** active, confirmed 2026-10-11, extended 2026-10-15 and 2026-10-16.
**Areas:** [XRPL](../areas/xrpl.md), [Node](../areas/node.md), [Roadmap](../roadmap.md)

## Facts

The original refuses realm `live` ("Livenet is not yet supported"), and its XRPL mainnet module is not registered.

## Decision

`live` stays disabled until all of these hold:

- the authentication of [D2](D02-peer-protocol-v0-or-v1.md) is in place;
- the ledger is durable (journal, reconciliation);
- XRPL reserve and result handling is complete (ACC-19 to ACC-22);
- releases are signed ([D11](D11-release-integrity.md));
- there has been an external review;
- a mainnet bootstrap node list exists.

The testnet scheme now handles the reserve, activation, fees and results (ACC-19 to ACC-22). Mainnet still needs a memo type of its own (ACC-13), the review and the bootstrap list.

**Mainnet is refused at the ledger too:** the testnet scheme stops unless its server reports the testnet's network ID. An owner who points it at a mainnet server gets a failed scheme, not mainnet XRP booked or signed for with a testnet wallet.
