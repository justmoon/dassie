# An issued asset's issuer is the owner's counterparty

**Areas:** [External assets](../designs/external-assets.md), [Settlement](../areas/settlement.md), [Connector](../areas/connector.md). A design, not built yet ([roadmap](../roadmap.md#external-assets-and-rlusd)).

## Facts

- A token on the XRP Ledger, such as RLUSD, is backed by its issuer. RLUSD's issuer can freeze a holder's line, which stops the holder paying anyone but the issuer, and deeper, receiving too; and it can claw back a holder's balance, with a transaction that is no Payment.
- What a peer can cost a node is bounded by the credit it extends plus what is in flight ([own-books](own-books.md)). Nothing bounds what an issuer can take: the whole holding.
- Rates are the median of three sources ([exchange-rates](exchange-rates.md)); the sources a node has quote fiat currencies and XRP. A dollar token priced as `USD` needs no new source, and converts one to one with every other dollar ledger.
- Test RLUSD is handed out by hand, in small amounts, and has no market value.

## Decision

- **The issuer is a counterparty the owner chooses to trust,** by running a scheme that settles in its token. What it takes is the owner's loss: a clawback is a withdrawal, booked when the ledger shows it, like any other ([Ledger](../areas/ledger.md#what-the-ledger-refuses-and-what-it-records)). A frozen line makes the scheme settle nothing until it thaws; peerings stay.
- **Peers' bounds do not change.** A peer still costs at most the credit extended plus what is in flight; an issuer's act against a peer is that peer's loss, and shows up for us only as a settlement that failed.
- **A dollar token is booked as `USD`, at par,** on a ledger of its own. It converts one to one with other dollar ledgers and needs no rate.
- **The scheme holds the line safe from others:** rippling is disabled on our side, and only the issuer's token on the scheme's own line counts.

## Accepted

- **A de-peg goes unseen.** Priced at par, a token worth less than a dollar converts at a dollar. On the testnet nothing has a price; real money needs this decided again.
- **An issuer can take a node's holding at any time,** and the node can only record it. Spending is then refused until the owner's funds recover, as after any loss.

## Rejected

- *The token as a currency of its own, with its own rate:* three independent sources for it, a currency the owner sees beside the dollar, and no conversion one to one with other dollar ledgers, for an asset whose point is to be a dollar.
- *Credit to the issuer's account, booked as exposure:* the issuer is not a peer; it settles nothing and is asked nothing, so a credit line to it would bound nothing.
