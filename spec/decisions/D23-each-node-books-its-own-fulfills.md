# D23. Each node books its own Fulfills

**Status:** active, decided 2026-10-18. Supersedes [D17](D17-the-upstream-decides-whether-a-fulfill-counted.md)'s downstream side and [D18](D18-one-grace-limit-per-peer.md).
**Areas:** [Settlement](../areas/settlement.md), [Connector](../areas/connector.md), [Peer protocol](../areas/peer-protocol.md), [Network](../areas/network.md), [Payments](../areas/payments.md)

## Facts

[D20](D20-trust-follows-what-a-peer-earned-us.md) has a senior extend its junior nothing, so D18's grace limit for a junior fell to its floor, and the simulation banned honest juniors for the Fulfills one of their crashes made them refuse. The limit counted two things that need no limit of their own:

- **Debt.** A peer can never owe us more than the credit we extend: past it, its packets are refused. Using all of it is what the credit is for.
- **Refused Fulfills.** D17 had the downstream book a loss when its upstream refused a Fulfill. Without that, a refusal changes nothing in our books: it only means the upstream does not see that it owes us more.

## Decision

Each node books the Fulfills it sends as final, and only its owner bans.

- **A node keeps its own Fulfills,** whatever its upstream does with them. No verdict is asked for or owed.
- **A peer costs us at most the credit we extend,** plus what it has in flight ([D24](D24-money-in-flight-with-a-peer-stays-within-a-few-packets.md)). That bound replaces the grace limit.
- **Only the owner bans.** Which peers are worth keeping is judged by what they earn us (D20), not by what they lose us.
- **A node knows only the credit it extends,** never the credit its peer extends to it. So only the creditor can tell when a peer should settle: **it asks, and the debtor decides how much,** settling in a way that never makes it ask back.

[Settlement](../areas/settlement.md) states the rules, and [Peer protocol](../areas/peer-protocol.md) the `settlementRequest` message and the fields kept for older nodes.

## Accepted

After a crash, two honest peers' books can drift apart: a node that crashed with Prepares in flight never learns that its downstream paid them, and a Fulfill that arrives after the upstream's expiry counts only downstream. Asked, the peer settles within its own limits, which restores what drift took from either side's liquidity as far as both sides' credit allows. Past that, the creditor's room for the debtor stays short of a packet with nothing settled, and the link is withdrawn and ended ([D12](D12-peering-lifecycle-and-liveness.md#refinement-a-peer-that-stays-short-of-room-2026-10-19), 2026-10-19): a link that lost most of its liquidity is not saved. What one crash drifts is at most what was in flight, which D24 caps.

## Rejected

- *Keeping D18, with the limit at least a share of the default credit line:* honest juniors would pass, but a limit would remain for losses the credit already bounds.
- *A grace limit for refusals alone:* a refusal does not cost us anything our books show.
- *The debtor settling on its own, by the middle of the range* (as the original does, ACC-24): it needs the credit the peer extends, which a node does not know, and drift leaves it no reason to settle.
- *A request naming an amount:* the creditor does not know what the debtor's books say it owes.
- *Persisting packets in flight on both sides,* so that a crash loses no Fulfill (first rejected under [D17](D17-the-upstream-decides-whether-a-fulfill-counted.md)): a write per Prepare, and a Fulfill that arrives after the upstream's expiry would still count on one side only.
