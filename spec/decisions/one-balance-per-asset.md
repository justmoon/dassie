# The owner holds one balance per asset, and the node manages the hub's part of it (the one balance shown not built)

**Areas:** [Settlement hubs](../areas/hubs.md#in-the-node), [Payments](../areas/payments.md#where-received-money-is-kept), [Wallet](../areas/wallet.md#pages), [Settlement](../areas/settlement.md#owner-books). The float, the sweep, the deposits on the owner's own account and a payment's deposit on demand are built; the one balance the owner sees waits on the [roadmap](../roadmap.md#one-balance-per-asset).

## Facts

- Peers settle only over hubs ([hubs-only](hubs-only.md)), so a node holds each asset it settles in on two ledgers: its account in the hub, which packets travel on, and the owner book on the asset's own ledger, which funds the hub and peers with nobody.
- The node already moves funds between the two by itself, keeping the hub account between a floor and a ceiling counted in largest packets ([Settlement hubs](../areas/hubs.md#in-the-node)), and its prices already count the two as one holding of the asset ([passive-rebalancing](passive-rebalancing.md)).
- The hub account is to the owner what a hold is to a peering: money the node sets aside so that it can settle, whose size follows what settling needs. Owners do not manage holds; they see what they have.
- Shown as two balances per asset, the hub account becomes the owner's to manage. Send asked which ledger to pay from, listed owner books, from which no route starts, and refused a payment the hub account alone did not cover although the asset did. Receive offered the hub's escrow with a destination tag, and a deposit with the wrong tag credits another member.
- What the node keeps in a hub is what it risks if the hub fails. Between the floor and the ceiling it is a float; everything beyond it is exposure that settling does not need.
- A deposit into the hub costs an XRP Ledger fee and the time it takes the hub to credit it, a few seconds; a withdrawal costs the hub's fee and the ledger's.

## Decision

- **The owner holds one balance per asset:** what the owner book holds, what the hub account holds and what is on its way between them. The hub account is the node's to manage, like its holds: its details are on the Hubs page, never a balance of its own.
- **The hub keeps a float, and the rest goes to the owner's own account.** Above the ceiling, the node withdraws to the owner book down to the midpoint; below the floor, it deposits up to it. There is no setting for where received money is kept.
- **Deposits go to the owner's own account,** on the asset's own ledger, never to the hub's escrow. The node moves into the hub what it needs there. The hub ledger names no deposit address.
- **A payment may spend the whole balance.** One the hub account does not cover waits while the node deposits the shortfall from the owner book, as a hold is funded on request, within the payment's deadline.
- **Send pays in an asset,** not from a ledger: the node pays from the hub's ledger, where the route starts.

## Accepted

- **A payment that needs a deposit is slower** by the deposit's time, and pays its fee.
- **What the node receives beyond the ceiling costs a withdrawal** each time the hub passes it, where keeping it in the hub cost nothing until it was spent.
- **A deposit is spendable only once the node moved it into the hub,** a few seconds and a fee later, where a deposit to the escrow was spendable once the hub credited it.
- **An owner who wants more in the hub cannot say so** other than by moving funds on the Hubs page, which pauses the node's own moves for a while.

## Rejected

- *Two balances per asset, with the owner choosing where received money is kept,* in the hub by default: the owner manages what the node should, a payment fails that the asset covers, everything received is exposed to the hub until the owner says otherwise, and the deposit with a tag lets a mistyped tag credit someone else.
- *Keeping the choice as a policy of the node's own moves,* how much it may keep in the hub: one balance shown, but a second way to set what the floor and the ceiling already bound.
- *Spending only what the hub account holds:* the balance shown would be more than a payment could spend at once.
- *Deposits to the hub's escrow:* money arrives spendable sooner, but only with the tag, which wallets and exchanges outside Dassie get wrong.
- *Turning the node's own moves off to keep money in the hub:* the floor would no longer be kept, and a hub account that runs short stops settling.
