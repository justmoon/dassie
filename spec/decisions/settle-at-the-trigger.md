# A node asks a peer to settle as soon as its room falls to the trigger

**Areas:** [Settlement](../areas/settlement.md#asking-a-peer-to-settle), [Peer protocol](../areas/peer-protocol.md), [Settlement hubs](../areas/hubs.md#in-the-node)

## Facts

- Only the creditor can tell when a peer should settle, so it asks, and the debtor decides how much ([own-fulfills](own-fulfills.md)). Over a hub, a request has the payer fund its hold toward the asker by about four and three quarters of the largest packet, at the default credit ([Settlement](../areas/settlement.md#positions-and-thresholds)).
- The node asked at its settlement check alone, which runs every few seconds over every method of every peering. A method whose room fell to the trigger waited for the next check: half an interval on average, a whole one at worst, while a funding takes about half a second from the request to the payee's books.
- Below the trigger, the payer has one and a half of the largest packets of room left. A busy link spends that in a fraction of a second, and its packets are then refused (T04) until the funding lands; senders back off on a refusal, swaps and STREAM alike, and keep backing off for a while after the room is back.
- So a link over a hub carried about one funding per check, some 1.1 of the largest packets a second at the production check of four seconds. A model of swaps between nodes over hubs found the time to top up holds, not the money in flight, to be what limits them, and asking as soon as the room fell to the trigger gave three to four times as much.
- Two core nodes skewed apart, swapping over an XRP hub and a DSUSD hub on the fake ledger, bore it out. With the check at four seconds, the room sat at or below the trigger for 2.6 s on average before the request went out, and up to 11 s; the swaps moved 0.6 of the largest packets a second, and their first fifty in 108 s. Asked in the step that took the room to the trigger, they moved 1.3 a second, and the first fifty in 33 s. At the dev timings' one-second check the gain is smaller, the first fifty in 22 s and 35 s rather than 29 s and 49 s, since the wait was shorter to begin with.
- What lowers a peer's room is, nearly always, a Prepare from it that the node takes: it is booked pending on the peer's account, and over a hub it takes room from the peer's hold toward us. Other changes, such as our credit following our own hold, or a settlement of ours, are rare.
- A request already bounds how much the debtor settles: one settlement at a time per method, the answer saying what is already on its way, and nothing less than the least settlement. The asker asks again once its books show a settlement from the peer, and otherwise at most once a minute.

## Decision

- **A node asks as soon as a Prepare it takes from a peer leaves the peer's room on that method at or below the trigger,** in the step that took it, by the same rule as the check: only while its scheme sees its ledger and can receive, and not with unlimited trust, on a method not being wound down.
- **The settlement check stays, as the backstop:** it asks for every method whose room fell otherwise, or whose peer was not due to be asked when it fell.
- **Every bound on asking stays.** A peer is asked again once our books show a settlement from it on that method, a refusal for not being peered, or a request that never reached it; otherwise at most once per `settlement_request_every_ms`. The payer still settles one settlement at a time per method, and answers a request with what is already on its way.
- **One request per method in flight.** The peer protocol sends no request to settle on a method while one to the same peer on it is in flight, until it is answered, refused or given up, and asserts it with its invariants. However many Prepares find the peer short, and even when a settlement booked lets the peer be asked again before the last answer came, a node never has two requests on a method outstanding.

## Accepted

- **A few lookups on the packet path:** each Prepare from a peer reads the peer's position and, over a hub, the room of its hold, unless the peer was asked and is not due again, which is one lookup. That is bounded by the peer and its method, never by the number of peers.
- **A payer that cannot fund when asked** answers that nothing is on its way, and is asked again only after the interval, a minute in production, as before: one whose hub account has nothing to spend until the node next moves funds into the hub. Links that move faster drain hub accounts faster, so this comes more often: three requests in 47 asked at the trigger, against one in 37 at the check, in the runs above.
- **Money in flight is unchanged:** the cap, the trigger and the least settlement stay what [thresholds-in-packets](thresholds-in-packets.md) and [packets-in-flight](packets-in-flight.md) make them, so what a link survives does not move ([crash-drift](crash-drift.md)).
- **A refused Prepare can ask too.** A Prepare the books refuse for want of room still has its peer asked if it is due: its room is at or below the trigger, which is when it should be asked.

## Rejected

- *A shorter check interval:* each check sweeps every method of every peering, downlinks included, and a method still waits half an interval on average. Dev and test timings follow the production rule ([engineering](../engineering.md)), so a shorter dev interval alone would only hide the wait.
- *A higher trigger, so that the check finds the peer sooner:* it takes room the credit has to keep for a crash's drift ([thresholds-in-packets](thresholds-in-packets.md)), and still waits for a check.
- *Asking on every Prepare at or below the trigger:* a busy peer would get a request per packet. The interval and one request in flight per method bound it.
- *The payer funding its hold on its own as the payee draws on it:* the payer knows only the credit it extends, not the room the payee leaves it, so it cannot tell when to settle ([own-fulfills](own-fulfills.md)).
