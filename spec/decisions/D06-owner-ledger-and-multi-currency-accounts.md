# D6. Owner ledger and multi-currency accounts

**Status:** active, confirmed 2026-10-11; per-ledger settings not built yet; owner sub-accounts confirmed 2026-10-13, caps 2026-10-14, the owner's group and display currency 2026-10-15.
**Areas:** [Ledger](../areas/ledger.md), [Connector](../areas/connector.md), [Wallet](../areas/wallet.md)

## Facts

In the original:

- the owner ledger is hard-coded to `stub+usd`, even on XRPL-only nodes (ACC-9);
- owner equity has no limit, so the owner can overspend;
- the UI shows only USD (UI-2), ILDCP always answers XRP/9 (ILP-7), and incoming payments always say USD/2 (ILP-10).

## Decision

- **An owner equity account per active ledger,** with a limit, so the owner's spendable balance per ledger is enforced.
- **Every endpoint has a ledger,** chosen at creation; ILDCP answers that ledger's asset.
- **Every amount the UI shows carries its currency,** and the owner may choose a display currency for the total.
- **Per-ledger settings** for the credit line and the largest packet. Not built yet: both are node-wide for now ([roadmap](../roadmap.md)).
- **The owner's funds on a ledger are one pool.** Each spending endpoint, such as a BTP token, gets a sub-account of the pool for attribution, and the limit applies to the group's total. Per-token spending caps are limits computed from the books, not allocations.
- **What the node gains or loses is the owner's:** the group takes in suspense, fees, losses and the exchange accounts, so the owner's balance is everything the node holds on the ledger beyond what it owes peers. A loss is then the owner's to bear, not money the owner could still spend.
- **The owner's own endpoints never convert,** which keeps the exchange accounts coherent inside the group: the owner pays from the ledger a route starts on.

[Ledger](../areas/ledger.md) states the accounts and the group, and [Connector](../areas/connector.md) the caps.

## Rejected

- *An account per token holding its own balance,* as `ilp-connector`'s child accounts do: refilling a token means scheduled top-ups, and funds cannot be committed to several tokens at once, so ten tokens that may each spend $10 would lock up $100.
