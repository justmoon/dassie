# A node becomes core by registering and being promoted

**Areas:** [Network](../areas/network.md), [Peer protocol](../areas/peer-protocol.md), [Periphery](../designs/periphery.md). Built ([Network](../areas/network.md#the-core)), but for demotion by an uplink once the peer it promoted is core in its view ([roadmap](../roadmap.md#periphery)): until then, the checks bind a peer only while it is a downlink.

## Facts

- [core-and-periphery](core-and-periphery.md) divides the network into a core, which forwards packets and keeps the node list, and a periphery, which reaches the core through uplinks and is paid at child addresses beneath them.
- What makes a node core is, on `main`, what the node says about itself: a URL in its link state. But any node may want a URL, to serve Open Payments say, so a URL says nothing about whether the node can carry other nodes' traffic.
- Being core costs the network something: other nodes route through a core node, extend it trust in the routing graph, and send its link state everywhere. A node that claims it and then drops packets or goes away harms everyone who routed through it.
- Every node already holds the same signed data: the bootstrap nodes' lists say who is registered, and link states say who advertises whom as a neighbor.

## Decision

**Every node starts in the periphery. A node is core when the signed data says so, and nothing it claims about itself makes it core.**

- **The definition.** Each node computes, from what it holds:
  1. the registered nodes, those on the majority of the bootstrap nodes' lists ([Network](../areas/network.md#membership));
  2. the edges between them that both ends advertise, in fresh link states;
  3. the 2-core of that graph: nodes with fewer than two such edges are dropped, again until none is left to drop;
  4. of its connected components, the one with the most bootstrap nodes. Among components with equally many, the one whose lowest bootstrap node ID is lowest;
  5. the core grown from that component's bootstrap nodes: a registered node with two edges to nodes already in the core joins it, again until none joins. A lone bootstrap node, the only one whose link state is fresh, first admits the nodes it advertises itself, since with one node in the core none could have two edges into it.

  Bootstrap nodes meet the same rule as everyone: they seed the core because they peer, not because they are configured. Every other node is core only through two edges into the core, so through two core nodes that promoted it: a group of registered nodes that advertise each other cannot join on the strength of one honest edge, however many of them there are. A network with fewer than three bootstrap nodes, as small dev and test networks have, asks for fewer edges, in the 2-core and to join: as many as it has bootstrap nodes less one, but at least one to join, so one bootstrap node is a core alone, two are one together, and a node with an edge to either joins them.
- **A node alone.** A node with no bootstrap nodes counts as its own bootstrap node, registered with itself, and so is a core alone. Refusing to start without bootstrap nodes would be cleaner, but a node has no ID until it is set up, so it cannot name itself in its configuration beforehand, and running alone is what tests and single-node dev setups do.
- **The dev core.** Dev and test networks name nodes that are core outright (`DASSIE_DEV_CORE_NODES`), whatever their edges, which only a build with debug assertions takes. So a dev network has core nodes from the start without the time promotion takes, and without making them bootstrap nodes, which would also make them registrars and list servers. Promotion keeps tests of its own.
- **The first core.** Bootstrap nodes peer with each other as core peers and advertise each other at once, without the checks below: the configured list is already the root every node trusts, for the node lists. So a fresh network's bootstrap nodes form its first core, and form it again if it ever empties. Every other node becomes core only through promotion.
- **What a node publishes, and how it is told apart.** Nothing in a link state says core or periphery: a peering's kind follows the peer's status as the definition computes it.
  - A node without a URL publishes an empty URL with the periphery extension, which now means only that: it has no URL. It cannot register, and its link state does not flood, as on `main`.
  - A node with a URL publishes an ordinary link state, core or not. It registers, and once registered its link state floods as every registered node's does.
  - The rules for which link states verify and which registrations are taken do not change, so the network does not split over them.
  - What changes is the kind of a peering: on `main` a peer whose link state has a URL is a core peer; under [promotion-to-core](promotion-to-core.md) a peer that is not core is a downlink, whatever its link state carries. An uplink reaches a downlink only over the connection the downlink opened, never at its URL, except to probe it for promotion. A downlink's channel is a downlink's channel by its status, never by its URL.
  - Home nodes ([design §5](../designs/periphery.md#5-reaching-a-periphery-node)) are drawn from the core, not from the majority list, which now also holds registered nodes that are not core.
- **Becoming core** is a process the owner starts:
  1. a public IP address, a domain, and TLS with a certificate a public authority issued;
  2. uplinks, as a periphery node: as many as the definition asks edges of, two in a network with three or more bootstrap nodes;
  3. registration with the bootstrap nodes, which probe the URL over QUIC and check it over HTTPS;
  4. asking each uplink for promotion.
- **Promotion** is an uplink advertising the node as its neighbor in its link state, which it does only once the node:
  - is registered;
  - answers at its URL, over QUIC and over HTTPS with a certificate a public authority issued, checked anew by the uplink;
  - has been its peer in good standing for at least an hour, and answered more than a set share of the heartbeats in that hour;
  - has at least twenty of the largest packets of room with the uplink, as the uplink's own books show it (its position for the node, the only side a node knows, [own-fulfills](own-fulfills.md));
  - has completed at least one settlement with it;
  - has a clock within bounds, as its link states' sequences show, and is not banned.

  An uplink with no free core relationship refuses. Core nodes advertise how many core relationships they have free, in a link-state entry, so that a node can choose uplinks that will be able to promote it.
- **A peering's kind follows the peer's status.** When a node becomes core in an uplink's view, their downlink peering becomes a core peering in place, with its accounts and settlement channel; when it stops being core, the reverse. The uplink keeps routing the node's child address beneath its own for as long as they are peered, so payment requests handed out under it keep working.
- **The node's own address.** A node uses its own address, `<scheme>.das.<node ID>`, once its links are advertised by both ends in the routing graph and it is core in its own view, and that has held for a set time. A node that stops being core in its own view goes back to its child addresses at once, since payments to its own address would then fail.
- **Demotion.** A bootstrap node that sees a registered node's URL or certificate fail for long enough, longer than becoming core takes, removes it from the list it serves, so that a node that went away loses its majority place. An uplink that sees its peer fail the promotion criteria stops advertising it. Either way the node drops out of the core, and its peerings become downlink peerings again.

## Accepted

- **The time to become core** is at least the hour of good standing, the time for registration to reach the majority, and the set wait before the own address. Short-lived nodes never become core through an honest uplink, which is the point.
- **Two core nodes vouch for whomever they both advertise.** The checks bind only honest uplinks: an owner with two core nodes can have both advertise registered nodes it controls, which then join the core without anyone else's checks. What that costs is two promoted core nodes first, and a registration for each node added, under registration's throttles today and its fee later. Each such node still has to answer at its URL and stay registered. An owner with one core node adds nobody by it alone.
- **The core rests on its bootstrap nodes being up.** Bootstrap nodes are the most trusted nodes, run by the network's operators: most of them down at once is a failure the core is not built for. A lone bootstrap node whose link state is still fresh admits the nodes it advertises, so that tests and simulations that stop bootstrap nodes keep a core; past that, while most are down, the core may shrink to them. The nodes beyond them keep forwarding over the edges they list among themselves, which is how the others route. A node whose edges all went to bootstrap nodes has none left to forward over, and a node with no edge forwards no peer's packets; but it keeps its downlinks while the peers it lost only went silent, still listing it, and they pay each other through it, beneath its address. Each is paid beneath its peers meanwhile, at child addresses that keep working, and is back in the core once the bootstrap nodes are. It takes no new downlink until then.
- **Seeds are core nodes in their own view from the start.** A bootstrap node, or a node on the dev core list, takes downlinks and uses its own address whatever the core it computes, so that a fresh network has core nodes to peer with before any edge exists. Other nodes count it core only by its edges, as the definition says, so this makes it core for nobody else.
- **A node forwards while others route through it.** Routing takes every edge both ends list, core or not, so a node with a URL forwards its peers' packets, and keeps its downlinks, while it has such an edge, whether or not it counts itself core: one promoted that does not count itself core yet, or a core node out of the core for a moment. Otherwise a sender's route through it would end in a final refusal. Being core decides the rest: whom it takes as a new downlink, and its own address.
- **A node's own status settles before it acts on it.** It uses its own address only once it has been core for a set time, and ends its downlinks only once it has had no edge as long, but for edges whose peers only went silent: views differ while link states spread, and a peer that restarts lists its neighbors again only once they confirm themselves. A peering that was core is listed by its health through such a dip, while the peer stays registered, and keeps that across a restart of either end, so a core node is back after its peers' hold-down, not after a new promotion.
- **Leaving the core cascades where the graph is sparse.** A node with just two edges into the core leaves it when one of those goes, and so do the nodes that joined through it. Core nodes keep more core peers than two, so this touches the edge of the core, and each node it touches is back once its edges are.
- **Bootstrap nodes trust each other without the checks.** The configured list is the root of trust already; a bootstrap node that misbehaves is taken off the list.
- **Joining or splitting components can tilt a tie** between components with equally many bootstrap nodes; an attacker can do that only with the edges it holds itself.
- **The core is what each node computes,** so for the time a link state takes to spread, two nodes may disagree on whether a third is core. The wait before a node uses its own address covers that, and the child address keeps working meanwhile.
- **Advertising free core relationships** tells everyone how full a node is, which is public anyway from its neighbors.

## Rejected

- *A URL as the mark of a core node* (`main`): any node may serve at a URL, and a URL proves nothing about forwarding.
- *A node declaring itself core* (a link-state entry, a "candidate" state): what a node says about itself is what the definition must not rest on.
- *Bootstrap nodes core by definition:* bootstrap nodes on different networks would each make their own core. They advertise each other without checks, but are core only through the edges, like everyone.
- *A link-state entry that says core or periphery,* or a "candidate" state for a node with a URL that is not core yet: what a node says about itself is what the definition must not rest on, and a new verification rule would split the network while nodes disagree on it.
- *The 2-core alone, without growing it from the bootstrap nodes:* a group of the attacker's own registered nodes that advertise each other in a cycle, one of them promoted by one honest uplink, would all be in the 2-core and in the bootstrap nodes' component, so in the core, at the cost of one honest promotion rather than two for each.
- *The lowest median node ID as the tiebreaker:* anyone can register nodes with chosen IDs into a component and move its median; nobody but the configuration chooses the bootstrap nodes.
- *Proving a second uplink with a zero-amount packet* sent through it: the routing graph already shows both peerings, signed by both ends.
- *Making a dev network's core nodes bootstrap nodes* rather than naming them in a dev core list: a bootstrap node also takes registrations, serves a list and keeps names, so it would change more of the network under test than who is core.
- *Promoting before registering:* a promoted node that is not yet registered would be an equal peer whom nobody else can route to, at its own address, with special cases everywhere.
