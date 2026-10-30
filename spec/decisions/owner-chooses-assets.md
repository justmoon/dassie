# The owner chooses assets, the node adds their schemes, and the balance counts everything the owner has

**Areas:** [Wallet](../areas/wallet.md#pages), [Settlement hubs](../areas/hubs.md#in-the-node), [Settlement](../areas/settlement.md#schemes-and-ledgers), [RPC](../areas/rpc.md).

## Facts

- Peers settle only over hubs ([hubs-only](hubs-only.md)). A scheme on an underlying ledger is an owner book, which peers with nobody; a hub's scheme moves funds only while its owner book runs, and a token hub's only while the XRP book of its network runs too, which pays its escrow's gas ([Settlement hubs](../areas/hubs.md#in-the-node)).
- The wallet's first screen listed every scheme the node offered as equals: two owner books and two hubs, each with a paragraph on how it settles. A hub picked alone was added without its books, and never got funds; only `dassie setup` added the books first.
- A token's account pays its fees and its line's reserve in its ledger's own asset: an owner of DSUSD holds XRP.
- The owner's balance in an asset counted what the books hold of it on the owner book and in the hub, our holds toward peers included, since funding a hold moves money between accounts outside the owner's group ([Ledger](../areas/ledger.md#limits-and-groups)). The books never count the reserves the ledgers hold back: the XRP Ledger account's, the hub account's and each hold's. They are the owner's, and come back when the account or the hold closes.
- Three pages showed the same money: Ledgers, a ledger at a time with the part of the balance on each; External assets, what the XRP Ledger reports of the account; and Hubs, the hub account's state and the node's moves.

## Decision

- **The owner chooses assets, never schemes.** An asset is, on the XRP Ledger, the owner book of the owner's own account and the hub over it, and in a development build the stub's dollars (`ASSETS` in `engine/core/schemes.rs`). The node adds an asset's schemes itself, the book before the hub, and lists what it offers (`ListAssets`, `AddAssets`): an asset whose every scheme it offers, so none over a hub while no default hub of it is built in.
- **A token brings the asset its fees are paid in, as a full asset.** Choosing DSUSD adds XRP first, its hub included. Every XRP the node holds is a balance the owner sees, and the node can pay and be paid in it.
- **An asset goes hub first.** An owner book is not removed while a hub funds from it, as its asset's book or as the book of its escrow's gas (`RemovalBlocker::HubFunds`): the XRP book stays while a DSUSD hub needs it.
- **The balance counts everything the owner has:** the books of every ledger of the asset, a move between them on its way, and the reserves the ledgers hold back. Holds toward peers are already in the books, and are not added again. The balance is three parts the node reports with it: what is in the owner's own account, what is in the hub, and what is on its way, with the reserves and what our holds still hold named within them. What a payment may spend now is reported beside it, as before.
- **The setup screen is a checklist of assets,** XRP ticked to start with, with one Continue. Ticking a token ticks and locks the asset it needs.
- **One Assets page, and a page per asset.** Ledgers and External assets become one Assets page, which lists each asset with its balance and what is spendable, and links to the asset's own page: its balance with its parts, Send and Receive in it, its deposit address, what the XRP Ledger holds of the account, its hub, and its ledgers, each with its removal. The dashboard's balance card names what is spendable beside each asset and links to its page. Hubs stays, for the hub's state and the owner's moves.

## Accepted

- **The balance is more than a payment can spend:** by the reserves, and by what unfinished payments and the node's held-back moves take. The card says what is spendable beside each asset, and Send says what it can pay now.
- **The account's part matches what an explorer shows for the account;** the hub's part is no number the hub shows, since the hub reports its spendable balance, its reserves and each hold apart.
- **An owner of DSUSD joins the XRP hub too,** and pays its account reserve once the node is paid in or deposits.
- **Adding an asset is not all or none.** What was added stays if a later scheme of it fails, and adding the asset again adds the rest.

## Rejected

- *One card per asset, each added on its own:* names what the owner holds as the checklist does, but a second asset takes a second visit.
- *No choice, every asset of the network added at setup:* the fewest steps, but a token's line costs XRP of reserve whether the owner uses it or not, real XRP on mainnet, and a network that offers no hub would leave setup with nothing to add.
- *The scheme list with the owner books taken out:* hardly a change, but it teaches what a settlement hub is before the owner needs it.
- *A token's XRP only as its fees, shown under the token:* no XRP hub the owner did not ask for, but XRP the owner has would be a footnote of another asset, and the wallet would need a second kind of asset with no balance.
- *The balance without the reserves:* closer to what a payment can spend, but the account's part would not match the ledger, and the reserve the hub took at joining would seem to vanish.
- *The breakdown folded into the balance card, or in a card of its own on the dashboard:* one press from the number, or none, but detail most owners do not need on the page they open first, where the asset's own page has room for its deposit, its hub and its ledgers too.
- *Keeping Ledgers, External assets and Hubs as they were:* nothing to build, but Ledgers would still show two balances for one asset.
- *One page per asset that holds the hub's controls too:* the shortest navigation, but a long page, and the most to build.
