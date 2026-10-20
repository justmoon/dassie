# D33. A peering ended for room stays apart, and earns no credit back

**Areas:** [Network](../areas/network.md), [Settlement](../areas/settlement.md).

## Facts

- A node ends a peering whose room in its books stayed short, without a settlement, for as long as a silent peer is kept, counting only the time the peer could be reached ([D12](D12-peering-lifecycle-and-liveness.md)). Withdrawing is prompt and ending is not, so an end is rare, and taken on strong evidence.
- After any end but an owner's, the node asked the peer again after a backoff of a couple of minimum backoffs, which did not grow from one peering to the next: a peering clears the failures. Nodes the node's routes do not reach are its first candidates, and a peer it withdrew and ended is one. So the pair formed again seconds after the end, and the link stuck again.
- A junior extends at least the default credit line, and raises a trust account kept from a time it was the senior up to it ([D20](D20-trust-follows-what-a-peer-earned-us.md)). When the pair formed again with the ending node as the junior, a peer that never settled got a new default line, once per cycle.

## Decision

- **The pair stays apart.** After a peering ends for want of room, neither side asks the other for the owner's cooldown (`owner_termination_cooldown_ms`), and each refuses the other's requests meanwhile, as after an owner's end. The node that decided for room keeps that reason when the peer tells it first. Like the owner's, the cooldown is not persisted.
- **No credit back.** The node that ends a peering for room remembers it on the settlement channel, which outlives peerings and is never removed. A later peering in which it is the junior keeps the trust account as it is, rather than raising it to the default: the peer's credit grows again only with what it earns the node.

Other ends keep the short backoff.

## Accepted

- **A restart forgets the cooldown,** of either node, so the pair may form again early. The credit stays where the end left it, so what that costs is churn, not exposure.
- **The memory is for good.** A peer once ended for room never gets the default line by being asked again; it earns credit by fees, as a junior does with its senior.
- **A pair kept apart may leave a node short of its peer target** for the cooldown, in a small network. The end needed a day of a link that could not carry a packet, which counted for nothing toward the target.
- **A peer that tells us its end is for room keeps us away** for the cooldown. It could already do so by naming its owner, and only from itself.

## Rejected

- *A backoff that grows across peerings:* the first cycles still form again within minutes, each with a new default line, and the growth would have to outlive peerings and restarts as this does.
- *The same cooldown after an end for silence:* a silent peer is often an honest node whose host was down, and is welcome back once it answers. A peer that is also short of room stays short, and is ended for room the next time.
- *Withholding only the credit, with no cooldown:* the pair would form again at once, and spend a day on a link that carries nothing before ending it again.
- *Banning the peer:* only the owner bans ([D23](D23-each-node-books-its-own-fulfills.md)).
