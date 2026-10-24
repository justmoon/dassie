# Bootstrap network for v1

**Areas:** [Network](../areas/network.md)

## Decision

The project runs the default bootstrap nodes itself: four, in one region, spread over its zones, so that one node down does not block majority (three of four lists). Four is what a settlement hub needs to tolerate one faulty validator (n = 3f + 1), and a region has three zones, so two of them share a zone. Their node IDs are checked against their keys at startup. They also run the XRPL testnet scheme, so that a new node has someone to peer with, and validate the testnet's default settlement hub, which needs its validators within tens of milliseconds of each other ([settlement-hubs](settlement-hubs.md)).

Each bootstrap node names all of them, itself included, in its own configuration (a systemd drop-in), rather than relying on the built-in default: a node takes registrations only while it is one of its own bootstrap nodes.

## Accepted

- Losing the zone that holds two of them leaves two of four lists, short of a majority: membership freezes, no new node becomes a member, and the default hub, which loses f + 1 of its validators, halts until the zone is back. A fifth node would keep a majority through any one zone's loss (three of five), but not the hub, whose four validators tolerate one down; the testnet takes the risk.
- One region's failure blocks majority and registrations for the whole network, and halts the default hub with them. Running the hub's validators apart from the bootstrap nodes would avoid that at the cost of more nodes to run; the testnet takes the risk.

## Operations

The nodes run in GCP project `dassie-390023`, which also holds the `dassie.xyz` DNS zone and their recovery phrases (Secret Manager, `dassie-boot<N>-phrase`). Four of them run in us-east1, boot1 and boot4 in zone b, boot2 in c, boot3 in d. Each looks for an update in an hour of its own (a drop-in on `dassie-update.timer`: boot*N* at 0*N*:05 UTC, within half an hour), so at most one restarts at a time: membership needs more than half of the configured lists, three of four, and the hub's validators tolerate one of four (f = 1).
