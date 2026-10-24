# An issued asset's issuer is the owner's counterparty, and a dollar token pauses off its peg

**Areas:** [XRPL](../areas/xrpl.md#rlusd), [Settlement](../areas/settlement.md#when-peers-cannot-pay-us), [Connector](../areas/connector.md#inventory-pricing). Built in the RLUSD scheme; transfer fees ([roadmap](../roadmap.md#not-built-yet)) and the peg guard ([roadmap](../roadmap.md#mainnet-schemes)) are not.

## Facts

- A token on the XRP Ledger, such as RLUSD, is backed by its issuer. RLUSD's issuer can freeze a holder's line, which stops the holder paying anyone but the issuer, and deeper, receiving too; and it can claw back a holder's balance, with a transaction that is no Payment.
- What a peer can cost a node is bounded by the credit it extends plus what is in flight ([own-books](own-books.md)). Nothing bounds what an issuer can take: the whole holding.
- Rates are the median of three sources ([exchange-rates](exchange-rates.md)); the sources a node has quote fiat currencies and XRP. A dollar token priced as `USD` needs no new source, and converts one to one with every other dollar ledger.
- **At par, a de-peg costs at the boundaries.** On the token's own ledger, the debts on both sides of a transit are in the token, so they match whatever it is worth. A conversion is where par is wrong: someone holding a token worth less than a dollar pays it in and takes XRP or another dollar out at par, and the node is drained; off the peg upward, the drain runs the other way. Settling for debt that arose in other assets is a conversion too.
- **The XRP Ledger prices the token.** An AMM pool of XRP and the token holds both, and their ratio times the XRP/USD rate the node already has is the token's price in dollars. A swap changes only the pool account's root (its XRP) and its line to the issuer (the token), both objects a verified transaction can tie ([verified-ledgers](verified-ledgers.md)). On mainnet the RLUSD/XRP pool is deep and trades every few minutes, and its price has stayed within about a percent of a dollar; dollar tokens that lost their peg moved several percent to tens of percent, for hours to days. On the testnet, DSUSD's pool is held at the market by its keeper, with short excursions it corrects within minutes ([dsusd](dsusd.md)); test RLUSD's pool is no dollar price at all.
- The issuer's account, where a global freeze and a transfer fee live, is threaded by every trust line made or deleted, so its state cannot practically be tied to a verified transaction ([verified-ledgers](verified-ledgers.md)).

## Decision

- **The issuer is a counterparty the owner chooses to trust,** by running a scheme that settles in its token. What it takes is the owner's loss: a clawback is a withdrawal, booked once a proven transaction on the line shows it, which a server can delay ([verified-ledgers](verified-ledgers.md)), like any other ([Ledger](../areas/ledger.md#what-the-ledger-refuses-and-what-it-records)). A frozen line makes the scheme settle nothing until it thaws; peerings stay. This holds on mainnet as on the testnet.
- **Peers' bounds do not change.** A peer still costs at most the credit extended plus what is in flight; an issuer's act against a peer is that peer's loss, and shows up for us only as a settlement that failed.
- **A dollar token is booked as `USD`, at par,** on a ledger of its own. It converts one to one with other dollar ledgers and needs no rate.
- **The scheme holds the line safe from others:** rippling is disabled on our side, and only the issuer's token on the scheme's own line counts.
- **The issuer's freeze and fee are read as hints.** Our line's freeze and deep freeze are tied to verified transactions as any line's state is; the issuer's global freeze and transfer fee are read from its account unverified. Both can only stop payments, and the ledger enforces them whatever the node believes.
- **A peg guard pauses a dollar token's scheme while its pool prices it off a dollar.** The price is the pool's ratio, read as tied state, times the node's own XRP/USD rate. The scheme pauses once every reading has been outside a band around a dollar for a while, and resumes once every reading has been back inside a narrower band for longer, so neither the pool's noise nor a pool pushed for a moment trips it. Off the peg upward or downward is the same pause.
- **What a pause stops, both ways:** the scheme settles nothing; it says it cannot receive, so the node asks no peer on it to settle, withdraws but never ends a peer short of room for it, and leaves it out of its link state ([Settlement](../areas/settlement.md#when-peers-cannot-pay-us)); and the connector converts nothing to or from its ledger, as for a ledger whose rate is stale ([Connector](../areas/connector.md#inventory-pricing)), so the rate feed names it no more. What arrives on the ledger is still booked, at par: refusing to book what happened would only make the books wrong. Transit within the token's own ledger still runs, inside credit and in-flight limits, and dies down as rooms run out.
- **It fails closed.** A reading counts only when both of the pool's balances are tied to one transaction, the pool holds enough XRP to be a price, its tie is recent and the rate is fresh. A swap changes both, so one tie gives a state the pool held; two ties of different transactions could pair an old XRP balance with a newer token balance, a ratio the pool never had. A deposit or withdrawal of one asset leaves no reading until the next swap. Without a valid reading for a while, the scheme pauses as if off its peg.
- **Which pool.** Mainnet RLUSD reads mainnet's RLUSD/XRP pool. On the testnet, DSUSD reads its DSUSD/XRP pool, so the guard runs before real money does. Test RLUSD is not guarded. The pool account is found by its asset pair and checked by its `AMMID`, so no server can substitute another account, and a pool made again is found without a release.
- **The guard is the token scheme's,** without IO, beside the issuer's freezes, whose effects it shares. Its state is kept with the scheme's, so a restart neither skips a recovery nor trips again. The owner sees it, with the last price and the ledger it is as of.

## Accepted

- **A de-peg inside the band goes unseen,** and one outside it costs what converts at par before the guard trips.
- **A pause halts the token's routing** once the rooms of its peerings are used, and a node can take no conversion into or out of it until it recovers.
- **A lying server can show the guard a real pool state** up to the guard's freshness bound old, and so hold it on the wrong side, or pause it, for that long. Proving a state current needs state proofs ([verified-ledgers](verified-ledgers.md)).
- **A problem reading the pool pauses the scheme:** a busy server, a pool too thin, a stale rate.
- **Test RLUSD is unguarded:** its pool prices it nowhere near a dollar, and a guard would keep it paused for good. On the testnet nothing has a price.
- **An issuer's flags can be lied about:** a server can pause the scheme by claiming a freeze or a fee, or hide one, so that our Payments fail on the ledger, each a booked fee with backoff. The books never go wrong.
- **An issuer can take a node's holding at any time,** and the node can only record it. Spending is then refused until the owner's funds recover, as after any loss.

## Rejected

- *The token as a currency of its own, with its own rate,* or priced at its pool rate: three independent sources for it, a currency the owner sees beside the dollar, and no conversion one to one with other dollar ledgers, for an asset whose point is to be a dollar.
- *Credit to the issuer's account, booked as exposure:* the issuer is not a peer; it settles nothing and is asked nothing, so a credit line to it would bound nothing.
- *Exchange tickers for the token:* off the ledger, few venues list it, and each would be a source the node trusts alone. The pool is on the ledger the token lives on, and the node can verify it.
- *The issuer's attestations or reserve reports:* the counterparty grading itself.
- *A price oracle object on the ledger:* one publisher's word.
- *Pausing only outgoing settlements:* a token almost always de-pegs downward, and then the loser is the side that takes it at par; a paused node would keep collecting while paying nothing, and honest peers would end it for room. It also leaves conversions at par, the real loss, open.
- *Guarding test RLUSD with DSUSD's pool price as a stand-in:* it would exercise the guard on the testnet's RLUSD scheme, but pause it for DSUSD's keeper lagging, not for anything RLUSD did; DSUSD is itself a dollar token its scheme converts at par, so its own pool is the price that guards it.
- *Failing open:* the guard would be blind exactly when de-pegs happen, with runs on the issuer, drained pools and busy servers.
- *Tying the issuer's account:* it would mean walking back thousands of thread-only steps, or reading a history that lists every payment in the token; in practice mainnet RLUSD would never settle until peer-network state proofs.
