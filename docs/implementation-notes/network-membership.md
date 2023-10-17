# Implementation Notes: Network Membership

## Context

Every peer-to-peer network has to contend with the Sybil attack - the ability of an attacker to create large numbers of nodes and join the network, creating scalability issues, outvoting honest nodes, and other such issues.

To prevent such attacks, we need some way to distinguish the real nodes from the attacker's fake nodes. Maybe the attacker has located all of their fake nodes in the same data center. Or perhaps they are all running the same software version. There may be any number of differentiating factors that are easier or harder to fake. However, this is fundamentally a losing game. Every new metric we add is merely a slightly taller hurdle for a would-be attacker to clear. Meanwhile, we are continuously adding more and more complexity to our protocol.

In order to solve this problem from first principles, let's consider two networks which are equivalent in every way, expect one is the "real" network and one is a "fake" network set up by an attacker. We have to recognize the fundamental (rather than incidental) difference between the two network. And this difference is that the real network is the one which contains those nodes which we are looking to transact with.

The perfect definition would be the one which lists all the real nodes by public key. Such a list would allow us to perfectly distinguish between real nodes and fake nodes. Of course, in practice, such a list would be constantly changing and therefore impossible to keep track of by hand. And even then, the nodes that any given participant wants to transact with is going to be different for each participant.

Fortunately, we don't need a perfect solution, only a good enough approximation.

## Solution

- We define a list of bootstrap nodes which is occasionally updated by the developers. These nodes are considered "mostly good", i.e. we assume that a majority of these nodes is honest. The list can be changed by the user and we call it the Bootstrap Node List (BNL).
- Each node maintains a list of all known nodes called the Known Node List (KNL). Node A will add a new node B to its KNL for any one of the following reasons:

  1. Any node on node A's BNL is automatically added to its KNL.
  2. When node B sends a request to node A asking to be added and pays an associated fee. The fee grows with the size of node A's KNL.
  3. When a majority of nodes on A's BNL adds node B.

- When a new node is launched, it starts out with the BNL provided by the developers and an empty KNL. It will then download the KNLs from each node on its BNL and populate its own KNL using the set of nodes that appears either on a majority of KNLs or on its own BNL.

- If a node B was added to node A's KNL via method 2 (direct request) but node B does not appear on a majority of the KNLs of node A's BNL after some long delay, then node A will eventually remove node B from its KNL.

- If a node A has not seen a signed node state update from node B after some long timeout, it will remove node B from its KNL even if it is still present on other node's KNLs. What happens next depends on which of the following occurs first:
  1. If it receives a fresh node status update, the node will be re-added to the KNL.
  2. If the node is present on less than a majority of BNL nodes' KNLs, the node is removed permanently.

- In order for a new node to join the network's KNLs, it first queries all of the nodes on its BNL to ask how much they charge to add a new node to their KNL. It then sends paid requests to the cheapest 75% of them.

## Rationale

### Why have KNLs at all? Why not just have BNLs only?

It's important to provide an automated way for new nodes to join the network. Otherwise there is a significant barrier to entry. Most nodes that are stable and online for a long time should eventually be added to the BNL. But in order to build up that track record those nodes need to spend some time on the KNL first.

In short, the KNL exists:
- to have a recruiting pipeline for the BNL
- to make joining the global routing table fair, fast, and automated
- to allow for variations in the BNLs used by different nodes while still having the KNL converge

As the network grows, more and more nodes can be added to the default BNL, therefore increasing decentralization to the maximum degree possible.

### Why charge a fee for adding nodes to the KNL?

It imposes a cost on any new node wishing to join the network. And this cost increases as the network grows. This makes many types of Sybil or Denial-of-Service attacks very expensive.

### Why not include all nodes found on any BNL node's KNL?

Requiring that nodes be present on a majority of BNL node's KNLs means that even if a malicious actor controls a number of nodes on the default BNL, it still does not significantly reduce their cost of creating fake nodes, because they still need to pay other nodes on the BNL until they reach a strong majority.

### Why not require that nodes be present on all or almost all BNL nodes' KNLs?

We need the amount of nodes that a new node pays for inclusion (currently 75%) to be larger than the minimum number required (currently 50%) to allow for some failures or malicious bootstrap nodes. However, we also don't want to pay too many nodes because then we start to include some of the nodes with the highest prices, increasing the cost of joining the network.

