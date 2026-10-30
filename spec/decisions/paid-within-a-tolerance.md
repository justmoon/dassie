# A payment is paid in full at its amount, and may land a hair over it

**Areas:** [Payments](../areas/payments.md), [STREAM](../areas/stream.md).

## Facts

- A STREAM sender sizes its last packet from the room the receiver reports, converted at the rate it saw the path deliver. Every hop converts and rounds down in its own units, so what arrives can land a little under or over what is left, by up to a unit of the coarsest asset on the path. No sender can size its last packet to the unit.
- A receiver that takes only exact amounts refuses a last packet a little over what is left, and is never offered one that fills it to the unit: the payment ends a little short, or not at all. `ilp-protocol-stream`'s receivers take up to 1% more than a stream's maximum for this reason.
- A receiver whose payment is a little short of its amount, and counts it as complete, stops taking money, but cannot tell its senders so: what a stream was told never falls ([close-rather-than-lower](close-rather-than-lower.md)). It says so only by closing on the next packet that offers money. Where the payment ends is then decided by the packets in flight when it came within reach, short of the amount as often as not.
- People read amounts to four decimal places: a dollar to a hundredth of a cent, an XRP to a ten-thousandth. What lands within half a ten-thousandth of an amount reads as that amount, whether it is rounded to four places or cut off there.
- Open Payments calls `incomingAmount` the most that should be paid into an incoming payment, and `receivedAmount` what has been paid. Neither says that one bounds the other.
- Dassie's ledgers count US dollars and XRP, both at scale 9, and a new ledger comes with the settlement module that serves it. Other wallets' assets have any code and scale.
- Both ends judge whether a payment is paid: the receiver to complete its incoming payment, Dassie's payer to stop sending.

## Decision

A payment counts as paid in full once what it received reaches its amount, and never before. It may land past it by its tolerance: less than half a ten-thousandth of the asset's unit, `ceil(10^(scale − 4) / 2) − 1` of its smallest unit and nothing for an asset with four decimal places or fewer, and never more than a thousandth of the payment's amount. So what it received reads as its amount to four decimal places. A payment without an amount has none.

- **The receiver** takes a packet that overshoots what is left by at most the tolerance, and completes the payment once it has its amount. A packet further over, and over what its streams were told, is refused with F99, and its reply tells the sender the room. A payment short of its amount, by however little, stays open and has room for the rest.
- **Senders land on the amount or just past it.** Dassie's sender converts what is left exactly, as hops round down, and its last packet rounds up to deliver at least what is left, as long as that stays within the tolerance; where it cannot, it delivers what fits under ([STREAM](../areas/stream.md#sending)). A receiver that refuses a packet stops the rounding up for the rest of that connection, so one that takes nothing over is paid short rather than offered the same packet again.
- **What an incoming payment received is reported as it is.** Its `receivedAmount` can exceed its `incomingAmount` by up to the tolerance; the specs call `incomingAmount` only the most that should be paid.
- **Dassie's payer** counts its payment delivered once the receiver has its receive amount, or once it is short by no more than that amount's tolerance and the receiver takes no more: it reports no room, or closes the connection with `NoError`, as one that is not Dassie may, or as a Dassie receiver does when another payment filled what it pays into.

## Accepted

- A receiver usually gets a few units more than it asked for, and up to the tolerance more: under half a ten-thousandth of a unit, and a thousandth of the payment.
- The tolerance is a share of the asset's unit, so it is worth what the unit is worth: under half a ten-thousandth of a dollar, and of an XRP, which at ten dollars an XRP is half a thousandth of a dollar. A ledger in an asset worth much more per unit would need it revisited; that ledger comes with its settlement module.
- A path through an asset with fewer decimal places, such as one in cents, rounds by more than the tolerance. Its last packet cannot round up within it, and the incoming payment stays open a little short, until it expires or its creator completes it. The payer still counts a payment short by less than one unit it sends is worth as delivered, so its side ends.
- A sender that is not Dassie and stops a few units short, less than a source unit delivers, leaves its incoming payment open a little short in the same way.
- A payment of less than a thousand smallest units has no tolerance and is paid exactly or not at all.

## Rejected

- *Exact amounts only:* no sender can size the last packet to the unit, so payments end short or with their last packet refused.
- *A tolerance under the amount as well as over it:* the receiver then stops at whatever lands within reach, mostly short, and its senders learn it only by offering a packet. A sender whose last packet fit no room left, rounded down, waited for its connection to go idle before it learned the payment was done.
- *A full ten-thousandth over:* a payment would read a ten-thousandth more than its amount once it is rounded to four places.
- *A fixed amount in dollars, converted at the node's rates:* rates move, so whether a payment is full would depend on when it was judged, and a restart could judge it otherwise. The payer does not know the receiver's rates, and an asset without a fresh rate needs another rule anyway.
- *A fixed number of smallest units:* a unit means nothing across scales. A thousand units are a millionth of a dollar at scale 9 and ten dollars at scale 2.
- *A share of the amount alone, as `ilp-protocol-stream`'s 1%:* it grows with the payment. A share of a large payment is far more than anyone ignores; the share here only keeps small payments exact.
- *Closing a connection that keeps offering packets over what its streams were told, after a refusal told it the room:* a too-large packet can be innocent, sized before its sender heard of a lower maximum and arriving after the refusal that told it, so a close would end senders that would have sent what fits. The refusal, F99 with the room, is all the receiver gives. A packet within what its streams were told but over what is left is another matter: the receiver closes its connection, since it cannot tell its sender less ([close-rather-than-lower](close-rather-than-lower.md)).
