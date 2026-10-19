# D2. Peer protocol: v0 or v1

**Status:** active, confirmed 2026-10-11, amended 2026-10-14.
**Areas:** [Peer protocol](../areas/peer-protocol.md), [Identity](../areas/identity.md)

## Facts

The original's peer protocol (v0) does not authenticate senders (NET-8): any peered node can forge `interledgerPacket` and `settlement` messages from another. Peering requests are not checked against their sender (NET-3, NET-4), the HTTP status means nothing (NET-11), there is no termination (NET-13) and no negotiation of settlement-module versions (NET-26).

## Decision

A new protocol version, v1 (envelope `version = 1`). It keeps v0's message set and OER layouts, and fixes what v0 lacks:

- **Sender authentication** by a MAC whose key comes from a static-static X25519 between the sender's and the recipient's node keys. The MAC covers the version, the sender, the recipient and the content.
- **Replay protection** by a timestamp plus a session ID and counter.
- **Authenticated responses**, so node lists, peering answers and registrations are authenticated end to end, not only by TLS.
- Peering requests carry the full signed node info, which must name the sender; the HTTP status means something; termination exists; scheme versions are exchanged when peering.

The amendments of 2026-10-14 shaped the scheme ([Peer protocol](../areas/peer-protocol.md) states it):

- **No ephemeral key.** For a scheme that only MACs, an ephemeral X25519 adds nothing: whoever holds the recipient's static key can forge with or without it, so there is neither forward secrecy nor protection against key-compromise impersonation. It also let anyone force an X25519 operation per request by sending fresh ephemerals.
- **Direction labels** and the ordered pair of keys in the key expansion, so a message cannot be reflected back to its sender.
- **The sender's key travels in the envelope** and must hash to the claimed node ID, so a recipient can check a node that is not in its table yet, such as a registrant.
- **A persisted per-peer floor** for the replay window, so a restart does not reopen it.
- **A post-quantum path:** the authentication CHOICE reserves a tag for a scheme built on the X-Wing hybrid KEM, and link-state extensions can carry its key. Signatures and node IDs stay Ed25519, since the passphrase-to-node-ID chain is fixed. Confidentiality against "harvest now, decrypt later" belongs to TLS (hybrid X25519MLKEM768), once the release's crypto provider is settled.

## Accepted

v1 nodes cannot talk to the two v0 test nodes. Both are operated by the project and were replaced ([D15](D15-bootstrap-network-for-v1.md)).

## Rejected

- *Staying v0-compatible and adding v1 later:* it ships a known sender-forgery hole.
- *An Ed25519 signature per message* over `version ‖ sender ‖ content ‖ timestamp`: a signature costs more than a MAC, for every message.
