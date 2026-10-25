# A peer is in memory only while it is needed: a downlink while it is connected, a core peer while its channel is open

**Areas:** [Ledger](../areas/ledger.md), [Settlement](../areas/settlement.md#settlement-channels), [Network](../areas/network.md#downlinks), [XRP Ledger](../areas/xrpl.md#state-and-restarts), [Periphery](../designs/periphery.md#4-downlinks-on-a-core-node). Not built yet ([roadmap](../roadmap.md#not-built-yet)): on `main`, a downlink is in memory while it is peered and its channel is open, and every peer's ledger accounts stay in memory for good.

## Facts

- The node writes every change to its peers, its channels and its books in the same transaction as the change ([Node](../areas/node.md#persistence)). What it holds of a peer in memory is a copy of what is on disk.
- An uplink reaches a downlink only over a connection the downlink opened ([core-and-periphery](core-and-periphery.md)). While none is open, nothing can be delivered to it: a packet for it is refused at once, and the uplink asks it for nothing. A periphery node keeps the connections it dials alive; connections between core nodes close when idle and are dialled again when needed ([Peer protocol](../areas/peer-protocol.md#quic)).
- A connection holds a place by the key it proved, and downlinks' connections have places of their own ([places-by-proved-key](places-by-proved-key.md)). Setting one up is bounded in all and per client address.
- A settlement channel closes to disk once idle, and is read back when needed ([closing-channels](closing-channels.md)). A peer's two ledger accounts stay in memory when it closes, since the journal refers to them by number, and account numbers are dense indices in the order accounts were opened. So the ledger's account bound caps the peers a node ever has; past it, a peering is refused for capacity.
- A downlink proves only that it holds its key, and keys cost nothing ([free-receivers-half](free-receivers-half.md)). State kept for every downlink ever peered, or peered lately, is state an attacker decides the size of.
- The XRP Ledger schemes recognise a peer's settlement on the ledger by its sender's address, which a scheme keeps in its state and forgets when the peer's channel closes. A Payment from an address it no longer knows is the owner's deposit ([closing-channels](closing-channels.md)).

## Decision

**What the node holds of a peer in memory is a cache of the disk, kept only while something needs it.**

- **A downlink is in memory while it is connected, or while something still needs it:** its peering, replay guard and keys, its settlement channel and its ledger accounts. They are read in when a connection proves the downlink's key and the node holds a peering or a channel with it. They are dropped once its last connection has closed and nothing needs them: no transfer of its pending in the books, no settlement of ours to it on its way, no scheme that keeps its channel (`keeps_channel`, as a settlement hub does for a hold with funds), no termination under way. There is no wait beyond these.
- **A core peer is in memory while its channel is open.** Its accounts close to disk with its channel, in the same batch, under the channel's conditions ([closing-channels](closing-channels.md)), and come back with it.
- **Dropping writes nothing.** The disk already holds everything; dropping frees memory. Reading back is a few lookups by node. A peering's status, a channel's being open or closed, and every bound that counts them do not change with what is in memory: the downlinks' places, and the free receivers' half of them, still count open channels, held in memory or not.
- **Account numbers stay for good.** They are taken from a counter and never given again, so the journal's references stay valid. The ledger holds only the accounts in memory, and for each ledger the totals of the accounts on disk, so every ledger still balances in memory. Only a peer's two accounts leave memory; the owner's never do.
- **One way to a peer's accounts:** found in memory, else read from disk, else opened. A peering, a settlement from the peer, by message or on the ledger, and a loss booked against it all take it. Accounts read for one step go back to disk at its end, if nothing else needs them.
- **Settlements are recognised from disk.** The schemes keep their peers' addresses in a table on disk keyed by address, not in their state, so a Payment from a peer that is not in memory is still booked to the peer's accounts, as the peer's settlement, not as the owner's deposit.
- **Timers for downlinks not in memory run from the disk.** Ending a peering with a downlink silent for as long as a silent peer is kept, and closing its channel once idle, are found by a sweep of the disk by when each was last heard from, bounded in what one sweep reads. A downlink whose peering ended while it was away is told when it next connects, as a former downlink is ([Network](../areas/network.md#termination)).
- **The owner's views read the disk** for what is not in memory: a peer's accounts and balances, and channels.

## Accepted

- **A read of the disk for each connection a downlink opens,** and for each late settlement from a peer not in memory. A connection that drops and comes back is read again. Both are bounded as connections and settlements already are.
- **Rows on disk for good.** A peer's accounts and its channel stay on disk for every peer and ledger the node ever had, as closed channels' rows already do. Memory no longer bounds how many peers a node ever has; disk does. A core peer costs nothing to make until [registration fees](../roadmap.md#registration-fees) exist.
- **A peer on several ledgers at once** takes two accounts on each. The account bound is sized for every peer in memory on one ledger; peers in memory on several at once can still fill it, and then a peering is refused for capacity, never a panic.
- **An account is found through a map** rather than an index, a few steps more on the packet path.
- **After a restart, downlinks are read in as they connect,** and core peers with open channels at start, as today. A downlink's timers run up to a sweep late.
- **What is in memory and what is on disk can disagree only through a bug,** and every close checks it: the close writes the totals it expects to find, and fails if the disk holds others. At a start, the totals of the accounts on disk are added up again from the disk.

## Rejected

- *Accounts leave memory only when the channel closes:* memory still holds every downlink peered with, or lately, up to its bound, and that bound is what keys made for nothing fill. A downlink that is away for a day costs as much as one that is connected.
- *Core peers leave memory when their connection closes:* connections between core nodes close when idle and are dialled again when needed, so a core peer would be read in and dropped again and again, for the few peers that carry the node's traffic.
- *A wait after a downlink disconnects:* what still needs its state already covers the moments after it goes, and reading it in again is cheap. A wait is worth adding only if counters show connections that flap.
- *Dropping the peers used least lately when memory runs short:* traffic, which an attacker controls, would then decide whose state is kept. The connection decides instead, and connections are bounded by key.
- *Deleting accounts with nothing on them:* a former peer's balance is almost never exactly nothing ([closing-channels](closing-channels.md)), and the journal refers to its accounts.
- *Moving a former peer's balance into one account per ledger for all former peers:* what each owed, and the trust each earned ([earned-trust](earned-trust.md)), would be lost to a later peering, and a late settlement could not be booked to the peer who sent it.
- *Giving account numbers again:* the journal would have to be rewritten.
- *Raising the account bound:* it only postpones the cap.
- *Reading every account from disk when used:* the ledger does no I/O, so it stays deterministic and can be simulated.
