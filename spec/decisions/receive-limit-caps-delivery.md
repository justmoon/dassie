# A receive limit caps what payments deliver, not only what they reserve

**Areas:** [Payments](../areas/payments.md), [STREAM](../areas/stream.md).

## Facts

- An outgoing payment has a debit amount and a receive amount. Quoted for a fixed debit, its receive amount is what the debit buys at the rate a probe measured, less STREAM's slippage. Every packet must deliver at least that rate; nothing bounds from above what a packet delivers.
- A grant's limits are reserved from its payments' amounts when they are made ([What a grant allows](../areas/payments.md#what-a-grant-allows)). The Open Payments specs count a receive limit against the payments' receive amounts.
- A payment that fixed its debit sends all of it. On a path that pays at the rate it measured it delivers about the slippage more than its receive amount, and more again when the rate improves: money past the receive limit that nothing counts.
- A sender learns what arrived only from what its receiver reports. A Dassie receiver keeps its count for a payment's credentials across connections, and a payment reopens its connection with the same credentials, which no other payment uses ([Money limit](../areas/stream.md#money-limit)).
- STREAM sizes packets against a receiver's room at the best rate the path showed. Until a reply shows one it has only the payment's own rate, which is the least the path delivers.

## Decision

Under a grant with a receive limit, a payment delivers no more than its receive amount, as its receiver counts it, even when it fixed its debit, within the bounds accepted below.

- **It fixes what it delivers.** It ends once its receiver has its receive amount, and what it did not send goes back to the grant's debit side.
- **Its stream has a delivery cap** of the receive amount. No packet is offered past what is left of it, converted at the best rate replies showed, which fulfilled packets show as refused ones do. Until a reply showed a rate, a packet is offered at most half of what is left, so the first packet stays within the cap on a path up to twice the payment's rate; if half fits no packet that delivers a unit but the whole does, the least such packet goes, or the stream would never send. A packet must deliver a unit at the payment's rate, so none pays for nothing as the cap's last units come near. A stream that can offer no such packet has reached its cap.
- **A new connection starts from what got there,** as far as the payment knows: what it sent at its own rate, or more if its receiver said so. Until the receiver's first reply, the cap counts that as delivered, so the halving applies to what is left.
- **Every payment that fixes what it delivers has the cap,** the owner's too: it never delivers more than it asked for.
- **A payment that fails gives back what it did not deliver:** at least what it sent delivered at its own rate, and more when its receiver reported more.

## Accepted

- A payment that fixed its debit may complete without sending all of it: its sent amount is less than its debit amount, and the client sees why in its receive amount.
- Packets in flight when the path's rate rises past the best rate replies showed overshoot the cap by that rise. Rates move little between packets of one payment.
- A rate learned from a small packet understates what a larger one gets after rounding, so the larger one can overshoot by about a unit per hop.
- The least packet sent when half the cap fits none overshoots by however much better than the payment's rate the path is: the least packet is a unit or a few.
- After a restart the payment knows only what it sent, at its own rate. On a better path more got there, so the first packet of the new connection can overshoot by that difference, halved, until the receiver's first reply says what it has.
- A payment can stop short of its cap by less than the least packet that delivers a unit: a unit or two.
- The node trusts what the receiver reports. A receiver that reports less than arrived is paid more, up to the payment's debit, which the debit limit still bounds.

## Rejected

- *Count only the payments' receive amounts, as the specs do:* every payment that fixed its debit delivers the path's surplus past the limit, about the slippage, so the limit bounds the books and not the money.
- *Count what was delivered once a payment ends:* a window's spend can pass its limit, and the client learns of it only once the money is gone.
- *Halve what is left before every packet:* it never overshoots, but every half costs a round trip, so the end of each payment crawls. Learning the rate from fulfilled packets sizes the rest in one.
- *Size packets against the cap at the payment's rate:* that is the least the path delivers, so on a better path every packet overshoots by the difference.
