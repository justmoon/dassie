# Ledger

The node's internal double-entry ledger: accounts per underlying ledger, pending and posted transfers with limits, the owner's funds as a group of accounts, and the journal that makes every posted transfer durable.

**Paths:** `crates/dassie-ledger/`, `crates/dassie-node/src/store/ledger.rs`, `crates/dassie-node/src/engine/core/ledgers.rs`
**Read with:** [Connector](connector.md), [Settlement](settlement.md), [XRPL](xrpl.md), [Payments](payments.md), [Node](node.md)

The ledger is sans-IO and bounded ([engineering](../engineering.md)): callers pass the time in and persist what it queues in its journal before they act on the result.

## Ledgers and amounts

A ledger is `<settlement family>+<currency>`, and the list is closed (`ledger_id.rs`): a new ledger comes with the settlement scheme that serves it. `xrpl-xrp+xrp` and `xrpl-rlusd+usd` exist so that the live ledgers have names, and are refused ([XRPL](xrpl.md#real-money)). A ledger's family is its slug, its scheme's ID, which names its receiver's and payer's addresses and its receiver's seed path, so no two ledgers share one; its wallet address names it by its asset instead, unique within its realm ([scheme-names](../decisions/scheme-names.md)): each dollar token's is a dollar ledger of its own beside its network's XRP ledger, DSUSD's `xrpl-testnet-dsusd+usd` and RLUSD's `xrpl-rlusd+usd` ([XRPL](xrpl.md#the-token-scheme)). A ledger stays in the list after its scheme is disabled, as test RLUSD's `xrpl-testnet-rlusd+usd` does, so the books of a node that ran it keep their name ([XRPL](xrpl.md#rlusd)). A ledger that is renamed is renamed in the database by a migration, with every account, journal row, scheme, peering and payment stored under it, so its balances stay. Its slug moves with it, and so does every address and seed path the slug names: its receiver's and payer's addresses and the receiver's secret change, so credentials the receiver issued before no longer pay, and a payment that is sending starts a new connection. A wallet address that is renamed moves the same way, and the grants and payments that name it follow, with new revisions so that live views see the change. Two tokens of one currency are two ledgers, never one: each has its own issuer, the owner's counterparty ([issued-assets](../decisions/issued-assets.md)).

Every amount is an unsigned integer at scale 9 for every asset, and ILP packet amounts are applied to the books in these units without conversion: ILDCP tells each endpoint scale 9. So a ledger's scale is the books' choice, not its underlying ledger's; the XRPL scheme converts drops at its edge.

## Chart of accounts

The account kinds, their path grammar and their limits are in `path.rs`. What the code does not say is how they fit:

- **A peer** has two accounts on its scheme's ledger. `assets/interledger/<peer>` carries both directions of debt, so its balance has either sign. `contra/trust/<peer>` holds the credit we extend: a `trust` transfer debits it and credits the interledger account, and the interledger account's limit (debits ≤ credits) then lets the peer run up debt to us up to that credit and no further. Raising the credit is another `trust` transfer ([Settlement](settlement.md#credit)); opening a peer's accounts again posts nothing. A downlink's open only once value first moves with it; opened for a packet the ledger then refuses, they are taken back as if never opened, which only the accounts opened last, with nothing booked on them, can be ([lazy-downlink-accounts](../decisions/lazy-downlink-accounts.md)).
- **`assets/settlement`** stands for the holding the ledger's scheme settles in. It is not a second record of the holding, which only the ledger says ([Settlement](settlement.md#external-assets)): it is kept in step with it, every change of the holding booked and any remainder reconciled through `equity/suspense`, since the owner's spending limit and the check before each settlement read it ([own-books](../decisions/own-books.md)).
- **The owner's accounts** (`equity/…`, `revenue/…`, `expenses/…`) form one group per ledger ([below](#limits-and-groups)).

## Limits and groups

A debits-≤-credits limit counts the account's posted and pending debits against its posted credits only: money in flight out counts, money in flight in does not (a credits-≤-debits limit, as on `contra/trust`, mirrors it). A pending transfer holds its place, so posting it checks nothing again.

**The owner's group.** On each ledger, `equity/owner` is the root of a group of every owner account: its endpoints' sub-accounts, `equity/suspense`, the fee and FX accounts and `expenses/losses`. The limit (debits ≤ credits) is on the group's total; no member has one of its own. So:

- The owner's funds on a ledger are everything the node holds on it beyond what it owes peers. A loss, a ledger fee or a reconciliation that finds less money is the owner's, and the owner cannot spend money the node does not have.
- A sub-account may go negative on its own; it attributes spending, and holds no allocation.
- A transfer between two members of one group needs no free funds, since it does not change the total.
- Peers' accounts and `assets/settlement` are in no group.

## The owner's accounts

Each ledger has one pool of owner funds, `equity/owner`. Every endpoint that spends for the owner gets a sub-account under it: one per BTP token, and one for the payer ([Payments](payments.md)). The node's STREAM receivers credit `equity/owner` itself.

**The owner's own endpoints never convert.** A packet from a BTP token, the payer or a receiver goes out only on its own ledger, and one whose first hop is on another ledger is refused ([Connector](connector.md#routing)). The owner pays from the ledger the route starts on, and the first hop that charges for a conversion converts; that keeps the FX accounts inside the owner's group coherent ([owner-ledger](../decisions/owner-ledger.md)).

- A BTP client paying the node's own receiver moves money within the group: the total is unchanged, and the journal still has a transfer per packet.
- A packet from a peer on another ledger into the node's receiver converts at the node's rates: the receiver's account shows what arrived, `expenses/fx` takes it back out on that ledger, and `revenue/fx` keeps it on the ledger it came from.
- A conversion between two peers pays out of the owner's funds on the outgoing ledger, so it needs them, and is priced by what the owner holds on each ledger ([Connector](connector.md#inventory-pricing)).
- Per-token spending caps are limits, not allocations, so they may add up to more than the funds ([Connector](connector.md#spending-caps)).

## Transfers

A transfer is pending, then posted or voided; or it is posted at once. Linked transfers are created all or none. A sequence of posted transfers can be checked whole before any is created, each as it would find the accounts with the ones before it booked, so a caller books all of it or none: a scheme's effects ([Settlement](settlement.md#the-scheme-interface)). Every refusal is a value, never a panic, since the input may come from a peer; the [connector](connector.md#booking-a-packet) maps them to ILP errors.

Each transfer carries a reference saying what it is for (`reference.rs`). A packet's reference is its **trace ID**, the boot epoch (persisted, incremented at every start) and a counter within the boot: it stays unique across restarts, and lines the journal up with the connector's [packet trace](connector.md#the-packet-trace).

## What the ledger refuses and what it records

- **Packets and settlements are refused** when they would break a limit.
- **Reports are recorded** even when they take the owner's group below zero: deposits, withdrawals, reconciliations and ledger fees say what already happened on the underlying ledger, and refusing them would only make the books wrong. Spending is then refused until the group recovers. So limits are not invariants: reports may break them.
- **Scheme balances are reconciled by difference.** A settlement scheme reports its on-ledger balance as a checkpoint, written in the same transaction as the transfers it explains, with its external asset's state, and activation books only the difference to the last checkpoint, through `equity/suspense` ([Settlement](settlement.md#checkpoints-and-activation)). So a restart never books a balance twice.

## The journal

Every engine step that changes the ledger queues journal entries. The engine persists all of one step's entries, with everything else the step changed, in one SQLite transaction, and acts on the step's outcome only once that is on disk ([Node](node.md#the-engine)). What the journal says is what happened.

- **Posted transfers** are journaled, with the accounts' cached totals in the same transaction.
- **Pending settlement transfers** are journaled too, and then their posting or voiding: the money may already be on its way when the node restarts, and the scheme reconciles them at activation ([Settlement](settlement.md#outgoing-settlements)). So are the holds of the owner's payments out of a scheme's account, for the same reason ([Settlement](settlement.md#the-scheme-interface)).
- **Pending packet transfers** are not: packets in flight cannot survive a restart, so a restarted ledger has none, and a voided one never touched a posted balance. Transfer IDs in the journal therefore have gaps.
- Rows are ordered by a write sequence, not by transfer ID, since a packet's transfer is written when it posts. Readers page by that sequence and see every change once.

At a start the ledger is rebuilt from the journal and refused if the cached totals, the account order or the pending settlements do not add up (`restore.rs`).

## Fulfills booked as final

A packet is posted when it is fulfilled, and stays posted whatever the upstream peer does with the Fulfill: each node books its own Fulfills, and what its receiver took counts at once ([own-fulfills](../decisions/own-fulfills.md)). A peer that does not count one owes us more than its books say; it can still owe us no more than the credit we extend.

Each node keeps its own books and enforces its own limits from them. It never compares them with a peer's, and does not measure drift ([own-books](../decisions/own-books.md)).

Nothing is booked when an upstream refuses a Fulfill. `expenses/losses` holds only `loss` rows: what a peering over a settlement hub cost beyond its hold ([Settlement hubs](hubs.md#packets-promises-and-claims)).

## Invariants

Asserted after every step (`ledger/invariants.rs`): every ledger balances, posted and pending alike, and every cached counter and group total is the sum of what it counts. The property tests also check that no transfer breaks a limit and that balances equal the journal.

## Decisions

- [database-continuity](../decisions/database-continuity.md): no carried-over totals from v0 databases.
- [owner-ledger](../decisions/owner-ledger.md): an owner account per ledger, the owner's group and its limit, sub-accounts instead of allocations.
- [real-money](../decisions/real-money.md): the live ledgers, `xrpl-xrp+xrp` and `xrpl-rlusd+usd`, stay refused.
- [tigerstyle](../decisions/tigerstyle.md): sans-IO, bounded, asserted.
- [own-books](../decisions/own-books.md): each node keeps its own books.
- [own-fulfills](../decisions/own-fulfills.md): Fulfills booked as final.
