# Trust follows what a peer earned us

**Areas:** [Settlement](../areas/settlement.md), [Network](../areas/network.md)

## Facts

If every node extends every new peer the same credit at once, a newcomer can peer, spend the credit its peers extend, and leave, and auto-peering lets it do so again under a new identity.

## Decision

The node that asked to peer takes the risk at first, and trust grows only from what the peering earned. [Settlement](../areas/settlement.md#credit) states the rules.

- **Roles.** The node whose request was accepted is the **junior**, the node that accepted it the **senior**. The junior extends the default credit line; the senior extends nothing, so the junior pays in advance and can still be paid at once.
- **Growth by a share k of what a peer earned us.** A packet comes in through one peer and leaves through another, so it counts toward both: k must stay at most 50% so that the credit a node hands out never exceeds what it earned. k is 10%, for a margin.
- **What counts is the fee,** toward each peer of the packet on whose ledger the fee was booked. A packet that converts counts toward the peer it came from only: counting its fee toward the other would need our exchange rates. The spread's gain is not booked per packet, and does not count.
- **A cap,** counted in the largest packet like every threshold of a link ([thresholds-in-packets](thresholds-in-packets.md)).
- **Measured from the books,** never kept in counters, so a crash can forget fees not read yet but credits none twice.
- **Nothing is announced** ([own-fulfills](own-fulfills.md)): more credit only means the creditor asks later.

## Accepted

**Retirement.** A node that leaves can spend everything its peers trust it with and disappear. Each peer loses at most k of what it earned through that node. Fees default to 0, so trust grows only once an owner charges for passing payments on; until then a junior keeps settling in advance.

## Rejected

- *Growth by the amount forwarded:* at a low fee a node would extend more than it earned, and lose money to every peer that retires.
- *Growth with time:* time earns nothing.
- *The same credit both ways at peering:* a newcomer gets credit it did not earn from every node it peers with.
