# Ledger

The node's internal double-entry ledger: one set of accounts per underlying ledger, pending and posted transfers with limits, the owner's funds as a group of accounts, and the journal that makes every posted transfer durable.

**Paths:** `crates/dassie-ledger/`, `crates/dassie-node/src/store/ledger.rs`, `crates/dassie-node/src/engine/core/ledgers.rs`
**Read with:** [Connector](connector.md), [Settlement](settlement.md), [XRPL](xrpl.md), [Payments](payments.md), [Node](node.md)

The ledger is a sans-IO state machine. It never reads a clock: callers pass `now_ms` in, and persist what it queues in its journal before they act on the result. Every table has a configured bound, its invariants are asserted after every step, and it counts the transfers it created, posted, voided and refused for a limit ([engineering](../engineering.md)).

## Ledgers and amounts

A ledger is named `<settlement family>+<currency>`. The list is closed: a new ledger comes with the settlement scheme that serves it.

| Ledger | Asset | Realm | Served by |
|---|---|---|---|
| `stub+usd` | USD, scale 9 | test | `stub` ([Settlement](settlement.md#the-stub-scheme)) |
| `xrpl-testnet+xrp` | XRP, scale 9 | test | `xrpl-testnet` ([XRPL](xrpl.md)) |
| `xrpl+xrp` | XRP, scale 9 | live | nothing: real money is refused ([XRPL](xrpl.md#real-money)) |

- Every amount is an unsigned integer in the asset's internal scale, 9 decimal places: 1 USD is 10^9 units, and 1 XRP drop is 1000 units.
- ILP packet amounts are applied to the ledger directly in these units. ILDCP tells each endpoint its ledger's asset code and scale 9.
- No account counter passes `MAX_TOTAL` = 2^63 − 1 (SQLite stores the totals as signed 64-bit integers): about 9.2 × 10^9 units of the asset over an account's lifetime. A transfer that would pass it is refused (`Overflow`).
- Balances are derived: an account's posted credits minus posted debits can have either sign.

## Account paths

```
AccountPath = LedgerId ":" AccountName
LedgerId    = "stub+usd" | "xrpl-testnet+xrp" | "xrpl+xrp"
AccountName = "assets/settlement"
            | "assets/interledger/" Name        ; a peer, by node ID
            | "contra/trust/" Name              ; a peer, by node ID
            | "equity/owner"
            | "equity/owner/btp/" TokenId       ; a BTP token's sub-account
            | "equity/owner/local/" Name        ; a local endpoint's sub-account
            | "equity/suspense"
            | "revenue/fees" | "revenue/fx"
            | "expenses/fees" | "expenses/fx" | "expenses/losses"
Name        = 1*64( ALPHA / DIGIT / "-" / "_" )
TokenId     = "0" / ( %x31-39 *DIGIT )          ; canonical decimal u64
```

- Paths parse only in this canonical form. Anything else is refused: no `liabilities/…` (the original documented them and never had them, ACC-1), no trailing `/`, no sign or leading zeros in a token ID, no path without a ledger (the original's `unreachable` sentinel for ILDCP).
- A peer's debt to us and ours to it share `assets/interledger/<peer>`, whose balance can have either sign.
- The only local sub-account in use is `equity/owner/local/pay`, the payer's ([Payments](payments.md)).

## Chart of accounts

Every active ledger has the **common accounts**: `assets/settlement`, `equity/owner`, `equity/suspense`, `revenue/fx`, `revenue/fees`, `expenses/fx`, `expenses/fees` and `expenses/losses`, opened when its scheme activates ([Settlement](settlement.md#schemes-and-ledgers)). Peer accounts are opened when a peering forms; owner sub-accounts when their endpoint is made.

An asset or expense account grows with debits; an equity or revenue account with credits. The contra account offsets the peer's interledger account.

| Account | Own limit | Group | Debited by | Credited by |
|---|---|---|---|---|
| `assets/settlement` | none | none | deposits, incoming settlements, gains found by reconciliation | withdrawals, outgoing settlements, ledger fees, losses found by reconciliation |
| `assets/interledger/<peer>` | debits ≤ credits | none | Prepares from the peer (pending), our settlements to it (pending) | trust, Prepares to the peer (pending), its settlements to us |
| `contra/trust/<peer>` | credits ≤ debits | none | trust: the credit we extend, and each raise of it | nothing |
| `equity/owner` | none | root | the owner's packets out, withdrawals | packets to the owner's receivers, deposits, a first activation's balance |
| `equity/owner/btp/<id>`, `equity/owner/local/<name>` | none | owner | the endpoint's packets out | packets to the endpoint |
| `equity/suspense` | none | owner | losses found by reconciliation | gains found by reconciliation |
| `revenue/fees` | none | owner | | our fee on transit ([Connector](connector.md#fees-and-spread)) |
| `revenue/fx` | none | owner | | the source leg of a conversion |
| `expenses/fees` | none | owner | fees the underlying ledger charged | |
| `expenses/fx` | none | owner | the destination leg of a conversion | |
| `expenses/losses` | none | owner | refusals booked by nodes up to 0.1.3 ([below](#fulfills-booked-as-final)) | |

The balance of `assets/interledger/<peer>` (posted debits − posted credits) is what the peer owes us, net of the credit we extend; negative, we owe it. The debit on `contra/trust/<peer>` offsets the matching credit, so the peer can run up debt to us up to that credit and no further.

## Limits and groups

A limit applies to an account's own counters:

- **debits ≤ credits**: posted plus pending debits, with the new debit, stay within posted credits. Pending credits do not count: money in flight out counts, money in flight in does not.
- **credits ≤ debits**: posted plus pending credits, with the new credit, stay within posted debits.

**The owner's group.** On each ledger, `equity/owner` is the root of a group whose members are itself, its sub-accounts, `equity/suspense`, the fee accounts, the FX accounts and `expenses/losses`. The group's total carries the limit **debits ≤ credits**; no member has a limit of its own. So:

- The owner's funds on a ledger are everything the node holds on it beyond what it owes peers. A loss, a network fee or a reconciliation that finds less money is the owner's, and the owner cannot spend money the node does not have.
- A sub-account may go negative on its own; only the group's total is limited.
- A transfer between two members of one group needs no free funds, since its effect on the total is zero. Pending transfers between members count as pending debits and credits of the group alike, and the limit looks past them.
- Peers' accounts and `assets/settlement` are in no group.

## Transfers

A transfer has an ID, a debit account, a credit account (different, on the same ledger), an amount (never zero), a reference and the time it was created. It is **pending**, then **posted** or **voided**; or it is posted at once.

- **IDs** are assigned in order, never reused, and resumed after the largest persisted one at a start. A voided packet transfer never reaches the database, so the journal's IDs have gaps.
- **References** say what a transfer is for:

| Reference | For | Journaled | A report |
|---|---|---|---|
| `packet` (trace ID) | an ILP packet | once posted | no |
| `settlement` (scheme, ID) | a settlement with a peer | pending too, then posted or voided | no |
| `trust` | credit we extend to a peer, or a raise of it | posted | no |
| `deposit` | money the owner put on the underlying ledger | posted | yes |
| `withdrawal` | money the owner took off it | posted | yes |
| `reconciliation` | a difference between the books and the underlying ledger | posted | yes |
| `refusal` (trace ID) | a Fulfill an upstream refused, booked by nodes up to 0.1.3 | posted | yes |
| `fee` (scheme, transaction) | a fee the underlying ledger charged | posted | yes |

- A packet's **trace ID** is the node's boot epoch, persisted and incremented at every start, and a counter within the boot. It keeps packet references unique across restarts, and lines the journal up with the [packet trace](connector.md#the-packet-trace).
- A settlement's ID is unique within its scheme: the stub's are 16 random bytes in lowercase hex, XRPL's the transaction hash in uppercase hex. An incoming settlement without an ID on its ledger gets `in<next transfer ID>`.

### Operations

- **Open an account.** Opens its group root first if needed. Opening an account that exists returns it unchanged (ACC-4).
- **Open a peer's accounts** (`assets/interledger/<peer>` and `contra/trust/<peer>`): when they are new, the given credit is posted as a `trust` transfer, Dr `contra/trust/<peer>`, Cr `assets/interledger/<peer>`. When they exist, nothing is posted again (ACC-5). The credit then changes only by explicit `trust` transfers ([Settlement](settlement.md#credit)).
- **Create a transfer**, pending or posted. It is checked in this order, and refused as a value, never a panic, since the input may come from a peer (ACC-8):
  1. a zero amount (`ZeroAmount`); the same account on both sides (`SameAccount`); an unknown account (`UnknownAccount`); accounts on different ledgers (`DifferentLedgers`);
  2. a counter that would pass `MAX_TOTAL` (`Overflow`);
  3. a **report** (deposit, withdrawal, reconciliation, refusal, fee) skips the limits from here: it records what already happened elsewhere;
  4. the debit account's limit (`ExceedsDebits`), then the credit account's (`ExceedsCredits`);
  5. unless both accounts are in the same group, the debit side's group limit (`ExceedsDebits`), then the credit side's (`ExceedsCredits`);
  6. room in the pending table (`PendingFull`) and in the journal (`JournalFull`).
- **Create linked transfers**, all or none: if one is refused, the ones before it are voided, and the error says which (ACC-7). Linked transfers are pending and not durable.
- **Post** a pending transfer: its amount moves. No limit is checked again: the pending amount already held its place.
- **Void** a pending transfer: its amount stays where it was. Only a durable (settlement) transfer's void is journaled.
- **Validate** a transfer without creating it: what it takes, it takes for a smaller amount too, until the ledger changes.

The [connector](connector.md#booking-a-packet) maps refusals to ILP errors.

### Bounds

| Bound | Ledger default | The node's |
|---|---|---|
| Accounts | 1024 | 4096 |
| Pending transfers | 8192 | 25,408: three for every packet the connector may have in flight, and 64 more |
| Journal entries waiting to be persisted | 1024 | 1024 |

## The journal

Every step of the node's engine that changes the ledger queues **journal entries**: an account opened, or a transfer created or changed state. The engine persists all of one step's entries, with everything else the step changed, in one SQLite transaction, and acts on the step's outcome only once that is on disk. What the journal says is what happened.

**What is journaled:**

- every transfer that reaches posted balances, with the account totals cached in the same transaction;
- settlement transfers while pending too, since the money may already be on its way when the node restarts (ACC-16), and their posting or voiding;
- nothing for a packet transfer that was voided: it never touched a posted balance.

**Tables.**

```
ledger_accounts   (id ≥ 0 dense, path UNIQUE, debits_posted ≥ 0, credits_posted ≥ 0)
ledger_transfers  (id, debit, credit ≠ debit, amount > 0,
                   state ∈ {pending, posted, voided}, created_at_ms, resolved_at_ms (NULL iff pending),
                   reference ∈ {packet, settlement, trust, deposit, withdrawal, reconciliation, refusal, fee},
                   packet_epoch, packet_sequence        -- iff packet or refusal
                   settlement_scheme, settlement_id     -- iff settlement or fee
                   sequence UNIQUE > 0)
```

- `sequence` is the order rows were written or last changed in. A packet's transfer is written when it is posted, which is not the order of transfer IDs, so readers page through the journal by `sequence`: a reader that pages with the last sequence it saw sees every row, and every change of a row, once.
- Account IDs are dense, assigned in opening order, and persisted, so journal rows refer to accounts by number.

**Restoring.** At a start the ledger is rebuilt from the accounts (in ID order), the transfers still pending (settlements only) and the next transfer ID. It is refused if:

- an account's cached totals are not the sums of its posted journal rows;
- account IDs are not 0, 1, 2, … in order, two accounts share a path, or a group member comes before its root;
- a ledger's posted debits and credits differ, or a counter passes `MAX_TOTAL`;
- a pending transfer is not a settlement, refers to an unknown account, or is not below the next transfer ID.

Packet transfers that were pending are gone: in-flight packets cannot survive a restart, so a restarted ledger has none.

### Persistence

| Data | Rule |
|---|---|
| Accounts and posted transfers | the append-only journal, with the totals cached in the same transaction |
| Pending ILP transfers | memory only |
| Pending outgoing settlements | journal rows with the scheme's reference (for XRPL the transaction hash), reconciled at activation ([Settlement](settlement.md#outgoing-settlements)) |
| Ledger fees | journal rows with their transaction (`fee`), Dr `expenses/fees` |
| Peer trust | posted once when the peer's accounts are opened; every change is an explicit transfer |
| Scheme balance checkpoints | written in the batch they explain ([Settlement](settlement.md#checkpoints-and-activation)) |
| A BTP token's spending cap usage | written with the journal rows that change it ([Connector](connector.md#spending-caps)) |

## The owner's accounts

Each ledger has one pool of owner funds, `equity/owner`. Every endpoint that spends for the owner gets a sub-account under it: `equity/owner/btp/<id>` for a BTP token, `equity/owner/local/pay` for the payer. A sub-account holds no allocation; the group's total is the limit. **The owner's own endpoints never convert.** So:

- A BTP client paying the node's own STREAM receiver books Dr `equity/owner/btp/<id>`, Cr `equity/owner`: the group total is unchanged, and the journal still has a transfer per packet (ACC-8).
- A packet from a BTP token, the payer or a receiver goes out on its own ledger only: one whose first hop is on another ledger is refused (F02 "pay from the route's ledger"). The owner pays from the ledger the route starts on, and the first hop that charges for a conversion converts.
- A packet from a peer on another ledger into the node's own receiver is converted at the node's rates and lands in the asset it came in: the receiver's account shows what arrived, `expenses/fx` takes it back out on that ledger, and `revenue/fx` keeps it on the ledger it came from.
- A conversion between two peers pays out of the owner's funds on the outgoing ledger, so it needs them.
- Each token's spending can be read from the journal. The per-token spending caps are limits, not allocations, so they may add up to more than the funds ([Connector](connector.md#spending-caps)).

**What the owner sees** for a ledger: the group's posted net credit (the pool and every member), what packets in flight hold of it (the group's pending debits), the scheme's balance on the underlying ledger as the books have it, what that ledger holds back (the reserve), the address to fund it, and the scheme's status. The total across ledgers in a display currency is in [Connector](connector.md#the-owners-total).

## What the ledger refuses and what it records

- Packets and settlements are refused when they would break a limit (T04 for a packet).
- Deposits, withdrawals, reconciliations, refusals and ledger fees report what happened on the underlying ledger, and are recorded even when they take the group below zero. Spending is then refused until the group recovers.
- Settlement schemes report their on-ledger balance as a checkpoint, and activation books only the difference to the last checkpoint, through `equity/suspense` (ACC-2, ACC-6). The checkpoint is written in the same transaction as the transfers it explains.
- Limits are not invariants: reports may break them.

## Fulfills booked as final

A packet from a peer is posted when it is fulfilled, and stays posted whatever the peer does with the Fulfill: each node books its own Fulfills, and what its receiver took counts at once. A peer that does not count one owes us more than its books say; it can still owe us no more than the credit we extend. Nodes up to 0.1.3 booked refused Fulfills as `refusal` rows, a reversal or a loss in `expenses/losses`; those rows stay in their journals, and `expenses/losses` holds only them.

Each node keeps its own books and enforces its own limits from them. It never compares its books with a peer's, and does not measure, store or show drift ([D19](../decisions/D19-each-node-keeps-its-own-books.md)).

## Invariants

Asserted after every step, in tests and simulations:

- every ledger balances: posted debits equal posted credits, and pending debits equal pending credits;
- each account's pending counters are exactly the sums of its pending transfers;
- each group's total is the sum of its members, and its internal pending amount the sum of pending transfers between them;
- no counter passes `MAX_TOTAL`, and the bounds hold;
- the path index matches the accounts.

The property tests also check that no transfer breaks a limit, and that balances equal the journal.

## Decisions

- [D3](../decisions/D03-database-continuity.md): no carried-over totals from v0 databases (ACC-5, ACC-6).
- [D6](../decisions/D06-owner-ledger-and-multi-currency-accounts.md): an owner account per ledger, the owner's group and its limit, sub-accounts instead of allocations.
- [D8](../decisions/D08-real-money.md): `xrpl+xrp` stays refused.
- [D16](../decisions/D16-engineering-style-pragmatic-tigerstyle-and-deterministic-simulation.md): sans-IO, bounded, asserted.
- [D19](../decisions/D19-each-node-keeps-its-own-books.md): each node keeps its own books.
- [D23](../decisions/D23-each-node-books-its-own-fulfills.md): Fulfills booked as final; why `refusal` rows exist in old journals ([D17](../decisions/D17-the-upstream-decides-whether-a-fulfill-counted.md), superseded).

## Original

- [5 §1–§2](../original/05-accounting-and-settlement.md): ledger IDs, scales, the account path grammar, the chart of accounts and the operations of the original's ledger, which kept only cumulative posted totals.
- [5 §3](../original/05-accounting-and-settlement.md): how the original mapped packets onto transfers.
- [5 §10](../original/05-accounting-and-settlement.md): the gaps ACC-1 to ACC-9 this ledger closes.
