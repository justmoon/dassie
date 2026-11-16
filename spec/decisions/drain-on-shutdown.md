# A node that stops waits for its packets in flight, until the latest of them is due

**Areas:** [Node](../areas/node.md#shutdown), [Connector](../areas/connector.md#responses-and-expiry)

## Facts

- A packet's pending transfers live in memory only: writing each Prepare to disk would cost a write per packet, and a late Fulfill would still count on one side only ([own-fulfills](own-fulfills.md)). So a node that stops with packets in flight forgets them; after the restart its books stand as if they had been rejected, and a Fulfill that came back is lost, as in a crash. The peer booked its Fulfill as final, so the two books drift apart by what was in flight ([crash-drift](crash-drift.md)).
- The cap on money in flight with a peer bounds what one stop costs a link ([packets-in-flight](packets-in-flight.md)), not what many stops add up to. Restarts are routine where crashes are not: every update restarts the node, and the desktop app stops its node each time its owner quits it or the session ends.
- Every packet in flight has a deadline the node knows: its outgoing expiry, at most the longest hold time from when it went out, or, for a packet to a peer over a hub, a backstop at most one message window later ([connector-timing](connector-timing.md)). By then it has been answered or rejected upstream.
- The answers come over the same connections as the packets: QUIC for peers, a WebSocket for BTP clients, and for a packet over a hub, the hub's blocks over the ledger connections.

## Decision

At SIGTERM the node drains before it stops.

- **No new packet goes in flight,** from any source: peers, BTP clients, the node's own payments, quotes and swaps. A new Prepare gets T03 "the node is stopping". The node's own payments send nothing more and wait; they continue after the restart from what was posted.
- **It waits until the latest deadline among the packets in flight** at the moment it began, which no packet can pass since none can join. Each is answered or expires by then, and it stops as soon as none is left, at once when none was there.
- **Everything that carries an answer stays up** until the engine has stopped: QUIC, the HTTP listeners with their BTP connections, the ledger connections and settlements.
- **A second SIGTERM stops it at once,** with the loss of a crash. The first one warns, when packets are in flight, that a second skips the wait.

## Accepted

- **A stop takes up to the longest hold time and a message window longer,** about half a minute, when a packet in flight is not answered. Most packets are answered in well under a second, so most stops take no longer than before. systemd's stop timeout and an update's restart allow for it.
- **Refusing new packets fails the payments that would have crossed the node during the wait.** Their senders take T03 as temporary and try again, after the restart or by another path; a payment of the node's own waits and goes on.
- **A drain that a second signal cuts short loses what was in flight,** as a crash does, within the same cap.
- **T03 also means a full table.** The two are told apart by the message only; neither Dassie's sender nor the node itself acts on which T code it got, and both mean that this node refuses for now.

## Rejected

- *A fixed wait,* as most servers have: it either stops before a slow packet is answered, or waits past the last deadline for nothing. The node knows when the last answer is due, so it waits exactly that long.
- *Closing the BTP clients at once,* as before: a client's connection carries the answers both ways. Closing it fails the packets sent to the client, though it may have fulfilled them already, so it loses what it paid out; and it drops the answers to the client's own packets, so a Fulfill the node booked never reaches the sender, which counts that money unpaid. Their new Prepares are refused anyway, so keeping them costs nothing.
- *T01 for the refusal,* as a peering that ends answers: T01 says a hop beyond the node cannot be reached, which is not so. *T00* says something failed, *T04* that liquidity ran out, and a final code would end payments that can go through after the restart.
- *Persisting the packets in flight,* so that a restart picks them up: [own-fulfills](own-fulfills.md) rejects it, and a drain makes it needless for an orderly stop.
