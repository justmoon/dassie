# The peer protocol and the hubs' messages travel over QUIC only

**Areas:** [Peer protocol](../areas/peer-protocol.md#transport), [Node](../areas/node.md#runtimes), [Network](../areas/network.md#periphery-nodes), [Periphery](../designs/periphery.md#2-transport-quic-on-every-peer-link), [Release](../areas/release.md).

## Facts

- Every node of the network takes QUIC ([core-and-periphery](core-and-periphery.md) chose QUIC for every peer link).
- The alternative is a fallback to HTTPS: `POST /peer` for a peer whose link state does not say it takes QUIC, whose QUIC cannot be reached or has no room for us, kept to for a while after either, and for a bootstrap node whose link state has not arrived; `POST /hub` the same way for a settlement hub's members.
- A core node whose UDP is blocked cannot do what a core node is for even with HTTPS: periphery nodes reach their uplinks only over QUIC, relayed streams run only over QUIC, and a hub's validators reach each other only over QUIC.
- A periphery node is reached only over a connection it opened. Over HTTPS toward an uplink, its own requests get there, but the uplink's requests to it have no way in until a QUIC connection is back.
- A peer that restarts, or moves to another address, cannot be dialled for a moment. A fallback that keeps to HTTPS for a while after a failed dial keeps a periphery node unreachable for that while, long after the peer is back: on the testnet, a market maker's trades through a bootstrap node that restarted failed for ten minutes.
- HTTPS as a second transport is a second door into the peer protocol: its own bounds on connections and requests, its own charges per address (the hub's members proved no key over it), its own rule against sending again what may have arrived over QUIC, and tests of each.

## Decision

**QUIC is the only transport of the peer protocol and of the hubs' messages. A node does not run without its QUIC endpoint.**

- **No `POST /peer`, no `POST /hub`.** HTTPS stays for what is not peer traffic: the wallet, Open Payments, ACME and front doors.
- **A node starts only with its UDP socket for QUIC,** at its HTTPS listener's port number, which its URL names ([Node](../areas/node.md#runtimes)). One that cannot have it there stops with an error, rather than run as a node nobody can reach at its URL. Core nodes take UDP: a node that serves only TCP cannot be core.
- **A request that cannot be sent is not delivered.** A dial that fails, or a connection the other side refuses for want of room, says nothing was sent, as a refused connection always did. The node then backs off from dialling that peer for a while that doubles from a second up to a minute, and stops backing off once the peer keeps a connection: one it opened to us, or one of ours a request got through on. A dial alone does not end it, since a node without room refuses only after the handshake. So a peer that restarts is asked again within a minute, or at once when it dials us first.
- **Bootstrap nodes from the configuration are dialled over QUIC at once,** pinned to the ID the configuration gives: dialling needs only the ID and the URL, not a link state.
- **Nothing says a node takes QUIC.** Every node does, at its URL's host and port, so a link state needs no flag for it, and a periphery node asks any core node to be its uplink.

## Accepted

- **A periphery node behind a network that filters UDP cannot take part** until it can carry its QUIC packets over a WebSocket ([quic-over-websocket](quic-over-websocket.md)). That keeps QUIC's streams and keys; an HTTP request per message does not, and periphery nodes could not be reached over it anyway.
- **A node whose places for anyone else are full takes no newcomers** until one frees: asking to peer, or registering, waits for room rather than coming in through a second door ([places-by-proved-key](places-by-proved-key.md)).
- **No new protocol version.** Every node already takes QUIC; the rule that a change both sides must agree on is a new version ([Overview](../overview.md#frozen-surfaces)) is about messages, and none changes. A node of an earlier release that falls back to HTTPS meets a 404, which its connector leaves to expire, until its daily update.
- **A request in a backoff fails at once,** within a minute of a peer coming back, even if the peer could have been reached. The backoff is short so that this costs little, and it spares a node that is down a handshake per request.

## Rejected

- *Keeping the HTTPS fallback:* it serves no node a core node can peer with usefully, hides a core node's blocked UDP, and keeps a periphery node unreachable for minutes after a peer restarts.
- *Keeping HTTPS only for a node without room for us:* a second pool of places behind a second door, with bounds and charges of its own, for the one case where waiting for room is the honest answer.
- *Keeping HTTPS for hub members only:* members are nodes, and a node runs QUIC; a member that cannot reach a validator over QUIC cannot reach its peers either.
- *A fallback over TCP for core nodes:* the tunnel of [quic-over-websocket](quic-over-websocket.md) is for periphery nodes their network keeps from UDP; a core node chooses its own network.
- *Running without QUIC, refusing only to peer:* a node nobody can reach still costs its peers a dial per request, and its owner learns why only from its log.
