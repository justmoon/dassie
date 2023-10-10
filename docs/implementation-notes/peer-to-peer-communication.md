# Implementation Notes: Peer-to-peer communication

## Two APIs

In the future, we envision that Dassie nodes have two main APIs:

1. The public HTTPS API
2. A private API which is exposed to peers only

The private API is used for sending Interledger packets between peers. It could use symmetric encryption which is initialized via the HTTPS API. All messages sent over the private API are Interledger packets which may encapsulate Dassie-specific messages using a `peer.das.` prefix. Given that we expect large amounts of Interledger packets and a small volume of Dassie-specific messages, it makes more sense to optimize for Interledger packets.

For efficiency, the private API uses AES128-GCM-SHA256 encrypted datagrams over UDP. Each datagram must be associated with a _session_ which corresponds to an encryption key that has been previously established via the public API (which itself is secured via TLS). In order to protect against replay attacks, each node maintains a cache of all received ILP packets that have not yet expired. When a node has recently restarted, it would wait MAX_PACKET_TIMEOUT seconds before it started accepting packets to prevent double processing of any packets that it received prior to the restart. Here is an example of what the format for each datagram might look like:

- Session ID (4 bytes)
- Initialization vector (12 bytes)
- Authentication tag (16 bytes)
- Encrypted contents (remainder)

The encrypted contents contain the Interledger packet and possibly additional metadata related to the settlement mechanism.

When peering, nodes would assign each other an IP/port pair for the private connection. This would make it very easy to scale Dassie in the future simply by having a separate process or even a separate host per peer.
