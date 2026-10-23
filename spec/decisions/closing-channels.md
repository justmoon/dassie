# A settlement channel left idle closes to disk, whatever the balance

**Areas:** [Settlement](../areas/settlement.md), [Network](../areas/network.md), [XRPL](../areas/xrpl.md).

## Facts

- A node keeps a settlement channel with every node it peered with, and the channel outlives the peering, so a settlement that arrives after the end is credited once ([peering-lifecycle](peering-lifecycle.md)). Channels are bounded in number, with core nodes and with downlinks. A node that kept every channel would refuse peering with a new node once the bound was full. Vanished nodes are what fill it.
- In memory, a channel holds the node's key and URL, the last sequence applied from it, the last one sent, our message on its way, and whether we once ended a peering with it for want of room. It also pins the node's replay state, and the XRPL scheme keeps the node's address for it. Once nothing is on its way, only three things matter for later: the two sequences and the mark of an end for room.
- The sequences are what make a settlement message arrive once. If one side forgets them, they no longer match. The other side's next message is then refused as out of order for ever, or a copy whose acknowledgement was lost is applied again.
- A former peer's balance is almost never exactly nothing: settlements round down to whole units of the ledger, and the last request of an end collects only half the way to the trigger ([ending-a-peering](ending-a-peering.md)). The peer's accounts keep that balance, and a later peering reuses them and settles it.

## Decision

- **A channel closes once it has been idle for as long as a silent peer is kept** (`peer_dead_ms`), counted from the later of the node's start, the end of the peering and the channel's last settlement either way. Only these close:
  - channels with a node we neither peer with nor are asking;
  - channels with nothing on their way: no message, and no settlement of ours under way;
  - channels whose node's books let them close: nothing of the node is pending in the ledger, and every scheme it has accounts with runs and sees its ledger.
- **Whatever the balance.** The node's accounts stay with what they show, and its trust with them. A later peering reuses them and settles what they show, as after any end.
- **What stays of it is on disk only:** its row in `settlement_channels`, marked closed, with its two sequences and the mark of an end for room. It leaves memory, frees its place under the bound, and lets go of the node's replay state. The schemes forget what they kept for the node's settlements (`on_closed`).
- **A closed channel is read back when it is needed.** A peering with the node opens it again where it was, so our next settlement follows the last and the peer applies it. A settlement that comes over it closed is applied once, by the sequence that stayed, and the channel stays closed.
- **The mark of an end for room stays for good, open or closed** ([ended-for-room](ended-for-room.md)).
- **Peers' accounts leave room for the node's own.** With channels freeing their places, former peers' accounts, which stay, would otherwise fill the ledger's accounts and leave none for what the owner opens: a BTP token's, or a newly activated ledger's common and payer accounts. Those are bounded (`OWN_ACCOUNTS`: every token ever made, and every ledger's), so new peers' accounts stop where that room begins, and the peering is refused for capacity. A node whose accounts are open peers again regardless.

## Accepted

- **Rows on disk for good.** A closed row is never removed. The number of rows is bounded by the ledger's accounts, which are never removed either, and every channel has them. The accounts, which stay in memory, still bound the nodes a node ever peers with, to what the node's own leave of them ([roadmap](../roadmap.md#not-built-yet)).
- **A ledger payment that lands after the close is the owner's deposit.** The scheme no longer knows the node's address. An honest node settles only when asked, and the close waits a day after the end with every scheme of the node's accounts seeing its ledger. Only a settlement held back past all that is lost to the node's account, and it goes to the owner, not to anyone else. The node's message for it is still applied, which moves the sequence and books nothing on the XRP Ledger.
- **A closed node needs room to peer again.** While its channel was open, it could peer past a full bound; closed, it waits for a place like a node we never held a channel with.
- **A restart starts every wait again.** When a channel was last active is kept only in memory, so a node that restarts closes its channels a day later than it would have, never sooner.
- **Reading a closed row costs a read of the database** for each peering with a node without an open channel, and for each settlement message from one. Both are rare, and are bounded as the requests that bring them are.

## Rejected

- *Close only when nothing is owed either way:* that almost never holds, so almost nothing would close. Nor does it need to hold, since the accounts keep the balance.
- *Forget the channel, and resynchronise the sequences:*
  - Starting each new channel's sequences from the time, with the receiver applying any sequence above its last, lets the sender go on after the receiver forgot.
  - A copy of a message whose acknowledgement was lost would then be applied twice. That books twice on the stub.
  - Old nodes, which take only the next sequence, would refuse a new channel's first message.
- *A close both sides agree to:* a vanished node never agrees, and vanished nodes are what fill the bound.
- *Keep closed channels in memory, smaller:* the memory still grows with every node ever peered with, which the bound exists to prevent, and the row on disk already holds what is needed.
- *Never close a channel marked for room:* each would hold its place for good. With the mark on disk, closing loses nothing.
