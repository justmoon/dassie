# D13. Connector timing and packet limits

**Areas:** [Connector](../areas/connector.md), [ILP](../areas/ilp.md)

## Decision

The connector behaves as ILPv4 says, where the original did not: it waits for a packet until the outgoing packet's expiry (the original gave up after a fixed few seconds and lost late Fulfills), rejects an invalid fulfillment at once, matches address prefixes on segment boundaries, and never throws on the packet path.

None of this is a real choice; it is recorded because it changes observable timing compared to the original.
