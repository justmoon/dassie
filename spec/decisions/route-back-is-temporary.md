# A route that leads only back to the packet's source answers T01, not F02

**Areas:** [Connector](../areas/connector.md#routing), [Network](../areas/network.md#routing).

## Facts

- Routing runs on link states, and each node recomputes its routes on its own debounce when the graph may have changed ([Network](../areas/network.md#routing)). After a new peering, the nodes around it hold both ends' link states at about the same moment, but each routes by them only at its next computation.
- With the same link states, two nodes' shortest paths never point at each other: if a node's shortest path to a destination goes through its peer, the peer is one hop closer, and none of its own shortest paths goes back through the node. A packet that arrives from the only first hop our route names therefore means the two nodes route by different views, one of them not recomputed yet. The next computation ends it.
- The connector never sends a packet back to its source. With no other option on the route, it had nothing to send the packet to, and answered F02.
- F02 is final. `ilp-protocol-stream` closes its connection on a final error from a node on the path; Dassie's own sender sends the money again ([path-failures-retried](path-failures-retried.md)). Each packet's next hop is drawn among the route's first hops ([weighted-next-hops](weighted-next-hops.md)), so a node that recomputed first sends packets through a new edge at once, before the far end of it does.
- The end-to-end test that pays across six nodes with `ilp-protocol-stream` saw this: a node that had just peered with the payee held the payee's link state listing it, but its routes still led to the payee through the sender, which had recomputed a few milliseconds earlier and drawn the new edge. The node answered F02, and the sender closed the payment.

## Decision

- **A route whose only options are the packet's source answers T01,** temporary: the sender backs off and tries again, and its next packet is drawn anew, through another hop or through ours once our routes caught up.
- **Only that case changes.** A route whose other options cannot be used now answers as before (T04 for no method available), and a destination without a route answers F02.
- **T01, not T04:** nothing is short of liquidity. T01 says that this connector has a route but cannot reach the next hop on it, which is the case: the next hop is the source.

## Accepted

- **A disagreement that lasts,** such as a link state that reached one node and not the other, refuses temporarily until a refresh or a sweep brings the missing state, and senders keep trying meanwhile rather than failing at once. They stop by their own limits, as for any temporary error.
- **A packet waits for a back-off** where sending it straight to a new peer would have delivered it.

## Rejected

- *A route to each peer of ours, straight to it, before the routes confirm the edge:* only the case where the destination is our own peer, not one beyond it; it breaks the rule that an edge counts once both ends list it, and a peer we withdrew would still draw packets.
- *Recomputing at once on a change next to us, past the debounce:* narrows the window but never closes it, since the other node may still compute first.
- *Sending the packet back:* a loop between the two, until it expires.
- *Having the test wait until peerings stop changing:* peerings change in a running network too, and a sender there meets the same F02.
