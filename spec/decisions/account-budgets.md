# Core peers and downlinks each have a share of the peers' accounts, counted by the kind they were opened as

**Areas:** [Settlement](../areas/settlement.md#settlement-channels), [Network](../areas/network.md#downlinks), [Ledger](../areas/ledger.md#chart-of-accounts).

## Facts

- A peer has two accounts on each ledger it peers on, and they stay for good, since the journal refers to them ([closing-channels](closing-channels.md)). What the ledger's accounts (`max_accounts`) leave beside the room kept for the node's own (`OWN_ACCOUNTS`) bounds the nodes a node ever peers with; past it, a peering is refused for capacity.
- Peering with core nodes is what keeps a node in the network. A core node peers only with nodes in the bootstrap nodes' lists, whose URL it probed, a few at a time.
- A downlink proves only that it holds its key: it is in no list, has no URL to probe, pays nothing and settles nothing to be taken ([Network](../areas/network.md#downlinks)). A key costs nothing to make. The downlinks a core node keeps at once are bounded (`max_downlinks`), and their places free a day after their peerings end, as their channels close.
- So one machine can peer a core node's whole bound of downlinks, end the peerings, wait a day, and do it again. With one share for all peers, it filled the room in a few days, and the node then refused every new peering for good: with core nodes and downlinks, ones it asked and ones that asked it. Every core node that takes QUIC takes downlinks.
- A node's kind can change: a core node that loses its URL turns periphery, and a periphery node that gains one turns core ([Network](../areas/network.md#periphery-nodes)). Its accounts stay as they were.

## Decision

- **Two shares.** What the node's own accounts leave is split evenly between the accounts of core peers and those of downlinks (`peer_budget` in `crates/dassie-node/src/engine/core/peers.rs`). A peer's new accounts open only while its kind's share has room for them; otherwise the peering is refused for capacity, as before. A node whose accounts on the ledger are open peers again regardless. A newcomer whose accounts would not fit is refused for capacity before it is told to pay to enter, so it never pays for a peering the share would then refuse ([pay-to-enter](pay-to-enter.md)).
- **Each share holds at least every open channel of its kind on one ledger.** The node refuses to start otherwise (`check_bounds`). With the default bounds, the core peers' share holds several times their channels, and the downlinks' share nearly twice theirs.
- **Accounts count as the kind they were opened as, for good.** The node records which peers' accounts it opened for a downlink (`downlink_accounts`, written in the batch that opens them), and counts every other peer's toward the core peers' share. Accounts once opened never move between the shares: a downlink that turns core peers again on the accounts it has, which stay the downlinks', and new accounts open in the share of the kind the peer is when they open.
- **A node that kept no record** finds its downlinks' accounts when it upgrades by their settlement channels, whose URL is empty for a downlink. A peer's channel is made with its peering, and outlives it; accounts without one count as a core peer's.

So no number of downlinks takes the room peering with core nodes needs, nor core peers the downlinks'. What the downlinks fill is the downlinks' share only: once it is full, the node takes no downlink with a new key, and its peerings with core nodes go on.

## Accepted

- **A node takes fewer nodes of each kind over its life** than one share for both held. Neither kind's history can now take the other's room, which is what the shares are for. Ending that bound altogether is [resident-peers](resident-peers.md), not built yet ([roadmap](../roadmap.md#not-built-yet)).
- **A periphery node takes no downlinks,** so it never uses the downlinks' share, and its uplinks have half the room they had. It still has room for far more core nodes than it peers with in its life.
- **Even shares, not shares by the bounds on channels.** Downlinks have twice the channels core peers have, and shares by those bounds would give them two thirds. Peering with core nodes is what the node cannot do without, and its core peers change far more slowly than its downlinks can, so the core peers keep half.
- **A node that changed kind is counted by its channel's URL when it upgrades.** A node that peered as a downlink and later as a core node, or the other way round, before the upgrade has its accounts counted as the kind it was last: a few nodes at most, and once counted, they stay where they are.
- **Filling the downlinks' share still costs nothing but keys and days,** and a core node full of downlinks' accounts takes no new downlink. Opening a downlink's accounts only when value moves with it is what makes filling the share cost money.

## Rejected

- *One share for all peers, as before:* the attack above.
- *Taking the kind of a peer's accounts from its settlement channel's URL each time they are counted:* the URL follows the node's kind now, so a node that changes kind would move its accounts from one share to the other, and push a share over its bound. Counting would also need every closed channel's row read at start and kept in step, and accounts opened for a peering whose channel then did not fit would have no kind at all.
- *A third, common share both kinds draw from:* whatever both kinds can draw from, downlinks can fill.
- *Bounding how fast downlinks are taken, without shares:* a slower attack still ends with the same full ledger, and honest downlinks would be refused while it runs.
