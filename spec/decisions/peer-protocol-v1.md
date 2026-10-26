# Peer protocol: v0 or v1

**Areas:** [Peer protocol](../areas/peer-protocol.md), [Identity](../areas/identity.md)

## Facts

The original's peer protocol (v0) does not authenticate senders: any peered node can forge `interledgerPacket` and `settlement` messages from another. Peering requests are not checked against their sender, and the HTTP status means nothing.

## Decision

A new protocol version, v1, that keeps v0's message set, less `settlementMessage`, and fixes its security:

- **Sender authentication by a MAC,** keyed from a static-static X25519 between the two node keys, over the version, sender, recipient and content.
- **Replay protection** that survives restarts.
- **Authenticated responses,** so node lists, peering answers and registrations are authenticated end to end, not only by TLS.
- **Peering requests carry the full signed node info,** which must name the sender; peerings can be terminated. They name the scheme alone: what a scheme means is versioned by its ID and by this protocol's version, so a version of the scheme negotiated beside them would be a third knob, which no scheme turned and none could rely on ([mainnet-schemes](mainnet-schemes.md)).

The shape of the scheme, and why:

- **No ephemeral key.** For a scheme that only MACs, an ephemeral X25519 adds nothing: whoever holds the recipient's static key can forge with or without it, so there is neither forward secrecy nor protection against key-compromise impersonation. It would also let anyone force an X25519 operation per request by sending fresh ephemerals.
- **Direction is bound into the key,** so a message cannot be reflected back to its sender.
- **The sender's key travels in the envelope** and must hash to the claimed node ID, so a node can check a sender it does not know yet, such as a registrant.
- **A post-quantum path is reserved:** a tag for a scheme built on the X-Wing hybrid KEM, and room for its key in link states. Signatures and node IDs stay Ed25519, since the passphrase-to-node-ID chain is fixed. Confidentiality against "harvest now, decrypt later" belongs to TLS.

`settlementMessage`, v0's channel for a scheme's own messages to its counterpart, is not kept: no scheme sends one, since a scheme's peers meet on its ledger, and a message the node only passes on is attack surface for nothing. Its tag stays reserved, so no other message ever takes it. A scheme that needs a message of its own gets one in the protocol, with its rules.

## Accepted

v1 nodes cannot talk to v0 nodes. The project ran the only v0 network, and replaced its bootstrap nodes ([bootstrap-network](bootstrap-network.md)).

## Rejected

- *Staying v0-compatible and adding v1 later:* it ships a known sender-forgery hole.
- *An Ed25519 signature per message:* a signature costs far more than a MAC, on every message.
