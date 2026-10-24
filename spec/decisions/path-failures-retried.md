# A sender sends money again after a too-low rate or a node's final error, until failures without progress stop it

**Areas:** [STREAM](../areas/stream.md#responses), [Payments](../areas/payments.md#outgoing-payments)

## Facts

- The packets of one payment may take different paths: the connector draws each packet's next hop ([weighted-next-hops](weighted-next-hops.md)). A failure on one path says little about the path the next packet takes.
- A receiver refuses a packet that delivers less than the sender's minimum, and says what arrived in its reply. That is one packet over one path.
- Only the receiver can write a reply that decrypts under the shared secret and names the request's sequence and type. A node on the path can return any code with any `triggered_by` address, so a Reject without such a reply proves nothing about the receiver.
- The sender stopped money at the first too-low refusal and closed the connection at the first final error, as `ilp-protocol-stream` closes. With one path per payment that was right. With a draw per packet, one node with a poor rate or a broken route would stop every payment some of whose packets it draws.
- Some receivers refuse without a reply: a Dassie receiver refuses a packet its application does not admit, or whose token it cannot read, with a plain Reject.

## Decision

- **The reply tells the receiver from the path.** A Reject with a reply that decrypts and matches the request is the receiver's, whatever its code; one without is a node's on the path.
- **A too-low refusal from the receiver, and a final or unknown-class error from a node on the path,** release the packet's money, which goes again in a later packet, without a back-off. Neither is congestion, and a back-off per failure would let one bad node slow every packet on every path.
- **The receiver's own final error closes the connection,** as before: the receiver will not take the connection's packets, whatever path they take.
- **A bound on failures without progress.** `MAX_FAILURES_WITHOUT_PROGRESS` such failures in a row with no Fulfill between stop money until the application sets a rate again, and an event tells the application, with the latest too-low refusal among them if there was one: it may set a lower rate, open a new connection, or give up. Only a Fulfill ends a run; temporary errors, F08 and the receiver's other refusals neither count nor end one. Once money stopped, a failure on the path backs off as a temporary error does, so data does not loop either.
- **A count, not a time,** so the bound is on packets a sender spends on paths that do not deliver, whatever the round trip. It is large enough that a run that long by chance, with a fair share of paths failing, is rare over a long payment.
- **Sender-side only.** Nothing on the wire changes, receivers answer as before, and the frozen surfaces hold ([overview](../overview.md#frozen-surfaces)). A rate measurement's probes are refused by design and are handled as before.
- **The node's payer reopens a connection whose money stopped,** as one that closed early, a bounded number of times, at the payment's own rate: the quote promised it, and a while later the paths may deliver at it again ([Payments](../areas/payments.md#outgoing-payments)).

## Accepted

- **A path that never delivers costs the bound in packets** before the application hears of it, where a final error once ended the connection at its first packet. With no back-off they go within a few round trips.
- **A receiver's refusal without a reply,** such as a refused admission, is taken for the path's and costs the bound too, so a payment to it fails later than it did.
- **A node can slow a payment** by failing its packets, and a dear node costs the sender Prepares, but neither stops it while another path delivers. A node that charges too much is refused and earns nothing, which rewards the cheaper ones.
- **A run of the bound can happen by chance** when most paths fail; the application then sets the rate again or reconnects, which the node's payer does a bounded number of times.

## Rejected

- *Stopping money at the first too-low refusal:* it assumes one path per payment, so one dear node on one path stops it.
- *Closing on any final error, as `ilp-protocol-stream` does:* compatible, but one node on one path ends the payment.
- *Retrying without a bound:* a sender that no path serves would loop for ever.
- *Trusting a Reject's code or `triggered_by` to tell the receiver from the path:* any node can write either, and so end a payment.
- *Backing off on a node's final error as on a temporary one:* it answers congestion that is not there, and lets one bad node slow every path; the bound limits the retries instead.
- *A time without progress as the bound:* its value depends on the round trip, and it bounds the packets spent only through how fast they go.
