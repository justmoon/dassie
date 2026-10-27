# The simulation rules out a link that crashes drifted past what it survives

**Areas:** [Testing](../areas/testing.md), [Settlement](../areas/settlement.md)

## Facts

- Each node books the Fulfills it sends as final ([own-fulfills](own-fulfills.md)). A Fulfill whose sender crashed, or failed a commit, before it took it counts in one node's books only, and the two books drift apart by its amount. One crash drifts them by at most what was in flight ([packets-in-flight](packets-in-flight.md)).
- A link survives drift of up to the credit both sides extend, less two triggers and twice the least settlement ([Settlement](../areas/settlement.md#credit)). Over a hub that credit is the two sides' holds toward each other, so it follows what each has funded; with unlimited trust no run drifts a link near it. Past that, the creditor's room for its debtor stays at its trigger while the debtor's books leave it nothing to settle, and the creditor withdraws the debtor and, in the end, ends the peering. [own-fulfills](own-fulfills.md) and [peering-lifecycle](peering-lifecycle.md) accept that such a link is not saved.
- The `node` simulation draws crashes and failed commits freely, so a seed can drift a link past what it survives. The heal must then settle down, which the design does not promise for that link, and the run failed at `settle-down` though every node did as the design says.
- Not every lost Fulfill is a crash's. A Fulfill can be refused, arrive after its expiry, or never get across a cut. Those are bugs, or faults the design bounds otherwise ([unreachable-peers](unreachable-peers.md)). A run that failed at `settle-down` had lost Fulfills to crashes and, more of them, to a refusal by a node that had just started, which was a bug.

## Decision

The simulation measures how far each pair of nodes' books drifted, by what crossed the network, and rules out a run in which crashes alone drifted a link past what it survives.

- **Measured from the wire, never from the books** ([own-books](own-books.md)). The simulation follows each Prepare with an amount, the Fulfill the peer sends for it, which the peer booked first, and whether its sender took that Fulfill: answered 200 by the same generation of the sender, before the expiry on its clock. A Prepare whose Fulfill was sent and never taken, once past being taken, is drift.
- **What really lost it.** A crash or a failed commit of the sender while it could still take the Fulfill lost it: its connector forgot the packet. A refusal (400, 401 or 409) of the peer's Fulfill, a Fulfill that came late or never got across a cut, is something else. A crash or a failed commit of the peer while its Fulfill could still arrive counts only if nothing else lost it: a copy already on its way may still arrive and be refused or late, and then the refusal or the delay lost it, never the crash, so that a bug that refuses Fulfills is never ruled out. Otherwise what lost it first stays.
- **Ruled out only when crashes alone drifted a link past what it survives,** with one side's room for the other at its trigger, the symptom the decisions accept. What it survives is counted by the rule above, from the credit each side extends on the link's first method as the run ends. The run that did not settle down is then `unsupported` at `drifted-past-what-it-survives`, after the checks of what must not happen even with money in flight. Otherwise it is a liveness failure at `settle-down`, whose message names any link drifted past what it survives, and by what.

## Accepted

- **An excused run checks no other liveness.** Something else that did not happen in the same run goes unseen there. Other seeds still catch it.
- **The measurement only ever falls short.** It counts only what surely drifted: a Fulfill booked but never sent, or not taken because the sender's peering had ended, is not seen. So a run may still fail where it should have been ruled out, and never is ruled out by drift that did not happen.
- **A crash during a cut counts as a crash's,** when the peer crashed while its Fulfill could not get across.

## Rejected

- *Comparing the two nodes' books:* it tells how far they drifted, but not why, so it would forgive a bug that loses Fulfills as readily as a crash. The refusal by a node that had just started was found because the wire told it apart.
- *Forgiving any drift past what a link survives:* drift from cuts and refusals is not accepted by the design ([unreachable-peers](unreachable-peers.md)), and forgiving it would hide exactly the bugs that cause it.
- *Fewer crashes in the generator,* or none with money in flight once a link's share is spent: every run would be judged in full, but no run would reach what happens past the bound, and the generator would be tuned for runs to pass.
- *More credit:* it moves the bound, and the generator reaches it a little later.
- *Saving the link in the node:* a reconciliation of the two books is what [own-books](own-books.md) rejects.
