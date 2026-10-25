# A downlink's accounts open once value first moves with it, in room kept for it while its channel is open

**Areas:** [Network](../areas/network.md#downlinks), [Settlement](../areas/settlement.md#settlement-channels), [Connector](../areas/connector.md#booking-a-packet), [Ledger](../areas/ledger.md#chart-of-accounts).

## Facts

- A key costs nothing, and a downlink proves only that it holds one ([Network](../areas/network.md#downlinks)). Accounts stay for good once opened ([closing-channels](closing-channels.md)), and downlinks' accounts take a share of the ledger's of their own ([account-budgets](account-budgets.md)). Opened when a downlink's peering is accepted, they let one machine fill that share with keys that never move anything, after which the node takes no downlink with a new key.
- A downlink that never moves value needs no accounts: nothing is owed either way, and the senior extends it no credit, so it can send no money until it settles in advance.
- The connector books a packet's pending transfers on its endpoints' accounts when the Prepare arrives, and posts them on the Fulfill ([Connector](../areas/connector.md#booking-a-packet)). A settlement books on the peer's account when the books take it ([Settlement](../areas/settlement.md#incoming-settlements)); over a hub, our settlement to a peer funds our hold toward it, booked on the peer's account too ([Settlement](../areas/settlement.md#settlement-hubs)).
- A downlink pays a node only when asked, and is asked at once, since the senior extends it no credit ([Settlement](../areas/settlement.md#asking-a-peer-to-settle)). Its settlement can land at any time while its channel is open.

## Decision

- **A downlink's accounts open when value first moves with it,** never when its peering is accepted: with the first Prepare to it that moves money, its settlement that the books take, an entry payment's included, booked as it peers ([pay-to-enter](pay-to-enter.md)), or, over a hub, our settlement to it when one is made. A core peer's accounts still open when its peering is made.
- **The books first, as for any peer.** Its settlement by message is checked against the accounts as they would open, with the credit they would open with, before the scheme sees it (`Ledger::validate_to_peer`), so what the books could not take is refused as any peer's is, and opens nothing ([Settlement](../areas/settlement.md#incoming-settlements)).
- **Until then it is a downlink with nothing on its accounts.** It has no credit, so a Prepare from it that carries money is refused as for want of liquidity (T04), and opens nothing. It owes nothing and is owed nothing: never short of room, never withdrawn or ended for room, asked to settle at once as such a downlink is, and given nothing when it asks us to settle, but over a hub. Its endpoint is the connector's like any peer's, without an account until they open (`OpensAccounts` in `dassie-connector`).
- **With the packet's transfers, or not at all.** Opened for a Prepare whose transfers the books then refuse, they are taken back before anything else happens (`Ledger::unopen_peer`), so a refused packet leaves nothing. Opening them on the packet path writes two accounts to the journal, taken into the batch first, and holds no pending transfer: the ledger's pending room (`check_bounds`) is the packets' as before.
- **The room is kept from acceptance.** The downlinks' share keeps room for every downlink we hold an open settlement channel with that has no accounts yet (`Downlinks` in `crates/dassie-node/src/engine/core/downlinks.rs`). A downlink is accepted only if its accounts fit beside those opened and kept, so value that moves with it opens them, and its settlement is never lost for want of room. What is kept frees when the channel closes, a day after the peering ended: a key that never moved value leaves nothing behind.
- **Its channel closes only while every scheme sees its ledger.** A peer's channel closes only while the schemes of its accounts see their ledgers, so a settlement it made before the close is booked as its own ([closing-channels](closing-channels.md)). A downlink without accounts may have settled on any scheme the node runs, so every one of them, stopped ones too, must see its ledger before its channel closes; until then its room stays kept.
- **Without room when value moves,** a Prepare to the downlink is refused as for want of liquidity (T04): room comes back as channels close. A settlement that arrived is booked, as one from a node without accounts: the owner's deposit. Either happens only to a downlink that had no room kept, below.
- **What is kept is never written:** it is the open channels and the ledger, read again at start.
- **Once peers are in memory only while needed** ([resident-peers](resident-peers.md), not built yet), a downlink that never moved value has no accounts on disk either, nor any to read in when it connects.

## Accepted

- **A Prepare the downlink then rejects still opens its accounts.** What opens them is the transfer booked at the Prepare, not the Fulfill. Filling the downlinks' share so costs a sender a path with liquidity to the node for each key, and a day for the downlinks' places, but no fees. Making it cost fees would mean opening them on the Fulfill, below.
- **Over a hub, a downlink that asks us to settle opens its accounts,** since we fund our hold toward it, as a payer over a hub does for any peer ([Settlement hubs](../areas/hubs.md#in-the-node)), and a payment to it needs that hold. The money that moves is ours, and stays ours until the downlink claims what we paid it, but the downlink pays nothing to open them.
- **A downlink with accounts on another ledger has no room kept** for those on the ledger it peers on now, since what is kept is counted per node. It is accepted only while they fit, and value that moves with it later finds room unless the share filled meanwhile.
- **A downlink's starting credit** (the senior's, nothing by default) is extended when its accounts open, not when its peering is made.
- **One scheme that is blind or stopped keeps every downlink without accounts open,** and its room kept, until it sees its ledger again: the cost of not knowing which scheme such a downlink settled on. A restart clears a stopped scheme, but one still blind after it keeps holding them, the channels' wait counted from the start again. Its places and room are bounded as the downlinks' are.
- **After a restart, a downlink that turned core while its peering was ending** stands as a core peer without accounts, and gets no endpoint for the rest of that peering. Its peering is ending, so nothing is lost but packets it could not have been sent anyway.

## Rejected

- *Accounts at acceptance, as for core peers:* every key leaves two accounts for good, so the share fills for nothing ([account-budgets](account-budgets.md)).
- *Opening them on the Fulfill:* the Prepare's transfer would have to be booked to an account of the node's and moved to the downlink's when fulfilled, for every packet's booking, its expiry and its trace, and the room for them kept for every packet in flight. Much more change for one rejected packet per key.
- *Refusing a payment to a downlink until it settled once:* a new periphery node could not be paid before it paid, and paying first is what an attacker does anyway.
- *No room kept, only a check when value moves:* a downlink asked to settle could pay into a share filled in the meantime, and lose its payment to the owner's funds.
- *Room kept as a worst case of every place for a downlink:* simpler, but the share would take new downlinks only until its accounts and the downlinks' places together filled it, nearly halving how many downlinks a node ever takes.
