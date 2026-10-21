# External assets

A settlement scheme that is also the node's wallet on its ledger works while it settles in the one asset its account holds. It stops working with the first scheme that needs more than one: a scheme that settles in RLUSD, Ripple's USD token on the XRP Ledger, pays its fees and its reserve in XRP.

This design separates the two. An **external asset** is something the node holds outside its books, such as an XRPL account; a **settlement scheme** settles with peers through the external assets it requires. Its first use is RLUSD on the XRPL testnet, as the scheme `xrpl-testnet-rlusd`.

This is a design ([external-assets](../decisions/external-assets.md), [issued-assets](../decisions/issued-assets.md)). Built: its wire format, the codec and reading the ledger for tokens ([XRPL](../areas/xrpl.md#tokens)); the fake ledger's tokens and issuer ([XRPL](../areas/xrpl.md#tokens-in-the-fake-ledger)); the XRPL account as an external asset, with `xrpl-testnet` a scheme on it ([§2](#2-the-external-asset), [§6](#6-migration)), described in [Settlement](../areas/settlement.md#external-assets) and [XRPL](../areas/xrpl.md#the-account); and the `xrpl-testnet-rlusd` scheme on the same account, with gas on or off the books ([§3](#3-accounting), [§4](#4-rlusd-on-the-xrpl-testnet)), described in [XRPL](../areas/xrpl.md#rlusd) and [Settlement](../areas/settlement.md#when-peers-cannot-pay-us). The rest is not built yet ([roadmap](../roadmap.md#external-assets-and-rlusd)).

## 1. Concepts

| Term | Meaning |
|---|---|
| **External asset** | Something the node holds outside its books, of a type: an XRPL account, later a set of Bitcoin outputs or a bank account. The node may hold none or several of each type. |
| **Holding** | What an external asset holds of one asset: an XRPL account's XRP, or its balance on one trust line, such as RLUSD of Ripple's issuer. |
| **Settlement scheme** | Settles with peers in one holding of one external asset, and books on one internal ledger. It names the types and holdings it requires. |
| **Gas** | A holding a scheme spends without settling in it: the XRP an RLUSD scheme pays fees and holds its reserve with. |
| **Issuer** | The account that issues a token on the XRP Ledger and backs it. It can freeze a holder's line or claw back its balance ([issued-assets](../decisions/issued-assets.md)). |

**What each scheme requires.**

- `xrpl-testnet` requires an XRPL testnet account, and settles in its XRP.
- `xrpl-testnet-rlusd` requires an XRPL testnet account with a trust line to RLUSD's issuer, settles in RLUSD and spends XRP as gas.

## 2. The external asset

Built: the asset owns its keys, its connection, what it signs and what it holds; each scheme on it keeps its own cursor; its state is written with what it explains; and a transaction it signed but never sent is remembered until its scheme reconciles it, which cancels it ([XRPL](../areas/xrpl.md#the-account), [XRPL](../areas/xrpl.md#state-and-restarts), [Settlement](../areas/settlement.md#external-assets)).

**`xrpl-testnet-rlusd` shares the node's first XRPL testnet account** with `xrpl-testnet` by default: one address for the owner to fund, and one supply of XRP for both.

Sharing has one cost. Until a stopped scheme's transaction that was never sent is dropped, the other scheme's transactions with later sequences cannot apply: they are looked up and cancelled past their last ledger, and that scheme backs off settling, as after any cancelled settlement ([Settlement](../areas/settlement.md#outgoing-settlements)). It costs one window of ledgers and a backoff, and nothing is paid twice.

## 3. Accounting

Built ([Settlement](../areas/settlement.md#external-assets), [XRPL](../areas/xrpl.md#the-account)): the ledger is authoritative for what an external asset holds, and the books keep `assets/settlement` only to book against, in step with the holding the ledger's scheme settles in. A holding is on the books only on the ledger whose scheme settles in it. On an account `xrpl-testnet` settles from, the RLUSD scheme's fees are ledger fees on `xrpl-testnet+xrp` and its line's reserve is reconciled there, and the account signs for it only while the owner's funds there cover the gas, and not at all while the books stopped `xrpl-testnet`; on any other, the gas is off the books, and only the reserve bounds it, which holds for real money too ([external-assets](../decisions/external-assets.md)).

## 4. RLUSD on the XRPL testnet

Built: the scheme `xrpl-testnet-rlusd` and its ledger `xrpl-testnet-rlusd+usd`, with the line set before it syncs, settling, booking by the line, and the issuer's freezes and clawbacks ([XRPL](../areas/xrpl.md#rlusd)); and what the node does while peers cannot pay us ([Settlement](../areas/settlement.md#when-peers-cannot-pay-us)).

## 5. The wallet

- **External assets** have a page of their own: each one's address, its holdings as the ledger reports them, the reserve, and a frozen line. It is where the owner funds them.
- **Ledgers** show what the books hold, the owner's funds to spend, as today.
- **Receive** for `xrpl-testnet-rlusd+usd` names both what the account needs: XRP for gas, from the faucet, and RLUSD, sent to the same address.

## 6. Migration

Built: the `xrpl-testnet` scheme's state is split between the account and the scheme by a database migration, and a node that restarts on it books nothing anew ([XRPL](../areas/xrpl.md#state-and-restarts)).

## 7. Open points

- **One scheme per pair of nodes.** A peering is on one scheme ([Network](../areas/network.md#peering)), so two nodes that run both XRPL schemes settle in one of them.
- **Per-ledger credit and largest packet.** Both are one value for every ledger ([roadmap](../roadmap.md#not-built-yet)), so the same thresholds count for more in XRP than in dollars.
- **Periphery nodes on RLUSD.** A token payment cannot create an account or a trust line, so the free receivers of the periphery ([designs/periphery.md](periphery.md)) need XRP first.
- **Bootstrap nodes** run only `xrpl-testnet` ([bootstrap-network](../decisions/bootstrap-network.md)). An RLUSD node peers only with other nodes that run the scheme.
- **Issuer transfer fees.** Paying with a transfer fee needs `SendMax` and an amount booked above what the peer receives; until then a fee makes the scheme `Unavailable`.
- **A mainnet scheme** needs what [real-money](../decisions/real-money.md) asks, its own memo type, and the issuer's conditions of [issued-assets](../decisions/issued-assets.md).
