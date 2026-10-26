# A scheme's ID names its ledger, network and asset, and a wallet address names the asset alone

**Areas:** [Settlement](../areas/settlement.md#schemes-and-ledgers), [XRPL](../areas/xrpl.md#the-payment-it-signs), [Ledger](../areas/ledger.md#ledgers-and-amounts), [Payments](../areas/payments.md#wallet-addresses), [Identity](../areas/identity.md#seed-paths).

## Facts

- A settlement scheme's ID is its ledger's slug, and its ledger is `<scheme>+<asset>`. A node advertises and peers by scheme ID, so two nodes agree on what a scheme settles in only by its name.
- The token schemes' IDs named their ledger, their network and their asset (`xrpl-testnet-dsusd`, `xrpl-rlusd`), and the hubs' do too (`hub-xrpl-testnet-xrp`, [hub-scheme-names](hub-scheme-names.md)), but the XRP schemes' left their asset out (`xrpl-testnet`, `xrpl`), and the memo types were in an order of their own (`xrp-testnet`, `dsusd-testnet`, `rlusd`).
- Peers settle only over hubs ([hubs-only](hubs-only.md)): the schemes on the underlying ledgers become the owner's books of each asset there, and a hub scheme sits on the owner's book of its asset.
- A ledger's slug named three things at once: its wallet address (`/pay/<slug>`), which people read, type and save; its receiver's and payer's ILP addresses (`stream-<slug>`, `pay-<slug>`); and its receiver's seed path. So a wallet address named plumbing, and renaming a ledger moved all three.
- No network depends on today's names: the testnet is reset before the first release.

## Decision

- **One grammar for scheme IDs:** `<ledger>-<network>-<asset>`, the network left out on mainnet, and `hub-` before it for a settlement hub's scheme. The XRP schemes become `xrpl-testnet-xrp` and `xrpl-xrp`, with their ledgers, `xrpl-testnet-xrp+xrp` and `xrpl-xrp+xrp`, so a hub scheme's ID is `hub-` before the ID of the owner's book it sits on.
- **The memo type is the scheme's ID:** `https://dassie.land/settlement/<scheme ID>/v1`, for every scheme on the XRP Ledger. A token scheme whose memo type changed speaks version 2 of its scheme, so a peer of version 1 is refused rather than read wrong; the XRP schemes, under new IDs, start at version 1.
- **A wallet address names its ledger for people,** by its asset: `/pay/xrp`, `/pay/dsusd`, `/pay/rlusd`, `/pay/usd` for the stub, and `/pay/hub-xrp` for a hub's, unique within a realm, which is all a node runs (`LedgerId::wallet_name`). The slug stays plumbing: the receiver's and payer's addresses, and the receiver's seed path until the identity work keys it by something stable.
- **Renamed by a migration** (`store/schema.rs`), as [hub-scheme-names](hub-scheme-names.md)'s rename was: the schemes, their books and peerings, and every stored wallet address, with the grants and payments that name them.

## Accepted

- **Credentials the receivers of the renamed ledgers issued before no longer pay,** since their address and secret follow the slug; a payment sending starts a new connection after the restart the upgrade needs anyway. The receiver secret guards no funds.
- **A payer who saved an old wallet address no longer reaches it.**
- **Mixed versions on the testnet:** a node not yet upgraded and one upgraded share no XRP scheme, and refuse each other on the token schemes, until both run the new names. The testnet's reset makes this short.
- **Longer XRP scheme names,** in the books, the wallet and every link state.

## Rejected

- *Keeping `xrpl-testnet` and `xrpl` as the one exception:* every hub scheme and every memo type would read differently from the scheme beneath it, and the asset of the most common scheme would be implicit.
- *The asset code alone as the wallet name, hubs included:* a node runs a hub's XRP ledger beside the owner's book of XRP, and two ledgers would share `/pay/xrp`.
- *The scheme ID as the wallet name, as before:* wallet addresses would keep naming plumbing, and move whenever a scheme is renamed.
- *Keying the receiver's secret by the wallet name:* it would move whenever a ledger is renamed for people; the secret is to be keyed by something stable instead.
