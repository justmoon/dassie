# Connector timing and packet limits

**Areas:** [Connector](../areas/connector.md), [ILP](../areas/ilp.md)

## Decision

The connector behaves as ILPv4 says, where the original did not:

- it waits for a packet until the outgoing packet's expiry (the original gave up after a fixed few seconds and lost late Fulfills), and for a packet to a peer over a settlement hub until the hub decides it, at most until the incoming packet's expiry, and one message window past its own: the peer may still win a claim for it on the hub ([settlement-hubs](settlement-hubs.md));
- it rejects an invalid fulfillment with F05 at once;
- F08 carries amount data;
- malformed packets get F01 and unroutable ones F02, and nothing throws on the packet path;
- address prefixes match on segment boundaries;
- request IDs are unique per endpoint.

None of the above is a real choice; it is recorded because it changes observable timing compared to the original.

The message window is the one real choice: **longer than `ilp-connector`'s second.** A peering over a settlement hub needs the window to hold the hub's margin, a far member's leg to the validators, and the faulty leaders the margin allows for; a hub whose validators span a region such as East Asia or the Caribbean, with seven of them, needs well over a second ([settlement hubs](../designs/settlement-hubs.md#4-peering-over-a-hub)). The window is the node's own: no peer sees it, so a release can change it without anyone else. A longer one leaves fewer hops within a packet's 30 seconds.
