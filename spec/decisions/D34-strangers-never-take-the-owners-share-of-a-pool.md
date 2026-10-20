# D34. Strangers never take the owner's share of a pool

**Areas:** [Payments](../areas/payments.md), [STREAM](../areas/stream.md).

## Facts

- Open Payments is for anyone. Asking for a grant needs only a key, and a key costs nothing; asking for one the owner decides needs only a wallet address that publishes a key. A quote grant needs no owner, and every read of a payment request hands out credentials of their own.
- Each pool behind these is bounded and shared: the grants kept, the grants waiting for the owner, the rate probes, which the owner's Send uses too, and the places in an incoming payment for the connections that pay it. Bounds are what keep a node's tables from growing (D16), but a shared bound is one a stranger can fill. Then the owner's own Send cannot quote, an app the owner uses cannot ask for a grant, or a payment link takes no new payer. No money moves; the node stops working for its owner.
- The rate limits per address slow a stranger down, but do not bound what a stranger holds. A pool fills over its entries' lifetime, an IPv6 /56 is cheap, and a customer of a provider usually has a whole one.
- The owner reaches payments over RPC, without grants. Among clients, the only ones the node knows to be the owner's choice are those the owner approved, by their key, which their wallet address publishes.
- A receiver counts a connection, and gives it a place, only when its money is posted, which happens in the step that admitted the packet. No money is ever on its way to a connection that has a place.
- What a receiver tells a sender it received is its own count. A sender whose count was forgotten and who comes back is told less than it delivered, and may send that again.

## Decision

Every such pool keeps a share strangers cannot take, and no one stranger holds more than a small share of it. Where a place must change hands, it goes from an idle holder that paid only dust to a new payer.

- **The owner's side** of the grants is the clients the owner approved before: a client with a grant the owner approved and still kept. Strangers leave them a share of the grants kept and of the places where grants wait. Grants approved at once leave room for every grant that may wait, so a flood of grants that need no owner never stops one that does.
- **The rate probes** keep a few slots for the owner's Send alone.
- **Shares per client and per network.** No one client, and no one network of strangers (an IPv4 /24, an IPv6 /56, IPv4 mapped into IPv6 counted as IPv4), holds more than a small share of the grants kept, the grants waiting and the probes. Each grant keeps the network it was asked for from, so the share holds across restarts. A client the owner approved is held to its own share only, not its network's. Loopback counts in the totals only where private peers are allowed, as other limits per client do, so development and tests run unchanged.
- **Places in an incoming payment.** Once every place is taken, a new connection takes the place of one that received less than dust and nothing for STREAM's idle timeout, the one idle longest. Dust is as little as the XRP Ledger can move, the same share of an asset on every ledger. The connection replaced is forgotten; what it received stays in the payment's total.

## Accepted

- A client the owner has never approved is a stranger: its first request competes with every other stranger's, within the shares. Nothing the node sees at that moment tells it apart from a stranger.
- A stranger who controls many networks still fills the strangers' share. The shares make that cost many networks, and leave the owner's share alone.
- A sender whose place was taken and who comes back is told it received nothing. It may send again what it was told it had delivered, less than dust. Dassie's own payer keeps the highest total it heard on a connection, so only one that reconnects can be told less.
- A stranger who pays dust or more on each connection keeps its places. Locking a payment link then costs dust for every place, paid to the owner.
- A request over its client's or its network's share gets a 429, as a rate limit does; one that finds its pool full gets a 503.

## Rejected

- *Larger pools only:* a stranger fills any shared bound, given time and networks.
- *Rate limits per address alone:* they bound how fast a stranger fills a pool, not how much of it a stranger holds.
- *Shares per network alone:* clouds and providers put many parties in one IPv4 /24, so a client the owner relies on would be refused for its neighbours' requests.
- *Identify the owner's grants by the owner's session:* grant requests come from the client, never with the owner's cookie, and the owner pays over RPC without grants.
- *Let any idle connection give up its place:* a sender that paid much and paused would be told on its return that it delivered nothing, and could pay it all again.
- *Refuse a returning connection whose place was taken:* telling it apart from a new one means remembering every key that lost its place, which a stranger can make grow without bound.
