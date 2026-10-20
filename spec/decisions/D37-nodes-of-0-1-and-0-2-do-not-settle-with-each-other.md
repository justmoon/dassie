# D37. Nodes of 0.1 and 0.2 do not settle with each other

**Areas:** [Peer protocol](../areas/peer-protocol.md#versions), [Settlement](../areas/settlement.md), [Release](../areas/release.md).

## Facts

- A node of 0.2 settles only when its peer asks, with `settlementRequest` ([D23](D23-each-node-books-its-own-fulfills.md)). A node of 0.1 never asks: it has no such message, refuses it with 400, and settles unasked, by a trigger worked out from the credit its peer announces, which a node of 0.2 sends as 0.
- So on a peering between the two, the node of 0.1 keeps settling and the node of 0.2 never does. Payments from the node of 0.2 stop once its debt reaches the credit the node of 0.1 extends, and an hour later the node of 0.1's grace limit bans it.
- Every node updates on its own daily timer ([Release](../areas/release.md)), so the two versions meet for up to about a day. A node that went back from 0.2, or whose owner stopped the timer, stays on 0.1.
- The network runs on the XRP Ledger testnet: no real money is at risk ([D8](D08-real-money.md)), and what a peer can cost a node is bounded by the credit it extends ([D23](D23-each-node-books-its-own-fulfills.md)).
- The spec's rule for the peer protocol is that a change both sides must agree on is a new protocol version, which older nodes refuse cleanly ([Overview](../overview.md)). Settling on request was added to v1 without one.

## Decision

0.2 is released as it is. While both versions run, payments between a node of 0.1 and a node of 0.2 stall once the node of 0.2 owes the credit it was given, and nodes of 0.1 ban the nodes of 0.2 they peer with. Each node ends the window by updating; updating to 0.2 lifts the bans its grace limit made ([D23](D23-each-node-books-its-own-fulfills.md)). Nothing in 0.2 settles toward, or refuses, nodes of 0.1.

## Accepted

- **A day of stalled payments across versions**, and bans of 0.2 nodes, the bootstrap nodes included, by the nodes of 0.1 they peer with. On the testnet that costs time, not money.
- **Nodes that stay on 0.1 are cut off** from nodes of 0.2 for good. An owner who went back to 0.1 or stopped the updater has chosen an old version; the network does not wait for it.
- **An exception to the version rule.** Before real money, and with every node updating itself, a version that older nodes refuse cleanly costs more than the day it would spare. Once real money runs, a change both sides must agree on is a new protocol version again.

## Rejected

- *Settling unasked toward peers that never ask,* by 0.1's trigger, recognised by a link state without the QUIC entry: legacy code and its tests, kept for a day's window on the testnet.
- *Refusing to peer with nodes of 0.1, and ending peerings with them:* it splits the network in two until every node has updated, which is the same day without the bans, at the cost of more code.
