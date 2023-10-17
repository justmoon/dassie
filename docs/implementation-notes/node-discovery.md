# Implementation Notes: Node Discovery

## Context

Note that node membership in the Dassie network is fairly well-defined which makes the node discovery process a lot simpler and more deterministic. Please read the [Network Membership](network-membership.md) implementation note for more detail.

When a node first comes online, it only knows whatever information is hardcoded in the Dassie implementation it is using. This hard-coded information is a list of bootstrap nodes called the Bootstrap Node List (BNL). All nodes have such a BNL, which can be the default BNL provided by the developers or a customized BNL provided by the node operator.

In addition, all nodes also have a Known Node List (KNL) which contains all nodes that the node considers to be valid top-level routing destinations in the Dassie network.

Given the BNL, node discovery is the process of initializing the KNL as well as downloading recent status updates for as many nodes on the KNL as possible.

## Solution

- As part of its startup process, a new node will contact each of the nodes on its BNL to download their latest KNL.
- Once a node is present on more than N/2 + 1 of such KNLs (where N is the length of the BNL), it is added to the node's own KNL.
- For each entry on the node's own KNL, the node will query a random node to get the latest signed status for that node. Nodes that respond quickly and with more up-to-date information will be chosen preferentially in future queries.

## End of Discovery Phase

Discovery is a continuous process, because we constantly want to know the latest node state for each node. However, during initialization we have to decide when we have done enough node discovery to move on to the next phase of node initialization (usually peer selection).

Some nodes may be unavailable and we will never get an up-to-date status for them. As a result, we can't wait for the node discovery process to be 100% completed.

The criteria for moving on from initial node discovery to the next step in the startup process is:

- When we have up-to-date information about all nodes, we conclude initial discovery. This is mostly useful in testing scenarios where we have a small number of nodes which are all local and online and therefore node discovery can complete very quickly. In these scenarios, we don't want to introduce any unnecessary delay because we want a quick development cycle where we can make a change, relaunch the network, and have it initialize very quickly so we can test whatever functionality we are currently working on.

- When a certain timeout is reached, we conclude node discovery. Timeouts are a simple catch-all which allows us to move on if we get stuck for some reason. The timeout needs to be somewhat generous to account for slower connections which may take longer to bootstrap as well as unforeseen delays.

- When we have received KNLs from a certain percentage of our BNL nodes and we have received node status from a certain percentage of our KNL nodes, we conclude discovery. For this initial version of Dassie, the required BNL percentage is >=75% and the required KNL percentage is >=50%.

Another important thing to note is that in the typical setup procedure for a production node, the node discovery process would start as soon as the Dassie daemon is started but the next phase (auto-peering) wouldn't start until after the user is done configuring ledger settings, depositing initial funds etc. which may take considerable time. The only times that the above criteria would come into play is if the auto-peering process is started very quickly such as during development or during rare scenarios where a node happens to be looking for new peers shortly after booting up. It is still important for these cases to do node discovery before starting auto-peering so we don't just choose peers from the BNL only.

## Node properties

Each node has a number of public properties:

- Public key
- ID (derived from the public key)
- API URL (the public HTTPS URL where the node can be queried)
- Agent (the name and version of the Dassie node implementation)
- Features (the set of Dassie protocol features that the node supports)
- Alias (a UTF8 string which represents the Node's readable name)
- Web URL (optional; a URL where interested parties can find more information about the node or its operator)
- Settlement methods (which settlement methods the node supports/accepts)
- Peers (which nodes the node is peered with)
