# Money in flight with a peer stays within a few packets

**Areas:** [Connector](../areas/connector.md), [Settlement](../areas/settlement.md)

## Facts

A node that crashes with Prepares in flight never learns that its downstream fulfilled them, so each one drifts the two peers' books apart by its amount ([own-fulfills](own-fulfills.md)). STREAM limits packets in flight per connection, not per peer, so what a node had in flight over one peer was bounded only by the credit. In the simulation, a senior that crashed while its owner's payments were running had most of its junior's credit in flight; asked, the junior settled less than its minimum while the senior's books left it less than a packet of room, and the link stalled for good.

## Decision

A node caps the money in flight with each peer, each way, at two and a half of its largest packets.

- **One number.** The largest packet and the most in flight both say how much the owner is prepared to lose on a link, so the second follows from the first, and the owner chooses only the packet size.
- **Amounts, not packets,** so packets from parts of the network with a smaller maximum just fit more of them.
- **Half a packet of slack.** A payment keeps two of the largest packets in flight, never a third; the half packet left takes smaller ones meanwhile, such as a quote's probes, which would otherwise be refused, shrink to one unit and fail.
- **Each way, with our own number.** A crash of either side raises what our books say the peer owes us, so each node bounds both ways with its own cap, without knowing the peer's.

## Accepted

- *Less throughput per peer:* two of the largest packets per round trip. The credit and the settlements bound a link long before the cap does.
- *Drift still adds up* over many crashes; the cap bounds what each one costs.

## Rejected

- *A share of the credit we extend:* the credit is asymmetric, and a senior extends none ([earned-trust](earned-trust.md)).
- *A share of the room between the peers:* each node knows only the credit it extends.
- *A share of recent settlements, or the rate times the round trip:* more moving parts than the problem needs.
