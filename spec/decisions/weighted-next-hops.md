# A packet's next hop is drawn at random among its first hops, weighted by the fees each earned us

**Areas:** [Connector](../areas/connector.md#routing), [Network](../areas/network.md#routing), [Stream](../areas/stream.md#responses). Not built yet ([roadmap](../roadmap.md#random-next-hops)).

## Facts

- Routing keeps a few first hops per destination, on shortest paths, sorted by node ID, and the connector takes the first it can use. So every packet to a destination leaves through one peer, picked by its ID, and a payment follows one path for as long as the routes hold.
- One node on that path can stop a payment, by refusing its packets or holding them. A node whose exchange rate is worse than its alternatives' loses nothing: it carries every packet anyway.
- A STREAM receiver refuses a packet that delivers less than the sender's minimum. A sender that sends that money again over another path pays only for packets that got a rate it accepts, so a node that charges too much sees Prepares and Rejects and earns nothing.
- What a node booked is the only cheap measure of what a peer did for it that no one else controls. Fulfills and Rejects are whatever the nodes past the peer make them: an attacker peered with a node can send itself packets through it and reject or fulfill them as it likes. The fees a node earned on packets fulfilled through a peer are money someone paid it; credit growth already reads them ([earned-trust](earned-trust.md)).

## Decision

- **Drawn per packet.** The connector draws each packet's next hop at random among the destination's usable first hops (an endpoint, not the packet's source, not draining), from the node's random number generator, so the simulation stays deterministic. Nothing is sticky per destination or per payment: a packet sent again after a failure may take another path.
- **Weighted by the fees each earned us.** A first hop's weight is a base weight plus what the node earned in fees on packets it sent to that peer and that were fulfilled, decaying over time. Fees, not outcomes: a Reject counts for nothing, so no one can make a hop look bad, and making one look good costs the node's fee on every unit pushed through it.
- **The base weight keeps every usable hop drawn,** so a new or idle peer can earn; **the decay** takes a hop that stopped delivering back down. With no fee charged, the default, every hop weighs the same.
- **Per peer, in memory.** A weight belongs to a peer, whatever the destination, so it is bounded by the peers. A restart starts every peer from the base weight: weights only pace routing.
- **Only among the shortest paths' first hops,** so no path grows longer and no packet loops. Where some of them keep the packet's ledger, the draw is among those ([settlement-methods](settlement-methods.md)).
- **The owner's own packets** draw only among the first hops on the ledger the owner pays from, since the owner's own endpoints never convert ([owner-ledger](owner-ledger.md)).
- **Room is not a weight.** A hop short of room answers T04 and earns nothing while it does, but its weight falls only as earnings decay, far slower than a settlement restores room. So a STREAM sender still sees T04 and slows down, and a short shortage moves no traffic for good.

## Accepted

- **A payment's packets meet different rates.** Its receiver refuses the ones below the sender's minimum: Prepares spent on expensive paths are the price of rewarding cheap ones.
- **The base weight always sends some packets to a poor hop.**
- **Weight can be bought,** at the node's fee on every unit. A hop bought up that then holds packets costs senders time and room in flight, not money, as their packets go again elsewhere.
- **Weights are forgotten at a restart.**

## Rejected

- *The first usable hop by node ID:* one path per destination, picked by ID, that a single node can stop, with nothing gained by a better rate.
- *Weights from Fulfills and Rejects:* an attacker peered with a hop sends itself packets through it and rejects them to ruin its standing, or fulfills them to raise it, for free.
- *Weights from the amount fulfilled:* with no fee charged, it costs nothing to push.
- *Weights per destination:* state grows with destinations times hops, for a gain not shown yet.
- *A draw per payment:* a packet sent again would meet the same bad node.
- *Weights read from the journal at each draw, as credit growth reads its fees:* a cost per packet, for a measure that may be forgotten.
