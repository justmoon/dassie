# A hub scheme's ID names its underlying ledger and its asset

**Areas:** [Settlement hubs](../areas/hubs.md#in-the-node), [Settlement](../areas/settlement.md#schemes-and-ledgers), [XRPL](../areas/xrpl.md). Both networks' are built; mainnet's run in the live realm only, with no default hub built in yet ([roadmap](../roadmap.md#mainnet-hubs)).

## Facts

- A settlement scheme's ID is its ledger's slug, and its ledger is `<scheme>+<asset>` ([Settlement](../areas/settlement.md#schemes-and-ledgers)). A node advertises and peers by scheme ID, so two nodes agree on what a hub scheme settles in only by its name.
- There is a hub scheme for each asset and underlying ledger ([the design](../designs/settlement-hubs.md#13-how-it-fits-the-node)). Hubs are to hold XRP and dollar tokens, on the XRP Ledger's testnet and mainnet ([token-hubs](token-hubs.md)).
- XRP and RLUSD exist on several ledgers, not only the XRP Ledger. A name that gives the asset and the network but not the ledger, as `hub-xrp-testnet` does, says nothing about which ledger the escrow is on.
- The schemes on the XRP Ledger are named for it, its network and their asset: `xrpl-testnet-xrp`, `xrpl-testnet-dsusd`, `xrpl-xrp`, `xrpl-rlusd` ([scheme-names](scheme-names.md)).
- The XRP hub scheme runs on the testnet only, with one hub, whose validators the project runs.
- A node is in at most one hub per asset ([hubs-only](hubs-only.md)), and has one account in each, whose key derives from the hub's ID, with no number. An XRPL account, by contrast, is numbered by its seed path, and holds several assets.

## Decision

- **A hub scheme's ID is `hub-`, its underlying ledger, and its asset:**
  - `hub-xrpl-testnet-xrp` and `hub-xrpl-testnet-dsusd` on the XRP Ledger testnet;
  - `hub-xrpl-xrp` and `hub-xrpl-rlusd` on mainnet.
- **Its ledger is `<scheme>+<asset>`,** as every scheme's: `hub-xrpl-testnet-xrp+xrp`, `hub-xrpl-testnet-dsusd+usd`, `hub-xrpl-xrp+xrp`, `hub-xrpl-rlusd+usd`. A dollar token books as `USD` ([issued-assets](issued-assets.md)).
- **The asset is always named,** even for XRP, so the hubs of one ledger read alike: a hub scheme's ID is `hub-` before the ID of the owner's book of its asset ([scheme-names](scheme-names.md)).
- **`hub-xrp-testnet` becomes `hub-xrpl-testnet-xrp`,** with its ledger.
- **The node's account in a hub is named by the scheme over the hub:** its ID as an external asset is the scheme's, with no number, `hub-xrpl-testnet-xrp` to `hub-xrpl-rlusd` ([Settlement](../areas/settlement.md#external-assets)). An XRPL account keeps `<kind>/<index>`. No kind's name holds a `/`, so the two forms never meet.

## Accepted

- **A rename on the testnet:** a node not yet upgraded advertises the old name, so its peerings over the hub with upgraded nodes drop until both run the new one.
- **Longer names,** in the books, the wallet and every link state.

## Rejected

- *Keeping `hub-xrp-testnet`, and naming the ledger only for new schemes:* one scheme would read differently from the rest, and its name would stay ambiguous.
- *The network in the hub's configuration, with one name for both:* peers would tell networks apart from each other's configs, as [mainnet-schemes](mainnet-schemes.md) rejects for the direct schemes.
- *Hub accounts named by network and a number, `hub-testnet-account/<n>` and `hub-account/<n>`:* the number meant the asset, by a table kept apart from the schemes, and named nothing a node has several of; the kind left out the ledger the scheme's ID names.
