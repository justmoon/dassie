# Implementation Notes: Peer State

## Choosing a peer

In order to connect to the network, Dassie nodes will first crawl the network to collect information about the nodes that are out there.

Out of the available nodes, a searching node will consider a number of criteria when evaluating a candidate node:

- Whether the candidate node is open for new peers
- Whether the candidate node offers the settlement method that the searching node is looking for
- Whether the candidate node supports protocol features that the searching node requires
- How central the candidate node is in the routing graph
- How much reputation the node has (this is a complex topic which deserves its own article)

## Establishing a peering relationship

When a Dassie node has identified a target node that it would like to peer with, it will send the target node a _peering request_. This request contains all of the necessary information required for peering such as:

- Information about the requesting node
- The desired settlement method
- Settlement method specific information
- Information on the peer data endpoint that the requesting node has assigned to the target node

While the request is pending, the requesting node will maintain the target in the `'request-peering'` state. In this state, the requesting node will accept some types of peer messages via the peer data endpoint.

When the target node receives the peering request, it will first ensure that it's valid and all criteria for peering are met. Once this has been confirmed, it will try to contact the requesting node via the peer data endpoint to confirm this is working. It will then assign its own peer data endpoint and put the node in the `'peered'` state. It will then respond to the original _peering request_ to indicate success.

After the peering request has completed successfully, the requesting node will also assign the `'peered'` state to the target node.

Once a node moves a given peer to the `'peered'` state, it will begin sending that node heartbeat messages to keep the peering relationship alive.

## Advertising the peering relationship

Nodes won't automatically advertise a peering relationship just because it exists. Each node regularly reviews its peering relationships. If the peering relationship is deemed to be routable, i.e. if the node believes that packets sent to this peer will reach that peer and be successfully processed, it will set the `advertise` flag to `true`. Otherwise, it will be set to `false`.

During review, nodes may consider:

- **Liquidity status**: Whether either party in the peering relationship has exhausted its available liquidity.
- **Reliability**: If a lot of failures have occurred in relation to a specific peer, that peer may not be advertised.
- **Recent heartbeat**: The peer will only be advertised if heartbeats have been received from the peer consistently over some period of time.
- **Flapping**: If the peer's advertise state keeps flipping on and off, then the criteria for being advertised will get more stringent. For example, if a node often misses heartbeats, it would not be advertised until it maintains a long period of consistent heartbeats again. This is to prevent additional strain on the routing protocol from peering relationships being excessively removed and re-added.

Aside from determining the advertising state of the peering relationship, the review may also cause a node to decide to terminate the relationship. Reasons for this could be:

- **Extended downtime**: The peer hasn't been seen or heard from in a long time
- **Insufficient liquidity**: There is insufficient liquidity and there are no pending settlements. This could happen if one node keeps sending settlements but the other (for whatever reason) doesn't credit them. This will eventually cause the nodes to totally disagree about their respective balances with each other. Because this condition is unrecoverable, the peering relationship will be permanently terminated.
- **Malicious behavior**: There may be some malicious behavior that may be provable such as a node signing two contradictory messages. Any node that is being presented with evidence of malicious behavior may decide to terminate peering with the offending node.
- **Manual termination**: The node operator can manually request the termination of a given peering relationship.

## Terminating a peering relationship

Once either party in a peering relationship wishes to terminate the relationship, it sends the other party a _terminate peering_ message and moves the peer state to `'terminating'`. Once in the terminating state, the peer state cannot be reestablished until after the termination process has successfully completed.

Termination can be `final` or not. If it is, the node's peer state upon completion of the termination process changes to `'banned'`. If it isn't, it will simply revert back to the default `'none'` state.

If the node initiating the termination owes the other node money, it will automatically send a settlement to pay this outstanding balance. If the other node owes the initiating node money, it will send a settlement upon receiving the _terminate peering_ message.
