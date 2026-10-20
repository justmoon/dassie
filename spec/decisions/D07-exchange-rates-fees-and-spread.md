# D7. Exchange rates, fees and spread

**Areas:** [Connector](../areas/connector.md)

## Decision

- **The rate is the median of several sources,** so that one source failing or being manipulated cannot take it outside what the others say. Rates are refreshed and persisted; a rate too old is not used, and cross-currency packets are refused meanwhile.
- **Three sources for every currency, from three upstreams.** A new node's sources cover each currency it keeps three times, and no two of them serve the same data: the fiat currencies come from Coinbase's data provider, Kraken's own market and the European Central Bank's reference rates, XRP from three exchanges. With three, one wrong source moves a rate only within what the other two say, and one silent source leaves two that must agree. Every source is free, needs no key, and may be asked every minute.
- **Exact arithmetic,** never floating point.
- **A connector fee and an FX spread,** set by the owner and zero by default, charged only on transit between two peers. The fee rounds down: rounded up, it would take a payment's last small packets whole and strand the payment short.

## Accepted

- The ECB publishes once a business day, so one of the three fiat sources is up to a day behind the markets, and several days over a holiday. As one of three it only pulls the rate as far as the nearer live source; with a live source silent, the remaining two must agree closely, which a quiet day allows and a sharp move refuses until the silent source is back.
- Kraken's ticker answers every pair in one request: a pair it stops listing fails its whole answer, XRP's included, until a release stops asking for the pair. Meanwhile the other two sources carry every currency, if they agree.

## Rejected

- *One source:* a single point of failure.
- *An ordered fallback list:* it still trusts whichever source answers first.
- *Mirrors of one upstream as separate sources,* such as a service that republishes the ECB's rates beside the ECB itself: they count twice and are wrong together.
- *Free rate services that need a key, require attribution, or limit how often a node may ask:* a node has no account, and asks every minute.
