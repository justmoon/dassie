# D7. Exchange rates, fees and spread

**Status:** active, confirmed 2026-10-11, detailed 2026-10-15.
**Areas:** [Connector](../areas/connector.md)

## Facts

The original fetches rates once from Coinbase, with no refresh, no spread and no fees. Its `revenue/fees` and `expenses/fees` accounts are unused (ACC-3, ILP-6).

## Decision

- **Rates are refreshed periodically,** a failed source asked again at the next refresh, and the last good ones persist. A rate too old is not used: cross-currency packets get T00.
- **Exact integer and decimal math throughout,** never floating point.
- **A configurable connector fee and FX spread** (default 0), charged only on transit from one peer to another and booked to the revenue accounts, as the original's internal-ledger design note describes. The fee rounds down: rounded up, it would take a payment's last packets whole.
- **The rate is the median of several sources,** so that one source failing or being manipulated cannot move it.

[Connector](../areas/connector.md) states the rules: the sources, the median and staleness under [exchange rates](../areas/connector.md#exchange-rates), and the fee and spread formulas.

## Rejected

- *Coinbase alone:* a single point of failure.
- *An ordered fallback list:* it still trusts whichever source answers first.
