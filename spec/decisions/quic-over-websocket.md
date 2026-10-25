# Where UDP is blocked, a periphery node carries its QUIC packets over a WebSocket

**Areas:** [Peer protocol](../areas/peer-protocol.md#transport), [HTTP](../areas/http.md#listeners), [Network](../areas/network.md#periphery-nodes), [Periphery](../designs/periphery.md#2-transport-quic-on-every-peer-link). Not built yet ([roadmap](../roadmap.md#periphery)).

## Facts

- Some residential and corporate networks block UDP, or only QUIC on UDP 443. They pass HTTPS, and most pass WebSockets over it, since web apps need them. Some intercept TLS, or let nothing out but through an HTTP proxy; those pass a WebSocket and stop a TLS connection whose ALPN they do not know.
- Every peer link, and every hub message, travels over QUIC ([quic-only](quic-only.md)), so a node behind such a network reaches neither its uplinks, nor its chosen relay, nor a hub's validators.
- A periphery node is reached only over the connections it opens ([core-and-periphery](core-and-periphery.md)). A fallback that carries only its own requests, as `POST /peer` did, leaves its uplinks no way to reach it.
- Everything above a connection works on QUIC's connections and streams: peer requests, one connection per pair of nodes with the other standing by, places by proved key and the `NO_ROOM` close ([places-by-proved-key](places-by-proved-key.md)), relayed streams, the hub's stream kinds and paired owner apps. QUIC runs over any path that carries datagrams, and quinn takes a socket of our own.
- Every core node already runs an HTTPS listener at its URL's host and port, over TCP, which upgrades `/btp` and `/rpc` to WebSockets.
- QUIC's handshake pins the other side's node key, so it needs nothing from the path beneath it to know whom it reached.

## Decision

**A periphery node whose QUIC cannot get through over UDP carries the same QUIC packets over a WebSocket, one packet per binary message, at `/quic` on the core node's HTTPS listener.** The connection inside is an ordinary QUIC connection of the node's endpoint, with the same handshake, streams and places.

- **Every core node serves the tunnel,** at its URL's host and port over TCP. So a periphery node never has to ask which uplinks take it, and no link state says so: the URL already names where. The `dassiepay.net` front door serves it too, so that the default relay reaches nodes behind such networks ([default-relay](default-relay.md)).
- **Only a node without a URL falls back.** A node with a URL is, or is to be, core, and a core node takes UDP ([quic-only](quic-only.md)): a fallback would only hide that its UDP is blocked.
- **The tunnel races UDP with a head start.** A periphery node dials over UDP, and dials the tunnel too once the handshake has not completed within the head start. The first connection to prove the key it asked for is kept, and the other is dropped. A dial fails, and counts toward the backoff, only when both fail. This holds for every dial: its uplinks, its chosen relay, bootstrap nodes and hub validators.
- **Nothing remembers that UDP was blocked.** A tunnel lasts as long as its TCP connection; when that ends, as it does when the node moves to another network, the next dial tries UDP first again.
- **The outer TLS is not checked.** The client names the URL's host, and takes whatever certificate it gets: the QUIC handshake inside proves the node key, so whoever holds the outer TLS sees only QUIC's ciphertext. A core node with a self-signed certificate is reached, and so is one behind a proxy that intercepts TLS.
- **Tunnels have bounds of their own.** Once upgraded, a tunnel's TCP connection leaves the HTTPS listener's places and holds one of the tunnels', in all and per client address, so tunnels and HTTPS never take each other's room. The QUIC connection inside then holds its place by the key it proves, as over UDP. Its address is the TCP connection's, which TCP has already shown receives there. A tunnel that ends closes its QUIC connection at once.

## Accepted

- **A tunnelled link pays twice.** It is encrypted twice, and QUIC's congestion control runs over TCP's: a TCP stall makes QUIC send again what TCP still holds. Only links behind such networks pay it, and they carry one node's traffic.
- **A dial through such a network waits out the head start** before it tries the tunnel, every time. Connections are kept open, so dials are rare.
- **A core node holds a TCP connection, TLS and WebSocket for each tunnelled downlink,** beside its QUIC connection. They are bounded as above.
- **No new protocol version.** No message changes. A node of an earlier release answers `/quic` with 404, which its client takes as a failed dial, until its daily update.

## Rejected

- *A stream multiplexer over TLS* (each stream framed by length, as the periphery design first had, or yamux): a second transport under everything QUIC carries, with its own streams, half-closes, resets, close codes and flow control, and each stream kind tested over both. yamux has no close codes, which `NO_ROOM` needs.
- *TLS over TCP with ALPN `dassie/1` on port 443, without HTTP:* a round trip less per dial and a few bytes less per packet, but a proxy that intercepts TLS, or only speaks HTTP, stops it on exactly the networks the fallback is for.
- *HTTPS requests per message, as `POST /peer` was:* a periphery node could not be reached over it ([quic-only](quic-only.md)).
- *Remembering that UDP is blocked,* and dialling the tunnel first: it keeps a node on TCP after it moves to a network that passes UDP, to save a head start on rare dials.
- *Tunnels for core nodes:* a core node chooses its network, and its peers, its downlinks and the hubs' validators reach it over UDP ([quic-only](quic-only.md)).
- *A link-state entry saying a core node takes the tunnel:* every core node does, at the URL it already gives.
