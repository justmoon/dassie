# A connection is adopted once verified, and the one it replaces closes

**Areas:** [Peer protocol](../areas/peer-protocol.md#quic), [Node](../areas/node.md#a-chosen-relay), [Network](../areas/network.md#registration).

## Facts

- Two nodes use one QUIC connection between them, both ways ([Peer protocol](../areas/peer-protocol.md#quic)). A second one comes up whenever a node dials while the other holds one: both dial at once, one restarts or loses its path and dials again, a node keeps a connection to its chosen relay beside a peering, or a node checks that another answers at its URL, which only a dial of the URL shows ([promotion-to-core](promotion-to-core.md)).
- A connection whose handshake completed has carried packets both ways once, not a request and its answer: a path can let the handshake through and still fail what follows.
- Each node decides on its own which connection it sends on. If the two decide differently, one sends on a connection the other closes, and requests on it fail.

## Decision

**A new connection to a node is adopted only once it is verified, and then one of the two survives, the same on both sides; the other closes. No connection is kept in case another fails.**

- **Verified** is a request answered over it: one we sent and the node answered, for a connection we dialled, or one the node sent and we answered, for one it opened. A dial that fails, or whose request goes unanswered, changes nothing: the connection held stays.
- **The first connection** to a node, with none held, is adopted at once: there is nothing to replace, and nothing to keep. So is one that waits for its verification once the one held closes.
- **Which survives,** of the one held and one verified: the one the lower node ID opened; of two the same node opened, the newer. Both nodes know who opened each and which came later on their side, so both judge alike and keep the same one. The one held survives only if it still answers a request: a connection whose other end crashed and came back reads as open until something is sent on it. The rule is judged under one lock, and again if what is held changed while it was probed.
- **The other closes** once the requests already on it are done, at most a peer request's time later (`DRAIN` in `crates/dassie-node/src/quic/mod.rs`): what the other node sent on it before it switched still finishes.
- **A connection kept open for its own sake,** a node's to its chosen relay, follows the same rule: it is verified by a request of its own, the relay's link state, which the relay answers and so adopts it too, and, the newer of two the node opened, survives. If it loses to one the relay opened, the relay reaches the node over that one: the node's closes, and the node counts its relay as held while that one lasts, and dials again once it closes.

## Accepted

- **A connection that dies after it was adopted is not replaced by one kept aside.** The nodes dial again, as they would after any loss, and requests in flight on it fail. Keeping a second connection open in case was what allowed two nodes to send on different ones.
- **A brief gap for a relay.** A node whose relay connection loses its place to a newer one it opened, and which then idles out, is out of its relay's reach until it dials again, within its relay's backoff.
- **A failed request on a new connection whose handshake succeeded** can leave the other node having switched to it already, if it answered and its answer was lost: it falls back to dialling once the connection closes.
- **A held connection that is slow to answer** its probe, past the time to connect, gives up its place to the verified newcomer on our side while the other node, whose probe it answered, may keep it: both drain what is on it, and the nodes dial again if they end on different ones. A connection that cannot answer one request in that time is failing its requests already.
- **While nodes of the release before run,** they keep the older of two connections the lower node ID opened, and this release the newer, so a second connection of that kind, a node's to its relay that is also its uplink, or a check of a URL, closes one the older node still sends on, and its requests in flight on it fail once. All nodes update together while there are no others' ([promotion-to-core](promotion-to-core.md)), so that window is the time an update takes.

## Rejected

- *A connection that stands by* and takes over once the one used closes: two connections open where one does, and a node that sends on the one the other keeps aside.
- *Adopting a connection at its handshake:* a handshake proves less than a request answered, and a node would switch to a path that then fails its requests.
- *A marker that tells a check's connection apart,* so that the other node never adopts it (a protocol version of its own in the ALPN): a new name on the surface that versions the protocol, which nodes of the release before do not know, for a case the rule settles anyway.
- *The newest verified always wins:* two nodes that dial at once each verify their own first and keep different ones.
