# D12. Peering lifecycle and liveness

**Status:** active, confirmed 2026-10-11, refined 2026-10-14 in two design reviews. Child peers are not built yet ([roadmap](../roadmap.md)).
**Areas:** [Network](../areas/network.md), [Peer protocol](../areas/peer-protocol.md)

## Facts

In the original, peers are never dropped, heartbeats don't update `lastSeen`, there is no termination, advertise flag or banning (NET-12, NET-13), and auto-peering stops at 2 peers with no backoff (NET-22).

## Decision

Implement the original's peer-state design: requesting, peered and terminating states; an advertise flag; termination that settles what is owed; candidate scoring and backoff; bans. The peer target stays 2, and the timings were set from the network simulation. [Network](../areas/network.md) states the rules.

The design reviews added:

- **Only confirmed peers count toward the target,** so a node whose stored peerings went stale looks for new peers instead of staying isolated.
- **A bridge rule joins islands:** while a majority node stays unreachable, a node tries one more peering, so two separate triangles of peers join up.
- **Inbound slots are capped** below the maximum, so requests alone cannot fill every slot.
- **Repeated and crossed requests resolve deterministically:** a repeat is answered from the stored peering (it covers an acceptance that got lost), and the lower node ID's request wins.
- **The responder probes the requester's URL,** as the original's design note intended.
- **The advertise flag follows liveness only,** with a hold-down that doubles per flap. Liquidity and failed deliveries are left out: withdrawing an edge removes it both ways for the whole network, turning a temporary T04 into a final F02, while the traffic coming back is what restores liquidity.
- **Termination ends a peering, not a relationship.** The accounts stay, with what is owed, and the settlement sequence carries on, so a settlement that arrives after the peering ended is still credited once. Peering again reuses both, without posting trust twice.
- **A ban is a list, not a peering state,** since a node can be banned while its peering is still terminating.

## Deferred

**Child peers** (NET-21, NET-23), routed beneath their uplink's address, need a node's ILP address to follow its uplink, which touches the connector, BTP and the STREAM receiver. They only matter once registration costs something: today any node with a URL registers automatically and becomes a member within minutes. They belong to the work on registration fees.

## Known limit

A node never closes a peer's accounts or settlement channel. It keeps at most 512 channels, one per node it ever peered with; after that, peering with a new node is refused for capacity. Closing a relationship whose balance is settled can lift this later.
