# D6. Owner ledger and multi-currency accounts

**Areas:** [Ledger](../areas/ledger.md), [Connector](../areas/connector.md). Per-ledger credit and packet settings are not built yet ([roadmap](../roadmap.md)).

## Facts

The original's owner account is hard-coded to one dollar ledger, even on nodes that only hold XRP; it has no limit, so the owner can overspend; and the UI shows only dollars.

## Decision

- **The owner's funds on each ledger are one pool, with a limit.** Each endpoint that spends for the owner, such as a BTP token, gets a sub-account of the pool for attribution, and the limit applies to the group's total. Per-token spending caps are limits computed from the books, not allocations.
- **What the node gains or loses is the owner's:** the owner's group takes in suspense, fees, losses and the exchange accounts. The owner's balance is then everything the node holds on the ledger beyond what it owes peers, and a loss is the owner's to bear, not money the owner could still spend.
- **The owner's own endpoints never convert.** The owner pays from the ledger a route starts on, which keeps the exchange accounts inside the group coherent.
- **Every endpoint has a ledger,** and ILDCP answers that ledger's asset.
- **Every amount shown carries its currency,** and the owner may choose a display currency for the total.

## Rejected

- *An account per token holding its own balance,* as `ilp-connector`'s child accounts do: refilling a token means scheduled top-ups, and funds cannot be committed to several tokens at once, so ten tokens that may each spend $10 would lock up $100.
