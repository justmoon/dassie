# Real money

**Areas:** [XRPL](../areas/xrpl.md), [Roadmap](../roadmap.md#real-money)

## Decision

The `live` realm stays disabled until all of these hold:

- peer messages are authenticated ([peer-protocol-v1](peer-protocol-v1.md));
- the ledger is durable and reconciles with the underlying ledger;
- the XRPL scheme handles reserves, fees and transaction results;
- releases are signed ([release-integrity](release-integrity.md));
- there has been an external review;
- a mainnet scheme with its own memo type, and a mainnet bootstrap node list, exist.

Mainnet is refused at the ledger too: the testnet scheme stops unless its server reports the testnet's network ID. An owner who points it at a mainnet server gets a failed scheme, not mainnet XRP signed for with a testnet wallet.
