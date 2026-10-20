# D36. A node becomes core by registering and being promoted

**Areas:** [Network](../areas/network.md), [Peer protocol](../areas/peer-protocol.md), [Periphery](../designs/periphery.md). Not built yet ([roadmap](../roadmap.md#periphery)): on `main`, a node with a hostname is core and one without is periphery ([Network](../areas/network.md#periphery-nodes)).

## Facts

- [D26](D26-core-and-periphery.md) divides the network into a core, which forwards packets and keeps the node list, and a periphery, which reaches the core through uplinks and is paid at child addresses beneath them.
- What makes a node core is, on `main`, what the node says about itself: a URL in its link state. But any node may want a URL, to serve Open Payments say, so a URL says nothing about whether the node can carry other nodes' traffic.
- Being core costs the network something: other nodes route through a core node, extend it trust in the routing graph, and send its link state everywhere. A node that claims it and then drops packets or goes away harms everyone who routed through it.
- Every node already holds the same signed data: the bootstrap nodes' lists say who is registered, and link states say who advertises whom as a neighbor.

## Decision

**Every node starts in the periphery. A node is core when the signed data says so, and nothing it claims about itself makes it core.**

- **The definition.** Each node computes, from what it holds:
  1. the registered nodes, those on the majority of the bootstrap nodes' lists ([Network](../areas/network.md#membership));
  2. the edges between them that both ends advertise, in fresh link states;
  3. the 2-core of that graph: nodes with fewer than two such edges are dropped, again until none is left to drop;
  4. of its connected components, the one with the most bootstrap nodes. Among components with equally many, the one whose lowest bootstrap node ID is lowest.

  The nodes of that component are the core. Bootstrap nodes meet the same rule as everyone: they are core because they peer, not because they are configured.
- **Becoming core** is a process the owner starts:
  1. a public IP address, a domain, and TLS with a certificate a public authority issued;
  2. at least two uplinks, as a periphery node;
  3. registration with the bootstrap nodes, which probe the URL as they do today;
  4. asking each uplink for promotion.
- **Promotion** is an uplink advertising the node as its neighbor in its link state, which it does only once the node:
  - is registered;
  - answers at its URL, over QUIC and over HTTPS with a certificate a public authority issued, checked anew by the uplink;
  - has been its peer in good standing for at least an hour, and answered more than a set share of the heartbeats in that hour;
  - has at least twenty of the largest packets of room with the uplink, as the uplink's own books show it (its position for the node, the only side a node knows, [D23](D23-each-node-books-its-own-fulfills.md));
  - has completed at least one settlement with it;
  - has a clock within bounds, as its link states' sequences show, and is not banned.

  An uplink with no free core relationship refuses. Core nodes advertise how many core relationships they have free, in a link-state entry, so that a node can choose uplinks that will be able to promote it.
- **A peering's kind follows the peer's status.** When a node becomes core in an uplink's view, their downlink peering becomes a core peering in place, with its accounts and settlement channel; when it stops being core, the reverse. The uplink keeps routing the node's child address beneath its own for as long as they are peered, so payment requests handed out under it keep working.
- **The node's own address.** A node uses its own address, `<scheme>.das.<node ID>`, once its links are advertised by both ends in the routing graph and it is core in its own view, and that has held for a set time. A node that stops being core in its own view goes back to its child addresses at once, since payments to its own address would then fail.
- **Demotion.** A node that sees a registered node fail the criteria for long enough removes it from the list it serves, so that a node that went away loses its majority place. An uplink that sees its peer fail them stops advertising it. Either way the node drops out of the core, and its peerings become downlink peerings again.

## Accepted

- **The time to become core** is at least the hour of good standing, the time for registration to reach the majority, and the set wait before the own address. Short-lived nodes never become core, which is the point.
- **Joining or splitting components can tilt a tie** between components with equally many bootstrap nodes; an attacker can do that only with the edges it holds itself.
- **The core is what each node computes,** so for the time a link state takes to spread, two nodes may disagree on whether a third is core. The wait before a node uses its own address covers that, and the child address keeps working meanwhile.
- **Advertising free core relationships** tells everyone how full a node is, which is public anyway from its neighbors.

## Rejected

- *A URL as the mark of a core node* (`main`): any node may serve at a URL, and a URL proves nothing about forwarding.
- *A node declaring itself core* (a link-state entry, a "candidate" state): what a node says about itself is what the definition must not rest on.
- *Bootstrap nodes core by definition:* bootstrap nodes on different networks would each make their own core.
- *The lowest median node ID as the tiebreaker:* anyone can register nodes with chosen IDs into a component and move its median; nobody but the configuration chooses the bootstrap nodes.
- *Proving a second uplink with a zero-amount packet* sent through it: the routing graph already shows both peerings, signed by both ends.
- *Promoting before registering:* a promoted node that is not yet registered would be an equal peer whom nobody else can route to, at its own address, with special cases everywhere.
