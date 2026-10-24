# Settlement

How a node moves money with its peers on an underlying ledger, and how its books learn about it: the settlement scheme interface, the external assets schemes settle through, the credit a node extends to each peer and how that grows, when it asks a peer to settle and how much it settles when asked, and how settlement messages are delivered exactly once.

**Paths:** `crates/dassie-settlement/`, `crates/dassie-node/src/engine/core/settlement.rs`, `crates/dassie-node/src/engine/core/growth.rs`, `crates/dassie-node/src/engine/core/ledgers.rs`, `crates/dassie-node/src/engine/core/schemes.rs`, `crates/dassie-node/src/engine/links.rs`, `crates/dassie-peer/src/node/settle.rs`, `crates/dassie-peer/src/channel.rs`
**Read with:** [Ledger](ledger.md), [XRPL](xrpl.md), [Network](network.md), [Peer protocol](peer-protocol.md), [Connector](connector.md)

Each node keeps its own books and decides from them alone: whether it takes a settlement, when it asks a peer to settle, how much it settles, and when it ends a peering. What a peer says is an input to these decisions, never trusted past the node's limits. However a peer behaves, and however far its books are from ours, **it costs us at most the credit we extend, plus what it has in flight** ([Connector](connector.md#money-in-flight-with-a-peer)). The node does not know the credit a peer extends to it, does not model the peer's books, and does not measure drift ([own-books](../decisions/own-books.md)).

## Schemes and ledgers

A settlement scheme serves one internal ledger, and a ledger has at most one active scheme. The node's registry of schemes and of the [external assets](#external-assets) they settle through is in `engine/core/schemes.rs`. Only ledgers of the test realm activate ([XRPL](xrpl.md#real-money)).

