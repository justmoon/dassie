# D23. Each node books its own Fulfills

**Areas:** [Settlement](../areas/settlement.md), [Connector](../areas/connector.md), [Peer protocol](../areas/peer-protocol.md)

## Facts

A Fulfill one hop posts may never count at its upstream: the downstream crashes before the Fulfill arrives, the upstream crashes with the Prepare in flight, or the Fulfill arrives after the upstream's expiry. Either the two books then disagree, or one side must be made to give way.

## Decision

Each node books the Fulfills it sends as final, and only its owner bans.

- **A node keeps its own Fulfills,** whatever its upstream does with them. No verdict is asked for or owed.
- **A peer costs us at most the credit we extend,** plus what it has in flight ([D24](D24-money-in-flight-with-a-peer-stays-within-a-few-packets.md)). Past the credit, its packets are refused; using all of it is what the credit is for. No other limit on what a peer costs us is needed.
- **Only the owner bans.** Which peers are worth keeping is judged by what they earn us ([D20](D20-trust-follows-what-a-peer-earned-us.md)), not by what they lose us.
- **A node knows only the credit it extends,** never the credit its peer extends to it. So only the creditor can tell when a peer should settle: **it asks, and the debtor decides how much,** settling in a way that never makes it ask back.

## Accepted

After a crash, two honest peers' books can drift apart. Asked, the peer settles within its own limits, which restores what drift took from either side's liquidity as far as both sides' credit allows. Past that, the creditor's room for the debtor stays short with nothing settled, and the link is withdrawn and ended ([D12](D12-peering-lifecycle-and-liveness.md)): a link that lost most of its liquidity is not saved. What one crash drifts is at most what was in flight, which D24 caps.

## Rejected

- *The upstream decides whether a Fulfill counted,* answering each with a verdict from a durable record, and the downstream booking a loss on a refusal. It costs durable state, retries and a verdict per Fulfill; a dishonest upstream can still refuse Fulfills it forwarded and keep the difference; and refusals then need a limit of their own. If we keep our own Fulfills, a refusal changes nothing in our books: it only means the upstream does not see that it owes us more.
- *A grace limit per peer,* a share of the credit we extend, counting refusals and overdue debt, past which a peer is banned. A senior extends its junior nothing, so the limit fell to its floor, and the simulation banned honest juniors for the Fulfills one of their crashes made them refuse. Both things it counted are already bounded by the credit.
- *The downstream deciding,* with the upstream taking any valid Fulfill however late: the upstream would have to persist every Prepare, and the downstream would hold a free option on every packet.
- *Holding transfers pending until a verdict:* pending transfers would pile up while a peer is silent, blocking everyone's packets.
- *Persisting packets in flight on both sides,* so that a crash loses no Fulfill: a write per Prepare, and a Fulfill that arrives after the upstream's expiry would still count on one side only.
- *The debtor settling on its own, by the middle of the range:* it needs the credit the peer extends, which a node does not know, and drift leaves it no reason to settle.
- *A request naming an amount:* the creditor does not know what the debtor's books say it owes.
