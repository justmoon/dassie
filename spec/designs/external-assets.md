# External assets

A settlement scheme today is also the node's wallet on its ledger: the `xrpl-testnet` scheme derives its account, follows it, signs for it and books what it holds ([XRPL](../areas/xrpl.md)). That works while a scheme settles in the one asset its account holds. It stops working with the first scheme that needs more than one: a scheme that settles in RLUSD, Ripple's USD token on the XRP Ledger, pays its fees and its reserve in XRP.

This design separates the two. An **external asset** is something the node holds outside its books, such as an XRPL account; a **settlement scheme** settles with peers through the external assets it requires. Its first use is RLUSD on the XRPL testnet, as the scheme `xrpl-testnet-rlusd`.

This is a design ([external-assets](../decisions/external-assets.md), [issued-assets](../decisions/issued-assets.md)); nothing in it is built yet ([roadmap](../roadmap.md#external-assets-and-rlusd)).

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

An external asset owns everything about its account that more than one scheme would otherwise race for:

- **Its keys.** It derives them from the node's seed. The node's first XRPL testnet account is the one it has today, at `node/settlement-module/xrpl-testnet/seed-testnet`, which is frozen ([Identity](../areas/identity.md#derivation-chain)). Further accounts of a type get new paths below `node` when they are built; no path gets a new meaning.
- **Its connection to its ledger:** the network check, the newest ledger seen, resets, the faucet ([XRPL](../areas/xrpl.md#following-the-account), [ledger-resets](../decisions/ledger-resets.md)).
- **What it signs:** the account's next sequence and its transactions in flight. Schemes ask it to sign and submit; it hands out sequences in order, so two schemes on one account never take the same one.
- **What it holds:** each holding as the ledger says, read from the ledger, never from the books.

It stays sans-IO, like the scheme today: it says what to send its ledger, and the node feeds it what comes back over the one connection it keeps for it ([Settlement](../areas/settlement.md#schemes-that-follow-a-ledger)).

**Its state is written with what it explains,** as a scheme's is today ([Settlement](../areas/settlement.md#checkpoints-and-activation)): the sequence and the transaction a scheme asked it to sign go to disk in the batch with that settlement's pending transfer, so nothing it signed is lost to a crash. A transaction it signed but never submitted, because the settlement was never executed, is dropped past its last ledger as today, and its sequence goes to the next transaction, whichever scheme asked. The account remembers it as never sent until its scheme has reconciled it, so a scheme that stopped and starts again cancels that settlement, rather than looking for a hash no ledger will ever hold and leaving it pending for good ([XRPL](../areas/xrpl.md#state-and-restarts)).

**Several schemes may share one external asset.** By default `xrpl-testnet-rlusd` uses the node's first XRPL testnet account, the one `xrpl-testnet` uses: one address for the owner to fund, and one supply of XRP for both. Each validated transaction is handed to every scheme on the account; each scheme keeps its own cursor into them, written with what it books, so a scheme that stopped ([Settlement](../areas/settlement.md#the-scheme-interface)) reads again what it missed, and the other is not held back.

## 3. Accounting

**The ledger is authoritative for what an external asset holds.** The wallet shows the holdings as the ledger reports them, through the node, never as the books add them up.

**The books keep an account to book against.** On each internal ledger, `assets/settlement` stands for the holding the ledger's scheme settles in. It is not a second record of the holding: it is kept in step with it, as the XRPL scheme keeps it today, by booking every change of the holding (settlements, deposits, withdrawals, fees) and reconciling any remainder through `equity/suspense` ([Settlement](../areas/settlement.md#checkpoints-and-activation)). It has to stay in step: the node decides from its own books ([own-books](../decisions/own-books.md)), and the owner's spending limit and the check before each settlement both read it. An account that drifted from the holding would let the owner spend money already gone, withdrawn outside the node, clawed back or spent as gas, and the node would owe its peers more than it can settle.

**A holding is on the books only on the ledger whose scheme settles in it.**

- The XRP of an account `xrpl-testnet` settles from is on the books of `xrpl-testnet+xrp`. Every change of it is booked there, whichever scheme caused it: the fee of an RLUSD settlement from that account is a ledger fee on `xrpl-testnet+xrp`, and the reserve of its trust line is reconciled there as any reserve change is.
- A holding no scheme settles in is gas, and is off the books. On a node that runs only `xrpl-testnet-rlusd`, the account's XRP is not booked anywhere: its fees and reserve leave no entry, and the wallet shows what is left from the ledger.

**Gas off the books holds for real money too,** on two conditions, both in the scheme: it settles nothing (`Unavailable`) while the gas cannot pay the fee above the reserve, and the owner sees the gas, from the ledger. What it leaves out is reporting: the owner's revenue and expenses show no fees paid in gas. A report can read them from the ledger.

**On a shared account the gas is booked XRP.** There the RLUSD scheme's fees and its line's reserve come out of what `xrpl-testnet+xrp` holds for the owner, and XRP the books owe to that ledger's peers is not the owner's to spend. So on a shared account the RLUSD scheme settles only while the owner's funds on `xrpl-testnet+xrp` cover the fee, and signs its `TrustSet` only while they cover the line's reserve; what it spends is booked there as a ledger fee and a reconciliation, like any other.

## 4. RLUSD on the XRPL testnet

RLUSD is a token on trust lines, issued by one account per network; its issuer and currency code are constants of the scheme (`dassie-xrpl`). The issuer requires no authorization to hold it, charges no transfer fee, and keeps the right to freeze and to claw back.

**The ledger** is `xrpl-testnet-rlusd+usd`. Its slug differs from `xrpl-testnet`'s, since a ledger's slug names its receiver, its seed path and its wallet address ([Payments](../areas/payments.md#wallet-addresses)). Its asset is `USD`, at par ([issued-assets](../decisions/issued-assets.md)), at scale 9 like every ledger, and its unit is a millionth of a dollar: the token's amounts are decimal, with about fifteen significant digits, so a millionth stays exact for any balance a node could hold.

**The trust line.** Before the scheme counts as synced, the account has a line to the issuer, with rippling disabled on our side (`tfSetNoRipple`), so no one can move our RLUSD through our account. If the line is missing, the scheme signs a `TrustSet` once the account holds XRP for it, and peers only once the ledger shows the line ([synced-before-peering](../decisions/synced-before-peering.md)). A peer's settlement into a missing line would fail, so no peer pays before it exists.

- **Its limit** is the largest amount the ledger can express, so no settlement fails for reaching it.
- **A line made outside the node,** with rippling enabled on our side or a lower limit, is corrected: the scheme signs a `TrustSet` that sets NoRipple and the limit, and syncs only once the ledger shows them.

**Settling.** A settlement is a `Payment` of RLUSD of the issuer, without paths and never a partial payment, with one memo of the scheme's own memo type naming our node ID. Its ID and message are the transaction's hash, as for XRP ([XRPL](../areas/xrpl.md#the-payment-it-signs)). `prepare_settlement` is `Unavailable` also while our line is frozen, while the issuer charges a transfer fee (not provided for yet), or while the gas cannot pay the fee.

**Booking a validated transaction** by what it did to the line, read from the transaction's metadata:

- **One of ours:** posted when it succeeded, voided when it failed. Its XRP fee is booked by §3.
- **A settlement from a peer:** a successful Payment to us, whose delivered amount is RLUSD of our issuer, whose memo has our memo type and names a peer, from the address that peer gave when peering. It is booked by the delivered amount, never the stated one.
- **Anything else that raises the line** is a deposit; **anything that lowers it** a withdrawal. The issuer's clawback, which lowers the line without a Payment, is a withdrawal.
- **Other currencies, other issuers and XRP** move nothing on this ledger.

After each, the books hold the line's balance in whole units; a remainder below a unit, which anyone can send, stays in the holding and is reconciled once it adds up.

**Freezes.** The scheme reads them from the ledger: our line's flags, and the issuer's account for a global freeze and a transfer fee. It reports each in the wallet.

- **A frozen line** (ours, by the issuer) stops our settlements: `prepare_settlement` is `Unavailable`. Peerings stay; a peer that asks us to settle gets nothing, and stops advertising us once its room is gone ([Network](../areas/network.md#advertised-peers)).
- **A line that cannot receive** (ours deep-frozen, or the issuer's global freeze, which stops every holder's payments but to the issuer) also stops what peers pay us, and a peer's settlement would only fail and cost it a fee. While it lasts the node treats the scheme as it treats one that does not see its ledger: it asks no peer on it to settle, a peer's time short does not grow, so no peer is withdrawn or ended for room for what it could not pay ([Network](../areas/network.md#advertised-peers), [ended-for-room](../decisions/ended-for-room.md)), and its link state leaves the scheme out, so no new peering is made on it.

**Testnet resets** take the line, its balance and the issuer's account with them. The scheme follows the account's reset as today, and makes the line again once the issuer exists. An issuer that does not come back at the same address leaves the scheme unsynced until a release names the new one.

**Funding.** The owner sends the account XRP for gas, from the testnet faucet as for `xrpl-testnet`, and RLUSD, which Ripple hands out only by hand and in small amounts. Dev, end-to-end tests and simulations use an issuer of the fake ledger, which mints, freezes and claws back on request ([XRPL](../areas/xrpl.md#the-fake-ledger)).

## 5. The wallet

- **External assets** have a page of their own: each one's address, its holdings as the ledger reports them, the reserve, and a frozen line. It is where the owner funds them.
- **Ledgers** show what the books hold, the owner's funds to spend, as today.
- **Receive** for `xrpl-testnet-rlusd+usd` names both what the account needs: XRP for gas, from the faucet, and RLUSD, sent to the same address.

## 6. Migration

The `xrpl-testnet` scheme becomes the node's first XRPL testnet account and a scheme on it. Its state is split between them: the sequence, the transactions in flight, the newest ledger seen and the faucet go to the account; the cursor, the peers' addresses and the settlements being looked up stay with the scheme. The account and its derivation are unchanged, the checkpoint keeps the balance it books, and settlements still pending are reconciled by their hash as before. A node that restarts on the new release books nothing anew.

## 7. Open points

- **One scheme per pair of nodes.** A peering is on one scheme ([Network](../areas/network.md#peering)), so two nodes that run both XRPL schemes settle in one of them.
- **Per-ledger credit and largest packet.** Both are one value for every ledger ([roadmap](../roadmap.md#not-built-yet)), so the same thresholds count for more in XRP than in dollars.
- **Periphery nodes on RLUSD.** A token payment cannot create an account or a trust line, so the free receivers of the periphery ([designs/periphery.md](periphery.md)) need XRP first.
- **Bootstrap nodes** run only `xrpl-testnet` ([bootstrap-network](../decisions/bootstrap-network.md)). An RLUSD node peers only with other nodes that run the scheme.
- **Issuer transfer fees.** Paying with a transfer fee needs `SendMax` and an amount booked above what the peer receives; until then a fee makes the scheme `Unavailable`.
- **A mainnet scheme** needs what [real-money](../decisions/real-money.md) asks, its own memo type, and the issuer's conditions of [issued-assets](../decisions/issued-assets.md).
