# D15. Bootstrap network for v1

**Areas:** [Network](../areas/network.md)

## Decision

The project runs the default bootstrap nodes itself: at least three, in separate regions, so that one failure does not block majority. Their node IDs are checked against their keys at startup. They also run the XRPL testnet scheme, so that a new node has someone to peer with.

Each bootstrap node names all three, itself included, in its own configuration (a systemd drop-in), rather than relying on the built-in default: a node takes registrations only while it is one of its own bootstrap nodes.

Operations: the nodes run in GCP project `dassie-390023`, which also holds the `dassie.xyz` DNS zone and their recovery phrases (Secret Manager, `dassie-boot<N>-phrase`).
