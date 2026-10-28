# Peer protocol: v0 or v1

**Areas:** [Peer protocol](../areas/peer-protocol.md), [Identity](../areas/identity.md)

## Facts

- The original's peer protocol (v0) does not authenticate senders: any peered node can forge `interledgerPacket` and `settlement` messages from another. Peering requests are not checked against their sender, and the HTTP status means nothing.
- The peer protocol travels over QUIC only ([quic-only](quic-only.md)). Its TLS 1.3 handshake proves both node keys: a dialler pins the server to the node it asked for, and a server takes only a client that proves a key, with no 0-RTT. Peer requests are never relayed: a relayed stream carries only payments and owner apps ([Peer protocol](../areas/peer-protocol.md#relayed-streams)).
- A settlement hub's streams already take their sender from the key the connection proved.

## Decision

A new protocol version, v1, that keeps v0's message set, less `settlementMessage`, and fixes its security:

- **The sender is the key its connection proved.** A request is the request of the node whose key the QUIC connection proved, and nothing in its bytes names a sender; an answer comes from the key the dialler pinned. Nothing a request carries is believed of its sender: a link state it carries must name that key.
- **Peering requests carry the full signed node info,** which must name the sender; peerings can be terminated. They name the scheme alone: what a scheme means is versioned by its ID and by this protocol's version, so a version of the scheme negotiated beside them would be a third knob, which no scheme turned and none could rely on ([mainnet-schemes](mainnet-schemes.md)).
- **Duplicates are the messages' own business:** a packet's request ID and a settlement's sequence tell a message the sender sent again from a new one, as they must anyway, since a sender sends again what it got no answer for.

`settlementMessage`, v0's channel for a scheme's own messages to its counterpart, is not kept: no scheme sends one, since a scheme's peers meet on its ledger, and a message the node only passes on is attack surface for nothing. Its tag stays reserved, so no other message ever takes it. A scheme that needs a message of its own gets one in the protocol, with its rules.

A post-quantum path stays open: the node ID commits to an SLH-DSA key beside the Ed25519 key, which a node can prove its identity with once Ed25519 falls, and link-state extension type 1 is kept for the signing key that proof is to vouch for ([post-quantum-node-id](post-quantum-node-id.md)). Confidentiality against "harvest now, decrypt later" belongs to TLS.

## Accepted

- **v1 nodes cannot talk to v0 nodes.** The project ran the only v0 network, and replaced its bootstrap nodes ([bootstrap-network](bootstrap-network.md)).
- **Peer messages need the transport's proof.** A message carries nothing that proves its sender by itself, so it cannot be relayed through a third node, or stored and checked later. Neither is a use the protocol has.

## Rejected

- *Staying v0-compatible and adding v1 later:* it ships a known sender-forgery hole.
- *A MAC per message,* keyed from a static-static X25519 between the two node keys, over the sender, recipient, a session, a counter and a timestamp, with authenticated responses: it suits a transport whose clients prove nothing, such as HTTPS. Over QUIC it proves again what the connection proves, and needs what the connection does not: replay windows per session, a bounded pool of strangers' replay state shared out by network, floors persisted with every effect so a restart reopens no replay, and the refusal of messages dated before a restart, which a sender must send again. Nothing compared its sender with the connection's either, so it added a second identity a transport could disagree with.
- *An Ed25519 signature per message:* a signature costs far more than the connection's proof, on every message, and proves nothing more to the recipient.
