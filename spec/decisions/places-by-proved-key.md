# A connection holds a place by the key it proved

**Areas:** [Peer protocol](../areas/peer-protocol.md#quic), [Network](../areas/network.md#downlinks), [Periphery](../designs/periphery.md#4-downlinks-on-a-core-node).

## Facts

- Every peer link runs over the node's one QUIC endpoint ([core-and-periphery](core-and-periphery.md)), and so do relayed streams. The connections it takes come from core peers, from downlinks, and from everyone else: clients of relayed streams, nodes that chose the node as their relay, registrants, nodes asking to peer, tools.
- The endpoint bounded them in one pool, in all and per client address, as the HTTPS listener bounds its connections. A core node takes up to `max_downlinks` downlinks, nearly as many as that pool has places, and each keeps a connection open for good, since it is reached only over a connection it opened. A busy uplink's downlinks would leave little room for anyone else.
- Core peers are few, and their links carry the network: a core node that cannot keep them stops forwarding for everyone. Relayed streams are how clients reach a periphery node at all.
- An endpoint learns who opened a connection only once the handshake has proved the other side's node key; before that, it knows only an address. One address can carry many nodes (a NAT), and one node many addresses.
- A key costs nothing. Our own peerings do not: the peer node bounds the core peers and the downlinks it keeps (`max_peers`, `max_downlinks`).

## Decision

**A connection holds a place in its class: core peer, downlink or anyone else. The class comes from the key the connection proved and from our own peerings with that key, never from what the node says.**

- **While it is set up,** a connection holds a place among those being set up, in all and per client address, for at most the time to connect. Once those run short, a client must show it receives at its address first, so spoofed handshakes hold nothing.
- **Once its key is proven,** it moves to its class's places. Anyone else's are bounded in all and per client address, as before. Core peers' and downlinks' are bounded per node: room for as many nodes as the peer node's own bound allows, and a small share of connections for each.
- **The class follows the peerings.** When a peering begins or ends, its node's connections move to their new class, or close if it is full. A node asking to peer holds anyone's place until its peering begins.
- **A node past its share replaces its own oldest connection,** by when each was made. The connection a node made last is the one it uses, and the oldest is the likeliest to be gone without our knowing; no other node's place is touched.
- **No room is said with a close code of its own.** A connection refused once its key is known closes before any stream on it is read, with a code that tells the client nothing it sent was delivered. The client then does what it does when a connection is refused outright: it asks over HTTPS, and keeps to HTTPS for a while.

So downlinks never take the room of core peers or of anyone else, and nobody takes the core peers' room. Downlinks have room for every downlink the peer node keeps.

## Accepted

- **A connection is refused only after its handshake** once its class is full: until then nobody knows its class. A connection refused before its handshake costs the node nothing but a packet; one refused after it costs a handshake, as any QUIC server's clients do. The bounds on connections being set up, and address validation, bound how many at once, and a client told there is no room keeps to HTTPS for a while rather than coming back over QUIC.
- **The endpoint holds more connections than before.** The places for connections being set up are a pool as large as anyone else's, beside it, so strangers, and the nodes a front door serves, can hold up to twice as many connections as with one pool, half of them only for the time to connect. The places for core peers and downlinks come on top: memory grows with `max_downlinks`, which the peer node chose to keep anyway.
- **A downlink whose peering ends while anyone else's places are full loses its connection,** and comes back once there is room. A newcomer that finds anyone else's places full asks for its peering over HTTPS, as it would of a node that refused its connection outright.
- **Two peerings that change at once can cross:** when one node's peering ends and another's begins while every node of the class holds its full share, the newcomer's connection can move before the leaver's is gone, find no room and close. It dials again, and finds the room the leaver left.
- **A node may open connections under other keys,** as anyone may, and send its own peer messages over them, since messages are authenticated by their own MACs, not by the connection. Those connections are anyone else's, bounded as a stranger's are, and the node is still reached only on a connection under its own key.
- **The connection replaced may be one still in use,** such as a downlink's connection to a chosen relay that is also its uplink. The node then opens it again, once.

## Rejected

- *One shared pool, larger:* downlinks still take the room of everyone else, and nothing keeps room for core peers.
- *A reserve for core peers within one pool:* a reserve can be held only by connections whose class is known, and until the handshake none is; the rest of the pool is still the downlinks' to fill, at the expense of relayed streams.
- *Counting a connection by its address alone:* an address says nothing of who opened it. A NAT carries many downlinks, and a host can open connections from many addresses.
- *Taking the class from what a node says,* its link state's periphery extension or the sender of its messages: a node could then claim whichever class has room.
- *Counting a connection whose class is not known among the downlinks* until it proves otherwise: anyone with a key would take the downlinks' room.
- *Refusing a node's connection past its share:* a node that opened a new connection while our side still held its old ones would be refused until they timed out, and could not be reached in the meantime.
- *Closing a connection without room as any other:* its client could not tell that nothing it sent was read, so it would take the request as possibly delivered, never ask over HTTPS, and come back over QUIC each time, costing a handshake.
