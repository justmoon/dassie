# A receiver says whether it is full or stopped short

**Areas:** [Payments](../areas/payments.md), [STREAM](../areas/stream.md).

## Facts

- A STREAM receiver that takes no more money closes the connection when a packet offers some, so that its sender stops at once rather than offering money until its deadline. Dassie's sender stops on any close the receiver sends. `ilp-protocol-stream` 2.7.2-alpha.2 stops at once on a close with another code than `NoError`; on `NoError` it ends the connection gracefully (`handleControlFrames` and `end` in `dist/src/connection.js`), still offering what it has left to send, each offer answered with the close, until its application gives up.
- An incoming payment takes no more in two ways: it received all it asked for, possibly from another payment, or its creator completed it, or it was closed for room, before that. A connection also takes no more once another payment took what it would have paid, while the incoming payment is still open: its receiver cannot tell it less room than it did ([close-rather-than-lower](close-rather-than-lower.md)). One without an amount never receives all it asked for.
- Wallets complete incoming payments early. One completes the incoming payment it created a fixed few seconds after its outgoing payment starts, whether or not that payment is still sending; a payment over a link's credit is still waiting for a settlement then.
- Only the receiver knows which way it took no more. The sender knows what it delivered, not what the incoming payment asked for or what other payments brought it.
- A `ConnectionClose` frame carries a code from RFC 29's list and a free-text message. `NoError` is a normal close; `ApplicationError` is the application ending the connection with an error.
- The sender's owner reads why a payment failed. "The receiver takes no more" reads as a request that was filled.

## Decision

The close code says why. A receiver closes with `NoError` once what the connection pays into received all it asked for, or another payment took what is left, and with `ApplicationError` once it stopped short of that. Dassie's payer stops on both, without reconnecting, and says which: "the receiver takes no more" for `NoError`, "the receiver stopped the payment before it was complete" for `ApplicationError`.

The STREAM server takes the code from the application with each admission (`exhausted`). The node's receivers keep, for each incoming payment completed lately, whether it was full, and read it back after a restart from the payment's amounts.

## Accepted

- A receiver that is not Dassie and closes early with `NoError` still reads as "the receiver takes no more". The payer can only go by what the receiver says.
- `ilp-protocol-stream` senders report an `ApplicationError` close as an error rather than a normal end. That is what it is to them too: their payment did not complete.
- A payer of a release before this one reads `ApplicationError` as a connection that closed early, reconnects a few times, and fails at the end with the close code in its reason.

## Rejected

- *A message in the close frame, under `NoError`:* invisible to other senders, but a convention only Dassie would read, kept in free text. The code is the protocol's own field for why a connection closed, and other senders learn from it that their payment was cut short.
- *Let the sender tell from the receive maximum:* a Dassie receiver never lowers a maximum it told, and one that does could mean that the incoming payment was completed early, but also that another payment filled it, so it does not tell the two apart.
- *Read the incoming payment again on a close:* it needs a grant at the receiver's node, which a payer to a wallet address does not have, and a request per failed payment to a node that may not answer.
