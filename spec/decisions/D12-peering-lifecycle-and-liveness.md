# D12. Peering lifecycle and liveness

**Areas:** [Network](../areas/network.md). Child peers are not built yet ([roadmap](../roadmap.md#periphery)).

## Facts

In the original, peers are never dropped, there is no termination, advertise flag or banning, and auto-peering has no backoff.

## Decision

A peering is requested, peered or terminating; a peer is advertised in our link state only while it is live and has room; termination settles what is owed; bans are the owner's. The peer target and the timings were set from the network simulation; change them only with its evidence. [Network](../areas/network.md) states the rules. The reasons for the less obvious ones:

- **Only confirmed peers count toward the target,** so a node whose stored peerings went stale looks for new peers instead of staying isolated.
- **A bridge rule joins islands:** while a majority node stays unreachable, a node tries one more peering, so two separate clusters join up.
- **Inbound slots are capped** below the maximum, so requests alone cannot fill every slot.
- **Repeated and crossed requests resolve deterministically:** a repeat is answered from the stored peering (it covers an acceptance that got lost), and the lower node ID's request wins.
- **Failed deliveries do not withdraw a peer.** Withdrawing an edge removes it both ways for the whole network, turning a temporary T04 into a final F02.
- **A peer that stays short of room is withdrawn, and in the end terminated.** A creditor asks its debtor to settle over the peer protocol, which needs no route, so settling, not traffic, restores room ([D23](D23-each-node-books-its-own-fulfills.md)). A peer that stays short past a grace is stuck, and every packet routed through it would be refused anyway. The grace covers a lost request; only the room a node gives its peer counts, never the room the peer gives it; and time the node could not ask (its own downtime or stall, its ledger out of sight) does not count against the peer. Each settlement booked from the peer starts the grace again: on a busy link, traffic takes up each settlement at once, so the room stays short at every check while the peer settles every few seconds, and an honest busy peer would otherwise be withdrawn.
- **Termination ends a peering, not a relationship.** The accounts and the settlement sequence stay, so a settlement that arrives after the peering ended is still credited once, and peering again does not post trust twice.
- **A ban is a list, not a peering state,** since a node can be banned while its peering is still terminating.

## Accepted

- Withdrawing a stuck peer stops packets toward it too, even those that would raise its room: it must settle to come back. A link whose books drifted past what it survives is not saved; it is withdrawn and ended.
- A node never closes a peer's accounts or settlement channel, so the number of nodes it has ever peered with is capped. Closing relationships whose balance is settled can lift that ([roadmap](../roadmap.md)).

## Deferred

**Child peers,** routed beneath their uplink's address, touch the connector, BTP and the STREAM receiver. They are the periphery's downlinks, nodes without a public IP ([D26](D26-core-and-periphery.md)), and wait for that work.
