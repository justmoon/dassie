# Core and periphery

**Areas:** [Periphery](../designs/periphery.md), [Network](../areas/network.md), [Peer protocol](../areas/peer-protocol.md), [Settlement](../areas/settlement.md), [Payments](../areas/payments.md). A design, partly built: periphery nodes and their downlink peerings ([Network](../areas/network.md#periphery-nodes)), and a node's names and signed responses ([Payments](../areas/payments.md#the-nodes-names)); the rest is on the [roadmap](../roadmap.md#periphery).

## Facts

- A node needs a public IP address and a certificate for its hostname: peers reach it at the URL in its link state, and so do Open Payments clients. Most computers people own have neither.
- Peer traffic is heavy. A busy core node forwards a great many packets, so no service can carry peer traffic for other nodes.
- The owner enters the passphrase into the wallet's page to log in, so the wallet cannot be served by anyone but the node.
- A node keeps every peer in its node table, in memory, with its link state, and a settlement channel for every node it ever peered with, under one limit.

## Decision

The network has a core and a periphery. [The design](../designs/periphery.md) describes them:

- **The core** is the network as it is: nodes with a public IP that register, keep the node list and forward packets. What makes a node core, and how it becomes one, is [promotion-to-core](promotion-to-core.md).
- **A periphery node** connects only to core nodes (its uplinks), never to another periphery node, and never forwards. It registers only to become core, and its link state floods only once it is registered ([promotion-to-core](promotion-to-core.md)). Each uplink gives it a child address beneath its own and routes that address to it. It always dials out, so it needs no public IP and no hole punching.
- **QUIC on every peer link,** with each side's node key in a self-signed certificate checked against its node ID. Either side opens streams, so an uplink sends over the connection its downlink opened. A periphery node falls back to TCP or WebSocket where UDP is blocked.
- **Separate limits on a core node.** Relationships with core nodes are few, with settled ones closed to make room. Downlinks have a table of their own, kept on disk and in memory only while connected, under a much larger limit.
- **Reaching a periphery node** goes through relays: its uplinks, and optionally a relay it chooses, which is its fast path. Anyone finds its uplinks and relays in a signed record, kept by home nodes that everyone computes from the core ([promotion-to-core](promotion-to-core.md)).
- **The wallet is never served through a relay.** Owners use `localhost`, or apps paired on `localhost` with device keys, which reach the node through a relay, end to end, with its key pinned.
- **Front doors** take Open Payments requests for periphery nodes at names derived from the node ID, or at an owner's own domain, and pass them through a relay. The node signs its responses with its node key, and Dassie-aware clients check that key against the node ID in the name. A periphery node answers only to the names its owner lists, under the name each request came through. Additions to Open Payments that let every client check are to be proposed to the Interledger Foundation.
- **Free receivers** receive without paying anything up front: a periphery node extends its uplink credit and pays nothing in advance, spends what it received through the same uplink, and the uplink's first settlement creates its account on the ledger.

## Accepted

- **Clients that do not check signatures trust the front door,** including with the STREAM secret of an incoming payment. The owner is told, and can turn off Open Payments through front doors.
- **A free receiver trusts its uplink** with up to the credit it extends. An uplink that evicts it collects first and pays nothing ([ending-a-peering](ending-a-peering.md)), so the receiver stays owed until it peers again, and loses at most that credit.
- **Anyone who grinds enough core node keys can hide a node's record,** within registration's limits and, later, its fee. A relay the node chose still reaches it.
- **Front door names share a reputation.** They are not on the Public Suffix List, which matters only for names that serve pages with scripts or cookies; a front door serves no page whose content a node controls, and sets no cookies.
- **A periphery node is reachable only through others.** If its uplinks and relays are gone, nobody can pay it until it connects again.

## Rejected

- *A tunnel service that carries peer traffic:* peer traffic is too heavy to relay.
- *Passing TLS through to each node, with a certificate per node,* so that a tunnel could not read it: the tunnel controls the DNS and can have certificates issued for any of its names, and browsers could then be served any page from its domain.
- *Hole punching between periphery nodes:* periphery nodes connect only to the core.
- *Serving the wallet through a relay:* the page could keep the owner's passphrase, which cannot be revoked.
- *Periphery nodes in the node list,* or carried in their uplinks' link states: every core node would hold every periphery node, while child addresses already route them.
- *Answering to any name a front door passes on:* the front door would decide which names the node signs for and counts as its own, and could hand it requests signed for another server.
- *A periphery node reachable only through one relay of its choice:* only that relay could reach it.
- *Uplinks that follow from the node ID,* so that no record is needed: a periphery node could no longer choose its uplinks by price and trust.
- *Sending every record to every core node:* each would hold every periphery node, and every reconnection would flood.
- *A smaller table for the debts of evicted free receivers:* a downlink's row is already small, and only connected downlinks are held in memory.
- *Paying a free receiver's reserve from the uplink:* every new identity would cost the uplink a reserve.
- *A payment channel from the uplink to each free receiver:* it locks up the uplink's funds for every receiver.
