# D20. Trust follows what a peer earned us

**Status:** active, decided 2026-10-16; what counts as earned set 2026-10-19.
**Areas:** [Settlement](../areas/settlement.md), [Network](../areas/network.md)

## Facts

In the original, every node extends every peer the same credit, 1.1 units, from the moment they peer, and it never changes. A newcomer can peer, spend the credit its peers extend, and leave; auto-peering lets it do so again under a new identity.

## Decision

The node that asked to peer takes the risk at first, and trust grows only from what the peering earned. [Settlement](../areas/settlement.md) states the numbers.

- **Roles.** The node whose request was accepted is the **junior**, the node that accepted it the **senior**. The junior extends the default credit line; the senior extends nothing, so the junior pays in advance and can still be paid at once.
- **Growth.** Each side raises the credit it extends by a share k of what it earned on packets through that peer. A packet comes in through one peer and leaves through another, so it counts toward both: k is at most 50%, so that the credit a node hands out never exceeds what it earned. k is 10%, for a margin.
- **What counts, for now:** the fee, toward each peer of the packet on whose ledger it was booked. The spread's gain is not booked per packet, and does not count yet.
- **A cap,** counted in the largest packet like every threshold of a link ([D25](D25-a-links-thresholds-are-counted-in-its-largest-packet.md)).
- **Measured from the books,** never kept in counters, so a crash can forget fees not read yet but credits none twice.
- **Nothing is announced** ([D23](D23-each-node-books-its-own-fulfills.md)): more credit only means the creditor asks later.

## Accepted

**Retirement.** A node that leaves can spend everything its peers trust it with and disappear. Each peer loses at most k of what it earned through that node. Fees default to 0, so trust grows only once an owner charges for passing payments on; until then a junior keeps settling in advance.

## Rejected

- *Growth by the amount forwarded:* at a low fee a node would extend more than it earned, and lose money to every peer that retires.
- *Growth with time:* time earns nothing.
- *The same credit both ways at peering,* as the original does: a newcomer gets credit it did not earn from every node it peers with.
