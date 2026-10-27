# Direct settlement

How two peers settle directly, each paying the other on an underlying ledger they share, with no settlement hub between them, and what bringing it back would take. Peers settle only over settlement hubs ([hubs-only](../decisions/hubs-only.md)), so none of this is in the code: the schemes on the XRP Ledger are the owner's books and peer with no one ([Settlement](../areas/settlement.md#owner-books)), the stub settles over a pretend hub or not at all ([Settlement](../areas/settlement.md#the-stub-scheme)), and a scheme that is neither over a hub nor of unlimited trust never becomes a settlement method ([Network](../areas/network.md#settlement-methods)). This design keeps what the code no longer says: the rules of credit a direct peering needs, how a settlement on the ledger reached the payee's books, and why each rule was what it was.

## 1. What a direct peering is

A direct peering settles on a scheme both nodes run on an underlying ledger: XRP or a dollar token on the XRP Ledger, or, in tests, a plain stub. The debtor pays the creditor's address on that ledger, and the creditor's scheme sees the payment arrive as it follows its account. Nothing holds the debtor to it: a peer can always walk away with what it owes, so **every direct peering is unsecured credit**, bounded only by the credit the creditor extends ([own-fulfills](../decisions/own-fulfills.md)). Each node keeps its own books, and whatever a peer does it costs at most that credit plus what it has in flight ([own-books](../decisions/own-books.md)).

Everything that does not depend on who holds the money is the same over a hub, and stays in the code: a peering's settlement methods, each with its own accounts, position and thresholds ([settlement-methods](../decisions/settlement-methods.md)); the creditor asking at its trigger and the debtor settling half the way to its own ([Settlement](../areas/settlement.md#settling-on-request)); thresholds counted in the largest packet `P` ([thresholds-in-packets](../decisions/thresholds-in-packets.md)); settlement channels that deliver each `settlement` message exactly once; withdrawing a peer short of room, and ending a peering whose room stays short for as long as a silent peer is kept ([ended-for-room](../decisions/ended-for-room.md)). What a direct peering adds is below.

## 2. Credit

Over a hub, the payer's hold is all the credit there is. A direct peering has no hold, so the credit a node extends is a policy of its own, and with it the loss a peer can cause.

- **Roles.** The node whose peering request was accepted is the **junior**, the node that accepted it the **senior**; when requests cross, the lower node ID's request wins, so the lower ID is the junior. A periphery node always asks, so it is always its uplinks' junior. The node that asked to peer takes the risk at first.
- **Starting credit.** The junior extends the senior the default credit line; the senior extends the junior nothing. Asked to settle at once, the junior pays in advance, and payments to the junior flow at once, within the credit it extends. On each method of a peering the junior extends the default line, in that method's ledger's units, a method added later included; no budget spans methods, which would need exchange rates in the credit rules.
- **Peering again** keeps the trust account and its credit. A junior still extends at least the default line: a node that was the senior before, or that accepted a request only one side recorded, raises the account to it.
- **No credit back after an end for room.** The node that ends a peering for room marks it on the settlement channel, which outlives peerings. A later peering in which it is the junior keeps the trust account as it is, on every method, rather than raising it to the default: a peer that never settled gets no new line by being asked again, and its credit grows again only with what it earns the node. The mark itself stays over hubs, for the cooldown that keeps the pair apart ([ended-for-room](../decisions/ended-for-room.md)).
- **Nothing is announced.** No message of the peer protocol carries the credit a node extends. More credit only means the creditor asks later.

**The least credit.** Asked at the asker's trigger, a debtor settles only if its own books leave it its trigger plus twice the least settlement. So a link survives drift of up to its credit less two triggers and twice the least settlement, and a node refuses to start with a default credit, or a senior credit other than zero, below that plus one crash's drift at the cap on money in flight. A link cut one way drifts no more than a crash, since a node takes no new Prepare from a peer its messages surely do not reach.

```
least credit = 2 · trigger + 2 · least settlement + money in flight = 3P + 2P + 2.5P = 7.5 P
```

