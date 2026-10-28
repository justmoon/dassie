# A node keeps a generous target in its hub, counting what it put in

**Areas:** [Settlement hubs](../areas/hubs.md#in-the-node), [Settlement](../areas/settlement.md#positions-and-thresholds), [Payments](../areas/payments.md#where-received-money-is-kept).

## Facts

- Over a hub, the credit a node extends a peer is its hold toward that peer, funded when the peer asks ([Settlement](../areas/settlement.md#credit)). Each peering takes a hold of its own, up to the default credit, so what a node needs in its hub grows with its peerings.
- A node that cannot fund a hold when asked stops settling with that peer, and payments through it fail for want of liquidity, for everyone routing through it.
- The owner's funds on the owner book do nothing for settling: only what is in the hub funds holds. A deposit takes an XRP Ledger fee and a few seconds before the hub credits it.
- A ceiling of a hundred of the largest packets, counting the node's holds toward its peers and theirs toward it, was full with about ten peerings. The node then deposited nothing, however little it could spend and however much its own account held, and its peers went unpaid.
- A peer's hold toward us is the peer's money until we take from it. What we take joins our balance.
- The XRP owner book pays the gas of the account's other payments: a token book's fees and a token hub's escrow gas.

## Decision

- **A generous target.** The node keeps a thousand of the largest packets in each hub: more than its holds are likely ever to need. Most owners hold less than that, and the node deposits all of it. Only funds truly in excess stay on the owner's own account.
- **What the node put in counts:** the balance, the reserves and its holds toward peers. Peers' holds toward it count nowhere until it takes from them.
- **A step either way.** The node deposits once what it keeps falls a tenth below the target, back up to it, and withdraws once it rises a tenth above, back down to it. So a deposit or a withdrawal carries a hundred packets' worth, never one per settlement.
- **The XRP book keeps the gas** of the account's other payments, twice a token hub's least escrow gas, out of the node's own deposits.
- **How the hub's funds are shared out among peers is the holds' business,** not the target's: the target says how much to deploy, and the holds say where.

## Accepted

- **More is at stake in a hub that fails:** up to the target, where the old ceiling held a tenth of it. Hubs are the way peers settle ([hubs-only](hubs-only.md)), and a node whose hub runs dry settles with nobody.
- **The target does not follow the peerings.** A node with more peerings than its target funds fully shares it out among them. One with far fewer keeps more in the hub than it uses.
- **An owner who wants less in the hub has no say yet.** The market maker's node in particular needs most of its funds on its owner books, where its rebalances spend.

## Rejected

- *Keeping the ceiling and counting holds apart from it:* this fixes the stall, but a ceiling of a hundred packets with a floor sized per peering still asks for a deposit at almost every settlement on a busy node, and still decides how much to deploy from how many peers there are, not from the owner's appetite for risk.
- *A ceiling that grows with the peerings:* one number would bound everything at stake, but its midpoint would then be the target for the balance too, a node with many peers would deposit for holds it may never fund, and a generous peer's hold toward us could fill it again.
- *Depositing everything, always:* simplest, but an owner with truly excess capital has every reason to keep it off a hub that could fail.
