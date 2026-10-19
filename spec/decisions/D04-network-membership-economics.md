# D4. Network membership economics

**Areas:** [Network](../areas/network.md). The fee is not built yet ([roadmap](../roadmap.md#registration-fees-and-child-peers)).

## Facts

The original's design calls for a paid registration in the bootstrap nodes' lists, with a fee that grows with the list, and for removing nodes that lose majority or go silent. The original implements none of it: registration is manual and free, and nodes are never removed.

## Decision

- **Removal and expiry:** majority loss, staleness, and registrations that expire unless renewed.
- **Automatic registration and renewal** with every bootstrap node.
- **The fee later,** as a protocol addition: a quote, and payment by ILP to the cheapest 75% of bootstrap nodes. Until then registration is free, and throttled.
- **A bootstrap node serves its unexpired registrations and itself,** not the majority list back. Serving the majority back would keep a node listed forever once it had made it, whether or not it renews.
- **Membership freezes when lists go stale.** A node loses its majority place only while most configured lists are fresh; otherwise a few bootstrap nodes going offline would empty every table in the network.
- **A downloaded list is trusted for its authenticated response,** with its hash computed from the bytes received. Comparing it with a hash polled earlier proves nothing: both answers come from the same server, and a busy list changes between the two requests.
- **The majority is strictly more than half of the *configured* bootstrap nodes,** not of those that answered, so an attacker cannot shrink the electorate by blocking some.
