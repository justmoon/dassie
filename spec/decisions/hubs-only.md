# Peers settle only over settlement hubs

**Areas:** [Settlement hubs](../designs/settlement-hubs.md#11-choosing-a-hub), [Settlement](../areas/settlement.md), [Network](../areas/network.md#peering), [XRPL](../areas/xrpl.md), [Testing](../areas/testing.md). Built; several hubs of one asset per node wait on the [roadmap](../roadmap.md#settlement-hubs).

## Facts

- Settling directly is up to the node that owes. A peer can always walk away with what it owes, so every direct peering is unsecured credit, which only the credit a node extends bounds ([own-fulfills](own-fulfills.md)). If every node extends every new peer the same credit at once, a newcomer can peer, spend the credit its peers extend, and leave, and auto-peering lets it do so again under a new identity.
- Keeping that loss small takes a machinery of its own: the node that asked to peer extends credit and the node that accepted none, credit that grows by a share of the fees a peer earned us, up to a cap, a least credit that a crash's drift leaves the link, and a mark on the settlement channel of an end for room, so that a peer that never settled gets no new credit by being asked again.
- Over a hub, the hold is the credit, all of it and nothing more, and the payee is paid from it whether the payer cooperates or not ([settlement-hubs](settlement-hubs.md)). What a peer can cost is what is in flight beyond it, and the costs [the design](../designs/settlement-hubs.md#13-how-it-fits-the-node) lists.
- A peering has settlement methods, each on its own scheme, which change while it lasts ([settlement-methods](settlement-methods.md)), so a pair in several hubs can settle over each of them within one peering.
- With both ways to settle, every rule of credit, settlement and ending a peering exists twice, and so does every test and simulation of them.
- Only a hub's validators need to be close to each other, so that a block is final soon after it is proposed. A member's legs to its peers are ordinary peer legs, and the rules already state an allowance for them; a member slower than it learns of some packets late, a cost it books ([the design](../designs/settlement-hubs.md#4-peering-over-a-hub)).
- The schemes on the underlying ledgers also hold the owner's funds that pay into a hub and that the hub pays out to ([Settlement hubs](../areas/hubs.md#in-the-node)).
- Most tests and simulations of a node are not about settlement, yet on a stub that settles directly they would run through the whole credit machinery.

## Decision

- **Peers settle only over settlement hubs.** Direct settlement leaves the code: no scheme on an underlying ledger peers any more, and the credit machinery that only direct peerings need goes with it. How direct settlement worked, and what it would take to bring it back, stays in [a design doc of its own](../designs/direct-settlement.md).
- **The schemes on the underlying ledgers stay, as the owner's books.** They hold the owner's funds on each ledger, pay into a hub and take what it pays out, and never peer.
- **Every hub two nodes share is a settlement method of their peering** ([settlement-methods](settlement-methods.md)). A node is in a hub once it has chosen it, whether or not it has an account there yet. A pair that shares no hub does not peer.
- **One hub per asset per node.** A node is in at most one hub for each asset and underlying ledger, and may be in hubs of several assets.
- **Members may be anywhere.** Only a hub's validators need to be close to each other. A node checks no distance before it joins a hub, and a member far from the validators books what it learns of too late, as the design has it.
- **Joining by being paid.** A node with no account in a hub is paid into it by its peer, whichever of them asked to peer, as [the design](../designs/settlement-hubs.md#5-joining) has it; a node need not deposit before it can peer. Until then it takes its peer's packets on what the peer owes it, and spends only that, through the same peer. The peer opens its account with `pay` only once what it owes the node reaches the hub's minimum for a new account, and pays in no more than it owes. No node extends credit to a node without an account: outside a hold, the only credit is what the new member is owed until it is paid in.
- **When a hub fails, its method becomes unavailable** on every peering over it. Nothing falls back to settling directly: a peering left with no method ends, as any does.
- **Tests that are not about settlement** run on a stub with unlimited trust, which bounds room but never asks to settle and never settles. Tests of settlement run on the stub of a hub's member, or on hubs in the simulation.

## Accepted

- **Liveness for safety.** A hub that stalls or shuts down cuts its members off from each other in its asset until they are in a working hub again, where direct settlement would have kept them paying, on credit.
- **A node pays to be in a hub:** the account reserve, the hub's fees, and the underlying ledger's fees to move funds in and out. A node paid into a hub pays its reserve out of what it is paid.
- **A node paid into a hub risks what it is owed until then,** up to the hub's minimum for a new account, if its peer goes away first. The peer that pays it in risks nothing, and a node that cycles fresh keys through a peer pays each key's minimum itself, which it gets back only less the hub's fees.
- **Every peering trusts validators.** A node trusts its hubs' validators with the balance it keeps there, and the default list of validators is the root of trust for every peering, not only for those that chose a hub.
- **A member far from its hub's validators** loses packets it learned of too late more often, and pays for it itself.
- **Tests that run on the unlimited stub** show nothing about settlement; settlement's guarantees rest on the hub's tests and simulations alone.

## Rejected

- *Keeping direct settlement as a fallback when no hub is shared or a hub fails:* the unsecured credit the hubs remove would come back exactly when something has gone wrong, and with it the credit machinery and every test of it. A peering that pauses is preferred to one that can lose money: the owner gives up some liveness for safety.
- *A distance check for members, a node's own round trip to the validators:* it would only keep a far node from paying the cost of its own slow legs, and with hubs only it would leave a node far from every hub with no one to settle with.
- *Several hubs per asset at launch:* volume spreads thin, each deposit reaches fewer peers, a node splits its funds between hubs, and the limit per validator operator that it needs waits until running hubs show it is worth it.
- *A starting credit until the `pay`, when the node without an account asked to peer:* unsecured credit, bounded only by the credit machinery this decision removes.
- *Unsecured credit that grows with what a peer earned us,* on top of a hold or in place of it. Growth by a share of the fees bounds what a retiring peer costs each node to that share of what it earned through it, while growth by the amount forwarded or with time hands out credit nothing earned; but either brings back the trust a hub removes, and every rule and test of the credit machinery with it.
- *A `pay` before the first packet, whatever is owed:* anyone could cycle fresh keys through a peer, have it pay each one the hub's minimum for a few drops' worth of packets, and `close` the accounts to take the minimum out, a drain on the peer at a near-zero cost per key.
