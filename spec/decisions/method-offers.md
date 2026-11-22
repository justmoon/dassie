# A peering's methods are what both nodes offer each other, and crossed requests are both taken

**Areas:** [Network](../areas/network.md#settlement-methods), [Peer protocol](../areas/peer-protocol.md#message-set), [Settlement](../areas/settlement.md). Built in the peer protocol (`crates/dassie-peer/src/node/offers.rs`) and the node's books; the asking path of the release before stays one release, for its nodes.

## Facts

- Who asked for a peering is something each side inferred from the requests it saw. Requests that cross, arrive twice, or lose their answer can leave both sides recording that the other asked. The node that asked was the only one to ask for further methods, so such a pair never added one: the simulation campaign found two such schedules, and Niq Dudfield's audit of open simulation failures traced both.
- Since peers settle only over hubs, who asked decides nothing about credit: each side's credit is its own hold, and a node without an account is paid in whichever asked ([hubs-only](hubs-only.md)).
- What a scheme exchanges for a peer is the same fact on both sides: over a hub, each side's membership; on the stub, its kind. A request and its acceptance swap it.
- A method one side recorded before the other could carry a Prepare the other refused; repairing that took a drop message, a queue of drops, and a rule that an acceptance after a drop adds nothing.
- A core peer's two accounts on a ledger open with its method, and stay for good, in a share of the ledger's accounts of their own ([account-budgets](account-budgets.md)). Two nodes with room for one more pair that each took a different scheme from the other ended with a pair each and no method.
- Each node keeps its own limits ([own-books](own-books.md)); its slot caps need not agree with its peer's.
- Two nodes that back off from each other on the same schedule try again in step: in a model of the crossing at the last pair, they converged one run in thirty until their backoff had jitter, and every run with it.
- A message tag may join a released protocol version; a node that does not know it answers 400 ([protocol-upgrades](protocol-upgrades.md)). Nodes update themselves daily, so a release cycle sees both.

## Decision

- **Each node offers each peer the methods it settles on with it** (`methodOffer`): the schemes, each with the scheme's own data, at a version of its own, with the newest of the peer's versions it has taken in. The answer is the recipient's own offer. A method is a scheme both offer that the node adopted. Nobody asks for one, so no side is the one that asks, and a node no longer runs one request for a method at a time with its backoffs.
- **An offer is a commitment.** A node adopts a scheme, opening its accounts, before anything it sends carries it while the peer's offer carries it too; one it cannot adopt leaves its offer for a backoff, with jitter. A scheme in an offer stays offered while it lasts, whatever the node's appetite for newcomers says since, so a peer that takes it finds it kept.
- **Room is kept before it is offered.** A scheme that would open a core peer's accounts is offered only while the core peers' share has a pair for it beside every pair the node's other offers and its peering attempt keep; the pair is kept until the method is adopted, or the scheme leaves the offer, as when the peer has seen it and its own offer does not carry it. A node short of room offers such schemes lowest scheme ID first, and asks a peering on the lowest too unless one opens no accounts, so two nodes short of room offer, and ask on, the same one.
- **No Prepare on a method before the peer has it:** a node sends on a method once the peer has seen an offer of ours as new as the one we held when we adopted it. The node that accepted a peering waits for the requester's first offer; the requester has the acceptance.
- **Versions belong to the pair,** kept on the settlement channel and never reset. A peering also keeps the newest version of the peer's it had taken in when it was made: an older offer, or a copy, from its time or an earlier peering's, is never taken.
- **Withdrawing is an offer without the method,** made once the node collected on it, as a method's winding down does, or at once where nothing can be collected; the peer drops it at once. Offered again only after a backoff, so a drop is never undone at once.
- **Crossed requests are both taken.** A request from a node we are asking is ours, as is the peering it makes, which takes the outbound place our attempt took and no inbound one; a request on a pair already peered offers its scheme. A peering takes an inbound place only if the node was not asking its peer when it was made.
- **The release before, for one release.** A node that answers an offer with 400 runs the release before: every method with it counts as confirmed, as it did then, it is offered no new one, a method it asks for (`methodAdd`) is taken if it would be offered, and one dropped is told with `methodDrop`, until it hears. The release after drops these, and their tags stay unused.

## Accepted

- The node that accepts a peering waits a round trip before its first Prepare.
- A node leaves a scheme out of an offer for a backoff after it could not adopt it, or after the peer turned down the room kept for it: local state, per peering, lost with a restart.
- Pairs of the release before and this one gain no new method while the network updates, a day or so, and a pair in two hubs converts what it would have carried on the second.
- The first method still comes from the peering request, which carries no offer, until the next protocol version folds the offer into it.
- A method one side adopted and the other could not, for want of data the scheme takes (a hub member's peer bound, records still unresolved), keeps the accounts the first opened, as every pair a peering opened does. Opening core peers' accounts only once value moves, as a downlink's do ([lazy-downlink-accounts](lazy-downlink-accounts.md)), would close it, and is a change of its own.
- More messages at a restart where the node's offer changed meanwhile: one per peering it changed for.

## Rejected

- *The lower node ID asks for further methods:* deterministic, but it keeps one side's protocol, its node-wide attempt, its backoffs and the drop repair.
- *Either side asks:* requests for the same method cross, and the races stay.
- *The link state as the offer:* offers differ per peer, by room, account budgets and refused data, and the gate needs an acknowledgment per peer.
- *No gate, a refused Prepare instead:* every new method would start with rejected packets, and no check could tell a race from a fault.
- *Offers as changes rather than whole sets:* a lost or reordered change needs repair; a whole set is idempotent.
- *Versions per peering:* a delayed offer of the pair's previous peering could drop the new one's first method, and end it.
- *The lower node ID's request winning a crossing:* a role each side infers can disagree, and nothing rests on it any more.
- *Refusal reasons per scheme on the wire:* nothing asks, so nothing backs off on them; the owner sees the node's own reasons.
- *Each node adopting in its own preference order at the last pair:* the two take different schemes, and neither can take the other's.
- *A new protocol version, `dassie/3`, for the offer in the peering request:* the network would split while it updates, or the code would run both machines for a release cycle; a new tag does neither.
