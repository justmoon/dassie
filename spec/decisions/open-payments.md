# Open Payments, fully, and no SPSP

**Areas:** [Payments](../areas/payments.md), [HTTP](../areas/http.md)

## Decision

Implement [Open Payments](https://github.com/interledger/open-payments-specifications) as its specs define it: the wallet address server, the GNAP authorization server and the resource server, with GNAP tokens and HTTP message signatures wherever the specs require them. [Payments](../areas/payments.md) states the choices the specs leave to an implementation.

Payment pointers stay only as input syntax. Card authorization is not supported (the node has no cards), and STREAM receipts are not issued (they are not part of Open Payments).

## Rejected

- **SPSP:** Open Payments has replaced it. Wallets that speak only SPSP cannot pay the node.
- **Creating incoming payments without a token:** the specs require GNAP, and a non-interactive grant costs a client one request.
- **Leaving out GNAP and outgoing payments:** without them no third-party client, and no other wallet's user, can pay from the node.
