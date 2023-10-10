# Implementation Notes: Node Discovery

The foundation of any peer-to-peer network is node discovery. For Dassie this means crawling the routing graph to discover nodes based on some set of initial, well-known nodes also called bootstrap nodes.

## When to stop discovering

We cannot assume that our crawling of the routing graph will be perfect. Some nodes may be unavailable but their peers are still advertising them. Some nodes may publish false advertisements, knowingly or unknowingly. We may not be able to reach some nodes even though they are working fine due to network issues, firewall rules, or Internet censorship.

For these reasons, we can't wait for the node discovery process to be 100% completed. We will have to crawl for some time until we are happy with the state of the crawl and then move on. Of course, we should still continue to update our knowledge about the network continuously, remove stale information and fill in missing details.

The criteria for moving on from initial node discovery to the next step in the startup process is:

- When we have up-to-date information about all nodes, we conclude initial discovery. This is mostly useful in testing scenarios where we have a small number of nodes which are all local and therefore node discovery can complete very quickly. In these scenarios, we don't want to introduce any unnecessary delay because we want a quick development cycle where we can make a change, relaunch the network, and have it initialize very quickly so we can test whatever functionality we are currently working on.

- When a certain timeout is reached, we conclude node discovery. Timeouts are a simple catch-all which allows us to move on if we get stuck for some reason. The timeout needs to be somewhat generous to account for slower connections which may take longer to bootstrap as well as unforeseen delays.

- When we have searched the network up to a certain depth, we conclude discovery. We also limit the number of nodes that we consider at each depth for this criterion. This gives us a finite number of nodes to look for. Of course, the network topology may be larger than that but we are only trying to get a decent set of nodes that would allow us continue with the next step of the initialization process.

Another important thing to note is that in the typical setup procedure for a production node, the node discovery process would start as soon as the Dassie daemon is started but the next phase (auto-peering) wouldn't start until after the user is done configuring ledger settings, depositing initial funds etc. which may take considerable time. The only times that the above criteria would come into play is if the auto-peering process is started very quickly such as during development or during rare scenarios where a node happens to be looking for new peers shortly after booting up. It is still important for these case so we don't choose peers only from the set of bootstrap nodes for example.

## Indirect discovery

We don't have to query each node individually about its state. Because the node state is always signed, we can ask any node for their knowledge about the target node. For an initial download, it would be most efficient to download most of the network state in bulk from nodes that have low latency and high bandwidth.

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