The default line, 11 P, clears it with room for about two crashes. A link whose crashes drifted it past what it survives is stuck as accepted: one side's room for the other stays at its trigger while the other's books leave it nothing to settle, and the simulation rules such a run out rather than call it a failure.

**Growth from what a peer earned us.** Each side raises the credit it extends to the other by k = 10% of what it earned on packets that came in or went out through that peer.

- **What counts** is the connector's fee, toward each peer of the packet on the ledger the fee was booked on. A packet's fee counts toward both peers it passed, so k must stay at most 50% for the credit a node hands out never to exceed what it earned; 10% leaves a margin. A packet that converts counts toward the peer it came from only, since counting it toward the other would need the node's exchange rates. What a conversion gains or loses is not booked per packet and does not count.
- **A cap** of a hundred largest packets. Credit above it, configured or from before, stays and never falls.
- **Measured from the books.** What a peer earned is read from the journal's fee rows, never kept in counters, and the raise is a `trust` transfer. Reading resumes after the last trust transfer, so a crash can forget fees not read yet, and credits none twice.
- **Retirement is accepted.** A node that leaves can spend everything its peers trust it with and disappear; each peer loses at most k of what it earned through that node. Fees default to zero, so trust grows only once an owner charges for passing payments on, and until then a junior keeps settling in advance.

Rejected: growth by the amount forwarded (at a low fee a node would extend more than it earned, and lose money to every peer that retires), growth with time (time earns nothing), and the same credit both ways at peering (a newcomer gets credit it did not earn from every node it peers with, and auto-peering lets it do so again under a new key).

## 3. Settling on the ledger

**On the XRP Ledger.** A settlement is a direct `Payment`, never partial, to the address the peer gave when peering, with one memo of the scheme's memo type naming the payer's node ID: XRP for the XRP schemes, the token of its issuer, without paths, for a token scheme. The settlement's ID is the transaction's hash. The memo type, `https://dassie.land/settlement/<scheme ID>/v1`, is a constant of the scheme and a contract between peers, so a Payment is never taken for a settlement of another scheme.

- **Peers' addresses.** The address travels as its 20-byte account ID: the peering info, a peering request's data and the peer's state are exactly those bytes. The scheme learns a peer's address only once the peering is made, never from an attempt that may still be refused, and keeps it, like the settlement channel, until the channel closes; then a Payment from it is the owner's deposit. It keeps at least as many addresses as the node has open channels, so no stranger's attempt displaces a peer's and turns its settlement into the owner's deposit.
- **Booked by the ledger.** As the account follows its ledger, proven, a successful Payment to it with the scheme's memo naming a peer, from the address that peer gave, is that peer's settlement, booked by what it delivered, never by what it stated. Anything else that adds to the account is a deposit.
- **The `settlement` message is a bare notice.** It states no amount, carries no data and books nothing, since the ledger says what arrived, once; it only moves the settlement channel on. An amount in one is ignored, so it can never refuse what the channel delivers. A Payment without the memo, from an address no peer gave, or that failed, is not a settlement.
- **When peers cannot pay us.** A token scheme whose line is frozen deep, whose issuer froze the token for every holder or charges a transfer fee, or whose peg guard pauses it, asks no peer to settle and is left out of the link state; a method's time short of room on it withdraws but never ends the peering. These rules stay for hub schemes whose ledger cannot receive.

