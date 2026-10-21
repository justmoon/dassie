# A node takes no Prepare from a peer it cannot reach

**Areas:** [Connector](../areas/connector.md), [Peer protocol](../areas/peer-protocol.md), [Settlement](../areas/settlement.md).

## Facts

- A Prepare and its Fulfill or Reject are separate one-way messages: each side sends its own requests. A link can be cut one way, as behind an asymmetric firewall: one node's requests reach its peer and are answered, while the peer's requests to it never arrive.
- Each node books the Fulfills it sends as final ([own-fulfills](own-fulfills.md)). Across such a cut, the peer that cannot reach us fulfills our Prepares and books them; its Fulfills never arrive, our packets expire, and we reject them upstream. Each such packet drifts the two books apart by its amount.
- The cap on money in flight bounds what is outstanding at once ([packets-in-flight](packets-in-flight.md)), not the drift a cut adds up: a packet that expired is no longer in flight, and the sender's STREAM tries again. So the books drifted by a packet a try, until the side that could not reach its peer had no room left for it, which may be the whole credit both sides extend.
- A link survives drift of up to its credit less two triggers and twice the least settlement ([Settlement](../areas/settlement.md#credit)). The node simulation cut a link one way for ten minutes while a payment crossed it: the books drifted by eight packets, past what the default credit survives, and settlement between the two stalled for good, the creditor asking and the debtor's books leaving it nothing to settle.

## Decision

A node takes no new Prepare from a peer its messages surely do not reach.

- **It stops** once an ILP packet of ours to the peer, a Prepare, Fulfill or Reject, surely did not arrive (not delivered). Only a packet counts: the protocol's other requests may be refused by our own bounds on requests in flight, which says nothing of the peer, and packets take no place in those bounds.
- **It resumes** once the peer answers any request of ours. Something always tries: the heartbeat goes to every peer every few seconds, and the Reject to each Prepare refused is sent again until it arrives or a bounded time passes.
- **The refusal is T04,** temporary, before the connector's other checks, as a depeering node's is ([ending-a-peering](ending-a-peering.md)). Our Prepares still go to the peer: one that does not arrive fails at once.
- **It is its own state,** apart from [ending-a-peering](ending-a-peering.md)'s refusal: either refuses, and lifting one leaves the other. A peering being made starts reached, since the two just answered each other; a peering that ends forgets it. It is not persisted: a restart tries the peer again.

So a cut one way drifts the books by at most what was in flight when it began, as one crash does, and the [least credit](../areas/settlement.md#credit) covers it.

## Accepted

- **A lost packet refuses for a moment.** One Fulfill or Reject lost on the way refuses the peer's Prepares until the next request it answers, about a second later when the packet goes again. The sender sees T04 and tries again.
- **What was in flight still drifts.** Packets fulfilled before the first of our messages fails are booked on our side only, up to the cap on money in flight each time a link is cut.
- **Packets only, not every request,** so a peer whose packets have not failed yet is still taken while our heartbeats to it fail. Its first packet tells.
- **The peer is not withdrawn.** It stays in our link state and its routes stay: a failed delivery withdraws no peer, since withdrawal turns a temporary T04 into a final F02 for the whole network ([peering-lifecycle](peering-lifecycle.md)). Packets routed to it are refused at its first hop instead.

## Rejected

- *The upstream notices:* the node whose Prepares are acknowledged but never answered before their expiry would stop routing to that peer. It needs a count of such expiries, learns only after a packet expired, and withdrawing the peer is network-wide, which [peering-lifecycle](peering-lifecycle.md) avoids for failed deliveries.
- *Accept the drift, and have the simulation forgive a link that drifted past what it survives:* [peering-lifecycle](peering-lifecycle.md) accepts that such a link is withdrawn and, after a day, ended. But a cut one way, not a fault of either node, would then eat a link's whole liquidity, and the checks would hide it.
- *Raise the default credit:* the drift across a cut grows with time, so no credit covers it.
