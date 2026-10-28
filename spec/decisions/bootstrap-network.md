# Bootstrap network for v1

**Areas:** [Network](../areas/network.md)

## Decision

The project runs the default bootstrap nodes itself: five, in one region, spread over its zones, so that two nodes down do not block majority (three of five lists). Four of them, boot1–boot4, validate the testnet's default settlement hubs: four is what a hub needs to tolerate one faulty validator (n = 3f + 1). The fifth, boot5, is a member of those hubs and no validator; it is there for the majority, which it keeps through the loss of any one zone. A region has three zones, so two of them hold two nodes each. Their node IDs are checked against their keys at startup. They also run the XRPL testnet scheme, so that a new node has someone to peer with, and the validators among them sit in one region because a hub needs its validators within tens of milliseconds of each other ([settlement-hubs](settlement-hubs.md)).

Each bootstrap node names all of them, itself included, in its own configuration (a systemd drop-in), rather than relying on the built-in default: a node takes registrations only while it is one of its own bootstrap nodes.

## Accepted

- Losing a zone that holds two of them leaves three of five lists, still a majority, but short of the names' quorum, four of five ([Network](../areas/network.md#holders)): the view of names freezes until the zone is back. Losing zone b, which holds two of the validators, also halts the default hubs, which lose f + 1 of their validators; losing c or d costs them one, which they tolerate. Making boot5 a fifth validator would not help, since a hub of five tolerates no more faults than one of four (f = 1); the testnet takes the risk.
- A name held before its holder's registrars included boot5 is listed by boot1–boot4 alone: boot5 answers its renewals that it does not hold it, and a holder cannot register again a name it holds. Until such a name is registered again, it needs all four of those lists fresh to reach the quorum of four of five, so one of boot1–boot4 stale for longer than a list stays fresh drops it from every view that counts five registrars until that registrar is back. The testnet takes this, since its names are free and can be reset; how a newly listed registrar takes over names already held is open.
- While nodes update to a release that lists boot5, those not yet updated register with boot1–boot4 alone, so two of those down no longer freeze membership for updated nodes: they still see three of five lists fresh, and drop the nodes not yet updated from the majority until the daily updates land.
- One region's failure blocks majority and registrations for the whole network, and halts the default hub with them. Running the hub's validators apart from the bootstrap nodes would avoid that at the cost of more nodes to run; the testnet takes the risk.

## Operations

The nodes run in GCP project `dassie-390023`, which also holds the `dassie.xyz` DNS zone and their recovery phrases (Secret Manager, `dassie-boot<N>-phrase`). All five run in us-east1, boot1 and boot4 in zone b, boot2 and boot5 in c, boot3 in d. Each looks for an update in an hour of its own (a drop-in on `dassie-update.timer`: boot*N* at 0*N*:05 UTC, within half an hour), so at most one restarts at a time: membership needs more than half of the configured lists, three of five, names a quorum of four of five, and the hubs' validators, boot1–boot4, tolerate one of four (f = 1).