**On the plain stub,** for tests: its ledger is a number. It accepts every peering, believes a peer's settlement as stated, and finalizes a settlement as it executes it, cancelling one it cannot pay; one still pending after a restart was never executed, so it is cancelled. An empty config is the stub with unlimited trust's ([Settlement](../areas/settlement.md#the-stub-scheme)), so a stub that settles directly would need a config of its own.

## 4. Entry payments

A periphery node new to an uplink whose free half of downlink places is full could pay its way in ([free-receivers-half](../decisions/free-receivers-half.md)). Only a scheme that follows its account on a ledger could take one.

- **The payment is the settlement it would make:** to the uplink's address, from the address its request will name, with the scheme's memo naming its node ID, of at least one largest packet. The request carries nothing new: the uplink matches what it follows against the address the request names, and the request is authenticated by the newcomer's key, so a payment counts only for the node its memo names, from the address it came from.
- **Held off the books** in a bounded table kept with the scheme's state, each for a bounded number of validated ledgers. As its node peers from that address, the scheme books it once as the node's settlement, which marks the channel as paid. One whose hold is over, one below a largest packet and one that finds the table full are the owner's deposit.
- **Admission skips the free half alone.** Every other bound still holds. Past a full free half, with room left, the uplink refuses a newcomer that has not paid with `PayToEnter` on a scheme that takes entry payments, so a newcomer pays only when told.
- **Accepted:** an attacker pays one largest packet per key, not lost but spendable back out through the uplink; a payment its node does not claim is the uplink owner's; the table can be filled, each place at a largest packet lost to the owner.

A hub takes no entry payments: a payer's funding is a hold that stays its own until the payee claims from it, so before a peering there is neither a payment to the payee nor a key to bind it to. Over hubs, a node with no account is paid into the hub by its peer instead ([hubs-only](../decisions/hubs-only.md)).

## 5. Why it left

[hubs-only](../decisions/hubs-only.md) has the reasons. In short: over a hub the hold is the credit, all of it and nothing more, and the payee is paid from it whether the payer cooperates or not, so a peer can cost no more than what is in flight beyond it. With both ways to settle, every rule of credit, settlement and ending a peering existed twice, and so did every test and simulation of them. Keeping direct settlement as a fallback when no hub is shared or a hub fails was rejected: the unsecured credit would come back exactly when something has gone wrong, and with it all of the machinery above.

## 6. Bringing it back

Direct settlement would come back as a settlement method of its own kind beside the hubs, through a decision that overturns that rejection. What it would take:

- **A scheme that peers directly:** the peer half of the XRP Ledger schemes (§3), and the methods' rule letting such a scheme become a method: today a method is a shared hub or a scheme of unlimited trust (`PeeringHooks::trusts_unlimited`), and a third kind would be added there.
- **The credit machinery** of §2, per method, with the least credit checked at start, growth read from the journal, and the end for room's lower credit on every method.
- **Credit per method.** A peering may have several direct methods, and each is unsecured, so the starting credit can no longer be the default line on each. With `N` the peering's direct methods and `P` the largest packet, the junior extends on each

  ```
  min(P · MAX_TOTAL_INITIAL_ROOM / N, P · MAX_PER_METHOD_INITIAL_ROOM)
  ```

  with `MAX_TOTAL_INITIAL_ROOM` about 20 and `MAX_PER_METHOD_INITIAL_ROOM` about 10: one or two methods start at 10 P each, more share 20 P. **It must never fall below the least credit,** 7.5 P, or a link would stick after a crash it was meant to survive. So the two bounds hold together only up to `MAX_TOTAL_INITIAL_ROOM / 7.5` direct methods, two at those values, and past that one of them gives way. Which one is open, for the decision that brings direct settlement back:

  - **Refuse a further direct method:** the total holds, so a peer can cost no more in all than it did with two methods, but a pair that runs more direct schemes settles on only the first of them, and a scheme its peer would rather be paid on may never become a method.
  - **Raise each method to the least credit:** every direct scheme both run becomes a method, but the total a peering extends grows by the least credit with each method past the bound, so what one peer can cost grows with the schemes it runs.

  What a method earns still raises its own credit, up to the cap.
- **Entry payments** (§4), if a free half still needs a way in that is not a hub.
- **Tests and simulations** of direct settlement: a peer that never settles costs at most its credit and is withdrawn; one short of room for a day is ended; one that spends its credit and vanishes costs at most that credit; a junior pays its senior in advance; credit grows with the fees a peer earned, counted on the peer's own ledger; a link that crashes drifted past what it survives is ruled out; and whole-node runs on the fake XRP Ledger with lying servers, where each node's books hold to its account. The simulations would run direct peerings beside hub peerings, each node judged by its own books.
- **A stub that settles directly,** under a config of its own (§3), for the tests whose subject is direct settlement.
