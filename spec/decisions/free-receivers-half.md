# Free receivers take at most half of the downlinks' places, and a downlink is free until its first settlement

**Areas:** [Network](../areas/network.md#downlinks), [Settlement](../areas/settlement.md#settlement-channels), [Periphery](../designs/periphery.md#8-free-receivers).

## Facts

- A core node takes downlinks, up to `max_downlinks`. A downlink proves only that it holds its key: it is not registered, not probed, and pays nothing to peer; the uplink extends it no credit ([earned-trust](earned-trust.md)). A key costs nothing, and keeping a peering costs a heartbeat now and then. So anyone could make keys and hold every downlink place for as long as they kept heartbeating, and the node would refuse real periphery nodes for capacity.
- A downlink's settlement channel holds one of those places until it closes, a silent peer's wait after its peering ended ([closing-channels](closing-channels.md)). Keys that peer and leave hold places too.
- The design gives free receivers, downlinks that have not yet paid anything, at most half of an uplink's downlinks ([Periphery](../designs/periphery.md#8-free-receivers)).
- A channel's `settled_in` moves with every settlement message the node applies. On the XRP Ledger and over a settlement hub the message books nothing: the ledger says what arrived ([Settlement](../areas/settlement.md#incoming-settlements)). Any message with well-formed data moves it, so it shows nothing about money. What the node's books took from a node as its incoming settlement does.
- Honest periphery nodes are idle most of the time. An attacker can fake any least amount of traffic.

## Decision

- **A free receiver is a downlink that never paid us:** the node's books never took a settlement of at least one largest packet from it, the least an honest node settles ([thresholds-in-packets](thresholds-in-packets.md)), on any ledger. The node marks the node's settlement channel when its books take one (`paid_us`), and the mark stays for good, open or closed, as the mark of an end for room does ([ended-for-room](ended-for-room.md)).
- **Free receivers hold at most half of the downlinks' places** (`Config::max_free_downlinks`, half of `max_downlinks` rounded down). A place is an open channel with a downlink, so they count those peered with and those not long ago, as the downlinks' bound does. Once free receivers hold that many, a new downlink that never paid us is refused for capacity. One that paid us is bounded by `max_downlinks` alone.
- **Only newcomers are refused.** A downlink whose channel is open as a downlink's holds its place already and gets in, and a repeated request is answered with the peering as it was made. Nothing is ever ended or displaced by the rule. A free receiver that pays holds a paying place from then on, and leaves its free one to another.
- **A node that takes downlinks has room for a free receiver:** `max_downlinks` is zero or at least two (`Config::validate`).

## Accepted

- **Keys made for nothing can hold the free half.** Free receivers that are new to the uplink then wait for a place, or peer with another uplink, as with any full uplink. A periphery node asks several.
- **A newcomer counts as free until its first settlement.** The uplink cannot know a node will pay before it has, and a node settles only once peered. So while the free half is full, a node new to the uplink gets in only by paying on the ledger before it asks, which the uplink holds for it until it peers ([pay-to-enter](pay-to-enter.md)); one that paid this uplink before always gets in.
- **What paying costs an attacker:** one settlement of a largest packet per key, which the uplink books from its own ledger. On the XRP Ledger that is a Payment with the key's memo from the address its peering named, and many keys may name one account. The money is not lost: it is the key's balance with the uplink, which it may spend back out through the uplink, so the same money can mark key after key, for each the ledger's fee, the uplink's fees and the time a settlement takes. On the testnet, whose XRP is free, that time is all it costs. On mainnet it takes a largest packet of capital at a time, and fees. Only a fee would make the cost final.
- **The mark is for good.** A downlink that paid once and spent it all keeps its paying place. A mark that followed the balance would turn a paying downlink free again with a payment it sends, so free receivers could come to hold more than half, and the rule would have to end or displace one to restore it.
- **The mark is not derived from the journal.** A relationship whose settlements were booked before the node kept it starts unmarked, and is marked at its next settlement; a downlink peered meanwhile holds its place.
- **On the stub scheme a settlement message is money.** The stub books what its message says, so a message alone marks its sender. The stub is test money.

## Rejected

- *Ending an idle downlink to make room:* honest nodes are idle most of the time, while an attacker can fake any least amount of traffic, so any rule by activity would let attackers end honest nodes.
- *Displacing an incumbent for a newcomer,* the oldest or any other: an attacker's new keys would push out honest downlinks as fast as it made them.
- *A rate limit per network on new downlinks,* as on probes and registrations: places stay held for as long as heartbeats go on, so a limit only slows the filling, and an attacker with a few networks, or an IPv6 range, fills them all in time.
- *Proof of work per key:* it slows the filling once, while places stay held for good, and an honest phone or browser node pays the same work as an attacker.
- *A fee to peer:* it makes the cost final, but a free receiver's point is to receive without paying anything first ([core-and-periphery](core-and-periphery.md)). Fees for using the network belong with [registration fees](../roadmap.md#registration-fees).
- *Counting a channel's `settled_in`:* any key moves it with a message, which books nothing on the XRP Ledger or over a hub.
- *Counting any amount the books took:* a Payment of one drop would do, for the ledger's fee.
- *Counting peerings rather than channels:* a channel holds a downlink place after its peering ended, so keys that peer and leave would fill the places while the peerings stayed under half.
- *Rounding up:* with an odd bound, free receivers would hold more than half, and with a bound of one, all of it.
