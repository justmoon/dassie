# A node peers on a scheme only once it has seen its ledger

**Areas:** [Settlement](../areas/settlement.md), [Network](../areas/network.md), [Settlement hubs](../areas/hubs.md#in-the-node).

## Facts

- A scheme that follows a ledger books what its account holds at its first sync as the owner's deposit. From then on it reads the account's transactions from a cursor kept with its state, and tells a peer's settlement from a deposit by who sent it. A payment that reached the ledger before the first sync is part of what the account held: the owner's.
- An honest peer settles only when asked. The routine request waits until the scheme sees its ledger. The last request of a node that ends a peering ([ending-a-peering](ending-a-peering.md)) does not: an end, a ban included, must finish in bounded time.
- So a node whose ledger could not be reached at its first start could peer, end the peering, and have the peer's answer booked as its owner's deposit, while its books still showed the peer owing that amount. [ending-a-peering](ending-a-peering.md) rules this outcome out. The node simulation reproduced it: the owner gained more than the node received.
- The first sync happens once per account. After it, a restart with the ledger out of reach loses nothing: once the ledger is back, the scheme reads what happened meanwhile from its cursor.

## Decision

A node offers, asks for and accepts peerings on a settlement scheme only once the scheme has synced with its ledger at least once (`has_synced`). Until then:

- its link state leaves the scheme out, so no node picks it as a candidate on it;
- it picks no candidate on it;
- it refuses a request on it as for a scheme not active here, so the requester backs off as from any refusal and asks again once the link state lists the scheme.

The first sync is kept with the scheme's state, so a restart with the ledger down peers as before. Peerings already made stay. A scheme without a ledger to follow, such as the stub, has synced from the start.

## Accepted

- **A node whose ledger is unreachable at its first start cannot peer on that scheme until it is.** A node in one settlement hub alone stays without peers until it has seen the hub's ledger. No XRPL scheme peers ([hubs-only](hubs-only.md)), so a node that has not seen the XRP Ledger peers all the same.
- **A testnet reset starts the account from nothing** and drops the cursor, while peerings on the scheme stay; the node takes no new ones until it has synced again. The reset takes the peers' accounts too, so a peer can hardly pay into it meanwhile.

## Rejected

- *Gate only the last request, with a time limit:* the node ending a peering would wait for its ledger, and past the limit either ask anyway, which brings the loss back for a long outage, or end without asking, which gives up the collection [ending-a-peering](ending-a-peering.md) has it make. A peering made before the first sync would still be exposed to every other payment that reaches the ledger before it.
- *Gate both peering and the last request:* with no peering before the first sync, the last request has nothing to wait for but a testnet reset, and the wait would add a state to every termination for it.
- *Find peers' settlements in the account's history at the first sync:* a server's history may not reach back, and nothing booked before the first sync can be checked against it.
