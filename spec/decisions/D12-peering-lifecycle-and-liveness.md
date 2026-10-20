# D12. Peering lifecycle and liveness

**Areas:** [Network](../areas/network.md). Child peers are not built yet ([roadmap](../roadmap.md#periphery)).

## Facts

In the original, peers are never dropped, there is no termination, advertise flag or banning, and auto-peering has no backoff.

## Decision

A peering is requested, peered or terminating; a peer is advertised in our link state only while it is live and has room; termination settles nothing ([D28](D28-a-node-that-ends-a-peering-collects-first-and-pays-nothing.md)); bans are the owner's. The peer target and the timings were set from the network simulation; change them only with its evidence. [Network](../areas/network.md) states the rules. The reasons for the less obvious ones:

- **Only confirmed peers count toward the target,** so a node whose stored peerings went stale looks for new peers instead of staying isolated.
- **A bridge rule joins islands:** while a majority node stays unreachable, a node tries one more peering, so two separate clusters join up.
- **Inbound slots are capped** below the maximum, so requests alone cannot fill every slot.
- **Repeated and crossed requests resolve deterministically:** a repeat is answered from the stored peering (it covers an acceptance that got lost), and the lower node ID's request wins.
- **Withdrawing is prompt, ending is not.** A withdrawal only takes an edge out of routing until the peer is healthy again, so it may come soon: a route kept to a stuck peer loses packets, and the hold-down keeps the flaps down. Ending a peering is for good, so it waits for much stronger evidence, a peer silent or short of room for as long as a silent peer is kept.
- **Failed deliveries do not withdraw a peer.** Withdrawing an edge removes it both ways for the whole network, turning a temporary T04 into a final F02.
- **A peer that stays short of room is withdrawn, and in the end terminated.** A creditor asks its debtor to settle over the peer protocol, which needs no route, so settling, not traffic, restores room ([D23](D23-each-node-books-its-own-fulfills.md)). A peer that stays short past a grace is stuck, and every packet routed through it would be refused anyway. The grace lets two requests go unanswered, and the third be answered and its settlement booked; a request that surely never reached the peer is asked again at the next check, so it costs none of the grace. Only the room a node gives its peer counts, never the room the peer gives it; and time the node could not ask (its own downtime or stall, its ledger out of sight) does not count against the peer. Nor does time every request surely failed to reach the peer count toward ending the peering for room, though it still withdraws the peer: a peer cut off had no chance to settle, and an end for room keeps the pair apart and gives no credit back ([D33](D33-a-peering-ended-for-room-stays-apart-and-earns-no-credit-back.md)), which an honest peer out of reach did nothing to earn. Silent too, it is ended for its silence. Each settlement booked from the peer starts the grace again: on a busy link, traffic takes up each settlement at once, so the room stays short at every check while the peer settles every few seconds, and an honest busy peer would otherwise be withdrawn.
- **Termination ends a peering, not a relationship.** The accounts and the settlement sequence stay, so a settlement that arrives after the peering ended is still credited once, and peering again does not post trust twice.
- **A ban is a list, not a peering state,** since a node can be banned while its peering is still terminating.

## Accepted

- Withdrawing a stuck peer stops packets toward it too, even those that would raise its room: it must settle to come back. A link whose books drifted past what it survives is not saved; it is withdrawn and ended.
- A node never closes a peer's accounts or settlement channel, so the number of nodes it has ever peered with is capped. Closing relationships whose balance is settled can lift that ([roadmap](../roadmap.md)).
- A peer short of room that cannot be reached is asked to settle at every check, not once a request interval. Each attempt fails before it reaches the peer, so it costs the peer nothing.
- A peer that refuses our connections while it keeps talking to us is never ended for room. It stays withdrawn, no packet crosses the link, the credit bounds what it owes, and it holds one of our peer slots until either owner ends the peering, or ours bans it.

## Rejected

- *Not counting time out of reach toward withdrawal either,* so that one total serves both: a peer cut off one way still hears us and is heard in the answers to its own requests, so neither side withdraws it for silence, and routes would stay on a link that carries nothing and lose packets.
- *Asking again at the next check after any request that surely had no effect, a refusal (400, 401) or an overloaded peer (503) too:* a peer that refuses or is overloaded would be asked at every check instead of once a request interval. Only a request that never reached the peer, or one refused for a peering it has not taken in yet (409), is asked again at once; the grace covers the rest.

## Deferred

**Child peers,** routed beneath their uplink's address, touch the connector, BTP and the STREAM receiver. They are the periphery's downlinks, nodes without a public IP ([D26](D26-core-and-periphery.md)), and wait for that work.
