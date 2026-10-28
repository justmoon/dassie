# A payer funds a request it could not fund as soon as it can

**Areas:** [Settlement](../areas/settlement.md#settling-on-request), [Settlement hubs](../areas/hubs.md#in-the-node)

## Facts

- A payer over a hub funds a request with what its hub account can spend, which may be part of what the rule asks, or nothing. With nothing, it answered that nothing was on its way, and forgot the request ([own-fulfills](own-fulfills.md)).
- The asker asks again once its books show a settlement from the payer, and otherwise only after the request interval, a minute in production ([Settlement](../areas/settlement.md#asking-a-peer-to-settle)). So a payer that answered nothing left its payee short of room for the whole interval, its packets refused (T04), however soon the payer could have funded it.
- The account runs short in two ways. Its balance runs out: the node deposits from the owner book only once the balance is below the floor, which is one request's funding per peering over the hub, and checks that at its funding check, every few seconds; a busy link asks again within a second or two of being funded, and the deposit takes seconds to land, so the next request finds the account empty. And for a moment as each block arrives, while the account reads up to the newest state it heard of, it takes no operation at all, whatever its balance.
- Two core nodes swapping over an XRP hub and a DSUSD hub on the fake ledger showed both. With the dev timings, a payer whose balance had fallen to 1.8 of the largest packets funded that, began a deposit at its next check, and was asked again 0.4 s later: it answered nothing. The deposit landed 0.8 s after that; the payee asked again 5.5 s after its request, when its interval let it. With the production check, three of the four requests answered with nothing came while the account held twenty to forty packets' worth and had not yet read up to the newest state it heard of. Across four runs, a request answered with nothing was funded 5 to 13 s later, with the re-ask interval at the dev timings' 5 s; at the production minute, it would have been up to a minute.
- A payer clamped to what its account could spend sometimes funded a fraction of a packet, a hub fee for a few drops, which the least settlement is there to prevent.
- A request carries no amount, and the payer alone decides what to settle ([own-fulfills](own-fulfills.md)). The asker does not know why it got nothing, nor when the payer will be able to fund it.
- Answered late, as decided below, every request answered with nothing in the same runs was funded 0 to 3 s later, without a second request, and no method's room sat at the trigger for more than 4 s, against up to 13.5 s before.

## Decision

- **A request the payer cannot fund in full is answered late.** The payer keeps it, one per method of a peering, in memory, with when it came. As soon as its scheme can spend at least the least settlement, and the method and the settlement channel are free, it funds it as the rule stands then, as if asked again at that moment. It keeps it while what it funded is still short of the rule, and forgets it once funded in full, once its own request interval has passed since the request, which is when an asker under the same policy asks again, or once the peering is being ended, the method is being wound down or the peering is gone. A restart forgets it, as the asker's requests are forgotten ([Settlement](../areas/settlement.md#asking-a-peer-to-settle)), and the asker asks again after its interval.
- **"A node never settles on its own" reads: a node settles only to answer a request,** at once or, when it could not fund it in full then, as soon as it can, while its asker waits before asking again. A request answered late is still the asker's: it asked, and the payer decides how much from its own books at the moment it funds.
- **Nothing less than the least settlement for want of funds.** A payer whose scheme could fund less than the least settlement funds nothing now, and the request in full once it can.
- **The scheme says what it can spend** (`SettlementScheme::spendable`), as it would clamp a settlement: over a hub, nothing while the account takes no operation. The node funds late only when that changed since a try that funded nothing, so a scheme that cannot settle for another reason is not asked at every step.
- **A short request starts the hub's funding check at once,** so that a deposit the floor calls for leaves now, not at the next check.
- The payee is unchanged: it asks at the trigger and at its check, again once its books show a settlement from the payer, and otherwise once per interval.

## Accepted

- **State on the payer:** at most one entry per method of a peering, the bound its invariants check, kept for at most one request interval. Every step reads the scheme's spendable funds for each kept request, which is bounded the same way and none in the usual case.
- **A late settlement can cross the end of a peering** the asker is ending: it asks once, may get nothing, and tells the payer the peering is over; funds that arrive in between are settled late. It is credited once to the payer's account, as any settlement that arrives after the end ([ending-a-peering](ending-a-peering.md)).
- **What the asker sees is unchanged:** it learns of a late funding from its books, as of any settlement, and asks again from there. An answer of nothing no longer means nothing will come before the asker asks again.
- **A deposit still has to land.** The payee waits for it, seconds, rather than for its interval, a minute; how often an account runs short is the floor's to change, below.
- **The stub with commitments counts as spendable only what a hub member could:** its balance less what the peers' holds toward it hold, not taken yet. Otherwise two stubs that asked each other could pay each other's late requests from each other's settlements.

## Rejected

- *The payee asks again sooner after an answer of nothing,* with a backoff from a second or so up to the interval. The payee cannot know when the payer's funds arrive, so it waits up to a step of its backoff after they do, and it sends requests to a payer that is empty for good several times as often. Requests carry no amount and the answer no reason, so the payee also cannot tell an empty payer from one whose method is busy.
- *A payer that keeps more in its hub while it is asked often,* a floor raised by its request rate, or a deposit earlier. It makes the account run short less often, but not never, and still leaves the payee waiting for its interval when it does; and how much a request funds, which sets the floor, is about to follow how often the payer is asked, so the floor follows that change rather than this one.
- *The payer tells the payee when it can fund,* with a message of its own: a change to the peer protocol, for what the payer can do by funding.
- *Persisting the requests kept:* a restart then funds a request whose asker may have asked again, been answered, or stopped needing it; forgetting it costs one interval at most, as before.
