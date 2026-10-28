# A swap short of the node's own room waits for it to grow; one the peer is short for backs off for at most a hub landing

**Areas:** [Connector](../areas/connector.md#the-swap-service), [Settlement](../areas/settlement.md#settlement-hubs), [Settlement hubs](../areas/hubs.md#packets-promises-and-claims). How a swap of [peer-swaps](peer-swaps.md) waits for room.

## Facts

- Peers settle only over hubs ([hubs-only](hubs-only.md)). A node's room to pay a peer on a method is its own hold toward the peer there, less what it owes on it and what its packets in flight promise, within its own cap on money in flight there ([packets-in-flight](packets-in-flight.md)). It funds the hold when the peer asks, at the peer's trigger, and the peer asks again as the node's packets bring it back there ([Settlement](../areas/settlement.md#asking-a-peer-to-settle)).
- The node knows its own room: the hub's scheme keeps what its hold has room for, and the connector what its packets in flight carry. It does not know the peer's. The way back of a swap is the peer's to fund, and a peer's T04 or T05 does not say which of its rooms it lacked: its hold toward the node, its hub's balance or its cap.
- A swap service that backs off as STREAM does, from a tenth of a second doubling to half a minute, waits out its backoff after its room came back, while a hub funding lands in under a second. A model of the rebalancing swapped up to 1.8 times as fast over slow hubs when it tried again as room came back, and on the `peer-swaps` end-to-end flow the two nodes swapped 1.5 to 1.7 times as fast, and took about half as long to swap their first 50 largest packets.
- A hub member waits a second for an operation it sent to land (`RESUBMIT_MS`) before it sends it again: past that, it takes the operation as lost.

## Decision

- **Room on the node's own way out: wait for it to grow.** A T04 or T05 the node gave its own swap before it left is for want of its own room on the method it pays on. That method waits, sending nothing, until the room grows from what it was when the swap went: the hold has more room, as a funding lands or a packet is rejected, less is in flight, or the method comes back. The swap goes in the step that sees it. The peer is not backed off from, and its other methods go on.
- **At most STREAM's longest backoff,** for a cause the room does not show, such as the owner's funds on that ledger.
- **Room on the peer's side: back off, for at most a hub landing.** A T04 or T05 from the peer, or from the way back, backs off from the peer as STREAM does, the first wait, then twice the last, but never longer than a hub member waits for an operation to land. The peer's room comes back as a funding lands on its hub or its packets resolve, both within that.
- **Every other temporary or relative error backs off as STREAM does,** up to its longest wait: a peer that cannot be reached or does not answer is no question of room.

## Accepted

- **A retry as room grows may find too little:** it is refused again at once, before it leaves, and waits again. Each growth costs at most a round of local refusals, never a packet to the peer.
- **A peer that stays short gets a round of swaps a landing:** at most `PACKETS_PER_PEER` packets a second, each holding room on both sides for a round trip.
- **The longest wait on the peer's side follows the hub's rule,** not the network the node is in: a hub that lands faster still gets a try a second at worst.

## Rejected

- *STREAM's backoff for every T04 and T05:* it waits out its backoff after room came back, a growing share of the time as settlement keeps up.
- *A short backoff for every refusal:* the node's own room would be tried blind, and still late by up to a wait after its funding landed.
- *A longest wait tuned to the landings measured:* it would fit one network's hubs; a hub member's wait for an operation is a rule every hub runs.
- *Learning the peer's landings from the node's own fundings on that hub:* state per hub for a bound the hub's rule gives, and a peer's T04 may be for its hub's balance, which a deposit on the XRP Ledger refills, not the hub.
- *Ending the backoff when the peer's hold toward the node grows,* which the node sees as payee: the peer's refusal does not say it lacked that hold, and the peer may not yet have read its funding when the node does.
