# D17. The upstream decides whether a Fulfill counted

**Status:** superseded by [D23](D23-each-node-books-its-own-fulfills.md) on 2026-10-18. Decided 2026-10-14.

## What it decided

A Fulfill one hop posted could fail to count at its upstream: the downstream crashed before it arrived, the upstream crashed with the Prepare in flight, or it arrived after the upstream's expiry. The downstream then held the upstream to a debt the upstream never had.

D17 had the upstream decide. Its reply to every Fulfill said `accepted` or `refused`, from a durable record of the Fulfills it took. The downstream sent each Fulfill until it got a verdict, and on `refused` booked the loss to `expenses/losses`, or reversed it if its own receiver had taken the packet.

## Why it was replaced

The verdict decided who bears a loss, as ILP's rule says, at the cost of durable state, retries and a verdict per Fulfill; a dishonest upstream could still refuse Fulfills it forwarded and keep the difference. Counting refusals against a peer needed [D18](D18-one-grace-limit-per-peer.md)'s grace limit, which failed honest juniors. [D19](D19-each-node-keeps-its-own-books.md) made agreement between books a non-goal, and [D23](D23-each-node-books-its-own-fulfills.md) has each node keep its own Fulfills as final: a refusal then changes nothing in the downstream's books.

## What remains

- The reply to a Fulfill keeps its one-byte body, always `accepted`, for nodes of 0.1, which read it as D17's verdict.
- Nodes up to 0.1.3 booked refused Fulfills as `refusal` rows, a reversal or a loss in `expenses/losses`; those rows stay in their journals.
