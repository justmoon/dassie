# Settlement

How a node moves money with its peers on an underlying ledger, and how its books learn about it: the settlement scheme interface, the credit a node extends to each peer and how that grows, when it asks a peer to settle and how much it settles when asked, and how settlement messages are delivered exactly once.

**Paths:** `crates/dassie-settlement/`, `crates/dassie-node/src/engine/core/settlement.rs`, `crates/dassie-node/src/engine/core/growth.rs`, `crates/dassie-node/src/engine/core/ledgers.rs`, `crates/dassie-node/src/engine/core/schemes.rs`, `crates/dassie-node/src/engine/links.rs`, `crates/dassie-peer/src/node/settle.rs`, `crates/dassie-peer/src/channel.rs`
**Read with:** [Ledger](ledger.md), [XRPL](xrpl.md), [Network](network.md), [Peer protocol](peer-protocol.md), [Connector](connector.md)

Each node keeps its own books and decides from them alone: whether it takes a settlement, when it asks a peer to settle, how much it settles, and when it ends a peering. What a peer says is an input to these decisions, never trusted past the node's limits. However a peer behaves, and however far its books are from ours, **it costs us at most the credit we extend, plus what it has in flight** ([Connector](connector.md#money-in-flight-with-a-peer)). The node does not know the credit a peer extends to it, does not model the peer's books, and does not measure drift ([D19](../decisions/D19-each-node-keeps-its-own-books.md)).

## Schemes and ledgers

A settlement scheme serves one internal ledger, and a ledger has at most one active scheme. The node's registry is in `engine/core/schemes.rs`. Only ledgers of the test realm activate ([XRPL](xrpl.md#real-money)).

