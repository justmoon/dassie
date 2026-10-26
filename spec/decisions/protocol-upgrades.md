# A peer-protocol version is an ALPN name, and a release speaks the one before too

**Areas:** [Peer protocol](../areas/peer-protocol.md#versions), [Overview](../overview.md#frozen-surfaces), [Release](../areas/release.md)

## Facts

- A change to the peer protocol that both sides must agree on is a new version ([Overview](../overview.md#frozen-surfaces)). Some changes are not: a new message tag, a new refusal reason or a new link-state extension is read as unknown by a node that does not know it ([Peer protocol](../areas/peer-protocol.md#versions)).
- The peer protocol travels over QUIC only ([quic-only](quic-only.md)), whose TLS handshake carries ALPN: the client offers a list of protocol names, and the server picks one of them or fails the handshake. QUIC allows no connection without a name both share (RFC 9001). rustls, on the server, picks the first name of its own list that the client offered.
- A version named twice, by the connection's ALPN and by a byte in every message, is two knobs that can disagree, and the byte costs every message while saying nothing the connection did not.
- Nodes update themselves daily, at times spread over the day, and a node that is down or whose update fails runs the release before for longer ([Release](../areas/release.md)). A node that speaks one version only cannot talk to a node of the release before it, so each new version would split the network while it updates, peerings and payments across the split included.
- Nodes of the releases before version 2 offer `dassie/1` alone, and carry a version byte in every message as well.

## Decision

**The version is the connection's ALPN, offered and accepted as a list.**

- **`dassie/N` names the layouts of every message on the connection.** No message carries a version: a request is its content. A change to any layout pinned by the golden vectors is a new name.
- **A node offers and accepts the versions it speaks, newest first,** and the two speak the newest they share: the server's first that the client offered. A node sharing none fails the handshake, before a message is read, so no node ever takes another's bytes for its own layouts.
- **N and N-1.** A release that brings version N speaks N-1 too, for one release cycle, so that a node updated and one not yet updated still talk while the network updates. The release after drops N-1. While both are spoken, every message is read and written by the version of its connection.
- **Version 2 is spoken alone.** `dassie/1` is not kept as its N-1: those releases name their version in a byte of every message as well, and differ from version 2 in more than its layouts, in how a sender is proved above all ([peer-protocol-v1](peer-protocol-v1.md)). Their nodes fail the handshake with version 2's, and the testnet moves to version 2 together.

## Accepted

- **Two sets of layouts in the code** for a release cycle at each new version, and tests of both.
- **A node that misses a whole release cycle** of updates cannot talk to updated nodes until it updates. The updater runs daily, so that takes a node that stayed down or failed to update for a cycle.
- **What a version adds waits for a peer that speaks it:** a node speaks N-1 to a node of the release before, and so has what N added only with nodes that updated too.

## Rejected

- *A version byte in each message, beside one ALPN name:* the connection already says what it carries, a node learns of a mismatch only message by message after the handshake, and the two can disagree.
- *One version at a time:* each new version splits the network for as long as the update takes to reach every node, a day at the least.
- *Speaking every version since the first:* the code and its tests grow without bound for nodes that no longer exist, against [tigerstyle](tigerstyle.md); a node that stays a release cycle behind is rare, and catches up by updating.
- *Negotiating a version by message, after the handshake:* a round trip on every connection, and a message that every version must read alike, for what ALPN does in the handshake already.
