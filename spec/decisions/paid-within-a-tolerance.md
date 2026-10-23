# A payment within a tolerance of its amount is paid in full

**Areas:** [Payments](../areas/payments.md), [STREAM](../areas/stream.md).

## Facts

- A STREAM sender sizes its last packet from the room the receiver reports, converted at the rate it saw the path deliver. Every hop converts and rounds down in its own units, so what arrives can land a little under or over what is left, by up to a unit of the coarsest asset on the path. No sender can size its last packet to the unit.
- A receiver that takes only exact amounts refuses a last packet a little over what is left, and is never offered one that fills it to the unit: the payment ends a little short, or not at all. `ilp-protocol-stream`'s receivers take up to 1% more than a stream's maximum for this reason.
- Open Payments calls `incomingAmount` the most that should be paid into an incoming payment, and `receivedAmount` what has been paid. Neither says that one bounds the other.
- Dassie's ledgers count US dollars and XRP, both at scale 9, and a new ledger comes with the settlement module that serves it. Other wallets' assets have any code and scale.
- The node has exchange rates to a common base, refreshed every minute, and may lack a fresh one for a while.
- Both ends judge whether a payment is paid: the receiver to complete its incoming payment, Dassie's payer to stop sending.

## Decision

A payment counts as paid in full once what it received lands within its tolerance of its amount, under or over. The tolerance is a ten-thousandth of the asset's unit, `10^(scale − 4)` of its smallest unit and nothing for an asset with fewer than four decimal places, and never more than a thousandth of the payment's amount. A payment without an amount has none.

- **The receiver** takes a packet that overshoots what is left by at most the tolerance, and completes the payment. It counts a payment within the tolerance under its amount as complete: a sender that offers more is closed with `NoError` ([full-or-stopped-short](full-or-stopped-short.md)), as its streams cannot report less room than they were told ([close-rather-than-lower](close-rather-than-lower.md)). A packet further over, and over what its streams were told, is refused with F99, and its reply tells the sender the room.
- **Senders aim at the amount itself.** The room a receiver reports is exact until something else takes from the payment; the tolerance only absorbs what rounding on the way puts over or under it.
- **What an incoming payment received is reported as it is.** Its `receivedAmount` can exceed its `incomingAmount` by up to the tolerance; the specs call `incomingAmount` only the most that should be paid.
- **Dassie's payer** counts its payment delivered once it is short of its receive amount by no more than that amount's tolerance and the receiver takes no more: it reports no room, or closes the connection with `NoError`.

## Accepted

- The tolerance is a share of the asset's unit, so it is worth what the unit is worth: a ten-thousandth of a dollar, and of an XRP, which at ten dollars an XRP is a thousandth of a dollar. A ledger in an asset worth much more per unit would need it revisited; that ledger comes with its settlement module.
- A path through an asset with fewer decimal places, such as one in cents, rounds by more than the tolerance. Its last packet can still be refused, and the incoming payment end a little short. The payer still counts a payment short by less than one unit it sends is worth as delivered, so its side ends.
- A receiver may get up to the tolerance more than it asked for, and its payer up to the tolerance less: at most a ten-thousandth of a unit, and a thousandth of the payment.
- A payment of less than a thousand smallest units has no tolerance and is paid exactly or not at all, so that none counts as paid before it received anything.

## Rejected

- *Exact amounts only:* no sender can size the last packet to the unit, so payments end short or with their last packet refused.
- *A fixed amount in dollars, converted at the node's rates:* rates move, so whether a payment is full would depend on when it was judged, and a restart could judge it otherwise. The payer does not know the receiver's rates, and an asset without a fresh rate needs another rule anyway.
- *A fixed number of smallest units:* a unit means nothing across scales. A thousand units are a millionth of a dollar at scale 9 and ten dollars at scale 2.
- *A share of the amount alone, as `ilp-protocol-stream`'s 1%:* it grows with the payment. A share of a large payment is far more than anyone ignores; the share here only keeps small payments exact.
- *Closing a connection that keeps offering packets over what its streams were told, after a refusal told it the room:* a too-large packet can be innocent, sized before its sender heard of a lower maximum and arriving after the refusal that told it, so a close would end senders that would have sent what fits. The refusal, F99 with the room, is all the receiver gives. A packet within what its streams were told but over what is left is another matter: the receiver closes its connection, since it cannot tell its sender less ([close-rather-than-lower](close-rather-than-lower.md)).
