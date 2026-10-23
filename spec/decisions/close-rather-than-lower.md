# A receiver never lowers a maximum it told: it closes a connection that no longer fits

**Areas:** [STREAM](../areas/stream.md), [Payments](../areas/payments.md).

## Facts

- RFC 29 §4.4.4: once an endpoint advertises a maximum receive amount, it MUST NOT advertise a smaller one later. Replies arrive out of order, so a sender MUST ignore a maximum that does not grow. Senders that keep to it only ever raise what they take as a stream's maximum ([stream.md](../areas/stream.md#money-limit)).
- An incoming payment can be paid by many connections at once, one for each payment that pays it ([Payments](../areas/payments.md#receiving)). Each connection is told what the payment still expects. When another connection pays, that falls.
- The receiver does not know how much a payer means to pay: nothing in STREAM or Open Payments tells it. A sender may end at once when the first maximum it hears is below what it has to deliver, so a receiver that told a payer less than its whole amount would fail it.
- So connections that pay at the same time must each be told more than their share, and what they were told can add up to more than the payment expects.
- A packet the receiver would not take, with nothing in its reply that changes, is offered again by such a sender until its deadline.

## Decision

A stream is told the most it was ever told: what it received plus what is left, or what it was told before, if more. A stream opened later starts from what is left, as it was never told more. It takes money while the money fits what the payment expects, within its tolerance. A packet that fits what its streams were told but no longer fits what is left closes the connection, with the code the application gave with the limit: `NoError` for an incoming payment that is open or full ([full-or-stopped-short](full-or-stopped-short.md)). Only a packet the receiver would fulfill closes it. A packet over what its streams were told is refused with F99 and the room, as before ([paid-within-a-tolerance](paid-within-a-tolerance.md)).

Dassie's payer takes the `NoError` close as the receiver taking no more. A payment short by no more than its tolerance completes on it, as it does when the receiver reports no room.

## Accepted

- Payments that together offer more than an incoming payment expects can leave it short: each was told room that the others took, and none can be told the last units. The incoming payment stays open for the rest, and the payments that were closed fail.
- `ilp-protocol-stream` ends a connection closed with `NoError` gracefully, still offering what it has left, each offer answered with the close, until its application gives up ([full-or-stopped-short](full-or-stopped-short.md)). Nothing more is paid, and it hears why; it stops no sooner than when it was refused.
- A receiver that forgot a connection, after a restart or to make room in its server's table, no longer knows what it told it, and may tell it less. So may one whose count for the connection fell, as when the node gave its place in an incoming payment to a new payer while it was still open: what it was told rested on that count, and it is told the room again. Dassie's sender still takes a maximum that fell, for such receivers and other implementations ([stream.md](../areas/stream.md#money-limit)).

## Rejected

- *Lowering the maximum, as before:* senders that keep to the RFC ignore it, and offer a packet that no longer fits until their deadline. Payments racing into one incoming payment could all fail, after seconds, while the incoming payment was paid in full.
- *Honouring every maximum told:* exact for payments that fit together, but payments that together offer more overpay the incoming payment, by up to every connection's whole amount but one.
- *Never promising the same room twice,* each connection told only what no other holds: exact, and a double payment fails cleanly. But payments that fit together fail whenever they overlap, as the first holds all that is left; and anyone who can read a payment request could hold its room by paying a unit now and then.
- *Closing every connection told more than is left, as soon as another pays:* it ends payments that would have fit, and leaves the incoming payment short even when they all fit together.
