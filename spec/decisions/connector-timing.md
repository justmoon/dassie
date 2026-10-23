# Connector timing and packet limits

**Areas:** [Connector](../areas/connector.md), [ILP](../areas/ilp.md)

## Decision

The connector behaves as ILPv4 says, where the original did not:

- it waits for a packet until the outgoing packet's expiry (the original gave up after a fixed few seconds and lost late Fulfills), and for a packet to a peer over a settlement hub until the hub decides it, at most until the incoming packet's expiry: the peer may still win a claim for it on the hub ([settlement-hubs](settlement-hubs.md));
- it rejects an invalid fulfillment with F05 at once;
- F08 carries amount data;
- malformed packets get F01 and unroutable ones F02, and nothing throws on the packet path;
- address prefixes match on segment boundaries;
- request IDs are unique per endpoint.

None of this is a real choice; it is recorded because it changes observable timing compared to the original.