By requiring a simple majority, we maximize the number of nodes an attacker must control to prevent inclusion of a new node while simultaneous maximizing the number of nodes an attacker must control or pay off to include a large number of their own fake nodes.

### Why impose a cost when there is already a cost involved with creating an on-ledger account, funding it, etc.?

Because we want the anti-Sybil mechanism to be completely ledger-agnostic, which has a number of benefits:

1. It is easier to add a new ledger to the network because we don't have to implement some new Sybil mechanism.
2. We can add a larger variety of ledgers because we can even use ledgers that don't provide us with a good way to prevent Sybil attacks like a way to verify account ownership.
3. Every network participant only needs to understand the ledgers that they support. If we were using the ledgers as our anti-Sybil criterion, every network participant would have to be able to verify the anti-Sybil criterion for any ledger used anywhere in the network.
4. By having just one anti-Sybil mechanism rather than one per ledger, the protocol is much simpler overall.

For example, the Lightning Network relies on the costs involved in using the underlying Bitcoin ledger to prevent Sybil attacks which means that to extend to other ledgers some ledger-dependent anti-Sybil mechanism would have to be considered for each additional ledger and every node in the network would have to understand all of these mechanisms in order to verify that a given node has paid the cost to join.

As an added benefit, the explicit registration fee also generates some revenue for bootstrap nodes which may be helpful in ensuring their reliable operation.

### Won't a registration fee discourage network growth and participation?

Please note that the fee only has to be paid by nodes who would like to become top-level nodes in the global routing table. If a node simply wants to participate in the network it can peer with any existing node and use it as its uplink. For example, a node A that doesn't want to pay the fee to register g.das.A could just peer with node B and use addresses of the form g.das.B.A for free.

The global routing table is a scarce, public resource so imposing some form of cost is the best (and possibly only practical?) way to prevent exhaustion of that resource.

### Why do nodes eventually add nodes to their KNL if they see them appear on BNL nodes' KNLs, even if they weren't themselves paid?

We want the KNLs of different nodes to converge. Nodes that are on most nodes' KNLs will tend to be added to other nodes' KNLs and nodes that appear on few nodes' KNLs will tend to eventually disappear from those.

### Why remove inactive nodes? They paid for their spot!

Nodes that have disappeared will eventually be removed from the KNLs. This is because the network pays a memory and bandwidth cost for keeping those nodes around.

By requiring that nodes remain online and active at all times, it also imposes a maintenance cost for keeping an KNL entry active. While the initial setup fee would be a strong deterrent for a short-term Sybil attacker, the cost to maintain a large number of active nodes is intended to act as a deterrent for a long-term Sybil attacker who is seeking to amortize the cost of the initial registration across a long period of malicious activities.

An explicit cost to maintain KNL entries could be added as well, but it's not quite clear how that would work in relation to the convergence criterion. (I.e. If a given node removes a node for not paying the maintenance fee, wouldn't it just re-add it because it would still be on other nodes' KNLs?) Hopefully, the implicit cost of maintaining active nodes will be enough to deter attackers.


## Notes

- A malicious node could accept payment for addition to the KNL but not actually add the requesting node anyway. There are protocols to potentially detect this case automatically (see [Token Payment Protocol](token-payment-protocol.md)) but it may not be a common enough attack to warrant a sophisticated solution. Perhaps the community can simply keep an eye out and if a small number of bootstrap nodes is accused of taking money without adding the requesting node to their KNL, they could be manually removed from the default BNL. Since the cost of honesty is just adding the node to the list and the cost of dishonesty is the risk of being caught and removed from the BNL, the expected value of doing this attack is likely negative, making it unattractive even for narrowly self-interested nodes. Finally, a small number of nodes doing this attack would not affect the overall functioning of the network.

- Because new nodes will try to buy KNL slots from the nodes charging the least for such spots, there should be some price competition. A node that raises its prices excessively will not earn much due to a lack of customers. At the same time, nodes should be disincentivized to lower their prices too much because then they also don't earn much and there is a greater risk to the functioning of the network. Hopefully, this will lead to an equilibrium at a price that balances the need to have sufficient security against Sybil attacks against the need to have a low cost barrier to entry for new nodes.
