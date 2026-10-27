# A packet's next hop is drawn at random among its first hops, weighted by the fees each earned us

**Areas:** [Connector](../areas/connector.md#routing), [Network](../areas/network.md#routing), [Stream](../areas/stream.md#responses). Built: the draw over each hop's methods, the weights per peer and ledger, the exploring share, the owner's packets kept on their ledger, a converting hop's method with the best rate, and the keyed cut. Shown in the node simulation, not yet ([roadmap](../roadmap.md#random-next-hops)).

## Facts

- Routing keeps a few first hops per destination, on shortest paths, sorted by node ID, and the connector takes the first it can use. So every packet to a destination leaves through one peer, picked by its ID, and a payment follows one path for as long as the routes hold.
- Routing keeps at most a few first hops per destination. Cut by the lowest node IDs, the same few would be kept by every node, and anyone can grind a key until its node ID is low enough to be among them.
- One node on that path can stop a payment, by refusing its packets or holding them. A node whose exchange rate is worse than its alternatives' loses nothing: it carries every packet anyway.
- A STREAM receiver refuses a packet that delivers less than the sender's minimum. A sender that sends that money again over another path pays only for packets that got a rate it accepts, so a node that charges too much sees Prepares and Rejects and earns nothing.
- What a node booked is the only cheap measure of what a peer did for it that no one else controls. Fulfills and Rejects are whatever the nodes past the peer make them: an attacker peered with a node can send itself packets through it and reject or fulfill them as it likes. The fees a node earned on packets fulfilled through a peer are money someone paid it.
- A fee is booked in the units of the ledger the packet came in on. Summed across ledgers without exchange rates, an asset with a finer unit would outweigh the others.

## Decision

- **Drawn per packet.** The connector draws each packet's next hop at random among the destination's usable first hops (an endpoint, not the packet's source, not draining), from the node's random number generator, so the simulation stays deterministic. Nothing is sticky per destination or per payment: a packet sent again after a failure may take another path.
- **Only among the shortest paths' first hops,** so no path grows longer and no packet loops.
- **First hops are cut by a keyed hash.** Where more first hops reach a destination than a route keeps, the node keeps those whose node IDs rank lowest under an HMAC keyed by a secret derived from its own node key. Each node keeps its own few, the same after a restart, and no ID is favoured by every node.
- **Among the hops that keep the packet's ledger, where there are any** ([settlement-methods](settlement-methods.md)): the draw runs among the usable first hops with a method on the ledger the packet came in on, and the packet goes on that method. Only when none has one does it run among all usable first hops, and the hop drawn takes its method with the best rate in the price table. So a converting packet's hop is drawn too: a deterministic pick would send every converting packet to one node, which could stop them all.
- **Weighted, in one ledger's units, by the fees each earned us.** A weight belongs to a peer and a ledger: what the node earned in fees on packets that came in on that ledger, went to that peer on it, and were fulfilled, decaying over time. The draw among hops that keep the ledger compares weights on that ledger alone, so no exchange rate enters it. A converting packet's fee counts toward no next hop, and the draw among converting hops is not weighted: every usable hop has the same chance. Fees, not outcomes: a Reject counts for nothing, so no one can make a hop look bad, and making one look good costs the node's fee on every unit pushed through it.
- **A share of the draws explores.** A fixed share of the draws ignores the weights and gives every usable hop the same chance; the rest follow the weights, or give every hop the same chance while none has earned. A share, unlike a base weight added to the fees, needs no unit and does not fade as a busy node's fees grow, so a new or idle peer keeps a chance to earn. **The decay** takes a hop that stopped delivering back down. With no fee charged, the default, every draw is even.
- **Per peer and ledger, in memory,** whatever the destination, so bounded by the peers and their methods. A restart starts every weight from nothing: weights only pace routing.
- **The owner's own packets keep the ledger the route starts on.** A quote or a payment picks that ledger once, as the ledger of the first usable first hop by node ID, as now, and pays from it ([Payments](../areas/payments.md)); each of its packets then draws among the first hops with a method on that ledger, since the owner's first hop's method is on the ledger the owner pays from ([settlement-methods](settlement-methods.md), [owner-ledger](owner-ledger.md)).
- **Room is not a weight.** A hop short of room answers T04 and earns nothing while it does, but its weight falls only as earnings decay, far slower than a settlement restores room. So a STREAM sender still sees T04 and slows down, and a short shortage moves no traffic for good.

## Accepted

- **A payment's packets meet different rates.** Its receiver refuses the ones below the sender's minimum: Prepares spent on expensive paths are the price of rewarding cheap ones.
- **The exploring share always sends some packets to a poor hop.**
- **Converting packets are drawn evenly,** however their hops performed: weighing hops on different ledgers would need exchange rates.
- **Weight can be bought,** at the node's fee on every unit, and what is bought toward one destination counts toward every destination behind that peer. A hop bought up that then holds packets costs senders time and room in flight, not money, as their packets go again elsewhere.
- **Weights are forgotten at a restart.**

## Rejected

- *The first usable hop by node ID:* one path per destination, picked by ID, that a single node can stop, with nothing gained by a better rate.
- *Cutting first hops by the lowest node IDs:* every node keeps the same few, and a node that grinds its ID lands among them everywhere.
- *A cut keyed by the node's public ID:* anyone can compute it, so a node can still grind its ID to be kept by a node it targets.
- *Weights from Fulfills and Rejects:* an attacker peered with a hop sends itself packets through it and rejects them to ruin its standing, or fulfills them to raise it, for free.
- *Weights from the amount fulfilled:* with no fee charged, it costs nothing to push.
- *Weights per destination:* state grows with destinations times hops, for a gain not shown yet.
- *A draw per payment:* a packet sent again would meet the same bad node.
- *Weights read from the journal at each draw:* a cost per packet, for a measure that may be forgotten.
- *A base weight added to the fees:* it needs a unit on every ledger, and its share shrinks as fees grow with traffic, so a busy node stops trying its other hops.
- *Fees valued at spot, to weigh hops on different ledgers:* exchange rates in a routing weight, for the converting draws alone, which [settlement-methods](settlement-methods.md) keeps out of every measure per peer.
- *The best-rate method among all first hops when none keeps the ledger:* one node would carry every converting packet, and could stop them all.
