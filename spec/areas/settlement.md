# Settlement

How a node moves money with its peers on an underlying ledger, and how its books learn about it: the settlement scheme interface, the schemes the node runs, the credit it extends to each peer and how that grows, when it asks a peer to settle and how much it settles when asked, and how settlement messages are delivered exactly once.

**Paths:** `crates/dassie-settlement/`, `crates/dassie-node/src/engine/core/settlement.rs`, `crates/dassie-node/src/engine/core/growth.rs`, `crates/dassie-node/src/engine/core/ledgers.rs`, `crates/dassie-node/src/engine/core/schemes.rs`, `crates/dassie-node/src/engine/links.rs`, `crates/dassie-peer/src/node/settle.rs`, `crates/dassie-peer/src/channel.rs`
**Read with:** [Ledger](ledger.md), [XRPL](xrpl.md), [Network](network.md), [Peer protocol](peer-protocol.md), [Connector](connector.md)

Each node keeps its own books and decides from them alone: whether it takes a settlement, when it asks a peer to settle, how much it settles, and when it ends a peering. What a peer says (a settlement, a request to settle) is an input to these decisions, never trusted past the node's limits. However a peer behaves, and however far its books are from ours, it costs us at most the credit we extend, plus what it has in flight ([Connector](connector.md#money-in-flight-with-a-peer)). The node does not know the credit a peer extends to it, does not model the peer's books, and does not measure drift.

## Schemes and ledgers

A settlement scheme serves one internal ledger. The node's registry:

