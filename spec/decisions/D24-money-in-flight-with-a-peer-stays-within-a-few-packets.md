# D24. Money in flight with a peer stays within a few packets

**Status:** active, decided 2026-10-18; the cap set by [D25](D25-a-links-thresholds-are-counted-in-its-largest-packet.md).
**Areas:** [Connector](../areas/connector.md), [Settlement](../areas/settlement.md)

## Facts

A node that crashes with Prepares in flight never learns that its downstream fulfilled them, so each one drifts the two peers' books apart by its amount ([D19](D19-each-node-keeps-its-own-books.md), [D23](D23-each-node-books-its-own-fulfills.md)). STREAM limits each connection to 100 packets in flight, and a node has many connections over one peer, so what it had in flight was bounded only by the credit the receiving side extends. In the simulation, a senior that crashed half a second after its owner started three payments had 8 packets of 0.1 in flight to its junior: 0.8 of drift on 1.1 of credit. Asked, the junior settled less than its minimum, while the senior's books left it less than a packet of room, so the junior's payments to the senior stalled for good.

## Decision

A node caps the money in flight with each peer, each way, at two and a half of its largest packets. [Connector](../areas/connector.md) states the rule.

- **One number.** The largest packet and the most in flight both say how much the owner is prepared to lose on a link, so the second follows from the first, and the owner chooses only the packet size.
- **Amounts, not packets,** so packets from parts of the network with a smaller maximum just fit more of them.
- **Half a packet of slack.** A payment's STREAM keeps two of the largest packets in flight, never a third; the half packet left takes smaller ones meanwhile, such as a quote's probes. Without it, a quote made while a payment fills the link would have its probes refused, shrink them to one unit, and fail.
- **Each way, with our own number.** A crash of either side raises what our books say the peer owes us, so each node bounds both ways with its own cap, without knowing anything of the peer's.

## Accepted

- *Less throughput per peer:* two of the largest packets per round trip. A round trip takes well under a second and a settlement on the XRP Ledger several, so the credit and the settlements bound a link long before the cap does.
- *Drift still adds up* over many crashes; the cap bounds what each one costs.

## Rejected

- *A share of the credit we extend:* the credit is asymmetric, and a senior extends none ([D20](D20-trust-follows-what-a-peer-earned-us.md)).
- *A share of the room between the peers:* each node knows only the credit it extends.
- *A share of recent settlements, or the rate times the round trip:* more moving parts than the problem needs for now.
