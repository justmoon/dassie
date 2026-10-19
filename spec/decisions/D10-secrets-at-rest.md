# D10. Secrets at rest

**Status:** active, confirmed 2026-10-11.
**Areas:** [Node](../areas/node.md), [HTTP](../areas/http.md)

## Facts

The original stores the node key, the TLS key, the ACME key, sessions and BTP tokens in plaintext in SQLite, with the default umask (RT-25).

## Decision

- The data directory is 0700 and the database file 0600.
- Session tokens, BTP tokens and GNAP access tokens are stored only as hashes.
- The node key and the settlement entropy stay in plaintext. The node must sign and settle unattended, and encrypting them with a key stored next to them would only be theatre.
- The original's design notes on signing in the frontend stay future work.
