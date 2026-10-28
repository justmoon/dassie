# A node shares what it can hold in a hub among its peers, core peers first and by what they earned it

**Areas:** [Settlement](../areas/settlement.md#credit), [Settlement hubs](../areas/hubs.md#in-the-node), [Network](../areas/network.md#downlinks).

## Facts

- Over a hub, the credit a node extends a peer is its hold toward the peer, funded when the peer asks to settle. A request funds the hold with what the node would have paid had it extended the default credit, counting the hold as paid in advance ([Settlement](../areas/settlement.md#credit)).
- What a node can hold toward its peers in a hub, the pool, is its balance there and its holds toward them. A node that keeps its [generous target](generous-hub-target.md) has far more than every peer's default credit, unless it has very many peers: the pool runs short when its owner's funds do.
- When the pool ran short, whoever asked first took what it asked, and the requests the node could not fund were answered later in node-ID order: a busy peer could wait behind an idle one, and a downlink take what a core peer needed.
- A payee lets what was in flight become claims, and lets claims add up to its redeem threshold before it redeems them. A hold that cannot carry the cap on money in flight with a peer and that threshold together never lets the link carry a full load.
- Core peers carry other nodes' traffic; downlinks carry their own ([Network](../areas/network.md#downlinks)). What a node earned in fees through a peer is money someone paid it, the measure the connector already weights next hops by ([weighted-next-hops](weighted-next-hops.md)).
- A hold the payee can take from, or a request funded and on its way, cannot be taken back by the node: it shrinks only as the payee takes from it or releases it.

## Decision

- **While the pool covers every peer's default credit, nothing caps a hold.** A request funds what the rule says, exactly as without allotments.
- **Otherwise each peer over the hub has an allotment,** a share of the pool (`engine/core/hub_allotment.rs`), worked out from the pool, the peers, whether each is a downlink, and what each earned the node on the hub's ledger:
  1. Every peer is brought to the **least hold**: the cap on money in flight and the redeem threshold (`policy::least_hold`), so a link can still carry a full load. Core peers first; while the pool cannot give a tier the least hold, its peers share what there is equally.
  2. What is left goes to the **core peers, up to the default credit,** a tenth of it spread evenly and the rest by what each earned the node: the same share of the remainder as the connector's draws that ignore earnings, so that a new or idle core peer still gets some and can start to earn, and a busy one gets most.
  3. What is left then goes to the **downlinks** the same way.

  Integer arithmetic; what does not divide goes a unit at a time in the order of priority, core peers by earnings and then downlinks, ties by node ID. No more is allotted than the pool, no peer more than the default credit.
- **The allotment caps the hold.** A request raises the hold toward the peer, its credit, at most to its allotment, and by no less than the least settlement or by nothing; the rule's size still counts the default credit. A request capped is kept and tried again at each step until its asker would ask again, since what frees room under the cap, the payee taking from the hold, changes nothing the scheme can spend ([settle-when-funds-arrive](settle-when-funds-arrive.md)).
- **A hold above a lowered allotment stays.** The node never takes back what it holds toward a peer: it shrinks as the peer takes from it.
- **Late requests go in the order of priority:** core peers by earnings, then downlinks, so that what arrives in the hub funds the peers that carry the most first.
- **What the node keeps spendable stays sized at the default credit** (`policy::hub_spendable`). An allotment only lowers what a request takes, so it stays the most the requests may ask at once. It owes nothing to the pool, so no deposit or withdrawal moves it, and the deposit it calls for while the pool is short is what brings the allotments back up.
- **The least default credit stays a floor on the default credit,** whatever the allotments (`policy::least_credit`): the rule a request is sized by still counts the default credit, so a link survives a crash's drift as before. The cap reads the hold, which the hub keeps and the books follow, not the books' drift. The least hold lies a trigger and more below the least default credit, so every default credit the node starts with covers it.
- **The owner sees each allotment** beside the node's hold toward that peer, in `GetHubStatus`, `dassie hub status` and the wallet's Hubs page.

## Accepted

- **A small allotment costs throughput.** With its hold at its allotment, a payee waits for its own redeems to free room under it, and each least settlement freed is funded on its own, a hub fee each. The pool is short, and a busier link would need funds the node does not have.
- **The pool counts what peers earned and have not taken yet,** which is still in the node's holds: until they take it, the allotments share a little more than the node's own.
- **Earnings decay and move,** so the order and the allotments move between requests. A peer that ranks lower later keeps what it holds.
- **A fixed tenth explores,** whatever the network's spread of earnings.
- **A peer may hold more than its allotment for a while,** after the pool shrank or others rose.

## Rejected

- *First come, first served:* the node shares nothing by what a peer carries, and a peer asking often takes what one asking rarely needs.
- *The allotment as the credit the rule counts,* in place of the default credit: a request at the trigger would need two triggers and twice the least settlement to fund anything, five packets, so at the least hold it funds nothing, and the hold would top out near a packet and a half, never the cap on money in flight.
- *By earnings alone, with no share for all:* a new peer, or one idle a while, gets the least hold and no more, and can never earn its way up.
- *Downlinks alongside core peers:* a node's downlinks are many and carry only their own payments, so a pool shared evenly would starve the core links everyone routes through.
- *Taking back holds above an allotment:* the node does not reduce its own holds yet.
- *Keeping spendable what the remaining allotments may ask:* tied to the pool, a withdrawal that lowers the pool would lower what the node keeps spendable, and a deposit raise it, a loop the target's step is meant to stay out of; and it would ask for no more than the default credit's sizing already covers.