Activating a scheme, when the owner adds it or at start, makes the external asset it settles through if that is not active yet, then the scheme, each from its config and its last [checkpoint](#checkpoints-and-activation), opens the ledger's common accounts, books the activation entry, and gives the ledger its STREAM receiver and payer ([Connector](connector.md#local-endpoints)). Removing a scheme is refused while anything still depends on it: tokens, peerings, or outgoing payments on its ledger. Its accounts and checkpoint stay, so adding it again picks up where it was, including the settlements still pending. An asset no scheme settles through any more is let go, its connection closed and its checkpoint kept.

## The scheme interface

A scheme is sans-IO: its methods return effects (`Effect` in `scheme.rs`) for the node to book or carry out, rather than calling back into the node. The contract the node relies on:

- **Prepare submits nothing.** `prepare_settlement` returns an ID unique within the scheme, an amount of at most what was asked in whole units of its ledger, and the message for the peer. The node books it before anything leaves ([below](#outgoing-settlements)).
- **Its asset is handed to it** whenever it may sign, submit or look something up (preparing, executing, discarding, reconciling), and after the asset heard from its ledger (`on_asset`). It keeps nothing of the asset but what the asset told it.
- **Execute only once the pending transfer is on disk,** and report `Finalize` or `Cancel`, now or later. After a restart, `reconcile` gets the settlements still pending and reports the same for each.
- **The message can be made again** from the settlement's ID, so a restart that lost it can still announce the settlement.
- **Peering keeps nothing until it is made.** Creating, accepting and finalizing a peering request only check the other side's data; the scheme keeps what it needs of a peer when the node tells it the peering is made (`on_peered`), after the peer's accounts are open. So attempts that fail cost the scheme no room.
- **Peering waits for the first sync.** A scheme's first sync books what its account holds as the owner's deposit; only after it can the scheme tell a peer's settlement from a deposit (`has_synced`). Until then the node neither offers, asks for nor accepts a peering on the scheme: it leaves the scheme out of its link state, picks no candidate on it, and refuses requests on it as for a scheme not active here ([Network](network.md#asking-and-answering), [synced-before-peering](../decisions/synced-before-peering.md)). The first sync is kept with the scheme's state, so a restart with the ledger out of reach peers as before, and peerings already made stay. A scheme without a ledger to follow has synced from the start.
- **Reports** (deposits, withdrawals, ledger fees, reconciliations) are recorded even past the owner's limit ([Ledger](ledger.md#what-the-ledger-refuses-and-what-it-records)). A reconciliation names its cause, which changes only how it is reported: a change of the ledger's reserve is expected and logged as information, any other difference as a warning. An incoming settlement is booked Dr `assets/settlement`, Cr `assets/interledger/<peer>`.
- **`Unavailable`** means not now (its ledger not seen, no funds to spare, too much in flight, an issuer's freeze). It is asked again at the next check as if nothing happened.
- **Whether peers can pay us** (`can_receive`): a scheme whose ledger stops what peers pay us, such as RLUSD while the issuer froze our line deep or charges a transfer fee, says so, and stays synced meanwhile ([below](#when-peers-cannot-pay-us)).
- **The settlement check** (`on_check`) lets a scheme read what it needs of its ledger at every check, through its asset, such as an issuer's account ([XRPL](xrpl.md#rlusd)); what it learns comes as news.
- **The scheme's reserve** is what its ledger holds back from the balance: never booked, so never spendable.
- **Its status,** as the owner sees it, is its asset's until the asset is synced, then its own.
- **Promises per packet** (`commitments.rs`): a scheme whose peerings carry them, as a settlement hub's do, signs a commitment with each Prepare, checks one with each Prepare it takes, and signs a claim into each reply to a Fulfill; as payer, it decides from its ledger the packets whose Reject upstream waits for it ([Connector](connector.md#responses-and-expiry)). A scheme without them settles as every other.
- **The owner's payments** (`pay_owner`): a scheme may pay the owner's funds out of its account to an address its ledger pays, one at a time, as the node moves them between two of its ledgers ([Settlement hubs](hubs.md#in-the-node)). The payment has an ID, and the scheme keeps with its state the one on its way and what became of the last, so it answers for it across restarts: on its way, applied, failed (its ledger recorded it as failed: it moved nothing but its fee, and the scheme says whether it was too little to create the account it paid), cancelled (it never reached its ledger, which can never apply it now: it moved nothing, not even a fee), or unknown, when the scheme never read its outcome and only where the money went can tell. **It is held against the owner's funds while it is on its way:** the node books it as a pending transfer out of the owner's group on the payment's ledger (Dr `equity/owner`, Cr `assets/settlement`, an `owner_payment` keyed by the scheme and the payment's ID) in the batch that keeps the payment, with a second hold for the most its fees may cost, and the books refuse them, before anything is signed, when the owner's funds do not cover both. So conversions, payer and BTP tokens and another scheme's gas see the owner's funds less it from the moment it leaves, and the owner's funds are never below zero when it lands. The scheme reports once it is no longer on its way (`Effect::OwnerPaid`): applied, the hold is posted; failed, cancelled, or of an outcome the scheme never read (its ledger's balance then said what it did), it is voided. The fee hold is voided either way, and the fee the payment cost booked. The hold is journaled while pending, as a settlement is ([Ledger](ledger.md#the-journal)); at a start, and when a scheme is activated again, each one the scheme no longer has on its way is posted or voided by what the scheme says became of it. The ledger the money reaches books it as the owner's deposit when it sees it.
- **What a peering over a hub costs beyond its hold** is a loss on the peer (`Effect::PeerLoss`): Dr `expenses/losses`, Cr the account the money left, as its cause says, recorded whatever the owner's funds ([Settlement hubs](hubs.md#packets-promises-and-claims)).

**A scheme's effects are booked all or none.** Whatever hands them over (the scheme's ledger or timers, a peer's settlement or message, executing or reconciling a settlement, the owner's stub deposit or request to a faucet), the books first check every transfer the effects make, in order, each as it would find the accounts with the ones before it booked (`Ledger::validate_posted`). If they refuse one, none of the effects is carried out, and the scheme's checkpoint does not move, so it still explains exactly what the books hold. Nor does anything its asset asked of its ledger with that input go out once no scheme on the asset runs: no message, no new connection and no request to a faucet. Posting or voiding a settlement of ours is not checked: it raises no total and tightens no limit, so it cannot make a later transfer fail.

**A scheme that fails stops alone.** Effects the books refuse stop the scheme, shown as failed, rather than the node, under the same policy wherever they came from. A stopped scheme is fed nothing more (nothing its asset learns, no settlements or messages from peers, executions, reconciliations, deposits or requests to a faucet) and its checkpoint is no longer written, so a restart reads from that checkpoint again what the books refused, and books it then or stops again, never twice. Once no scheme on an asset runs, the asset's checkpoint is not written either, so the two stay as they were written together, and its connection is closed, so the driver does not make it again for ever; so is the connection of an asset that stopped by itself, with every scheme on it. A scheme activated on the asset, or a restart, connects anew. Nor is a stopped scheme peered on until the restart: the node leaves it out of its link state, asks for no peering on it and refuses requests on it, as before its first sync ([Network](network.md#asking-and-answering)). Peerings already made on it stay, and a settlement a peer announces on it is refused (400) until the restart; the peer sends a channel's settlements one at a time, again until one is taken, so the rest of that channel waits behind it until then ([Peer protocol](peer-protocol.md#message-set)).

## External assets

An external asset is something the node holds outside its books, of a type: an account on the XRP Ledger testnet today ([XRPL](xrpl.md#the-account)), later others. A scheme names the asset it settles through, and several schemes may share one ([external-assets](../decisions/external-assets.md)). The types and IDs are in `dassie-settlement` (`asset.rs`); a type and its number name an asset.

- **It owns what its schemes would otherwise race for:** its keys, its connection to its ledger, what it signs and the order it signs in, and what it holds.
- **What it learns, every scheme on it is told,** each booking all or none, before the asset forgets it. Each scheme keeps its own place in what it learns, written with what it books, so a scheme that stopped reads again what it missed, and the others are not held back. What concerns a scheme's own transactions while it is not on the asset (stopped or removed), or what the asset told it in the input whose bookings the books refused, the asset keeps for it until it is back and has read it, or has told it what it could not read again, which the scheme then looks up and books as one it never read, so its settlements are booked once, neither twice nor never; what it tells again for that scheme alone, the others booked when they were first told. An asset written off meanwhile, everything it held gone (a testnet reset), is written off by the scheme too, once back and before it follows the asset again: the asset counts its write-offs, and each scheme keeps the count it last saw with its state, so a place on a chain gone never holds the asset back.
- **A scheme that joins an asset that has heard from its ledger** has the node connect the asset anew, so it reads again from the earliest place of its schemes, the new one's included, and tells them all it has synced; one whose connection was closed is connected again.
- **The ledger is authoritative for what it holds.** The node shows each holding as the ledger last said (`asset_views`, which the owner sees on the wallet's External Assets page, [Wallet](wallet.md#pages)), never as the books add it up. The books keep `assets/settlement` only to book against, in step with the holding the ledger's scheme settles in ([Ledger](ledger.md#chart-of-accounts)).
- **It is active while a scheme settles through it:** made with the first, from its checkpoint, and let go with the last.
- **A holding is on the books only on the ledger whose scheme settles in it.** Another scheme on the asset may spend it as gas, such as the XRP an RLUSD scheme pays fees and reserves with: where a scheme on the asset settles in that holding, the gas is booked on that scheme's ledger, as a ledger fee or a reconciliation, and spent only within the owner's funds there, which the node tells the asset before each call that may sign (`owner_funds`); where none does, the gas is off the books, and the asset only keeps it above its ledger's reserve ([external-assets](../decisions/external-assets.md)). A scheme the books stopped still holds what it settles in on its books, so until it runs again after a restart no other scheme spends that holding; one the owner removed holds it no longer, as after a restart, when the asset cannot know it was there. The node tells the asset which it is (`detach`).

### Assets that follow a ledger

An asset with a ledger to follow names a WebSocket URL; it says what to send and the node feeds it what comes back, then tells its schemes what it learnt and carries out what it asks. The driver (`engine/links.rs`) runs one connection per asset, however many schemes settle through it, bound to an epoch, so a message from a connection the engine has replaced is dropped. A connection whose queue overflows drops messages, and the asset, hearing nothing, connects anew. Names resolve under the node's outbound rules: public addresses only, unless private ones are allowed (dev and tests).

### Assets that talk to several endpoints

An asset whose ledger is several parties it talks to apart, a settlement hub's validators, names their endpoints instead (`endpoints`), in an order that does not change while it runs. It sends to one by its index, and the node feeds it what came back from that endpoint under the same index, so the asset may take who sent it as said: the driver authenticates each endpoint. The asset asks again on its own timers, so a message lost costs nothing but time. The node's driver for them is not built yet ([Settlement hubs](hubs.md#the-members-side)).

## Checkpoints and activation

The books keep, for each scheme, a **checkpoint**: the balance they record for it and its state; and for each external asset, its state. Each is written in the batch it explains, and a scheme's state and its asset's are also written with the pending transfer of each settlement it prepared, so what the asset signed is kept.

Activation books only the difference between the scheme's balance and the last checkpoint (`activation.rs`): with no checkpoint, the whole balance is the owner's first deposit; after that, a gain or a loss goes through `equity/suspense` as a reconciliation. So a restart never books the same money twice, and money that moved while the node was down is the owner's.

## Credit

**Roles.** The **junior** is the node whose peering request was accepted, the **senior** the node that accepted it; when requests cross, the lower node ID's request wins, so the lower ID is the junior ([Network](network.md)). The node that asked to peer takes the risk at first ([earned-trust](../decisions/earned-trust.md)). A periphery node always asks, so it is always its uplinks' junior ([Network](network.md#periphery-nodes)).

**Starting credit.** The junior extends the senior the default credit line; the senior extends the junior nothing. Asked to settle at once, the junior then pays in advance, and payments to the junior flow at once, within the credit it extends. Both are policies of the node (`Policies` in `crates/dassie-node/src/node/mod.rs`), not owner settings, and one value for every ledger ([roadmap](../roadmap.md#not-built-yet)).

- **Peering again** keeps the trust account and its credit. A junior still extends at least the default line: a node that was the senior before, or that accepted a request only one side recorded, raises the account to it. Not when it once ended a peering with that node for want of room: then it keeps the lower credit, so a peer that never settled gets no new line by being asked again ([ended-for-room](../decisions/ended-for-room.md)).
- **Nothing is announced.** The credit a node extends is its own business: the peer protocol's `credit` fields are sent as 0 and ignored ([Peer protocol](peer-protocol.md)). More credit only means the creditor asks later.

**The least credit.** Asked at the asker's trigger, a debtor settles only if its own books leave it its trigger plus twice the least settlement ([below](#settling-on-request)). So a link survives drift of up to its credit less two triggers and twice the least settlement, and the node refuses to start with a default credit (or a senior credit other than 0) below that plus one crash's drift at the cap on money in flight (`policy::least_credit`). A link cut one way drifts no more than a crash: a node takes no new Prepare from a peer its messages surely do not reach, so its Fulfills that cannot arrive are only those in flight when the cut began ([unreachable-peers](../decisions/unreachable-peers.md)).

```
least_credit = 2 · trigger + 2 · least settlement + max_in_flight = 3P + 2P + 2.5P = 7.5 P
```

The default clears it with room for about two crashes.

**Growth.** Each side raises the credit it extends to the other by k = 10% of what it earned on packets that came in or went out through that peer (`policy::GROWTH_PPM`, `engine/core/growth.rs`).

- **What counts** is our connector's fee ([Connector](connector.md#fees-and-spread)), toward each peer of the packet on the ledger the fee was booked on. A packet's fee counts toward both peers it passed, so k must stay at most 50% (asserted) for the credit a node hands out never to exceed what it earned. A packet that converts counts toward the peer it came from only, since counting it toward the other would need our exchange rates. What the conversion itself gains or loses, the spread and the curve's move from spot, is not booked per packet and does not count: it is the net of the FX accounts, and negative when the node pays above spot for an asset it lacks ([passive-rebalancing](../decisions/passive-rebalancing.md)).
- **The cap.** A peer earns at most a hundred of the largest packets of credit. Credit above it, configured or from before, stays and never falls.
- **Measured from the books.** What a peer earned is read from the journal's fee rows, never kept in counters, and the raise is an explicit `trust` transfer. Reading resumes after the last trust transfer, so a crash can forget fees not read yet, and credits none twice.
- **Retirement is accepted.** A node that leaves can spend everything its peers trust it with and disappear; each peer loses at most k of what it earned through that node.
- Fees default to 0, so trust grows only once an owner charges for passing payments on. Until then a junior keeps settling in advance.

## Positions and thresholds

A node knows only the credit it extends to a peer, not the credit the peer extends to it, so only the creditor can tell when a peer should settle. It asks; the debtor decides how much ([own-fulfills](../decisions/own-fulfills.md)). Both rules are in `crates/dassie-settlement/src/policy.rs`.

```
position = what the peer may still owe us: the credit we extend, plus what we owe it,
           net of our settlements in flight (from assets/interledger/<peer>)

ask the peer to settle   when position ≤ trigger
when asked, settle       (position − trigger) / 2, or nothing if that is less than the least settlement
```

Every threshold of a link is counted in its [largest packet](connector.md#the-largest-packet) `P`, not in the credit, which differs on each side and grows with earnings ([thresholds-in-packets](../decisions/thresholds-in-packets.md)). So even a senior that extends nothing asks while its junior still has room.

| Threshold | In `P` | Why |
|---|---|---|
| Trigger to ask | 1.5 | Below the cap on money in flight, so a busy peer can still have a packet refused (T04) while its settlement is on its way; a trigger at the cap left too little credit for one crash's drift. |
| Least settlement | 1 | A peer that keeps asking cannot make us pay a ledger fee for every few drops. |
| Money in flight with a peer, each way | 2.5 | [Connector](connector.md#money-in-flight-with-a-peer) |
| Room below which a peer is no longer advertised, past a grace | 1 | [Network](network.md#advertised-peers) |
| Least default credit | 7.5 | [Above](#credit) |
| What a payee over a hub lets claims add before it redeems | 2 | Each redeem costs the hub's fee; what is not taken is lost if the hub fails ([Settlement hubs](hubs.md#packets-promises-and-claims)) |
| Most credit earned | 100 | [Above](#credit) |
| What a node keeps spendable in its hub, per peering over it | 4.75, at least the least settlement, at most 50 in all | A payer over a hub asked to settle funds its hold as a junior that extends the default credit (11) would: at most half the way from it down to the trigger (1.5), 4.75 ([Settlement hubs](hubs.md#in-the-node)). Kept for every peering, so no settlement asked of us waits for a deposit. At most half the ceiling, so that floor and ceiling stay far apart: ten peerings are funded fully |
| Most a node keeps in its hub, its holds and those toward it included | 100 | As much as the most credit a peer earns: a failing hub costs no more than a peer that earned all it could and left. Its own holds count, since each is the credit it extends over the hub, paid in advance. The midpoint then lies at least 25 from either bound, so a move carries many times its fees ([Settlement hubs](hubs.md#in-the-node)) |

Amounts are rounded down to whole units of the scheme's ledger; what is less than a unit stays owed. Both rules are deliberately simple, to be tuned later (expected settlement latency, fees, volume on the link).

## Asking a peer to settle

At every settlement check the node asks each peer whose position has fallen to its trigger, naming only the scheme ([Peer protocol](peer-protocol.md)). Nothing about a request is kept: a restart looks again.

- **It asks again** at the next check once its books show a settlement from that peer, not on the `settlement` message alone: on the XRP Ledger the message books nothing, so asking on it would make the peer pay twice. It also asks again at the next check after a 409, since our acceptance may not have reached the peer yet, and after a request that surely never reached the peer (not delivered, [Peer protocol](peer-protocol.md#outbound-requests)), which costs the peer nothing and must not use up its grace before it is withdrawn ([Network](network.md#advertised-peers)). Otherwise it asks at most once a minute, so a peer that does not answer, refuses (400, 401) or is overloaded (503) is not flooded.
- **Only while its scheme is synced.** Until then, what the peer pays is not booked, and the node cannot tell whether it paid. An honest peer settles only when asked, and no peering is made before the scheme's first sync ([above](#the-scheme-interface)), so what a peer settles, asked routinely or by a node ending the peering, reaches the ledger after that sync booked what the account held as the owner's deposit, and is booked as the peer's settlement.
- **Only while the peer's settlement can reach us** ([below](#when-peers-cannot-pay-us)).
- With no credit extended, it asks at once, and asks again once what the peer paid in advance is down to the trigger.
- **A peering the node is ending** is asked once more, by the peer node itself, before the peer is told, whatever its position; then no more ([Network](network.md#termination)).

### When peers cannot pay us

A scheme's ledger can stop what peers pay us while the scheme still sees it: an issuer that froze our line deep, or froze its token for every holder, lets no peer's payment reach the line, and a peer's settlement would only fail and cost it a fee; one that charges a transfer fee leaves every peer on the scheme unable to settle, as none pays the fee ([issued-assets](../decisions/issued-assets.md)). The scheme says so (`can_receive`) for as long as it lasts, which may be for good, and stays synced. Meanwhile:

- **The node asks no peer on it to settle,** as while the scheme does not see its ledger.
- **A peer's time short of room withdraws it but never ends it** ([Network](network.md#advertised-peers)). It grows as usual, so a peer short of room past the grace is withdrawn, since no settlement can restore its room while it lasts; but it does not count toward ending the peering for room, so no peer is ended for what it could not pay ([ended-for-room](../decisions/ended-for-room.md)). The grace stays, so our own packets toward a peer that dips below its room only briefly can restore it before it is withdrawn.
- **The link state leaves the scheme out,** so no new peering is made on it ([Network](network.md#asking-and-answering)). This is a rule of its own: a scheme that only lost its ledger stays in the link state once it has synced.

Peerings already made stay, and our own settlements on the scheme go on while the scheme can make them.

## Settling on request

A request says that the debtor stands near the asker's trigger. The debtor settles half the way from where it stands to its own trigger, which lands near the middle of the two.

- **It never pays itself into asking back:** settling lowers its own position, however far two nodes' books have drifted apart.
- **One settlement at a time.** It settles nothing while a settlement to that peer is on its way (prepared, executing, pending in the ledger, or announced and not yet acknowledged): that one is the answer. Nor while a cancelled settlement holds it off ([below](#outgoing-settlements)).
- **The answer says what is on its way:** the settlement it makes now, or the one already under way, or nothing. So an asker can tell a settlement still coming from none ([ending-a-peering](../decisions/ending-a-peering.md)).
- **A lying asker gets at most the credit the debtor extends to it.**
- **A node never settles on its own,** not even at a peering's end.

## Settling at a peering's end

Nothing is settled for a peering's end ([ending-a-peering](../decisions/ending-a-peering.md)). From the moment a node knows a peering is ending, by deciding it or by being told, it settles nothing more with that peer: neither side has a reason to pay at the end. The node that ends it collects first: it waits for what the peer had on its way when last asked, so no settlement crosses the end between honest nodes ([Network](network.md#termination)). A ban collects nothing: the node tells the peer at once. What each side's books show stays on the peer's accounts, and a later peering with the node settles it as usual. A settlement already under way still arrives, and is credited once ([below](#incoming-settlements)).

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
- **From a channel, not a peering.** A `settlement` is taken from any authenticated node we hold a [settlement channel](#settlement-channels) with, open or closed, so a settlement that arrives after the peering ended, or after its channel closed, is still credited.
- A settlement the scheme sees on its ledger from a node without accounts there cannot be from a peer: it is booked as the owner's deposit.

## Settlement channels

The node keeps a settlement channel with every node it has ever peered with (`crates/dassie-peer/src/channel.rs`). A channel belongs to the pair of nodes, not to a peering: it outlives the peering and the node's entry in the node table, and a new peering uses it again. It holds the sequences and our message on its way.

**Delivery is exactly once.** Each `settlement` message carries a sequence per sender and recipient that never resets. The sender sends one at a time, in order, again until it is acknowledged, across restarts, never dropping it; one that got no answer goes again as soon as the node answers us at all ([Peer protocol](peer-protocol.md#message-set)). The receiver applies only the next sequence, recording it in the batch that books it, acknowledges anything at or below the last one without applying it again, and refuses anything else. So a copy from an old peering is never applied twice, and none is lost.

**Open channels are bounded:** with core nodes under one bound, and with downlinks under their own ([Network](network.md#downlinks)). Once all of a kind are taken, peering with a new node of that kind is refused for capacity; nodes it holds an open channel with can still peer. So is peering with a node whose accounts would leave the ledger less room than the node's own accounts may need, since peers' accounts stay when their channels close ([closing-channels](../decisions/closing-channels.md)).

**A channel left idle closes, to disk** ([closing-channels](../decisions/closing-channels.md)). Once it has been idle for as long as a silent peer is kept, with a node we neither peer with nor ask, with nothing on its way and nothing of the node pending in the books, every scheme the node has accounts with seeing its ledger, and no scheme still needing it (`keeps_channel`, as a settlement hub does for a hold with funds), it leaves memory and frees its place. Its balance does not matter: the node's accounts stay with what they show, for a later peering to settle. Its row stays on disk with its sequences and the mark of an end for room, so delivery stays exactly once:

- **A peering opens it again where it was,** so our next message follows the last one the peer applied.
- **A settlement over it closed** is read against the sequence that stayed: applied once, and acknowledged again as a copy. The channel stays closed.
- **The schemes forget** what they kept of the node for its settlements (`on_closed`): a ledger payment from it after the close is the owner's deposit.

## Settlement hubs

A peering over a [settlement hub](hubs.md#the-members-side) settles by funding the payer's hold toward the payee, which is all the credit the payee extends: it extends nothing beyond it, and the credit does not grow with earnings ([settlement-hubs](../decisions/settlement-hubs.md)). Settling stays [on request](#settling-on-request), so the node's rules for settling are the same:

- **Funding a hold is a settlement.** The payer books it as its outgoing settlement, finalized once agreed statements show the hold grew. The payee books it as an incoming settlement only then, from what the hub says, as the XRPL scheme books what its ledger reports: the settlement message books nothing.
- **The payer's hold is its credit.** The credit the payer extends to the payee is its hold toward it and its settlements to it on their way ([Settlement hubs](hubs.md#in-the-node)): a `trust` transfer raises it before the funding's pending transfer is booked, so the books take a settlement the payer does not owe, and one back lowers it, Dr `assets/interledger/<peer>`, Cr `contra/trust/<peer>`, as the payee takes from the hold, never past what the peer may still owe us. Its settlement thus moves the payer's money into the hold without moving its room for the payee, and the payee's room for the payer grows by it.
- **What a payee takes moves nothing in its books.** The holds toward it from its peers are part of what it holds on the hub's ledger, so a redeem or a claim moves money within `assets/settlement`, and books only its fee.
- **A hold's reserve is the ledger's,** never booked, as the XRP Ledger's reserves are; opening a hold reconciles it out of the books.

## The stub scheme

Test money without an underlying ledger (`crates/dassie-settlement/src/stub.rs`). Its "ledger" is a number the checkpoint keeps; its first activation books a starting balance as the owner's deposit, and the owner may deposit more. It accepts every peering, and finalizes a settlement as it executes it; one still pending after a restart was never executed, so it is cancelled.

A peer's settlement is believed as stated. Anyone peered over the stub can mint credit, which is why it only ever holds test money.

**With commitments,** for tests of peerings over a settlement hub without a hub: given `{"commitments": N}`, the stub signs and takes promises per packet as a hub's member does ([Settlement hubs](hubs.md#in-the-node)), so the node sends a commitment with each Prepare to a peer on it, checks one with each Prepare from it, and carries a claim in each reply to a Fulfill. Every stub with commitments is a member of one pretend hub, so nodes on it peer over it.

- **Packets.** It holds at most N of a peer's packets in flight, and N of ours to it, and refuses past that; a commitment is the Prepare's amount and condition, and a claim is fixed; it keeps a fulfillment until the payer's claim covers it, a bounded number per peer; and a packet that waits for the hub it decides at once without a fulfillment, as a hub whose final block passed the commitment's expiry with no claim for it. What it holds of packets is in memory, as the connector's packets are, so a restart forgets both together.
- **Holds.** It emulates the holds as each side sees them: our settlement to a peer funds our hold toward it once it executes, with at most what the stub can spend, and the peer's settlement funds its hold toward us; a fulfilled packet is taken from the payer's hold as a redeem takes it, on the payer's side when it writes its claim for a Fulfill its books took, and on the payee's side when that claim arrives, or when it takes the packet as claimed on the hub. The room of a peer's hold toward us is what is left in it, less the packets of it taken and fulfilled that no claim covers yet, as a member reads it. Its state keeps the holds across restarts; a plain stub has no state, and refuses one.

## Decisions

- [owner-ledger](../decisions/owner-ledger.md): the default trust line becomes the junior's starting credit.
- [real-money](../decisions/real-money.md): only test-realm ledgers activate.
- [external-assets](../decisions/external-assets.md): what the node holds apart from how it settles, and the ledger authoritative for holdings.
- [peering-lifecycle](../decisions/peering-lifecycle.md): channels kept after a peering, and their limit.
- [closing-channels](../decisions/closing-channels.md): a channel left idle closes to disk, whatever the balance.
- [ending-a-peering](../decisions/ending-a-peering.md): nothing settled for a peering's end.
- [synced-before-peering](../decisions/synced-before-peering.md): no peering on a scheme before its first sync.
- [own-books](../decisions/own-books.md): each node decides from its own books, and the bound that holds whatever a peer does.
- [earned-trust](../decisions/earned-trust.md): junior and senior, starting credit, growth from fees.
- [ended-for-room](../decisions/ended-for-room.md): no credit back for a peer once ended for room.
- [own-fulfills](../decisions/own-fulfills.md): the creditor asks, the debtor decides how much.
- [packets-in-flight](../decisions/packets-in-flight.md), [thresholds-in-packets](../decisions/thresholds-in-packets.md): thresholds in the largest packet, and the least credit.
- [unreachable-peers](../decisions/unreachable-peers.md): a link cut one way drifts no more than a crash.
