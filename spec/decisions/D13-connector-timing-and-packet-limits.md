# D13. Connector timing and packet limits

**Status:** active, confirmed 2026-10-11.
**Areas:** [Connector](../areas/connector.md), [ILP](../areas/ilp.md), [Network](../areas/network.md)

## Facts

The original's connector times out after 5 s whatever the packet's expiry (up to 30 s), so Fulfills that arrive late are lost (ILP-1). It drops invalid Fulfills (ILP-2), and throws on some malformed packets (ILP-3).

## Decision

Behave as the standard says:

- time out at the outgoing packet's expiry;
- reject an invalid fulfillment with F05 at once, and stop waiting;
- F08 carries amount data;
- malformed packets get F01 and unroutable ones F02; nothing throws on the packet path;
- prefix matching is aware of segment boundaries (NET-33);
- request IDs are unique per endpoint.

None of this is a real choice. It is recorded because it changes observable timing.
