# A peering ended for room stays apart

**Areas:** [Network](../areas/network.md), [Settlement](../areas/settlement.md).

## Facts

- A node ends a peering whose room in its books stayed short, without a settlement, for as long as a silent peer is kept, counting only the time the peer could be reached ([peering-lifecycle](peering-lifecycle.md)). Withdrawing is prompt and ending is not, so an end is rare, and taken on strong evidence.
- After any end but an owner's, the node asked the peer again after a backoff of a couple of minimum backoffs, which did not grow from one peering to the next: a peering clears the failures. Nodes the node's routes do not reach are its first candidates, and a peer it withdrew and ended is one. So the pair formed again seconds after the end, and the link stuck again.

## Decision

**The pair stays apart.** After a peering ends for want of room, neither side asks the other for the owner's cooldown (`owner_termination_cooldown_ms`), and each refuses the other's requests meanwhile, as after an owner's end. The node that decided for room keeps that reason when the peer tells it first. Like the owner's, the cooldown is not persisted.

Other ends keep the short backoff.

## Accepted

- **A restart forgets the cooldown,** of either node, so the pair may form again early. Over a hub the credit is the hold ([Settlement](../areas/settlement.md#credit)), so a peer that never settled gets nothing by being asked again: what that costs is churn, not exposure.
- **A former downlink may forget it at once.** A downlink takes no place among the node table's other nodes, so once its peering ends it stays in the table only if it gets one; with the table full it is removed at once, and the cooldown with its entry ([Network](../areas/network.md#downlinks)). It may then ask again straight away. Over a hub the node extends it no more than its own hold toward it, so a downlink that never settled costs churn, not exposure, as after a restart; the owner bans one to keep it away.
- **A pair kept apart may leave a node short of its peer target** for the cooldown, in a small network. The end needed a day of a link that could not carry a packet, which counted for nothing toward the target.
- **A peer that tells us its end is for room keeps us away** for the cooldown. It could already do so by naming its owner, and only from itself.

## Rejected

- *A backoff that grows across peerings:* the first cycles still form again within minutes, and the growth would have to outlive peerings and restarts.
- *The same cooldown after an end for silence:* a silent peer is often an honest node whose host was down, and is welcome back once it answers. A peer that is also short of room stays short, and is ended for room the next time.
- *Remembering the end for good,* on the settlement channel: over a hub no credit is withheld by it, so a mark that outlives the cooldown would keep nothing away that the cooldown and a ban do not.
- *Banning the peer:* only the owner bans ([own-fulfills](own-fulfills.md)).
