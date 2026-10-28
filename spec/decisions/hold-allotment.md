# A node shares what it can hold in a hub among its peers, core peers first and by what they earned it

**Areas:** [Settlement](../areas/settlement.md#credit), [Settlement hubs](../areas/hubs.md#in-the-node), [Network](../areas/network.md#downlinks).

## Facts

- Over a hub, the credit a node extends a peer is its hold toward the peer, funded when the peer asks to settle. A request funds the hold with what the node would have paid had it extended the default credit, counting the hold as paid in advance ([Settlement](../areas/settlement.md#credit)).
- What a node can hold toward its peers in a hub, the pool, is its balance there and its holds toward them. Funding a hold also costs the hub's fee, and a new hold locks a reserve.
- The node keeps [a target in its hub](generous-hub-target.md), which does not follow its peerings. A node with more peers than its target covers at the default credit, as a core node with many downlinks, has a pool short of every peer's default credit while its balance holds plenty, and its funding calls for no deposit.
- When the pool ran short, whoever asked first took what it asked, and the requests the node could not fund were answered later in node-ID order: a busy peer could wait behind an idle one, and a downlink take what a core peer needed.
- A payee lets what was in flight become claims, and lets claims add up to its redeem threshold before it redeems them. A hold that cannot carry the cap on money in flight with a peer and that threshold together never lets the link carry a full load.
- Core peers carry other nodes' traffic; downlinks carry their own ([Network](../areas/network.md#downlinks)). What a node earned in fees through a peer is money someone paid it, the measure the connector already weights next hops by ([weighted-next-hops](weighted-next-hops.md)).
- Most peers are idle at any time. A share set aside for each peer, whether it asks or not, sits in the balance.
- A hold the payee can take from, or a request funded and on its way, cannot be taken back by the node: it shrinks only as the payee takes from it or releases it.

## Decision

- **While the pool covers every peer's default credit, nothing caps a hold.** A request funds what the rule says, exactly as without allotments.
- **Otherwise each peer over the hub has an allotment,** a share of the pool (`engine/core/hub_allotment.rs`), worked out from the pool, the peers the node still funds (not one it is ending, nor a method winding down), whether each is a downlink, and what each earned the node on the hub's ledger:
  1. Every peer is brought to the **least hold**: the cap on money in flight and the redeem threshold (`policy::least_hold`), so a link can still carry a full load. Core peers first; while the pool cannot give a tier the least hold, its peers share what there is equally.
  2. What is left goes to the **core peers, up to the default credit,** a tenth of it spread evenly and the rest by what each earned the node: the same share of the remainder as the connector's draws that ignore earnings, so that a new or idle core peer still gets some and can start to earn, and a busy one gets most.
  3. What is left then goes to the **downlinks** the same way.

  Integer arithmetic; what does not divide goes a unit at a time in the order of priority, core peers by earnings and then downlinks, ties by node ID. No more is allotted than the pool, no peer more than the default credit.
- **A peer can always count on a largest packet, and may take more from what the others leave unused.** The allotment is what a peer gets while every peer asks. What it can always count on is its **reserve**: a largest packet (`policy::minimum`), or, while the pool cannot give every peer one, its equal share of the pool, core peers first. A request raises the hold toward the peer up to the default credit, as the rule asks, as far as what the hub account can spend leaves once every other peer's reserve is set aside: what its hold still lacks of it. A packet is the least a link settles with, and what an idle peer that turns busy needs to start at once; it grows from what is spare. So an idle peer's share strands nothing a busy one could use, since the target does not grow with the peerings to make up for it, and no peer takes another's packet. Under true scarcity, when the reserves take the whole pool, nothing is spare, and the pool splits equally, core peers first.
- **A peer pays its own hold's opening,** the hub's fee and a new hold's reserve, out of what it gets. One opening stays spendable for the peers that hold nothing yet: a peer that already holds something never takes it, and the first of them to ask does.
- **A request capped this way is kept,** says so (`RequestAllotted`, not `RequestShort`), and is tried again at each step until its asker would ask again, since what frees room, another peer's hold filling its reserve or the funds arriving, may change nothing the scheme can spend ([settle-when-funds-arrive](settle-when-funds-arrive.md)).
- **A hold above a lowered allotment stays.** The node never takes back what it holds toward a peer: it shrinks as the peer takes from it.
- **Late requests go in the order of priority:** core peers by earnings, then downlinks, so that what arrives in the hub, and what is spare, funds the peers that earn the most first.
- **What the node keeps spendable stays sized at the default credit** (`policy::hub_spendable`). The allotments only lower what a request takes, so it stays the most the requests may ask at once. It owes nothing to the pool, so no deposit or withdrawal moves it.
- **The least default credit stays a floor on the default credit,** whatever the allotments (`policy::least_credit`): the rule a request is sized by still counts the default credit, so a link survives a crash's drift as before. The cap reads the holds and what the account can spend, which the hub keeps and the books follow, not the books' drift. The least hold lies a trigger and more below the least default credit, so every default credit the node starts with covers it.
- **The owner sees each allotment** beside the node's hold toward that peer, in `GetHubStatus`, `dassie hub status` and the wallet's Hubs page.

## Accepted

- **Idle peers' packets stay set aside, and their openings do not.** With many peers and a pool short of their default credit, the busy ones share what is left above a packet for every other peer: at a thousand packets and two hundred peers, eighty reach the default credit and the other hundred and twenty can count on their packet, before what opening the holds costs. Each hold's opening, at the hub's reserve of two packets, comes from what is spare: then about sixty-six reach the default credit, and once the spare is spent, a peer that holds nothing yet keeps its packet set aside but can open its hold only once an opening's worth is spare again, from what the node earns or deposits.
- **A small reserve costs throughput.** With its hold at what the others leave it, a payee waits for its own redeems, or another's hold to fill, to free room, and each least settlement freed is funded on its own, a hub fee each.
- **First come, first served above the reserves,** for requests as they come; only the requests answered late go in the order of earnings.
- **The pool counts what peers earned and have not taken yet,** which is still in the node's holds: until they take it, the allotments share a little more than the node's own.
- **Earnings decay and move,** so the order and the allotments move between requests. A peer that ranks lower later keeps what it holds.
- **A fixed tenth explores,** whatever the network's spread of earnings.
- **A peer may hold more than its allotment,** after it took what others left, the pool shrank or others rose.

## Rejected

- *First come, first served for all of it:* the node shares nothing by what a peer carries, and a busy peer can take the last packet another needs.
- *The allotment as a hard cap on the hold:* idle peers' shares sit in the balance while busy peers stay capped below what the rule would fund, and no deposit comes, since the balance is plentiful; a node with more peers than its target covers would fund its busy peers less than first come, first served did.
- *Depositing beyond the target for requests the allotments cap:* the target would grow with the peerings, against [generous-hub-target](generous-hub-target.md), and a node's appetite for risk would follow how many peers it has.
- *Setting aside every other peer's least hold:* what each could count on would be a full load, but idle peers' least holds would sit in the balance: at a thousand packets and two hundred peers, about fifteen busy peers reach the default credit.
- *Setting aside each idle peer's opening as well:* every peer could then always open its hold, but a reserve per peer that has no hold yet, set aside whether it ever asks or not, strands again what most peers never lock: at a thousand packets and two hundred peers, forty reach the default credit.
- *Setting aside every other peer's whole allotment:* what an idle peer was allotted by its earnings and the exploring tenth would sit unused again.
- *The allotment as the credit the rule counts,* in place of the default credit: a request at the trigger would need two triggers and twice the least settlement to fund anything, five packets, so at the least hold it funds nothing, and the hold would top out near a packet and a half, never the cap on money in flight.
- *By earnings alone, with no share for all:* a new peer, or one idle a while, gets the least hold and no more, and can never earn its way up.
- *Downlinks alongside core peers:* a node's downlinks are many and carry only their own payments, so a pool shared evenly would starve the core links everyone routes through.
- *Taking back holds above an allotment:* the node does not reduce its own holds yet.
- *Keeping spendable what the remaining allotments may ask:* tied to the pool, a withdrawal that lowers the pool would lower what the node keeps spendable, and a deposit raise it, a loop the target's step is meant to stay out of; and it would ask for no more than the default credit's sizing already covers.
