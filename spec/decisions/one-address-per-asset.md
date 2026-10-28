# One wallet address per asset, booking where the asset arrives, and where the owner keeps it is a setting of the node's

**Areas:** [Payments](../areas/payments.md#wallet-addresses), [Settlement hubs](../areas/hubs.md#in-the-node), [Wallet](../areas/wallet.md#pages), [Ledger](../areas/ledger.md).

## Facts

- Peers settle only over settlement hubs ([hubs-only](hubs-only.md)), so everything a node receives in an asset over Interledger arrives on the ledger of its hub of that asset. The schemes on the underlying ledgers are the owner's books, which peer with nobody.
- A wallet address had one per ledger: `/pay/xrp` for the owner book of XRP on the XRP Ledger, `/pay/hub-xrp` for the hub's. A payment to `/pay/xrp`, and to `$name` while the owner book was the default, arrived on the hub's ledger and was converted into the owner book inside the node.
- Payers save wallet addresses, and grants and resources record them. A URL that names where the owner keeps money ties every payer and grant to that choice: changing it would move them, or strand them on an address that books somewhere the owner no longer wants.
- The hub's funding already keeps the account between a floor and a ceiling: it deposits from the owner book below the floor, and withdraws above the ceiling, each time back to the midpoint ([Settlement hubs](../areas/hubs.md#in-the-node)).
- A deposit to a hub's escrow with an account's tag is credited to that account whoever sends it, and the member's scheme books what its counters show as the owner's deposit. Only a deposit with tag 0 needs the memo that names a new account's key, which wallets outside Dassie rarely write.

## Decision

- **One wallet address per asset,** `/pay/<asset>`, the asset's name for people (`LedgerId::wallet_name`), which a hub's ledger shares with the owner book of its asset. No address names a hub.
- **It books on the ledger the asset arrives on:** the hub's where the node runs one, the asset's own ledger otherwise, such as the stub's in debug builds. Nothing is converted on the way in.
- **`$name` names an asset,** the owner's chosen default or the first active one, never a ledger.
- **Where received money is kept is the owner's setting per asset:** in the hub, the default, or on the owner's own account on the underlying ledger. On the own account, the node's existing funding withdraws what is beyond the ceiling, back to the midpoint; in the hub, it withdraws nothing by itself. The owner sets it over RPC, in the wallet, or for every asset with `dassie setup --keep-funds`; the database keeps it per asset, for a hub joined later too.
- **The deposit address follows the setting.** Kept in the hub, Receive shows the hub's escrow with the node's account's number as the destination tag, once the account has one; before that, or kept on the own account, the owner's account on the underlying ledger.

## Accepted

- **Payers and grants of `/pay/hub-xrp` and the other hub addresses no longer reach the node,** and a default chosen as a ledger becomes its asset's address. The testnet runs no payments that rely on them before the launch.
- **By default the owner trusts the hub's validators with everything the node receives,** not only what settling needs. The owner who would rather not keeps it on the own account, which costs a withdrawal's fees each time the hub passes the ceiling.
- **A deposit with the wrong tag reaches another member.** The escrow refuses one without a tag, but not one with another member's number, which the wallet guards against only by saying so.
- **The owner book of an asset with a hub has no wallet address,** so money is asked for there only by depositing on its ledger.

## Rejected

- *One address per ledger, as before:* the owner's choice of where to keep money would be in every URL a payer or a grant keeps, and the address most people used, the owner book's, would book through a conversion the node does only because of the URL.
- *The asset's address booking on the owner book, converting what arrives:* every payment would cross from the hub's ledger to the book inside the node, a conversion that is no trade and that moves nothing on any ledger, while the money stays in the hub all the same until the funding withdraws it.
- *A node setting in place of the asset's:* nodes hold several assets, and an owner may keep a token on the own account and XRP in the hub.
- *Turning the node's own moves off to keep money in the hub:* the floor would no longer be kept, and a hub account that runs short stops settling.
