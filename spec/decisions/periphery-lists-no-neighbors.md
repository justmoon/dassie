# A periphery node's link state lists no neighbors, so it may keep many uplinks

**Areas:** [Network](../areas/network.md#periphery-nodes), [Peer protocol](../areas/peer-protocol.md#link-states), [Connector](../areas/connector.md#a-periphery-node).

## Facts

- A market maker trades through core nodes from periphery nodes of its own, pinned to them ([Network](../areas/network.md#pinned-uplinks), [passive-rebalancing](passive-rebalancing.md)). Two such nodes can trade through every core node they are both peered with, so a market maker wants many uplinks per node, far more than the peering slots a node has.
- A link state holds a bounded number of entries and bytes, and every node refuses one past them ([Peer protocol](../areas/peer-protocol.md#link-states)). Those bounds are protocol: changing them is a new version that older nodes refuse.
- A periphery node's link state that lists its uplinks as neighbors stops fitting past a few dozen of them: the node would sign one nobody decodes.
- Nothing routes by a periphery node's neighbors. An edge counts only if both ends list each other, and an uplink never lists a downlink, whose address lies beneath its own ([core-and-periphery](core-and-periphery.md)). A periphery node routes through the uplinks it advertises, from its own table, not from its link state. Its link state is seen only by its uplinks.
- The one reader was an uplink's hint of a one-sided peering: a node's own link state that lists the recipient presumes a peering ([Network](../areas/network.md#one-sided-peerings)).
- A node that tries to become core shows its uplinks to the network in its link state, for promotion ([promotion-to-core](promotion-to-core.md), not built).

## Decision

**Only core nodes, and nodes trying to become core, list neighbors in their link state. A periphery node's lists none, so its size does not grow with its uplinks.**

- **The hint is kept another way.** A periphery node sends its own link state only to its uplinks, in heartbeats and when it issues a new one, so one arriving from it presumes a peering, whatever it lists. An uplink that ended the peering while the two were cut off tells the periphery node at its next heartbeat, through the settlement channel it keeps with it, since the downlink left its table when the peering ended.
- **A pinned periphery node has slots of its own,** as many as it pins, up to `MAX_UPLINKS`. Every node's tables are sized for that many core peers: the connector's addresses beneath the uplinks and its packet table, the ledger's pending transfers and the settlement channels.
- **A node never signs a link state others cannot decode.** Its neighbors and schemes are bounded, and the configuration is checked at start against the link state's bounds; signing one that does not decode is an assertion, not a silent fallback.

## Accepted

- **An older uplink loses the hint** from a newer periphery node, whose link state no longer lists it. It ends a one-sided peering only for silence, as it would a peer whose termination never arrived, and the periphery node's own side still notices its uplink's silence.
- **The bounds hold the most uplinks on every node,** core nodes too. The tables are filled only as they are used, so the room costs nothing until a node has the peers to use it.
- **A periphery node is less visible** to its uplinks: one can no longer read from its link state which other uplinks it has. Nothing used that.
- **Many uplinks cost the periphery node** a connection, heartbeats and credit at risk for each. The owner who pins them chooses that.

## Rejected

- *Raising the link state's bounds:* a protocol change every node must take before any periphery node may use it, for a list nothing reads.
- *Keeping the pin limit below the link state's bounds:* a market maker would need more nodes, each with its own funds, to reach the same core nodes, and the limit would still couple a local choice to a protocol bound.
- *Listing only some uplinks:* the list would still mean nothing to routing, and which ones would be arbitrary.