| Scheme | Name | Ledger |
|---|---|---|
| `stub` | Stub | `stub+usd` ([below](#the-stub-scheme)) |
| `xrpl-testnet` | XRP Ledger Testnet | `xrpl-testnet+xrp` ([XRPL](xrpl.md)) |

Scheme IDs follow the peer protocol's `SettlementSchemeId` ([Peer protocol](peer-protocol.md)). The original's `xrpl` mainnet scheme is not in the registry ([XRPL](xrpl.md#real-money)).

**Activating** a scheme, when the owner adds it or at start:

1. The scheme is made with its config, which is passed through to it (ACC-12), and with what the books last kept for it, its [checkpoint](#checkpoints-and-activation). An unknown scheme, or a config or state it refuses, is an error.
2. A ledger whose realm is not test is refused: "real money is not supported yet" ([XRPL](xrpl.md#real-money)). A ledger another active scheme serves is refused.
3. The ledger's common accounts are opened ([Ledger](ledger.md#chart-of-accounts)), and the [activation entry](#checkpoints-and-activation) is booked.
4. The scheme's connection is opened, if it follows a ledger; the node's link state advertises its schemes; the ledger gets its STREAM receiver and its payer ([Connector](connector.md#addresses-and-endpoints)).
5. A scheme added again hands back the settlements still pending from when it was removed ([below](#outgoing-settlements)).

Active schemes persist in `settlement_schemes (id, config, activated_at_ms)`.

**Removing** a scheme is refused while anything uses it: BTP tokens not revoked on its ledger, peers that settle with it (or are ending a peering that does), peerings that ended and still have to be settled with it, or outgoing payments from its ledger that have not finished. Its checkpoint stays, and its accounts with their balances.

## The scheme interface

A scheme is sans-IO: its methods return **effects** for the node to book or carry out, rather than calling back into the node.

| Method | Contract |
|---|---|
| `id`, `ledger`, `supported_versions` | its ID, its ledger, the scheme protocol versions it speaks, negotiated when peering (ACC-14, NET-26) |
| `balance` | its balance on the underlying ledger in internal units, as the books hold it: what the node checkpoints |
| `status` | `Synced`, `Connecting`, `Syncing`, `Unfunded` or `Failed(why)`, for the owner; a scheme without an underlying ledger is always `Synced` |
| `connection` | a WebSocket URL, if it follows a ledger |
| `deposit_address` | where the owner sends money to fund it, if anywhere |
| `unit` | the least amount its ledger can move: settlements are whole multiples of it |
| `reserve` | what its ledger holds back from the balance: never spendable, never booked |
| `state`, `state_version` | what it must keep across restarts beyond its balance; a number that changes whenever the state does |
| `on_ledger`, `deadline`, `on_timer` | what came from its connection or its faucet; its timers |
| `peering_info`, `create_peering_request`, `accept_peering_request`, `finalize_peering_request` | the peering hooks; the last two return the peer's scheme state, which the node keeps (ACC-15) |
| `prepare_settlement(peer, amount, peer state)` | prepares a settlement and submits nothing: its ID (unique within the scheme), its amount (at most what was asked, in whole units), the message for the peer, and the peer's new scheme state if it changed |
| `discard_settlement` | forgets a settlement prepared and never executed: it is never sent. The node does not call it: it checks the books before it prepares ([below](#outgoing-settlements)) |
| `execute_settlement` | executes a prepared settlement; called only once its pending transfer is on disk; reports `Finalize` or `Cancel`, now or later |
| `settlement_message(id)` | makes the message for a settlement of ours again from its ID, after a restart lost it |
| `reconcile(pending)` | settles the outcome of settlements whose pending transfers survived a restart (ACC-16): each gets `Finalize` or `Cancel`, now or later |
| `handle_settlement`, `handle_message` | a peer's `settlement` and `settlementMessage` |
| `deposit(amount)` | adds money for the owner, where the scheme can; `Unsupported` where the owner deposits on the underlying ledger |
| `check_invariants` | asserts the scheme's bounds and how its parts relate |

**Effects** and what the node does with them:

| Effect | The node |
|---|---|
| `IncomingSettlement {peer, amount, id}` | posts Dr `assets/settlement`, Cr `assets/interledger/<peer>`, with the ID on the underlying ledger as the reference where it has one ([below](#incoming-settlements)) |
| `Finalize {settlement}` | posts our settlement's pending transfer, and announces it ([below](#outgoing-settlements)) |
| `Cancel {settlement}` | voids it |
| `Deposit {amount}` | posts Dr `assets/settlement`, Cr `equity/owner` |
| `Withdrawal {amount}` | posts Dr `equity/owner`, Cr `assets/settlement` |
| `Fee {amount, id}` | posts Dr `expenses/fees`, Cr `assets/settlement`, with the transaction as its reference |
| `Reconcile {amount, gain}` | posts through `equity/suspense`: a gain Dr `assets/settlement`, Cr `equity/suspense`; a loss the reverse |
| `SendMessage {peer, message}` | sends a `settlementMessage` to a peer, once: schemes do not depend on them arriving |
| `LedgerSend {message}` | sends it over the scheme's connection |
| `Reconnect` | makes the connection anew |
| `Fund {url, body}` | posts `body` to the faucet `url` over HTTPS; the answer comes back as `Funded(ok)` |

Deposits, withdrawals, fees and reconciliations are reports: they are recorded even past the owner's limit ([Ledger](ledger.md#what-the-ledger-refuses-and-what-it-records)).

**Errors** a scheme returns: `UnknownScheme`, `InvalidConfig`, `Unsupported`, `InvalidData`, `TooManyPrepared`, `Overflow`, and `Unavailable`: not now, because it cannot reach or does not yet see its ledger, has no funds to spare, or has as much in flight as it takes. `Unavailable` is asked again at the next check as if nothing happened, without backoff, and the owner is warned at most every 10 minutes.

**A scheme that fails.** Effects the books cannot take stop the scheme, shown as failed ("the books could not take what the ledger said; restart the node"), rather than the node. Its state is not written, so a restart reads the ledger again. A scheme that stops, for that or for a reason of its own, has its connection closed, so the driver does not make it again; removing the scheme and adding it again, or a restart, connects anew.

### Schemes that follow a ledger

A scheme with a ledger to follow names a WebSocket URL. It says what to send (`LedgerSend`), and the node feeds it what comes back: `Connected`, `Message`, `Disconnected`, and the faucet's answer `Funded`.

- The driver runs one WebSocket per scheme, opened and closed when the engine says, and bound to an **epoch**: a message from a connection the engine has replaced is dropped.
- A connection that drops is made again, backing off from 1 s to 60 s. Connecting may take 10 s. A message is at most 1 MiB. At most 64 messages wait to go out on a connection; beyond, they are dropped, and the scheme, hearing nothing, connects anew.
- `wss://` connects with the node's TLS trust; `ws://` only where private addresses are allowed (dev and tests). Names resolve as for the node's other requests: public addresses only, unless private ones are allowed.
- A faucet request has 30 s and at most 64 KiB of answer.

## Checkpoints and activation

The books keep, for each scheme, a **checkpoint**: the balance they record for it and its state, in `settlement_checkpoints (scheme, balance ≥ 0, updated_at_ms, state ≤ 256 KiB)`.

- The checkpoint is written in the batch it explains, whenever the scheme's state version or its balance changed, not at every ledger. A scheme's state is also written with the pending transfer of each settlement it prepared, so what it signed is kept.
- **Activation** books only the difference between the scheme's balance and the last checkpoint (ACC-6):

| Checkpoint | Balance | Booked |
|---|---|---|
| none | 0 | nothing |
| none | `b` | the owner's first deposit: Dr `assets/settlement`, Cr `equity/owner`, `b` |
| `c` | `b > c` | a gain: Dr `assets/settlement`, Cr `equity/suspense`, `b − c` (`reconciliation`) |
| `c` | `b < c` | a loss: Dr `equity/suspense`, Cr `assets/settlement`, `c − b` (`reconciliation`) |
| `c` | `c` | nothing |

## Peering

The settlement side of peering, whose messages and roles are in [Peer protocol](peer-protocol.md) and [Network](network.md):

- The responder serves its scheme's peering info; the requester builds its request from it; the responder accepts or refuses it (a refusal is reason 6, refused by the scheme), and the requester finalizes the acceptance. Each side keeps the peer's scheme state with the peering, and the scheme's version both speak.
- A peer's accounts are opened when the peering forms, with the starting credit ([below](#credit)); an existing trust account keeps its credit.
- Every peer gets a [settlement channel](#settlement-channels).

## Credit

**Roles.** The **junior** is the node whose peering request was accepted, the **senior** the node that accepted it; when requests cross, the lower node ID's request wins, so the lower ID is the junior ([Network](network.md)).

**Starting credit.**
- The junior extends the senior the default credit line: 1.1 units (1,100,000,000 at scale 9).
- The senior extends the junior 0. Asked to settle at once, the junior then pays in advance: `(1.1 − 0.15) / 2 = 0.475` at the default, and can send that much before it owes anything. Payments to the junior flow at once, within the credit it extends.
- **Peering again.** The trust account of a peer stays when its peering ends, with the credit it holds, and a later peering keeps it without posting trust again. A junior extends at least the default credit line all the same: a node that was the senior before, or that accepted a request only one side recorded, raises the account to it by a trust transfer.
- Both starting credits are policies of the node, set in its code, not owner settings. They are one value for every ledger: per-ledger values are not built ([roadmap](../roadmap.md#not-built-yet)).
- The credit a node extends is kept with the peering (`peers.credit_out`). It is not announced: the `credit` of `peeringRequest` and of its response is sent as 0 and ignored ([Peer protocol](peer-protocol.md)).

**The least credit.** Asked at the asker's trigger, a debtor settles only if its own books leave it its trigger plus twice the least settlement. So a link survives drift of up to its credit less two triggers and twice the least settlement. A node refuses to start (an assertion on its policies) with a default credit below that plus one crash's drift at the cap on money in flight:

```
least_credit = 2 · trigger + 2 · minimum + max_in_flight = 3P + 2P + 2.5P = 7.5 · P     -- 0.75 units at the default
```

The default of 1.1 clears it with room for about two crashes. A senior credit other than 0 must clear it too.

**Growth.** Each side raises the credit it extends to the other by k = 10% of what it earned on packets that came in or went out through that peer. A packet's fee counts toward both peers it passed, so k is at most 50% (asserted), and the credit a node hands out never exceeds what it earned. The rule is the same both ways; only the starting points differ.

- **What counts** is our connector's fee ([Connector](connector.md#fees-and-spread)), toward each peer of the packet on the ledger the fee was booked on, the ledger the packet came in on. A packet that passes on one ledger counts toward both its peers; one that converts counts toward the peer it came from only. The spread's gain is not booked per packet, and does not count.
- **The cap.** A peer earns at most a hundred of the largest packets of credit: 10 units at the default. Credit above it, configured or from before, stays and never falls. k and the cap are node constants.
- **Measured from the books.** What a peer earned is read from the journal, never kept in counters. At every settlement check the node reads, per active ledger, the fees posted to `revenue/fees` since the last check, a page of at most 4096. The credit is raised by an explicit trust transfer, Dr `contra/trust/<peer>`, Cr `assets/interledger/<peer>`. Growth below a whole base unit is carried in memory to the next check.
- **Restarts.** Reading resumes after the last trust transfer in the journal. A crash can forget the fees not read yet, and credits none twice. A node's first start with growth also reads from its last trust transfer.
- **Retirement is accepted.** A node that leaves can spend everything its peers trust it with and disappear; each peer loses at most k of what it earned through that node.
- Fees default to 0, so trust grows only once an owner charges for passing payments on. Until then a junior keeps settling in advance.

## Positions and thresholds

A node knows only the credit it extends to a peer, not the credit the peer extends to it, so only the creditor can tell when a peer should settle. It asks; the debtor decides how much.

```
IL         = L:assets/interledger/<peer>,  TR = L:contra/trust/<peer>
position   = IL.credits_posted − IL.debits_posted − IL.debits_pending     -- at least 0
credit_out = TR.debits_posted − TR.credits_posted
P          = the largest packet (0.1 units at the default)
trigger    = 1.5 · P                                -- 0.15
minimum    = P                                      -- 0.1, the least settlement

ask the peer to settle   when position ≤ trigger
when asked, settle       (position − trigger) / 2, or nothing if that is less than minimum
when a peering ends      settle position − credit_out
```

- `position` is what the peer may still owe us: the credit we extend, plus what we owe it, net of our settlements in flight.
- Every threshold of a link is counted in its largest packet, not in the credit, which differs on each side and grows with earnings. So even a senior that extends nothing asks while its junior still has room.
- The trigger is below the cap on money in flight (2.5 P), so a busy peer can still have a packet refused (T04) while its settlement is on its way.
- Every amount is then rounded down to whole units of the scheme's ledger. What is less than a unit stays owed, for good at the end of a peering.
- The creditor's predicate and the debtor's amount are simple rules, to be tuned later (expected settlement latency, fees, volume on the link).

| Threshold | Of P | At the default |
|---|---|---|
| Trigger to ask | 1.5 | 0.15 |
| Least settlement | 1 | 0.1 |
| Money in flight with a peer, each way | 2.5 | 0.25 |
| Least default credit | 7.5 | 0.75 |
| Most credit earned | 100 | 10 |

## Asking a peer to settle

Every 4 s the node checks its peers. It asks each peer whose position has fallen to its trigger with `settlementRequest`, naming only the scheme ([Peer protocol](peer-protocol.md)):

- **Again at the next check** once its books show a settlement from that peer: not on the `settlement` message alone, which on the XRP Ledger books nothing, so the peer would pay twice.
- **Again at the next check** once the peer refused the request for not being peered with it (409, as when our acceptance had not reached it yet).
- **Otherwise at most once a minute,** so a peer that does not answer is not flooded.
- **Only while its scheme sees its ledger** (`Synced`). An honest peer settles only when asked, so no settlement reaches the ledger before the scheme first synced, which books what the account held by then as the owner's deposit. A settlement a peer makes unasked before then is that deposit.
- With no credit extended, it asks at once, and asks again once what the peer paid in advance is down to the trigger.
- Nothing about a request is kept: a restart looks again.

## Settling on request

A request says that the debtor stands near the asker's trigger. The debtor settles half the way from where it stands to its own trigger, which lands near the middle of the two.

- Settling lowers its position, so it never pays itself into asking back, however far two nodes' books have drifted apart. With the same credit both ways, a debtor asked owes about 0.95 of 1.1, stands at 2.05, and settles 0.95, back to the middle.
- It settles nothing less than its least settlement, so a peer that keeps asking cannot make it pay a ledger fee for every few drops.
- It settles nothing while a settlement to that peer is on its way: one prepared or executing, a settlement transfer to it pending in the ledger, or a `settlement` message not yet acknowledged (ACC-17). That one is the answer.
- It settles nothing while a cancelled settlement holds it off ([below](#outgoing-settlements)), and only with a peering on the scheme named.
- A lying asker gets at most the credit the debtor extends to it.
- A node never settles on its own, but at a peering's end.

## Settling at a peering's end

When a peering ends and the exchange of `peeringTermination` completed ([Network](network.md)), each side settles everything it owes: `position − credit_out`, in whole units. That is kept on the settlement channel (its scheme and the peer's scheme state), across restarts, until it is made; a cancelled or failed attempt is tried again at a later check. Nothing owed, or less than a unit, settles it at once. An unresponsive peer is not settled with: the debt stays on the books. A new peering with the node settles normally, and a final settlement still due becomes part of it.

## Outgoing settlements

1. **Check.** The node validates a pending transfer of the amount, Dr `assets/interledger/<peer>`, Cr `assets/settlement`, before it asks the scheme, so a refusal leaves nothing prepared behind. It first moves the ledger's waiting journal entries into the step's batch, so the journal has room. If the books cannot take the transfer, the node reports it, and does not settle with the peer this time.
2. **Prepare.** The scheme prepares the settlement, for at most the amount, in whole units. `Unavailable` and other scheme errors prepare nothing ([above](#the-scheme-interface)).
3. **Book.** The node books the durable pending transfer, with reference (scheme, settlement ID), keeps the peer's new scheme state if the scheme changed it, and checkpoints the scheme. Nothing is booked between the check and this step, and the prepared amount is at most the amount checked, so the books always take it: a refusal here is a broken invariant, which stops the engine. So a prepared settlement is always booked, and none is discarded.
4. **Execute.** Only once that transfer is on disk does the scheme execute the settlement. A restart in between hands the scheme its pending settlements to reconcile (ACC-16).
5. **Finalize.** Only a settlement that executed is posted and announced to the peer, in one batch. If an earlier settlement message to the peer is still on its way, the posting and the announcement wait for it, and are tried at each check. Posting ends any backoff with the peer, and settles a final settlement that was due.
6. **Cancel.** A cancelled settlement is voided, and the node backs off settling with that peer, from 1 s doubling with each cancellation in a row, up to 10 min. A settlement posted to the peer ends the backoff.

**After a restart,** settlements still pending in the journal are handed to their scheme when it activates, by ID and amount, to post or void as its ledger says. One the scheme does not have in flight is announced with a message the scheme makes again from its ID.

## Incoming settlements

A `settlement` message is taken from any authenticated node we hold a settlement channel with, peered or not, so a final settlement that arrives after the peering ended is still credited ([below](#settlement-channels)). `settlementMessage` needs a peering.

- The node checks that it can book the settlement before the scheme sees it, so the scheme's balance never moves without the books: the scheme must be active, the sender must have accounts on its ledger, and the books must take Dr `assets/settlement`, Cr `assets/interledger/<sender>` for the amount. Otherwise the reply is 400 "the settlement cannot be booked", and nothing changes.
- The scheme then handles it. A scheme that refuses it (for example data that is not a transaction hash) gets the same 400, and nothing changes. The stub books the amount stated; the XRPL scheme books nothing on the message, since its ledger says what arrived ([XRPL](xrpl.md#settlement-messages)).
- A settlement the scheme sees on its ledger from a peer without accounts there cannot be from a peer: it is booked as the owner's deposit.

## Settlement channels

The node keeps a settlement channel with every node it has ever peered with. A channel outlives its peering and the node's entry in the node table: it keeps the node's public key and URL (the node's current link state's URL is preferred). A new peering with the node uses its channel again.

**At most 512 channels, never closed.** A node never closes a peer's accounts or its settlement channel, so it keeps at most 512 channels, one per node it has ever peered with. Once all 512 are taken, peering with a new node is refused for capacity: a request from it gets reason 4 (capacity), the node does not ask it, and an acceptance that comes back once there is no room counts as a failed attempt. Nodes it holds a channel with can still peer. This is a known limit ([roadmap](../roadmap.md#not-built-yet), [D12](../decisions/D12-peering-lifecycle-and-liveness.md)).

| Kept | Meaning |
|---|---|
| `settled_in` | the highest settlement sequence applied from the node |
| `settled_out` | the last sequence we used |
| outbox: sequence, scheme, amount, data, attempts | our settlement message on its way, if any: always the last one |
| final: scheme, scheme state | a settlement of everything owed, due since a peering ended |

Channels persist in `settlement_channels`, written in the batch that changes them.

**Delivery is exactly once.**
- The `settlement` message carries a `sequence` per sender and recipient, increasing and never reset: it belongs to the pair of nodes, not to the peering.
- The sender sends one at a time, in order. It is sent again until a 200 arrives, with a backoff from 1 s doubling up to 1 h, and is never dropped; it survives restarts.
- The receiver keeps the highest sequence it applied from each sender. It acknowledges anything at or below it without applying it again, applies the next one (recording the sequence in the batch that books it), and refuses any other with 400 "settlement out of order". A sender it holds no channel with gets 409.

## The stub scheme

Test money without an underlying ledger, for the test realm only.

- Its "underlying ledger" is a number, which the checkpoint keeps across restarts. Its first activation starts at 100 USD (100,000,000,000 units), booked as the owner's deposit.
- It takes no config: empty or `{}`.
- Its peering info, request and response are empty, and so is a peer's scheme state; it accepts every request. It speaks version 1, moves any whole unit, and is always `Synced`.
- **Settling:** at most 64 settlements prepared and not executed. A settlement's ID is 16 random bytes in lowercase hex; its message is empty. Executing it takes the amount off the balance and finalizes it at once, or cancels it if the balance is short.
- **After a restart,** a settlement still pending was never executed (it executes right after its transfer is on disk, and its balance is checkpointed with the result), so it is cancelled.
- **Incoming:** a peer's settlement is believed as stated, and its amount added (ACC-23). Anyone peered over the stub can mint credit, which is why it only ever holds test money.
- **Deposits:** the owner may add any non-zero amount. The books must take it before the stub's balance moves.

## Timers and limits

All values are policies of the node, set in its code; the values in brackets are the dev values, which tests and simulations use, and which `DASSIE_DEV_TIMINGS` switches on in the daemon ([Node](node.md)), except where the row says otherwise.

| What | Value |
|---|---|
| Settlement check: ask peers, grow credit, final settlements, deferred announcements | 4 s (1 s) |
| Settlement requests to a peer that did not settle since | at most one a minute (5 s in tests and simulations; `DASSIE_DEV_TIMINGS` leaves it at a minute) |
| Settlement message retries | 1 s doubling to 1 h, never dropped (1 s to 10 s) |
| Backoff after a cancelled settlement | 1 s doubling to 10 min |
| Warning that a scheme cannot settle now | at most every 10 min |
| Ledger connection retries | 1 s doubling to 60 s |
| Fees read per ledger per check | 4096 |
| Settlement channels | 512, never closed; then no peering with a new node |
| Default credit, senior credit | 1.1 units, 0 |
| Growth share, credit cap | 10% of fees, 100 P |

## Decisions

- [D6](../decisions/D06-owner-ledger-and-multi-currency-accounts.md): the default trust line becomes the junior's starting credit.
- [D8](../decisions/D08-real-money.md): only test-realm ledgers activate.
- [D12](../decisions/D12-peering-lifecycle-and-liveness.md): settling at a peering's end; channels kept after it, and the limit of 512.
- [D19](../decisions/D19-each-node-keeps-its-own-books.md): each node decides from its own books, and the bound that holds whatever a peer does.
- [D20](../decisions/D20-trust-follows-what-a-peer-earned-us.md): junior and senior, starting credit, growth from fees.
- [D23](../decisions/D23-each-node-books-its-own-fulfills.md): the creditor asks, the debtor decides how much.
- [D24](../decisions/D24-money-in-flight-with-a-peer-stays-within-a-few-packets.md), [D25](../decisions/D25-a-links-thresholds-are-counted-in-its-largest-packet.md): thresholds in the largest packet, and the least credit.

## Original

- [5 §4](../original/05-accounting-and-settlement.md): the original's settlement framework, module interface, host methods and debtor-initiated trigger (a midpoint × 1.1 every 4 s).
- [5 §5](../original/05-accounting-and-settlement.md): its peer-protocol touch points.
- [5 §6.1](../original/05-accounting-and-settlement.md): its stub module.
- [5 §10](../original/05-accounting-and-settlement.md): the gaps ACC-10 to ACC-26.
