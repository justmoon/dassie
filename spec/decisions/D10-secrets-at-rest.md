# D10. Secrets at rest

**Areas:** [Node](../areas/node.md), [HTTP](../areas/http.md)

## Decision

- Bearer secrets (session, BTP and GNAP tokens) are stored only as hashes.
- The data directory and database are readable by the node's user only.
- The node key and the settlement entropy stay in plaintext. The node must sign and settle unattended, and encrypting them with a key stored next to them would only be theatre.

Signed or independent frontends, so that the wallet need not trust the node that serves it with the passphrase, stay future work ([roadmap](../roadmap.md)).
