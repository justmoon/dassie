# D12. Peering lifecycle and liveness

**Status:** active, confirmed 2026-10-11, refined 2026-10-14 in two design reviews, and on 2026-10-19 (a peer that stays short of room). Child peers are not built yet ([roadmap](../roadmap.md)).
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
- **The advertise flag follows liveness,** with a hold-down that doubles per flap. Failed deliveries are left out. So was liquidity, until room counted from 2026-10-19 (below): withdrawing an edge removes it both ways for the whole network, turning a temporary T04 into a final F02, and the traffic coming back restored liquidity.
- **Termination ends a peering, not a relationship.** The accounts stay, with what is owed, and the settlement sequence carries on, so a settlement that arrives after the peering ended is still credited once. Peering again reuses both, without posting trust twice.
- **A ban is a list, not a peering state,** since a node can be banned while its peering is still terminating.

## Refinement: a peer that stays short of room (2026-10-19)

Owner's decision. A node stops advertising a peer once the room its books give that peer (its position, [Settlement](../areas/settlement.md)) has stayed below one of its largest packets for longer than a grace, without the peer settling: 2 min (10 s in dev), two requests to settle, so that one lost request withdraws nothing. A node does not start with a grace shorter than two of its request intervals.

- **Why the 2026-10-14 trade-off changed.** Since [D23](D23-each-node-books-its-own-fulfills.md), a creditor asks its debtor to settle over the peer protocol, which needs no route: settlement, not traffic, restores the room. A peer that stays short past the grace is stuck, unable or unwilling to settle, and every packet routed through it would be refused (T04). A peer waiting for a routine settlement is short for seconds and stays listed.
- **A node counts only the limit it imposes:** the room it gives the peer, never the room the peer gives it. If the peer is the one stuck, we withdraw it; if we are, it withdraws us.
- **Counted at every settlement check, while the node can ask.** Each settlement booked from the peer starts the grace again: on a busy link, traffic takes up each settlement at once, and the room stays short at every check while the peer settles every few seconds. The time short is a total, grown at each check by the time since the last check that found the peer short while the node could ask, by at most two check intervals, so a stall of the node (its host suspended, its clock jumped) is not counted against the peer. So neither the node's downtime nor an outage of its own ledger counts: while its scheme does not see its ledger, it cannot ask, and the total neither grows nor starts again. A junior that has not paid its senior in advance within the grace of being asked is withdrawn.
- **The same damping** as a peer that falls silent: a withdrawal for room is a flap, held down `30 s · 2^flaps`.
- **While withdrawn,** the peer is still peered, and still asked to settle and settled with, which is how it comes back. No packet crosses the edge either way: both ends' routes drop it. It does not count toward the peer target, so the node looks for another peer meanwhile. The total is kept with the peering, so a restart neither forgives it nor starts the 24 h again (review of #126): a node that restarts more often than daily still ends a link that stays short, and the time it was down does not count against the peer (review of #137).
- **Ended:** a peer that stays short, settling nothing, for as long as a silent peer is kept (24 h, 2 min in dev) is terminated, with reason 4, "short of room".
- **Accepted:** the edge goes both ways. Packets toward the stuck peer, which would raise its room, stop too, so the peer must settle to come back. That includes a link whose books drifted past what it survives ([D23](D23-each-node-books-its-own-fulfills.md#accepted)), whose honest debtor, by its own books, owes nothing: most of its liquidity is gone, and it is not saved. It is withdrawn, and ended.

## Deferred

**Child peers** (NET-21, NET-23), routed beneath their uplink's address, need a node's ILP address to follow its uplink, which touches the connector, BTP and the STREAM receiver. They only matter once registration costs something: today any node with a URL registers automatically and becomes a member within minutes. They belong to the work on registration fees.

## Known limit

A node never closes a peer's accounts or settlement channel. It keeps at most 512 channels, one per node it ever peered with; after that, peering with a new node is refused for capacity. Closing a relationship whose balance is settled can lift this later.
