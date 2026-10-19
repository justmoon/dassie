# D28. A node that ends a peering collects first, and pays nothing

**Areas:** [Network](../areas/network.md), [Settlement](../areas/settlement.md), [Peer protocol](../areas/peer-protocol.md), [Connector](../areas/connector.md). Not built yet ([roadmap](../roadmap.md#ending-a-peering)).

## Facts

- On `main`, a termination is a two-sided exchange. Once it completes, each side settles everything its own books say it owes ([D12](D12-peering-lifecycle-and-liveness.md)).
- That rule assumes both sides pay at the end, and neither has a reason to. The node that ends a peering wants to collect all it can. It could always stop settling and send packets to an address it controls until it reaches the credit the peer extends. The node being ended wants to pay nothing once it knows.
- The node simulation found that the rule also loses money between honest nodes. A settlement the peer was making on request was still on its way when the peering ended. The node that ended it worked out its final settlement without that settlement, paid, and then owed the peer what arrived, with nothing left to settle it.

## Decision

Ending a peering is run by the node that ends it, which collects first and pays nothing. The node being ended stops at once and pays nothing either.

**The node that ends a peering:**

1. **Marks the peer as depeering, privately.** It tells no one, and changes nothing the peer can see: a peer it still advertised stays in its link state.
2. **While depeering, it takes nothing new from the peer:** it refuses the peer's new Prepares, with a temporary error as for a peer short of room, and settles nothing the peer asks for. It answers each request as usual, so that the refusal does not tell. It still sends its own Prepares through the peer: each Fulfill the peer forwards is paid for by its upstream, and is never settled. Fulfills and Rejects keep flowing both ways.
3. **Waits for its own settlements to the peer that are under way** (prepared, executing, pending in the ledger, or announced and not yet acknowledged). That money has left, and the peer should count it.
4. **Asks the peer to settle, once.** The answer to `settlementRequest` says how much the peer has on its way: a settlement already under way, which under the one-at-a-time rule is its whole answer, or the one it makes now, 0 for nothing. The peer answers as it would any request, so it settles half the way from where it stands to its own trigger, not everything it owes.
5. **Waits until that amount is booked,** or a time derived from how long a settlement takes to reach the ledger and be announced has passed. A peer whose answer carries no amount, as an older node's does, gets the whole of that time.
6. **Sends `peeringTermination`.** Once it is acknowledged, or the termination times out, the peering ends.

**The node being ended,** once `peeringTermination` arrives, ends the peering at once:
- it stops advertising the peer and routing to it, takes no new Prepares from it, and answers none of its settlement requests;
- Fulfills and Rejects for packets already in flight still flow both ways until those packets resolve or expire;
- it settles nothing.

**If both sides end the peering at once,** each answers the other's request as a depeering node does, settling nothing. The first `peeringTermination` to arrive makes its recipient the node being ended, which drops its own waits and ends the peering at once.

**Both sides keep the peer's accounts and the settlement channel,** for auditing and accounting. "Pays nothing" holds for this peering only: what the accounts still show carries into a later peering, where asking and settling as usual collect it.

**A settlement that arrives after the peering ended** is treated as any unexpected payment on the underlying ledger is today. A payment from a node we hold accounts with is credited to its account, once, as [D12](D12-peering-lifecycle-and-liveness.md) has it. On the XRP Ledger that is the ledger payment, which books the settlement. Its `settlement` message is acknowledged, so the sender's channel moves on.

**Every way a peering ends follows this,** whether the owner ends it, bans the node, or the node ends it for silence or for staying short of room. A step that needs an answer from an unresponsive peer times out.

## Accepted

- **Whoever is owed when a peering ends stays owed.** For the node being ended, that is at most the credit it extended ([D23](D23-each-node-books-its-own-fulfills.md)). Credit already bounds what a peer can take, so the rule does not add exposure: a node that wanted more could take it by routing packets to itself.
- **The last request collects only half the way** to the peer's trigger, as any request does. Asking more than once would collect more; by default the node asks once, for simplicity.
- **The end is no longer prompt.** It waits for the node's own settlements under way and for the peer's answer, so a slow ledger delays it.
- **The node being ended learns late.** Until `peeringTermination` arrives, it sees only refused Prepares and a settlement it was asked for.

## Rejected

- *Both sides settle what they owe at the end (`main`):* neither has a reason to. The rule also loses money when a settlement crosses the end.
- *Settle a late settlement back, once:* it fixes the crossing, but it still has the node that ends a peering pay, and moves money there and back.
- *Hold the final settlement while a request is open:* without the amount in the answer, the node must wait out a bound whenever the peer settles nothing.
- *The payer holds the end until its settlement is acknowledged:* a protocol change on the other side, and a ledger outage holds every termination.
- *Stop sending Prepares through the peer while depeering:* each Fulfill it forwards is money the node is paid upstream and never settles.
- *Announce the depeering first, by withdrawing the peer from the link state:* the link state floods to the peer, which would stop settling.
- *Book a settlement that arrives after the end as the owner's deposit:* the peer's account would still show it as owed, and a later peering would ask for it again.
