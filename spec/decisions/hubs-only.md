# Peers settle only over settlement hubs

**Areas:** [Settlement hubs](../designs/settlement-hubs.md#11-choosing-a-hub), [Settlement](../areas/settlement.md), [Network](../areas/network.md#peering), [XRPL](../areas/xrpl.md), [Testing](../areas/testing.md). Not built yet ([roadmap](../roadmap.md#hubs-only)).

## Facts

- Settling directly is up to the node that owes. A peer can always walk away with what it owes, so every direct peering is unsecured credit, which only the credit a node extends bounds ([own-fulfills](own-fulfills.md)). Keeping that loss small takes a machinery of its own: juniors and seniors, credit that grows with earnings, a least credit, the mark of an end for room ([earned-trust](earned-trust.md), [ended-for-room](ended-for-room.md)).
- Over a hub, the hold is the credit, all of it and nothing more, and the payee is paid from it whether the payer cooperates or not ([settlement-hubs](settlement-hubs.md)). What a peer can cost is what is in flight beyond it, and the costs [the design](../designs/settlement-hubs.md#13-how-it-fits-the-node) lists.
- A peering has settlement methods, each on its own scheme, which change while it lasts ([settlement-methods](settlement-methods.md)), so a pair in several hubs can settle over each of them within one peering.
- With both ways to settle, every rule of credit, settlement and ending a peering exists twice, and so does every test and simulation of them.
- Only a hub's validators need to be close to each other, so that a block is final soon after it is proposed. A member's legs to its peers are ordinary peer legs, and the rules already state an allowance for them; a member slower than it learns of some packets late, a cost it books ([the design](../designs/settlement-hubs.md#4-peering-over-a-hub)).
- The schemes on the underlying ledgers also hold the owner's funds that pay into a hub and that the hub pays out to ([Settlement hubs](../areas/hubs.md#in-the-node)).
- Most tests and simulations of a node are not about settlement, and today run on a stub that settles directly, through the whole credit machinery.

## Decision

- **Peers settle only over settlement hubs.** Direct settlement leaves the code: no scheme on an underlying ledger peers any more, and the credit machinery that only direct peerings need goes with it. How direct settlement worked, and what it would take to bring it back, stays in a design doc of its own, written as it leaves.
- **The schemes on the underlying ledgers stay, as the owner's books.** They hold the owner's funds on each ledger, pay into a hub and take what it pays out, and never peer.
- **Every hub two nodes share is a settlement method of their peering** ([settlement-methods](settlement-methods.md)). A pair that shares no hub does not peer.
- **One hub per asset per node.** A node is in at most one hub for each asset and underlying ledger, and may be in hubs of several assets.
- **Members may be anywhere.** Only a hub's validators need to be close to each other. A node checks no distance before it joins a hub, and a member far from the validators books what it learns of too late, as the design has it.
- **Joining by being paid.** A node with no account in a hub is paid into it by a peer that owes it, which opens its account, as [the design](../designs/settlement-hubs.md#5-joining) has it; a node need not deposit before it can peer.
- **When a hub fails, its method becomes unavailable** on every peering over it. Nothing falls back to settling directly: a peering left with no method ends, as any does.
- **Tests that are not about settlement** run on a stub with unlimited trust, which bounds room but never asks to settle and never settles. Tests of settlement run on the stub of a hub's member, or on hubs in the simulation.

## Accepted

- **Liveness for safety.** A hub that stalls or shuts down cuts its members off from each other in its asset until they are in a working hub again, where direct settlement would have kept them paying, on credit.
- **A node pays to be in a hub:** the account reserve, the hub's fees, and the underlying ledger's fees to move funds in and out, before and while it peers at all.
- **Every peering trusts validators.** A node trusts its hubs' validators with the balance it keeps there, and the default list of validators is the root of trust for every peering, not only for those that chose a hub.
- **A member far from its hub's validators** loses packets it learned of too late more often, and pays for it itself.
- **Tests that run on the unlimited stub** show nothing about settlement; settlement's guarantees rest on the hub's tests and simulations alone.

## Rejected

- *Keeping direct settlement as a fallback when no hub is shared or a hub fails:* the unsecured credit the hubs remove would come back exactly when something has gone wrong, and with it the credit machinery and every test of it. A peering that pauses is preferred to one that can lose money: the owner gives up some liveness for safety.
- *A distance check for members, a node's own round trip to the validators:* it would only keep a far node from paying the cost of its own slow legs, and with hubs only it would leave a node far from every hub with no one to settle with.
- *Several hubs per asset at launch:* volume spreads thin, each deposit reaches fewer peers, a node splits its funds between hubs, and the limit per validator operator that it needs waits until running hubs show it is worth it.
