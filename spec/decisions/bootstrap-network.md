# Bootstrap network for v1

**Areas:** [Network](../areas/network.md)

## Decision

The project runs the default bootstrap nodes itself: at least three, in one region, each in a zone of its own, so that one zone's failure does not block majority. Their node IDs are checked against their keys at startup. They also run the XRPL testnet scheme, so that a new node has someone to peer with, and validate the testnet's default settlement hub, which needs its validators within tens of milliseconds of each other ([settlement-hubs](settlement-hubs.md)).

Each bootstrap node names all of them, itself included, in its own configuration (a systemd drop-in), rather than relying on the built-in default: a node takes registrations only while it is one of its own bootstrap nodes.

## Accepted

- One region's failure blocks majority and registrations for the whole network, and halts the default hub with them. Running the hub's validators apart from the bootstrap nodes would avoid that at the cost of more nodes to run; the testnet takes the risk.

## Operations

The nodes run in GCP project `dassie-390023`, which also holds the `dassie.xyz` DNS zone and their recovery phrases (Secret Manager, `dassie-boot<N>-phrase`).
