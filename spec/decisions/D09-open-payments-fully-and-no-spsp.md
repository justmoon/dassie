# D9. Open Payments, fully, and no SPSP

**Status:** active, decided 2026-10-11, revised 2026-10-14: the first version served SPSP and deferred GNAP, quotes and outgoing payments.
**Areas:** [Payments](../areas/payments.md), [HTTP](../areas/http.md), [Wallet](../areas/wallet.md)

## Facts

The original serves an Open-Payments-style wallet address and unauthenticated incoming payments with hard-coded amounts. It has no SPSP, no GNAP authorization server, no quotes and no outgoing-payment resources (ILP-10).

## Decision

Implement [Open Payments](https://github.com/interledger/open-payments-specifications) v1.4.0 as its specs define it: the wallet address server, the GNAP authorization server and the resource server, with GNAP tokens and HTTP message signatures (RFC 9421) on every call the specs protect. [Payments](../areas/payments.md) states the behaviour.

The choices the specs leave to an implementation:

- **One wallet address per ledger,** since a wallet address has one asset; `$host` names the default ledger.
- **Grants that move the owner's money or read beyond a client's own payments need the owner's consent** in the wallet; incoming-payment and quote grants do not. Reading any incoming payment needs no consent, because a sender's wallet reads a payment it did not create to learn how to pay it.
- **Clients never crowd out the owner:** the owner's incoming payments and quotes are bounded apart from clients'.
- **A grant limit caps the amount debited or the amount received, never both,** as in the specs and their reference implementation, Rafiki; each access item keeps to its own limits, and only one item per wallet address may allow `create`, since which limits apply would otherwise be unclear.
- **Each payment has a STREAM connection of its own,** so two payments to one incoming payment never take each other's delivery for their own.
- **The owner's Send uses the same logic as third-party clients,** over RPC and without a grant.
- **Payment pointers stay only as input syntax.**

Not supported: card authorization (the node has no cards), and STREAM receipts, which are not part of Open Payments.

## Rejected

- **SPSP:** Open Payments has replaced it, and its specs no longer mention it. Wallets that speak only SPSP cannot pay the node.
- **Creating incoming payments without a token,** as the first version of this decision chose: the specs require GNAP, and a non-interactive grant costs a client one request. Grant requests and incoming payments are rate-limited, and unused incoming payments expire.
- **Deferring GNAP and outgoing payments:** without them no third-party client, and no other wallet's user, can pay from the node.
