# D7. Exchange rates, fees and spread

**Areas:** [Connector](../areas/connector.md)

## Decision

- **The rate is the median of several sources,** so that one source failing or being manipulated cannot move it. Rates are refreshed and persisted; a rate too old is not used, and cross-currency packets are refused meanwhile.
- **Exact arithmetic,** never floating point.
- **A connector fee and an FX spread,** set by the owner and zero by default, charged only on transit between two peers. The fee rounds down: rounded up, it would take a payment's last small packets whole and strand the payment short.

## Rejected

- *One source:* a single point of failure.
- *An ordered fallback list:* it still trusts whichever source answers first.
