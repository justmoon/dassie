# D4. Network membership economics

**Status:** active, confirmed 2026-10-11, refined 2026-10-14. The fee is not built yet ([roadmap](../roadmap.md)).
**Areas:** [Network](../areas/network.md), [Peer protocol](../areas/peer-protocol.md)

## Facts

The original's design calls for a paid registration in the Known Node List whose fee grows with the list, paid to the cheapest 75% of bootstrap nodes, and for removing nodes that lose majority or go silent (NET-17, NET-18). The original implements none of it: registration is manual and free, and nodes are never removed.

## Decision

- **Removal and expiry now:** majority loss, a staleness timeout, and registration expiry without renewal.
- **Automatic registration and renewal now:** a node registers with every bootstrap node, and renews before expiry.
- **The fee later,** as a protocol addition once outgoing payments work: a quote message, and payment by ILP with a signed quote condition (the "naive" token payment variant of the original's design). The fee schedule is set then; registration is free until then. The payment goes to the cheapest 75% of bootstrap nodes, as the design says; the original UI's 80% changes to match.

The design review of 2026-10-14 added ([Network](../areas/network.md) states the rules):

- **A bootstrap node serves its unexpired registrations and itself,** not `majority ∪ registered` as the original does. Serving the majority back would keep a node listed forever once it had made it, whether or not it renews.
- **Membership freezes when lists go stale.** A node loses its majority place only while more than half of the configured lists are fresh; otherwise two bootstrap nodes going offline would empty every table in the network.
- **A downloaded list is trusted for its authenticated response** (NET-15), with its hash computed from the bytes received. Comparing it with a hash polled earlier proves nothing: both answers come from the same server, and on a busy bootstrap node the list changes between the two requests.
- **Free registration is throttled** until the fee arrives: a global bucket, one registration per URL host and port, a cap per /24 or /48, and a reachability probe of the registrant's URL.

## Kept

The majority rule: strictly more than half of the *configured* bootstrap nodes (NET-16). The code was right; the original's design note ("N/2 + 1") was the outlier.