Activating a scheme, when the owner adds it or at start, makes it from its config and its last [checkpoint](#checkpoints-and-activation), opens the ledger's common accounts, books the activation entry, and gives the ledger its STREAM receiver and payer ([Connector](connector.md#local-endpoints)). Removing a scheme is refused while anything still depends on it: tokens, peerings, final settlements due, or outgoing payments on its ledger. Its accounts and checkpoint stay, so adding it again picks up where it was, including the settlements still pending.

## The scheme interface

A scheme is sans-IO: its methods return effects (`Effect` in `scheme.rs`) for the node to book or carry out, rather than calling back into the node. The contract the node relies on:

- **Prepare submits nothing.** `prepare_settlement` returns an ID unique within the scheme, an amount of at most what was asked in whole units of its ledger, and the message for the peer. The node books it before anything leaves ([below](#outgoing-settlements)).
- **Execute only once the pending transfer is on disk,** and report `Finalize` or `Cancel`, now or later. After a restart, `reconcile` gets the settlements still pending and reports the same for each.
- **The message can be made again** from the settlement's ID, so a restart that lost it can still announce the settlement.
- **Reports** (deposits, withdrawals, ledger fees, reconciliations) are recorded even past the owner's limit ([Ledger](ledger.md#what-the-ledger-refuses-and-what-it-records)). An incoming settlement is booked Dr `assets/settlement`, Cr `assets/interledger/<peer>`.
- **`Unavailable`** means not now (its ledger not seen, no funds to spare, too much in flight). It is asked again at the next check as if nothing happened.
- **The scheme's reserve** is what its ledger holds back from the balance: never booked, so never spendable.

**A scheme that fails stops alone.** Effects the books cannot take stop the scheme, shown as failed, rather than the node; its state is not written, so a restart reads the ledger again. A stopped scheme has its connection closed, so the driver does not make it again for ever; removing and adding it, or a restart, connects anew.

### Schemes that follow a ledger

A scheme with a ledger to follow names a WebSocket URL; it says what to send (`LedgerSend`) and the node feeds it what comes back. The driver (`engine/links.rs`) runs one connection per scheme, bound to an epoch, so a message from a connection the engine has replaced is dropped. A connection whose queue overflows drops messages, and the scheme, hearing nothing, connects anew. Names resolve under the node's outbound rules: public addresses only, unless private ones are allowed (dev and tests).

## Checkpoints and activation

The books keep, for each scheme, a **checkpoint**: the balance they record for it and its state. It is written in the batch it explains, and a scheme's state is also written with the pending transfer of each settlement it prepared, so what it signed is kept.

Activation books only the difference between the scheme's balance and the last checkpoint (`activation.rs`): with no checkpoint, the whole balance is the owner's first deposit; after that, a gain or a loss goes through `equity/suspense` as a reconciliation. So a restart never books the same money twice, and money that moved while the node was down is the owner's.

## Credit

**Roles.** The **junior** is the node whose peering request was accepted, the **senior** the node that accepted it; when requests cross, the lower node ID's request wins, so the lower ID is the junior ([Network](network.md)). The node that asked to peer takes the risk at first ([D20](../decisions/D20-trust-follows-what-a-peer-earned-us.md)).

**Starting credit.** The junior extends the senior the default credit line; the senior extends the junior nothing. Asked to settle at once, the junior then pays in advance, and payments to the junior flow at once, within the credit it extends. Both are policies of the node (`Policies` in `crates/dassie-node/src/node/mod.rs`), not owner settings, and one value for every ledger ([roadmap](../roadmap.md#not-built-yet)).

- **Peering again** keeps the trust account and its credit. A junior still extends at least the default line: a node that was the senior before, or that accepted a request only one side recorded, raises the account to it.
- **Nothing is announced.** The credit a node extends is its own business: the peer protocol's `credit` fields are sent as 0 and ignored ([Peer protocol](peer-protocol.md)). More credit only means the creditor asks later.

**The least credit.** Asked at the asker's trigger, a debtor settles only if its own books leave it its trigger plus twice the least settlement ([below](#settling-on-request)). So a link survives drift of up to its credit less two triggers and twice the least settlement, and the node refuses to start with a default credit (or a senior credit other than 0) below that plus one crash's drift at the cap on money in flight (`policy::least_credit`):

```
least_credit = 2 · trigger + 2 · least settlement + max_in_flight = 3P + 2P + 2.5P = 7.5 P
```

The default clears it with room for about two crashes.

**Growth.** Each side raises the credit it extends to the other by k = 10% of what it earned on packets that came in or went out through that peer (`policy::GROWTH_PPM`, `engine/core/growth.rs`).

- **What counts** is our connector's fee ([Connector](connector.md#fees-and-spread)), toward each peer of the packet on the ledger the fee was booked on. A packet's fee counts toward both peers it passed, so k must stay at most 50% (asserted) for the credit a node hands out never to exceed what it earned. A packet that converts counts toward the peer it came from only, since counting it toward the other would need our exchange rates; the spread is not booked per packet and does not count.
- **The cap.** A peer earns at most a hundred of the largest packets of credit. Credit above it, configured or from before, stays and never falls.
- **Measured from the books.** What a peer earned is read from the journal's fee rows, never kept in counters, and the raise is an explicit `trust` transfer. Reading resumes after the last trust transfer, so a crash can forget fees not read yet, and credits none twice.
- **Retirement is accepted.** A node that leaves can spend everything its peers trust it with and disappear; each peer loses at most k of what it earned through that node.
- Fees default to 0, so trust grows only once an owner charges for passing payments on. Until then a junior keeps settling in advance.

## Positions and thresholds

A node knows only the credit it extends to a peer, not the credit the peer extends to it, so only the creditor can tell when a peer should settle. It asks; the debtor decides how much ([D23](../decisions/D23-each-node-books-its-own-fulfills.md)). Both rules are in `crates/dassie-settlement/src/policy.rs`.

```
position = what the peer may still owe us: the credit we extend, plus what we owe it,
           net of our settlements in flight (from assets/interledger/<peer>)

ask the peer to settle   when position ≤ trigger
when asked, settle       (position − trigger) / 2, or nothing if that is less than the least settlement
when a peering ends      settle position − credit we extend
```

Every threshold of a link is counted in its [largest packet](connector.md#the-largest-packet) `P`, not in the credit, which differs on each side and grows with earnings ([D25](../decisions/D25-a-links-thresholds-are-counted-in-its-largest-packet.md)). So even a senior that extends nothing asks while its junior still has room.

| Threshold | In `P` | Why |
|---|---|---|
| Trigger to ask | 1.5 | Below the cap on money in flight, so a busy peer can still have a packet refused (T04) while its settlement is on its way; a trigger at the cap left too little credit for one crash's drift. |
| Least settlement | 1 | A peer that keeps asking cannot make us pay a ledger fee for every few drops. |
| Money in flight with a peer, each way | 2.5 | [Connector](connector.md#money-in-flight-with-a-peer) |
| Room below which a peer is no longer advertised, past a grace | 1 | [Network](network.md#advertised-peers) |
| Least default credit | 7.5 | [Above](#credit) |
| Most credit earned | 100 | [Above](#credit) |

Amounts are rounded down to whole units of the scheme's ledger; what is less than a unit stays owed, for good at the end of a peering. Both rules are deliberately simple, to be tuned later (expected settlement latency, fees, volume on the link).

## Asking a peer to settle

At every settlement check the node asks each peer whose position has fallen to its trigger, naming only the scheme ([Peer protocol](peer-protocol.md)). Nothing about a request is kept: a restart looks again.

- **It asks again** at the next check once its books show a settlement from that peer, not on the `settlement` message alone: on the XRP Ledger the message books nothing, so asking on it would make the peer pay twice. It also asks again at the next check after a 409, since our acceptance may not have reached the peer yet. Otherwise it asks at most once a minute, so a peer that does not answer is not flooded.
- **Only while its scheme is synced.** An honest peer settles only when asked, so no settlement reaches the ledger before the scheme first synced, which books what the account held by then as the owner's deposit. A settlement a peer makes unasked before then is that deposit.
- With no credit extended, it asks at once, and asks again once what the peer paid in advance is down to the trigger.

## Settling on request

A request says that the debtor stands near the asker's trigger. The debtor settles half the way from where it stands to its own trigger, which lands near the middle of the two.

- **It never pays itself into asking back:** settling lowers its own position, however far two nodes' books have drifted apart.
- **One settlement at a time.** It settles nothing while a settlement to that peer is on its way (prepared, executing, pending in the ledger, or announced and not yet acknowledged): that one is the answer. Nor while a cancelled settlement holds it off ([below](#outgoing-settlements)).
- **The answer says what is on its way:** the settlement it makes now, or the one already under way, or nothing. So an asker can tell a settlement still coming from none ([D28](../decisions/D28-a-node-that-ends-a-peering-collects-first-and-pays-nothing.md)).
- **A lying asker gets at most the credit the debtor extends to it.**
- A node never settles on its own, but at a peering's end.

## Settling at a peering's end

When a peering ends and the exchange of `peeringTermination` completed ([Network](network.md)), each side settles everything it owes, in whole units. What is due is kept on the [settlement channel](#settlement-channels) across restarts until it is made, and a cancelled or failed attempt is tried again at a later check. An unresponsive peer is not settled with: the debt stays on the books. A new peering with the node settles normally, and a final settlement still due becomes part of it.

## Outgoing settlements

The order makes a settlement reach the books exactly once, whatever crashes:

1. **Check.** The node validates the pending transfer (Dr `assets/interledger/<peer>`, Cr `assets/settlement`) before it asks the scheme, so a refusal leaves nothing prepared behind.
2. **Prepare.** The scheme prepares the settlement, for at most that amount.
3. **Book.** The node books the durable pending transfer, keeps the peer's new scheme state, and checkpoints the scheme. Nothing is booked between the check and this step, so the books always take it: a refusal here is a broken invariant, which stops the engine. So a prepared settlement is never discarded.
4. **Execute,** only once that transfer is on disk. A restart in between hands the scheme its pending settlements to reconcile.
5. **Finalize.** Only a settlement that executed is posted and announced to the peer, in one batch, after any earlier announcement to that peer arrived.
6. **Cancel.** A cancelled settlement is voided, and the node backs off settling with that peer, exponentially; a settlement posted to the peer ends the backoff.

After a restart, settlements still pending in the journal are handed to their scheme when it activates, to post or void as its ledger says. One the scheme no longer holds is announced with a message it makes again from the ID.

## Incoming settlements

- **The books first.** The node checks that it can book a settlement before the scheme sees it, so the scheme's balance never moves without the books. Otherwise it answers 400 and nothing changes.
- **The scheme decides what it is worth.** The stub books the amount stated; the XRPL scheme books nothing on the message, since its ledger says what arrived ([XRPL](xrpl.md#settlement-messages)).
- **From a channel, not a peering.** A `settlement` is taken from any authenticated node we hold a [settlement channel](#settlement-channels) with, so a final settlement that arrives after the peering ended is still credited.
- A settlement the scheme sees on its ledger from a node without accounts there cannot be from a peer: it is booked as the owner's deposit.

## Settlement channels

The node keeps a settlement channel with every node it has ever peered with (`crates/dassie-peer/src/channel.rs`). A channel belongs to the pair of nodes, not to a peering: it outlives the peering and the node's entry in the node table, and a new peering uses it again. It holds the sequences, our message on its way, and a final settlement due.

**Delivery is exactly once.** Each `settlement` message carries a sequence per sender and recipient that never resets. The sender sends one at a time, in order, again until it is acknowledged, across restarts, never dropping it; one that got no answer goes again as soon as the node answers us at all ([Peer protocol](peer-protocol.md#message-set)). The receiver applies only the next sequence, recording it in the batch that books it, acknowledges anything at or below the last one without applying it again, and refuses anything else. So a copy from an old peering is never applied twice, and none is lost.

**Channels are never closed,** so a node keeps a bounded number of them, one per node it has ever peered with. Once all are taken, peering with a new node is refused for capacity; nodes it holds a channel with can still peer. Closing a settled channel safely is not built ([roadmap](../roadmap.md#not-built-yet), [D12](../decisions/D12-peering-lifecycle-and-liveness.md)).

## The stub scheme

Test money without an underlying ledger (`crates/dassie-settlement/src/stub.rs`). Its "ledger" is a number the checkpoint keeps; its first activation books a starting balance as the owner's deposit, and the owner may deposit more. It accepts every peering, and finalizes a settlement as it executes it; one still pending after a restart was never executed, so it is cancelled.

A peer's settlement is believed as stated. Anyone peered over the stub can mint credit, which is why it only ever holds test money.

## Decisions

- [D6](../decisions/D06-owner-ledger-and-multi-currency-accounts.md): the default trust line becomes the junior's starting credit.
- [D8](../decisions/D08-real-money.md): only test-realm ledgers activate.
- [D12](../decisions/D12-peering-lifecycle-and-liveness.md): settling at a peering's end; channels kept after it, and their limit.
- [D19](../decisions/D19-each-node-keeps-its-own-books.md): each node decides from its own books, and the bound that holds whatever a peer does.
- [D20](../decisions/D20-trust-follows-what-a-peer-earned-us.md): junior and senior, starting credit, growth from fees.
- [D23](../decisions/D23-each-node-books-its-own-fulfills.md): the creditor asks, the debtor decides how much.
- [D24](../decisions/D24-money-in-flight-with-a-peer-stays-within-a-few-packets.md), [D25](../decisions/D25-a-links-thresholds-are-counted-in-its-largest-packet.md): thresholds in the largest packet, and the least credit.
