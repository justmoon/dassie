# 5. Accounting, ledgers and settlement

Source citations refer to the original implementation at commit `e962ab78` on `main` (see [the index](README.md#sources)). All paths below are relative to `packages/app-dassie/src/` unless they start with `packages/` or `docs/`.

Scope: internal double-entry ledger (`accounting/**`), the settlement-scheme framework and modules (`ledgers/**`), how ILP packets and settlement events turn into ledger transfers, the persistence of both, the peer-protocol touch points, and the related constants and DB migrations.

Convention: **MUST** marks behavior a compatible reimplementation has to reproduce, either because it shows up on the wire or because it changes balances. "Open question / known gap" (OQ) marks TODOs, bugs and divergences between the docs and the code. A reimplementation should decide on each one deliberately rather than copy it blindly.

---

## 1. Concepts and numeric model

### 1.1 Ledger IDs (internal ledgers)

`accounting/constants/ledgers.ts`:

| LedgerId (string) | currency | used by settlement module |
|---|---|---|
| `stub+usd` | `USD` | `stub` |
| `xrpl+xrp` | `XRP` | `xrpl` (mainnet; module exists but is NOT registered, see §6.6) |
| `xrpl-testnet+xrp` | `XRP` | `xrpl-testnet` |

- `LedgerId` is a tagged string. Only these three values are valid. `castLedgerId(id)` (`accounting/utils/cast-ledger-id.ts`) throws `Unknown ledger ID: <id>` for anything else.
- Naming convention: `<settlement-scheme-family>+<currency lowercase>`.
- `getCurrencyFromLedgerId(ledgerId)` (`accounting/functions/get-currency-from-ledger-id.ts`) maps LedgerId to `LEDGERS[id].currency` and then to `CURRENCIES[code]`. It throws `"Failed to map ledger ID to currency"` (and logs) when either lookup fails.

### 1.2 Currencies and scales

`exchange/constants/currencies.ts`:

| code | scale |
|---|---|
| USD | 9 |
| XRP | 9 |

- **All internal ledger amounts are unsigned integers in the currency's internal scale (9 decimal places).** 1 USD = 1_000_000_000 units. 1 XRP = 1_000_000_000 units, which is 1000 units per XRP drop.
- ILP packet amounts are applied to the ledger directly in these units. The node's ILP asset scale is effectively 9 for both currencies.
- TypeScript uses `bigint` everywhere. In Rust use `u64`/`i128` for computation. The SQLite storage is `INTEGER`, i.e. signed i64, so a posted total can be at most 9_223_372_036_854_775_807, about 9.2e9 currency units.
- No negative amounts exist. Account balances are derived values and can be "negative" conceptually, e.g. debit minus credit < 0.

### 1.3 Exchange conversion (only the part accounting needs)

Accounting uses it via `exchange/functions/convert.ts` and `exchange/functions/calculate-outgoing-amount.ts`:

- `INTERNAL_PRECISION = 15` (`exchange/constants/internal-precision.ts`).
- Rates come from a JSON feed `{data:{currency, rates:{CODE: "decimal string"}}}`. The feed is loaded once at startup, and again whenever `exchangeRateUrl` changes (`exchange/load-exchange-rates.ts`). Each rate is normalized to `BigInt(Math.round(Number(rate) * 1e15))`. The rate means "units of CODE per 1 base currency".
- `toBaseFrom(from, x) = x * 10^(2*15 - from.scale) / rate[from.code]` (integer division).
- `fromBaseTo(to, b) = b * rate[to.code] / 10^(2*15 - to.scale)`.
- `convert(from,to,x) = fromBaseTo(to, toBaseFrom(from, x))`. It throws `No rate available for currency X` when a rate is missing.
- `calculateOutgoingAmount(srcPath, dstPath, incoming)`: if the currency of the source ledger equals the currency of the destination ledger, the result is `incoming`. Otherwise it is `convert(...)`. **Note:** this compares *currencies*. The ledger-transfer logic (§3) compares *ledger IDs*. So `xrpl+xrp` and `xrpl-testnet+xrp` share a 1:1 amount but still route through the FX accounts.

---

## 2. Internal ledger (double-entry, TigerBeetle-inspired)

Main implementation: `accounting/stores/ledger.ts`. Design doc: `docs/implementation-notes/internal-ledger.md`.

### 2.1 Account path grammar

```
AccountPath  = LedgerId ":" AccountName
LedgerId     = "stub+usd" | "xrpl+xrp" | "xrpl-testnet+xrp"
AccountName  = "assets/settlement"
             | "assets/interledger/" NodeId
             | "contra/trust/" NodeId
             | "equity/owner"
             | "equity/suspense"
             | "revenue/fees"
             | "revenue/fx"
             | "expenses/fees"
             | "expenses/fx"
NodeId       = 2*45( ALPHA / DIGIT / "-" / "_" )     ; peer-schema.ts nodeIdSchema
```

- Types are in `accounting/types/account-paths.ts`. `liabilities/…` is documented but has **no concrete account type** and is never created (see ACC-1).
- The ledger ID is extracted as the substring before the **first** `:` (`accounting/functions/get-ledger-id-from-path.ts`). If the path contains no colon, an assertion throws (`AssertionError`, logged as "unmet assertion: account paths must contain a colon").
- Special pseudo-path: ILDCP endpoints map to the literal string `"unreachable"` (`ilp-connector/functions/get-endpoint-account-path.ts`). No account by that name exists. See §3.5 for the consequences.
- Prefix queries (`getAccounts(prefix)`) are plain string `startsWith`. They are implemented with `PrefixMap.filterPrefix` (`routing/utils/prefix-map.ts`). Iteration order is the PrefixMap's sorted order: key length descending, then lexicographic descending. `getAccounts("")` returns every account.

### 2.2 Chart of accounts (what flows through each account)

The account types come from the doc. An asset or expense account increases with debits. A liability, equity or revenue account increases with credits. A contra account adjusts another account.

| Account | Created when | Limit flag | Debited by | Credited by |
|---|---|---|---|---|
| `L:assets/settlement` | common accounts of ledger L (§2.6) | no_limit | deposits (`reportDeposit`), incoming settlements (`reportIncomingSettlement`) | withdrawals (`reportWithdrawal`), outgoing settlements (pending → posted on finalize) |
| `L:assets/interledger/<peer>` | peer becomes `peered` (§2.7) | **debits_must_not_exceed_credits** | prepares *from* that peer (pending), outgoing settlements to that peer (pending) | initial trust (posted), prepares *to* that peer (pending), incoming settlements from that peer (posted) |
| `L:contra/trust/<peer>` | same | **credits_must_not_exceed_debits** | initial trust extension (posted, 1.1e9) | never (in current code) |
| `L:equity/owner` | common accounts | no_limit | owner-originated packets (BTP/local/HTTP endpoint is the source), withdrawals | owner-destined packets, deposits |
| `L:equity/suspense` | common accounts | no_limit | never used (ACC-2) | never used |
| `L:revenue/fx` | common accounts | no_limit | — | source leg of cross-ledger packets |
| `L:revenue/fees` | common accounts | no_limit | never used (ACC-3) | never used |
| `L:expenses/fx` | common accounts | no_limit | destination leg of cross-ledger packets | — |
| `L:expenses/fees` | common accounts | no_limit | never used (ACC-3) | never used |

Meaning of the peer interledger account. Its balance `debits − credits` (posted) is what the peer owes us. A negative value means we owe the peer. The contra/trust account holds the credit line we extend to the peer. Its debit offsets the matching credit on the interledger account, so the peer can run up debt to us up to that amount.

### 2.3 Data model

```ts
interface LedgerAccount {
  path: string
  debitsPending: bigint      // in-memory only
  debitsPosted: bigint       // persisted
  creditsPending: bigint     // in-memory only
  creditsPosted: bigint      // persisted
  limit: "credits_must_not_exceed_debits" | "debits_must_not_exceed_credits" | "no_limit"   // NOT persisted; set by creator
}

interface Transfer {
  state: "pending" | "posted" | "voided"   // mutated in place
  debitAccount: AccountPath
  creditAccount: AccountPath
  amount: bigint
  immediate: boolean        // = !pending at creation
}

interface CreateTransferParameters {
  debitAccountPath: AccountPath
  creditAccountPath: AccountPath
  amount: bigint
  pending?: boolean         // default false => posted immediately
}
```

- Transfers have **no ID**. Pending transfers are held by object identity in an in-memory `Set<Transfer>`. Callers keep a reference to the `Transfer` object, either in the ILP pending-packet record or in the settlement map, and pass that object back to post or void it. A Rust port needs an explicit transfer handle or ID. No ID ever reaches the wire or the database, so the choice is free.
- No transfer history is stored. Only the per-account cumulative `debits_posted` and `credits_posted` are persisted.

### 2.4 Operations (exact semantics)

`createAccount(path, {limit?})`:
1. `SELECT * FROM accounts WHERE path = ?` (first row).
2. Build the account: `debitsPending = 0`, `creditsPending = 0`, `debitsPosted` and `creditsPosted` from the row or 0, `limit` from the option or `"no_limit"`.
3. If there was no row, `INSERT INTO accounts (path) VALUES (?)`. The columns default to 0.
4. Log "load account" or "create account" at debug level.
5. `map.set(path, account)`. This **overwrites** any existing in-memory account object for that path. The pending counters reset to 0 and the posted values reload from the DB (ACC-4).

`createTransfer({debitAccountPath, creditAccountPath, amount, pending})`, in this order:
1. Assert `credit != debit`. Violation throws an AssertionError, not a Failure.
2. Assert both paths have the same ledger ID. Violation throws an AssertionError.
3. If the debit account is missing, return `InvalidAccountFailure("debit", path)`.
4. If the debit account has limit `debits_must_not_exceed_credits` and `debitsPosted + debitsPending + amount > creditsPosted`, return `ExceedsDebitsFailure`. The check uses **posted credits only** and ignores pending credits, which is conservative.
5. If the credit account is missing, return `InvalidAccountFailure("credit", path)`.
6. If the credit account has limit `credits_must_not_exceed_debits` and `creditsPosted + creditsPending + amount > debitsPosted`, return `ExceedsCreditsFailure`.
7. Build `Transfer{state: pending ? "pending" : "posted", immediate: !pending, ...}`.
8. If pending: `debit.debitsPending += amount`, `credit.creditsPending += amount`, add to the pending set, then emit `PendingTransfersTopic(transfer)`.
9. If posted: `debit.debitsPosted += amount`, `credit.creditsPosted += amount`, emit `PostedTransfersTopic(transfer)`, then persist (§2.5). In this path the emit comes **before** the persist.
10. Return the transfer.

The `amount` is not checked for 0 or negative. Zero-amount packets never reach the ledger (§3.2).

`postPendingTransfer(transfer)`:
1. Assert `transfer.state === "pending"`. Assert both accounts exist (looked up by path *now*).
2. Set `state = "posted"`. Then `debit.debitsPending -= amt; debit.debitsPosted += amt; credit.creditsPending -= amt; credit.creditsPosted += amt`.
3. Remove from the pending set, persist (§2.5), then emit `PostedTransfersTopic`. In this path the persist comes **before** the emit.
4. No limit re-check happens at post time.

`voidPendingTransfer(transfer)`:
1. Assert pending and that the accounts exist. Set `state = "voided"`. Decrement `debitsPending` and `creditsPending`. Remove from the pending set. Emit `VoidedTransfersTopic`. Nothing is persisted.

Other read APIs: `getAccount(path)`, `getAccounts(prefix)`, `getPendingTransfers()` (an array copy), and `getLedgerIds()`. The last one returns the distinct ledger-ID prefixes of all in-memory accounts.

Failures (`accounting/failures/*.ts`) are values, not exceptions:
- `InvalidAccountFailure {name:"InvalidAccountFailure", whichAccount:"debit"|"credit", accountPath}`
- `ExceedsCreditsFailure {name:"ExceedsCreditsFailure"}`, a singleton
- `ExceedsDebitsFailure {name:"ExceedsDebitsFailure"}`, a singleton

Topics (`accounting/topics/*.ts`): `PendingTransfersTopic`, `PostedTransfersTopic` and `VoidedTransfersTopic` each carry the `Transfer` object narrowed to its state. Consumers:
- `TotalOwnerBalanceSignal` recomputes on Posted (§2.9).
- The debug RPC `subscribeToLedgerAccount` streams every transfer touching a given path (`rpc-server/routers/debug.ts`).

### 2.5 Persistence

Table `accounts` (migration 0011, `database/migrations/0011-create-accounts-table.ts`; table def `accounting/database-tables/accounts.ts`):

```sql
CREATE TABLE accounts (
  path TEXT PRIMARY KEY NOT NULL,
  debits_posted INTEGER NOT NULL DEFAULT 0,
  credits_posted INTEGER NOT NULL DEFAULT 0
) STRICT
```

`persistPostedTransfer` runs in one SQLite transaction:
```sql
UPDATE accounts SET debits_posted  = debits_posted  + :amount WHERE path = :debitAccount;
UPDATE accounts SET credits_posted = credits_posted + :amount WHERE path = :creditAccount;
```
- Persistence is synchronous. The DB is better-sqlite3 at `<dataPath>/dassie.sqlite3` with application_id `0x1DA53B81` (`database/schema.ts`, `database/open-database.ts`).
- Pending amounts, the pending-transfer set, account limit flags, voids and transfer history are **not** persisted.
- The DB holds no snapshot or event log. The persisted state is the per-account cumulative posted totals.

### 2.6 Common accounts and their initialization points

`initializeCommonAccounts(ledger, L)` (`accounting/functions/manage-common-accounts.ts`) creates these accounts in this order: `L:assets/settlement`, `L:equity/owner`, `L:equity/suspense`, `L:revenue/fx`, `L:revenue/fees`, `L:expenses/fx`, `L:expenses/fees`. All have `no_limit`.

It is called from two places:
1. `ManageBuiltinAccountsActor` (`accounting/manage-builtin-accounts.ts`) for the **owner ledger**, read from `OwnerLedgerIdSignal`. Its value is hard-coded to `stub+usd` (`accounting/signals/owner-ledger-id.ts`). This runs inside `AccountingActor`, the first service started after node identity exists (`daemon.ts`).
2. Each settlement-scheme instance actor, for the scheme's ledger (`ledgers/manage-settlement-scheme-instances.ts`, before `module.behavior` is invoked).

When the `stub` scheme is active, the stub+usd common accounts are therefore created twice. The second call replaces the in-memory objects (ACC-4). Pending amounts are still 0 at that point, so the replacement is harmless at startup.

### 2.7 Peer accounts and the initial credit

`initializePeer(ledger, L, peerId)` (`accounting/functions/manage-peer.ts`):
1. `createAccount("L:assets/interledger/<peer>", {limit: "debits_must_not_exceed_credits"})`
2. `createAccount("L:contra/trust/<peer>", {limit: "credits_must_not_exceed_debits"})`
3. A **posted** transfer: Dr `L:contra/trust/<peer>`, Cr `L:assets/interledger/<peer>`, amount `DEFAULT_CREDIT = 1_100_000_000n`. That is 1.1 units, i.e. 1.1 USD or 1.1 XRP. On failure it throws `Could not extend initial trust to peer: <name>`.

`cleanupPeer` is a no-op with the TODO "void all pending transfers?".

Caller: `CreatePeerLedgerEntriesActor` (`peer-protocol/create-peer-ledger-entries.ts`). It is mapped over `PeersSignal`, so one actor runs per peered node. It asserts `peerState.id === "peered"`, sets `L = getLedgerIdForSettlementScheme(peerState.settlementSchemeId)`, then calls `initializePeer`. It is started last inside `PeerProtocolActor`.

**ACC-5 (important):** the trust transfer is posted and persisted **every time** the peer actor starts: on every node restart and on every re-peer. Each restart adds another 1.1e9 to `contra/trust` debits and to `interledger` credits. The credit line grows without bound. A compatible-but-correct port should extend trust only when the account is newly created, or set it to a target value.

### 2.8 Ledger startup and reload sequence

1. `Database` opens and runs migrations.
2. `AccountingActor` creates or loads the owner-ledger (`stub+usd`) common accounts.
3. `ExchangeRatesActor` loads the rates.
4. `LedgersActor` runs `ManageSettlementSchemeInstancesActor` for every active scheme and **awaits** all of them (`Promise.all(sig.runMap(...))`). This includes network I/O for XRPL. Each instance creates or loads its ledger's common accounts and runs `module.behavior`. The behaviors call `reportDeposit` with the *entire* current on-ledger balance (§6.4, §6.5). Then `SendOutgoingSettlementsActor` is mapped over `PeersSignal`.
5. `PeerProtocolActor` runs `PersistNodeTableActor`, which loads nodes, peers and registrations from the DB, and eventually `CreatePeerLedgerEntriesActor`, which creates or loads peer accounts and posts the trust transfer again.

Only accounts that code re-creates are loaded into memory. Account rows for former peers or inactive ledgers stay in the DB but are invisible to `getLedgerIds()` and `TotalOwnerBalanceSignal`.

**ACC-6 (important):** every module re-reports its full on-ledger balance as a *deposit* on each start. That is Dr `assets/settlement`, Cr `equity/owner` for the whole balance, posted and persisted. Because posted totals persist, `assets/settlement` and `equity/owner` inflate by the full balance on every restart. The stub module re-reports `100_000_000_000` (100 USD) each start. Intended behavior is presumably to reconcile against the persisted value, using `equity/suspense`.

### 2.9 Total owner balance

`TotalOwnerBalanceSignal` (`accounting/computed/total-owner-balance.ts`):
- For each ledger L in `getLedgerIds()`, `ledgerBalance(L)` is the sum over all accounts with prefix `L:assets/`, `L:liabilities/` or `L:contra/trust/` of `debitsPosted − creditsPosted − creditsPending`. Pending credits count against the balance. Pending debits are ignored.
- The result is the sum of the ledger balances. Ledgers other than the owner ledger are converted to the owner currency with `convertCurrencyAmounts`.
- It is recomputed and written on every `PostedTransfersTopic` event, not on pending or voided events, so a newly pending credit is reflected only at the next post.
- RPC: `general.subscribeBalance` (protected) streams this signal (`rpc-server/routers/general.ts`).
- In effect it measures net worth: settlement assets plus peer receivables net of trust, or equivalently `equity/owner` + revenue − expenses.

---

## 3. ILP packet → ledger mapping

Files: `ilp-connector/functions/{calculate-prepare-packet-outcome,process-prepare-packet,process-fulfill-packet,process-reject-packet,schedule-timeout,trigger-late-rejection,get-endpoint-account-path}.ts`, `accounting/functions/apply-interledger-packet.ts`.

### 3.1 Endpoint → account path

| Endpoint type | Account path |
|---|---|
| `peer` (nodeId) | `${L}:assets/interledger/${nodeId}` where `L = module(peerState.settlementSchemeId).ledger`. Computed in `peer-protocol/handlers/interledger-packet.ts` for incoming packets and in `routing/functions/resolve-ilp-address.ts` for the next hop. |
| `btp`, `local`, `http` | `${OwnerLedgerId}:equity/owner`, i.e. `stub+usd:equity/owner` |
| `ildcp` | literal `"unreachable"` |

`getLedgerIdForSettlementScheme(id)` (`ledgers/functions/get-ledger-id.ts`) returns `LoadedSettlementModulesStore.get(id).ledger`. It **throws** `Unknown settlement scheme '<id>'` if the module is not loaded. That can happen when a peer is persisted with a scheme that is not currently active.

### 3.2 Prepare processing (ledger part)

In `calculatePreparePacketOutcome`, the ledger work happens after routing and expiry checks:
1. Route lookup failure → `F02 Unreachable`. Expiry: `delta = expiresAt − now − 1000`. If `delta < 1000` → `R02 Insufficient timeout`. Outgoing expiry = `now + min(delta, 30000)`.
2. If `amount > 0`:
   - If `amount > MAX_PACKET_AMOUNT (100_000_000n)`, i.e. more than 0.1 unit → `F08 Amount too large` (empty data).
   - `outgoingAmount = calculateOutgoingAmount(srcPath, dstPath, amount)` (§1.3).
   - `transfers = applyPacketPrepareToLedger(srcPath, dstPath, amount, outgoingAmount)`. Each one is passed to `ledger.createTransfer` **in order**:
     - `InvalidAccountFailure` → `T00 Internal error "Missing internal ledger account"`
     - `ExceedsCreditsFailure` or `ExceedsDebitsFailure` → `T04 Insufficient liquidity "Insufficient liquidity"`
   - The created transfers are stored in the `PreparedIlpPacketEvent.pendingTransfers` array of the pending-packet entry. The key is `"<uniqueEndpointId(dest)>#<outgoingRequestId>"`.
3. If `amount == 0`, no transfers are created.

`applyPacketPrepareToLedger(src, dst, in, out)`:
- **Same ledger ID**: assert `in == out`, then one pending transfer: **Dr src, Cr dst, amount = in**.
- **Different ledger IDs** (Ls ≠ Ld), two pending transfers:
  1. Dr `src` (on Ls), Cr `Ls:revenue/fx`, amount = `in`
  2. Dr `Ld:expenses/fx`, Cr `dst` (on Ld), amount = `out`
- No fee transfers are applied (ACC-3).

**ACC-7:** if transfer 2 fails after transfer 1 succeeded, the function returns a failure without voiding transfer 1. Transfer 1 stays pending forever (in memory) and locks `debitsPending` on the source account.

### 3.3 Fulfill

`processFulfillPacket`:
1. Look up the pending entry by `<sourceEndpointId>#<requestId>`. If it is missing, warn and drop.
2. Verify `sha256(fulfillment) == executionCondition`. On mismatch, warn and **drop**. The pending entry and the transfers are kept, so the timeout will void them later.
3. Delete the entry and abort the timeout.
4. `postPendingTransfer` each transfer in array order, which persists them.
5. Forward the fulfill upstream.

### 3.4 Reject and timeout

- `processRejectPacket`: look up and delete the entry, abort the timeout, `voidPendingTransfer` each transfer, forward the reject upstream.
- Timeout: `scheduleTimeout` waits a **fixed 5000 ms** from forwarding (`ilp-connector/functions/schedule-timeout.ts`). This value is independent of the packet expiry or MAXIMUM_HOLD_TIME. On expiry it synthesizes `R00 "Packet timed out"` and runs `processRejectPacket`, which voids the transfers.
- Early rejection (the prepare outcome is a failure) sends the reject upstream directly. No transfers exist, except for the leak in ACC-7.

### 3.5 Packet edge cases

- **Owner → owner** (e.g. a BTP client paying the node's own STREAM receiver): both paths are `stub+usd:equity/owner`. `createTransfer` asserts debit ≠ credit and **throws** an AssertionError instead of returning a failure (ACC-8).
- **ILDCP source with amount > 0**: `calculateOutgoingAmount` calls `getLedgerIdFromPath("unreachable")`, which throws an AssertionError because there is no colon. ILDCP traffic normally carries amount 0 and never reaches this code.
- **Owner accounts have no limit**: `equity/owner` is `no_limit`, so owner-originated payments are never refused for insufficient funds by the ledger (ACC-9).
- Peer sending to us: the limit on `assets/interledger/<peer>` (debits ≤ posted credits) is the only credit control. A peer can push packets to us until `debitsPosted + debitsPending` reaches `creditsPosted`. `creditsPosted` = trust + incoming settlements + fulfilled packets we forwarded to that peer.
- We sending to a peer: crediting the peer's interledger account has **no limit**. Our debt to a peer is unbounded. It is only reduced by our outgoing settlements (§5).

### 3.6 Worked example (same ledger, stub+usd, peers A and B, trust 1.1e9 each)

The initial state after `initializePeer` for each peer: `interledger/A: credits=1.1e9`, `contra/trust/A: debits=1.1e9`, and the same for B.

A 50_000_000 packet arrives from A to B:
- At prepare: pending Dr `interledger/A` 5e7, Cr `interledger/B` 5e7. Check: `0 + 0 + 5e7 ≤ 1.1e9`, OK.
- At fulfill: posted. Now `interledger/A`: debitsPosted 5e7, creditsPosted 1.1e9. `interledger/B`: creditsPosted 1.1e9 + 5e7.
- Settlement metric for B (§5.2): `balance = 1.15e9`, below the threshold of 1.21e9, so no settlement yet.

---

## 4. Settlement framework (`ledgers/**`)

### 4.1 Persistent configuration: settlement schemes

Table `settlement_schemes` (migration 0004; def `ledgers/database-tables/settlement-schemes.ts`):
```sql
CREATE TABLE settlement_schemes (
  id TEXT NOT NULL PRIMARY KEY,   -- SettlementSchemeId
  config TEXT NOT NULL            -- JSON, module-specific (currently always "{}")
) STRICT
```
- `SettlementSchemeId` type is `"stub" | "xrpl" | "xrpl-testnet"` (`peer-protocol/types/settlement-scheme-id.ts`). On the wire it is IA5String of size 2..128 over the characters `[a-z0-9-]` (`peer-protocol/peer-schema.ts`).
- `SettlementSchemesStore` (`ledgers/database-stores/settlement-schemes.ts`) loads all rows at construction (`config` is `JSON.parse`d). It has two actions:
  - `addSettlementScheme(id, config)` pushes to the list and inserts the row.
  - `removeSettlementScheme(id)` deletes the row. **Bug ACC-10:** the in-memory removal is `draft.splice(1)`, which truncates the array after the first element, instead of `splice(index, 1)`.
- `ActiveSettlementSchemesSignal` (`ledgers/signals/active-settlement-schemes.ts`) is the `Set<SettlementSchemeId>` of store ids. It updates on add and remove store actions. **Bug ACC-11:** each update is produced from the *initial* set captured in the closure, not the current one. A second add or remove in the same process discards earlier changes.
- Config is not passed to modules. The `config` column is unused (ACC-12).
- Dev tooling inserts schemes directly with `config "{}"` (`packages/app-dev/src/utils/prefill-database.ts`).

### 4.2 Module registry and loading

`ledgers/modules/index.ts` maps scheme id to a lazy import:
- `stub` → `./stub`
- `xrpl-testnet` → `./xrpl/xrpl-testnet`
- (`xrpl` mainnet is **not** registered → ACC-13)

`ManageSettlementSchemeInstancesActor` (`ledgers/manage-settlement-scheme-instances.ts`) is mapped over `ActiveSettlementSchemesSignal`. For each scheme id:
1. Read `realm` from `DatabaseConfigStore`. Its scalar is `config.realm`, one of `"test" | "live"`, default `"test"`.
2. Load the module (cached in `LoadedSettlementModulesStore`, a Map of id to module). An unknown id throws `Unknown settlement scheme '<id>'`.
3. If `realm !== module.realm`, throw `"Subnet module is not compatible with realm"`. This keeps real-money and virtual settlement from being mixed.
4. `L = module.ledger`. Call `initializeCommonAccounts(L)`.
5. Build the host methods (§4.4) and `return module.behavior({sig, settlementSchemeId, host})`. The resolved object becomes the actor's API. Callers invoke methods as `actor.api.<method>.ask(params)` (request/response) or `.tell(params)` (fire-and-forget).

### 4.3 Module interface (`ledgers/types/settlement-scheme-module.ts`)

```ts
interface SettlementSchemeModule<TPeerState extends object> {
  readonly name: string                 // scheme id
  readonly supportedVersions: number[]  // all modules: [1]; NOT used anywhere (ACC-14)
  readonly realm: "test" | "live"
  readonly ledger: LedgerId
  behavior(p: { sig, settlementSchemeId, host: SettlementSchemeHostMethods })
    => Promisable<SettlementSchemeActorMethods<TPeerState>>
}

interface SettlementSchemeActorMethods<TPeerState> {
  getPeeringInfo(): Promisable<{ data: Uint8Array }>
  createPeeringRequest(p: { peerId, peeringInfo: Uint8Array }): Promisable<{ data: Uint8Array }>
  acceptPeeringRequest(p: { peerId, data: Uint8Array })
    : Promisable<{ peeringResponseData: Uint8Array, peerState: TPeerState } | false>
  finalizePeeringRequest(p: { peerId, peeringInfo: Uint8Array, data: Uint8Array })
    : Promisable<{ peerState: TPeerState }>
  prepareSettlement(p: { amount: bigint, peerId, peerState: TPeerState }): Promisable<{
     message: Uint8Array, peerState?: TPeerState, settlementId: string,
     execute: () => Promisable<{ peerState?: TPeerState }> }>
  handleSettlement(p: { amount: bigint, peerId, settlementSchemeData: Uint8Array, peerState: TPeerState }): Promisable<void>
  handleMessage(p: { peerId, message: Uint8Array }): Promisable<void>
  handleDeposit(p: { amount: bigint }): Promisable<void>
  getBalance(): bigint            // on-ledger balance in internal units
}
```
Contract from the doc comments:
- `prepareSettlement` must not submit anything.
- `settlementId` must be unique per scheme.
- The module must later call `finalizeOutgoingSettlement` once the settlement executes, or `cancelOutgoingSettlement` once failure is final.
- The `peerState` values returned by `prepareSettlement` and `execute` are **ignored** by the host (ACC-15).

### 4.4 Host methods given to modules (exact ledger effects)

Here `L` is the ledger ID the module passes in, which in practice is its own ledger. `S` is the scheme id.

| Host method | Ledger effect |
|---|---|
| `sendMessage({peerId, message})` | Fire-and-forget peer message `settlementMessage{settlementSchemeId:S, message}` to `peerId`. |
| `reportIncomingSettlement({ledgerId:L, peerId, amount})` | **Posted** Dr `L:assets/settlement`, Cr `L:assets/interledger/<peerId>`. Throws `Could not credit settlement in internal ledger: <failure>` if the peer account is missing. |
| `finalizeOutgoingSettlement({settlementId})` | key = `"S:<settlementId>"`. Look it up in `PendingSettlementsMap`. If missing, throw `"Could not finalize outgoing settlement: transfer not found"`. Otherwise `postPendingTransfer(transfer)` and delete the key. |
| `cancelOutgoingSettlement({settlementId})` | Same lookup (throws if missing). Then `voidPendingTransfer` and delete the key. No module calls this today. |
| `reportDeposit({ledgerId:L, amount})` | **Posted** Dr `L:assets/settlement`, Cr `L:equity/owner`. |
| `reportWithdrawal({ledgerId:L, amount})` | **Posted** Dr `L:equity/owner`, Cr `L:assets/settlement`. On failure the error text wrongly says "Could not credit deposit…". |
| `getEntropy({path})` | Deterministic 32 bytes, `getPrivateSeedAtPath(nodePrivateKey, "node/settlement-module/<S>/<path>")` (§4.5). Throws if `path` is empty. |

`PendingSettlementsMap` (`ledgers/values/pending-settlements-map.ts`) is an in-memory `Map<string, Transfer>` keyed by `"<schemeId>:<settlementId>"`. It is not persisted (ACC-16).

### 4.5 Deterministic entropy (needed for wallet compatibility)

`crypto/utils/seed-paths.ts` and `constants/seed-paths.ts`:
- `SEED_PATH_SETTLEMENT_MODULE = "node/settlement-module"`.
- `getPrivateSeedAtPath(nodePrivKey, path)`:
  - If `path == "node"`, return the raw node private key.
  - The path must start with `"node/"`. Strip that prefix and split the rest on `/`.
  - `key = nodePrivKey`, the 32-byte raw Ed25519 private key taken from PKCS#8 PEM in scalar `config.dassieKey`.
  - For each segment: `key = HMAC-SHA256(key, utf8(segment))`. Return `key` (32 bytes).
- Example for xrpl-testnet: segments `["settlement-module","xrpl-testnet","seed-testnet"]`.
- Example for xrpl mainnet: segments `["settlement-module","xrpl","seed-mainnet"]`.

### 4.6 Outgoing settlement trigger (`ledgers/send-outgoing-settlements.ts`)

Constants:
- `SETTLEMENT_CHECK_INTERVAL = 4000` ms
- `SETTLEMENT_RATIO = 0.2`
- `RATIO_PRECISION = 8`

`multiplyAmountWithRatio(a, r) = a * BigInt(round(r * 1e8)) / 1e8` (bigint division truncates).

One actor per peer in `PeersSignal`. Each runs a scheduled task with interval 4000 ms and no jitter. The first run happens 4 s after start, and the timer re-arms itself every tick. The handler is synchronous: it fires an async chain and returns, so the "skip if running" overlap policy never kicks in (see ACC-17).

Each tick:
1. Read `peerState` from the node table and assert it is `peered`. Take `settlementSchemeId` and `settlementSchemeState`.
2. Find the scheme actor. If it is missing, warn "settlement scheme actor not found, skipping settlement" and return.
3. `L = getLedgerIdForSettlementScheme(S)`.
4. `calculateSettlementAmount(L, peer)`:
   ```
   IL = L:assets/interledger/<peer>, TR = L:contra/trust/<peer>, SA = L:assets/settlement   (assert all exist)
   balance          = IL.creditsPosted − IL.debitsPosted − IL.debitsPending     // what we owe the peer, + our trust
   outgoingCredit   = TR.debitsPosted − TR.creditsPosted                       // credit we extended
   incomingCredit   = outgoingCredit      // TODO: should be the credit the peer extended to us
   midpoint         = (incomingCredit + outgoingCredit) / 2                    // bigint
   threshold        = multiplyAmountWithRatio(midpoint, 1 + 0.2/2)             // = midpoint * 110000000 / 100000000
   if balance < threshold: return 0
   amount = balance − midpoint, clamped to [0, balance]
   ```
   With the default credit of 1.1e9 (single restart, see ACC-5): midpoint = 1.1e9 and threshold = 1.21e9. The node settles once it owes the peer at least 0.11 units net. It pays `balance − 1.1e9`, which is everything it owes net, bringing the net position back to zero.
5. If `amount > 0`:
   1. `{message, settlementId, execute} = await schemeActor.api.prepareSettlement.ask({amount, peerId, peerState: settlementSchemeState})`
   2. `processSettlementPrepare(L, peer, amount, "outgoing")` (`accounting/functions/process-settlement.ts`) creates a **pending** transfer Dr `L:assets/interledger/<peer>`, Cr `L:assets/settlement`. For "incoming" it would be the reverse, but the incoming path is unused; incoming settlements post directly (§4.4). A failure throws `Settlement failed, invalid <debit|credit> account <path>`, `Settlement failed, exceeds credits` or `Settlement failed, exceeds debits`. The limit on IL is debits ≤ creditsPosted, so a settlement larger than the posted credits fails. That cannot happen under the formula because `amount ≤ balance`.
   3. `PendingSettlementsMap.set("S:<settlementId>", transfer)`.
   4. Fire-and-forget peer message `settlement{settlementSchemeId:S, amount, settlementSchemeData: message}` to the peer. This is sent **before** the on-ledger execution.
   5. `await execute()`. On error, warn "failed to execute outbound settlement". The pending transfer is left pending (ACC-16).
   6. Any error in prepare is logged as warn "failed to prepare outbound settlement".

The midpoint already includes our trust offset, so subtracting it gives the real net debt. Pending debits, i.e. in-flight settlements, reduce `balance`, which prevents re-settling the same debt once the pending transfer exists.

Nothing asks a peer to settle *to* us. Settlement is purely debtor-initiated. The ratio logic is effectively hard-coded: there are no per-peer thresholds or config.

### 4.7 Incoming settlement (peer side)

`peer-protocol/handlers/settlement.ts`:
1. If the scheme actor for `settlementSchemeId` is missing, return silently with an empty response.
2. If the sender is not `peered`, throw `Settlement failed, peer <id> is not peered`.
3. `await module.handleSettlement({peerId: sender, amount, settlementSchemeData, peerState: settlementSchemeState})`.

What the module then does is scheme-specific (stub §6.1, XRPL §6.3.6).

`peer-protocol/handlers/settlement-message.ts`: if the scheme actor is missing, return. Otherwise `module.handleMessage.tell({peerId: sender, message})`. It does not check whether the sender is peered.

Both `settlement` and `settlementMessage` require an authenticated peer message (they are not in `ALLOW_ANONYMOUS_USAGE`, `peer-protocol/constants/anonymous-messages.ts`).

### 4.8 RPC router `ledgers` (`ledgers/rpc-routers/ledgers.ts`, mounted at `ledgers` in `rpc-server/app-router.ts`; all routes protected/authenticated)

| route | kind | input | behavior / output |
|---|---|---|---|
| `getList` | query | — | For each row of `SettlementSchemesStore`: `{id, balance}`. `balance` = `await actor.api.getBalance.ask()`, or `0n` if the actor is not running. This is the on-ledger balance in internal units. |
| `checkLedgerDeletePrerequisites` | query | `string` scheme id | `{isActive: actor exists, isUnused: no peer in PeersSignal has peerState peered with this settlementSchemeId}` |
| `addSettlementScheme` | mutation | `{id: string, config: {}}` | `SettlementSchemesStore.addSettlementScheme(id, config)`. Inserts the DB row and triggers instance creation via the signal. |
| `removeSettlementScheme` | mutation | `{id: string}` | `removeSettlementScheme(id)`. Subject to ACC-10 and ACC-11. |
| `stubDeposit` | mutation | `string` amount (decimal) | Requires an active `stub` actor, otherwise throws "Stub settlement scheme is not enabled". Calls `handleDeposit.ask({amount: BigInt(amount)})` and returns `true`. The GUI sends `"100000000000"` (100 USD). |

Related RPCs:
- `general.getBasicState` (public) includes `activeSettlementSchemes: SettlementSchemeId[]`.
- `general.subscribeBalance` (protected) streams the total owner balance.
- `debug.getLedger` returns all `LedgerAccount`s.
- `debug.subscribeToLedgerAccount(path)` emits the account, then every matching transfer.
- `debug.getDatabaseTableRows("accounts" | "settlementSchemes" | …)`.

GUI references: `packages/gui-dassie/src/pages/ledgers/**`. The create page offers only `stub`.

---

## 5. Peer-protocol interface points (wire formats specified in [3 §5](03-peer-network.md))

`peer-protocol/peer-schema.ts`, OER encoded. Only the parts relevant to settlement:

```
settlementSchemeIdSchema = IA5String SIZE(2..128) FROM ("a".."z" | "0".."9" | "-")

peerMessageContent CHOICE {
  peeringRequest      [0] SEQUENCE { settlementSchemeId, nodeInfo OCTET STRING (CONTAINING signedPeerNodeInfo), settlementSchemeData OCTET STRING }
  ...
  settlement          [4] SEQUENCE { settlementSchemeId, amount UINT64 (bigint), settlementSchemeData OCTET STRING }
  settlementMessage   [5] SEQUENCE { settlementSchemeId, message OCTET STRING }
  ...
  peeringInfoRequest  [9] SEQUENCE { settlementSchemeId }
}
responses:
  peeringRequest      SEQUENCE { accepted BOOLEAN, data OCTET STRING }
  settlement          EMPTY
  settlementMessage   EMPTY
  peeringInfoRequest  SEQUENCE { settlementSchemeData OCTET STRING }
link-state entries (nodeInfoEntry CHOICE): settlementScheme [1] SEQUENCE { settlementSchemeId }
```

- `settlement.amount` is in the sender's internal ledger units (scale 9), encoded as an OER uint64 (8 bytes big-endian).
- Each node advertises its active schemes as `settlementScheme` entries in its signed link state (`peer-protocol/maintain-own-node-table-entry.ts`). The link state is regenerated whenever `ActiveSettlementSchemesSignal` changes.

### 5.1 Peering handshake and the module hooks

Initiator (`peer-protocol/maintain-peering-relationships.ts`) runs every `PEERING_CHECK_INTERVAL = 1000` ms while there are fewer than `MINIMUM_PEERS = 2` peers:
1. Pick a random node from the node table that has a link state, is not us, has `peerState none`, and shares a scheme with our active set. The common scheme is the first element of the candidate's advertised list that is also in our set.
2. Send `peeringInfoRequest{settlementSchemeId}` (anonymous allowed). The responder handler (`peer-protocol/handlers/peering-info-request.ts`) returns `{settlementSchemeData: module.getPeeringInfo().data}`. If the scheme is not active it throws "Settlement scheme not found".
3. `data = module.createPeeringRequest({peerId, peeringInfo})`.
4. Send `peeringRequest{nodeInfo: ownLinkState.lastUpdate, settlementSchemeId, settlementSchemeData: data}`.
5. Responder (`peer-protocol/handlers/peering-request.ts`):
   - Reject with `{accepted:false, data: empty}` if the scheme is not active, or if the requester's nodeId is unknown in the node table.
   - Call `module.acceptPeeringRequest({peerId, data})`. A result of `false` means rejected.
   - Otherwise add the node, process its link state, and set `peerState = {id:"peered", lastSeen: now, settlementSchemeId, settlementSchemeState: result.peerState}`.
   - Respond `{accepted:true, data: result.peeringResponseData}`.
6. Initiator on `accepted`: `module.finalizePeeringRequest({peerId, peeringInfo /* from step 2 */, data /* response */})`, then set `peerState = peered` with the returned `peerState`.
7. Both sides then get peer ledger accounts via `CreatePeerLedgerEntriesActor` (§2.7) and an outgoing-settlement actor (§4.6).

### 5.2 Persistence of peer settlement state

`peers` table (migrations 0006 + 0010):
```sql
CREATE TABLE peers (
  node INTEGER PRIMARY KEY NOT NULL,           -- nodes.rowid
  settlement_scheme_id TEXT NOT NULL
) STRICT;
ALTER TABLE peers ADD COLUMN settlement_scheme_state TEXT NOT NULL DEFAULT "{}";   -- JSON of module peerState
```
- Loaded at startup by `PersistNodeTableActor` (`peer-protocol/persist-node-table.ts`), which left-joins `nodes`, `peers` and `registrations`.
  - If either peers column is null, the node's `peerState` is `{id:"none"}`.
  - Otherwise it is `{id:"peered", lastSeen:0, settlementSchemeId, settlementSchemeState: JSON.parse(state)}`. If the parsed state is not an object it throws "Invalid settlement scheme state".
- **ACC-18:** the app never *writes* `peers`. The add, update and register watchers are TODO, so peerings made at runtime are lost on restart. Only dev-prefilled peers (`packages/app-dev/src/utils/prefill-database.ts`) survive.
- XRPL peer state JSON shape: `{"address":"r..."}`. Stub: `{}`.

---

## 6. Settlement modules

### 6.1 `stub` (`ledgers/modules/stub.ts`)

- `name "stub"`, `supportedVersions [1]`, `realm "test"`, `ledger "stub+usd"`.
- WARNING in the source: for testing only. Anyone can claim settlements.
- State is an in-memory `balance`, starting at `INITIAL_BALANCE = 100_000_000_000n` (100 USD at scale 9). It resets on every start.
- At behavior start it calls `reportDeposit({ledgerId:"stub+usd", amount: balance})` (ACC-6).
- `getPeeringInfo` → `{data: empty}`. `createPeeringRequest` → `{data: empty}`. `acceptPeeringRequest` → `{peeringResponseData: empty, peerState: {}}`, so it always accepts. `finalizePeeringRequest` → `{peerState: {}}`.
- `prepareSettlement({peerId, amount})`:
  - `settlementId = hex(random 16 bytes)` (32 lowercase hex chars). `message = empty`.
  - `execute()`: if `amount > balance`, log and throw "Insufficient balance". Otherwise `balance -= amount`, call `host.finalizeOutgoingSettlement({settlementId})` synchronously, and return `{}`.
- `handleSettlement({peerId, amount})`: `balance += amount`, then `host.reportIncomingSettlement({ledgerId:"stub+usd", peerId, amount})`. The amount is **trusted from the peer message**.
- `handleMessage`: no-op.
- `handleDeposit({amount})`: `balance += amount`, then `host.reportDeposit({ledgerId, amount})`.
- `getBalance()` → `balance`.

Stub settlement end to end:
- Sender: pending Dr `interledger/B` Cr `assets/settlement`. Send the `settlement` message with the amount. On execute, post.
- Receiver: posted Dr `assets/settlement` Cr `interledger/A`.

### 6.2 XRPL constants

- `ledgers/modules/xrpl/constants/asset-scale.ts`: `XRP_ON_LEDGER_SCALE = 6` (drops), `XRP_INTERNAL_SCALE = 9`, `XRP_VALUE_FACTOR = 10^(9−6) = 1000n`. `xrpl-testnet.ts` redeclares the same values locally.
- `ledgers/modules/xrpl/constants/settlement-memo-type.ts`: `XRP_SETTLEMENT_MEMO_TYPE = hex(utf8("https://dassie.land/settlement/xrp-testnet/v1"))`, uppercase or lowercase hex accepted on parse. Mainnet uses the same string, including "xrp-testnet".

### 6.3 XRPL modules (`xrpl-testnet.ts`, `xrpl.ts`, `functions/create-settlement-engine.ts`)

| | xrpl-testnet | xrpl (mainnet) |
|---|---|---|
| name | `xrpl-testnet` | `xrpl` |
| realm | `test` | `live` |
| ledger | `xrpl-testnet+xrp` | `xrpl+xrp` |
| entropy path | `seed-testnet` | `seed-mainnet` |
| server | `wss://s.altnet.rippletest.net:51233` | `wss://s1.ripple.com/` |
| auto-fund | yes, via faucet | no |
| registered in `modules/index.ts` | yes | **no** (ACC-13) |

The library is xrpl.js `^3.1.0` (installed 3.1.0). `supportedVersions [1]`.

#### 6.3.1 Wallet derivation (MUST match to keep the same on-ledger account)

1. `entropy32 = host.getEntropy({path})`. See §4.5: HMAC chain with segments `settlement-module`, `<schemeId>`, `seed-testnet|seed-mainnet`.
2. `seed16 = entropy32[0..16]`.
3. `Wallet.fromSeed(encodeSeed(seed16, "ed25519"))`. The XRPL family seed uses the ed25519 prefix bytes `0x01 0xE1 0x4B`, base58 → `sEd…`. Under ripple-keypairs 2.0 the derivation is: `rawPriv = SHA-512(seed16)[0..32]`, `pub = 0xED || Ed25519Pub(rawPriv)`, `address = base58check(0x00 || RIPEMD160(SHA256(pub)))` with the XRPL alphabet.

#### 6.3.2 Startup behavior

1. Create a `Client(url)`. Register an `error` listener that logs "xrpl client error". `await client.connect()`. On actor cleanup, `client.disconnect()`. No reconnection logic beyond xrpl.js defaults.
2. `getAccountInfo(own address)`, i.e. `{"command":"account_info","account":addr,"ledger_index":"validated"}`. An `actNotFound` rippled error yields `false`. Other errors are rethrown.
   - **If found**: `reportDeposit(L, BigInt(account_data.Balance) * 1000)`, i.e. the full balance including the reserve (ACC-19, ACC-6).
   - **If not found (testnet)**: `await client.fundWallet(wallet)`. xrpl.js picks the faucet host `faucet.altnet.rippletest.net`, path `/accounts`, from the URL containing "altnet" and polls until the balance increases. Re-query `account_info`. If still missing, throw "Account not found after funding". Then `reportDeposit(full balance * 1000)`.
   - **If not found (mainnet)**: nothing. Balance stays 0 and the operator must fund the account externally. The first incoming payment that creates the account fires the transaction handler, which sees a `CreatedNode`, not a `ModifiedNode`, so the activation deposit is **missed** (ACC-20).
3. `createSettlementEngine({client, wallet, host, ledgerId: L})`:
   - `client.request({"command":"subscribe","accounts":[address]})`. The `accounts` stream delivers validated transactions only.
   - `balance = account_info.Balance * 1000`, or `0` if not found. This is used only for `getBalance()`.
   - Register a `client.on("transaction", handler)` (§6.3.4).

#### 6.3.3 OER schemas (module-private payloads inside peer messages)

```
peeringInfoSchema    = SEQUENCE { address IA5String SIZE(25..35) }      -- ledgers/modules/xrpl/oer-schemas/peering-info-data.ts
peeringRequestSchema = peeringInfoSchema                                  -- peering-request-data.ts ("same as peering info right now")
peeringResponseSchema= EMPTY (0 bytes)                                    -- peering-response-data.ts
settlementProofSchema= SEQUENCE { transactionHash OCTET STRING SIZE(32) } -- settlement-proof.ts
```

Byte layout: none of these sequences has optional fields or an extension marker, so there is no preamble.
- `address`: 1-byte length prefix (25..35) followed by ASCII.
- `transactionHash`: exactly 32 raw bytes, no length prefix.
- `peeringResponse`: 0 bytes.

XRPL peer state: `{ address: string }` (`types/peer-state.ts`).

#### 6.3.4 Peering hooks

- `getPeeringInfo()` → `peeringInfoSchema({address: own classic address})`.
- `createPeeringRequest()` → `peeringRequestSchema({address: own address})`. It ignores the `peeringInfo` argument.
- `acceptPeeringRequest({peerId, data})`:
  - Parse; on failure, debug log and return `false`.
  - `account_info(address, validated)`. If the account is not found, debug "peer account not found" and return `false`.
  - Otherwise `{peeringResponseData: empty bytes, peerState: {address}}`.
- `finalizePeeringRequest({peeringInfo})` → `parseOrThrow(peeringInfo)` → `{peerState: {address: responder address}}`. It does not verify the responder's account exists.

#### 6.3.5 Outgoing settlement (`prepareSettlement`)

1. `client.autofill(tx)` where
   ```json
   { "TransactionType": "Payment",
     "Account": "<own address>",
     "Amount": "<ceil(amount / 1000)>",          // drops, decimal string; (amount + 999n) / 1000n
     "Destination": "<peerState.address>",
     "Memos": [ { "Memo": { "MemoType": "<hex of https://dassie.land/settlement/xrp-testnet/v1>",
                            "MemoData": "<hex of utf8(our NodeId)>" } } ] }
   ```
   xrpl.js autofill adds:
   - `Sequence` from `account_info` (current).
   - `Fee` from the server fee × load, capped at `maxFeeXRP` (default 2 XRP).
   - `LastLedgerSequence = current validated ledger index + 20`.
   - `NetworkID` only when the network id is > 1024, so not for mainnet or testnet.
2. `signed = wallet.sign(prepared)`.
3. Return:
   - `settlementId = signed.hash`, the uppercase hex tx hash as returned by xrpl.js.
   - `message = settlementProofSchema({transactionHash: bytes(signed.hash)})`.
   - `execute = async () => { await client.submitAndWait(signed.tx_blob); return {} }`. It logs before and after. `submitAndWait` throws if the transaction fails or expires, and the host only logs that.

The host-side pending transfer amount is the unrounded internal `amount`. The on-ledger payment is rounded **up** to whole drops, so the peer credits up to 999 internal units more than we debit (ACC-21).

#### 6.3.6 Transaction stream handler (the actual source of truth for settlement accounting)

For every `transaction` stream message, iterate `meta.AffectedNodes`. Act only on nodes that satisfy all of:
- `ModifiedNode.LedgerEntryType === "AccountRoot"`
- `FinalFields.Account === own address`
- `FinalFields.Balance` and `PreviousFields.Balance` are both strings and they differ.

Then:
```
newBalance = BigInt(FinalFields.Balance) * 1000 ; oldBalance = BigInt(PreviousFields.Balance) * 1000
balance = newBalance                         // module-local getBalance()
r = isSettlement(transaction)
if newBalance > oldBalance:
   if r.isSettlement && r.direction == "incoming":
        host.reportIncomingSettlement({ledgerId: L, peerId: r.peerId, amount: newBalance − oldBalance})
   else:
        host.reportDeposit({ledgerId: L, amount: newBalance − oldBalance})
else:
   if r.isSettlement && r.direction == "outgoing":
        assert(transaction.transaction.hash)
        host.finalizeOutgoingSettlement({settlementId: transaction.transaction.hash})   // posts pending transfer
   else:
        host.reportWithdrawal({ledgerId: L, amount: oldBalance − newBalance})
```

`isSettlement(tx)` (`functions/is-settlement.ts`):
1. `TransactionType !== "Payment"` → not a settlement.
2. Find a memo with `MemoType.toUpperCase() === XRP_SETTLEMENT_MEMO_TYPE.toUpperCase()`. None → not a settlement.
3. `peerId = utf8(hexDecode(MemoData ?? ""))`.
4. If `peerId === our NodeId` → `{isSettlement: true, direction: "outgoing"}`.
5. If the peer is unknown in the node table → warn "received settlement from unknown peer" → not a settlement, so it is treated as a deposit.
6. If the peer is not `peered` → warn → not a settlement.
7. If `peerState.settlementSchemeId !== "xrpl-testnet"` (hard-coded) → warn "wrong settlement scheme" → not a settlement. On mainnet `xrpl`, all incoming settlements therefore become deposits (ACC-13).
8. Otherwise `{isSettlement: true, direction: "incoming", peerId}`.

`handleSettlement` (peer message) only logs "received settlement claim". The XRPL module **ignores** the claimed amount and the proof hash. Credit happens only when the on-ledger transaction is observed. `getTransaction(client, hash)` (`functions/get-transaction.ts`, `tx` command, `txnNotFound` → false, asserts meta is an object) exists but is **unused**.

Other XRPL methods:
- `handleMessage` is a no-op.
- `handleDeposit` throws "not implemented".
- `getBalance` returns the tracked `balance`.

Errors: `isRippledErrorWithId(error, id)` (`utils/is-rippled-error.ts`) checks for `RippledError` with `data.error === id`.

#### 6.3.7 XRPL settlement sequence, end to end

| step | sender node A | XRPL | receiver node B |
|---|---|---|---|
| 1 | Tick: amount X computed. `prepareSettlement` → autofill and sign. | | |
| 2 | Pending Dr `xrpl-testnet+xrp:assets/interledger/B`, Cr `…:assets/settlement` (X). Map key `xrpl-testnet:<HASH>`. | | |
| 3 | Peer msg `settlement{ "xrpl-testnet", X, proof(hash) }` → B | | `handleSettlement` logs only |
| 4 | `submitAndWait(tx_blob)` | Payment validated | |
| 5 | Stream: own AccountRoot balance ↓ by ceil(X/1000)+fee drops; memo = A's NodeId → outgoing → `finalizeOutgoingSettlement(hash)` → post (X) | | Stream: B's AccountRoot ↑ by ceil(X/1000) drops; memo NodeId = A, peered via xrpl-testnet → `reportIncomingSettlement(A, ceil(X/1000)*1000)` → posted Dr `assets/settlement` Cr `assets/interledger/A` |

The fee and the rounding difference are never booked on A's side, so `assets/settlement` drifts from the real on-ledger balance (ACC-2, ACC-21).

### 6.4 Reserve handling

There is none. The whole `Balance`, including the XRPL base reserve and owner reserves, is reported as a deposit and shown as `getBalance`. Settlements are not checked against the spendable balance, so an overdraft fails on-ledger with `tecUNFUNDED_PAYMENT` or a similar code (ACC-19). There are no payment channels, no trust lines and no destination tags. Settlements are direct XRP `Payment`s.

### 6.5 Deposits and withdrawals (owner funds)

- A deposit is any validated tx that increases our AccountRoot balance and is not an incoming settlement from a peered xrpl-testnet peer. It becomes posted Dr `assets/settlement`, Cr `equity/owner`.
- A withdrawal is any decrease that is not an outgoing Dassie settlement. It becomes posted Dr `equity/owner`, Cr `assets/settlement`. Fees of non-settlement txs are included because the whole balance delta is used.
- The node has no UI or RPC for withdrawing XRP. Withdrawals happen externally with the same keys.

### 6.6 Module gaps summary

- `xrpl` mainnet is not registered.
- The memo type and the `isSettlement` scheme check are both hard-coded to testnet.
- Neither module uses `supportedVersions` or `config`.
- `cancelOutgoingSettlement` is never called.

---

## 7. Timers and constants (consolidated)

| Name | Value | File |
|---|---|---|
| DEFAULT_CREDIT (trust per peer) | 1_100_000_000n (1.1 units) | `accounting/functions/manage-peer.ts` |
| SETTLEMENT_CHECK_INTERVAL | 4000 ms | `ledgers/send-outgoing-settlements.ts` |
| SETTLEMENT_RATIO | 0.2 (threshold = midpoint × 1.1) | same |
| RATIO_PRECISION | 8 | same |
| Stub INITIAL_BALANCE | 100_000_000_000n (100 USD) | `ledgers/modules/stub.ts` |
| XRP_VALUE_FACTOR | 1000n | `ledgers/modules/xrpl/constants/asset-scale.ts` |
| XRPL memo type | hex("https://dassie.land/settlement/xrp-testnet/v1") | `ledgers/modules/xrpl/constants/settlement-memo-type.ts` |
| XRPL testnet server | wss://s.altnet.rippletest.net:51233 | `ledgers/modules/xrpl/xrpl-testnet.ts` |
| XRPL mainnet server | wss://s1.ripple.com/ | `ledgers/modules/xrpl/xrpl.ts` |
| xrpl.js autofill LastLedgerSequence offset | +20 ledgers | xrpl.js 3.1.0 |
| MAX_PACKET_AMOUNT | 100_000_000n (0.1 unit) | `ilp-connector/constants/max-packet-amount.ts` |
| ILP_MESSAGE_WINDOW / MAXIMUM_HOLD_TIME | 1000 ms / 30000 ms | `ilp-connector/constants/expiry-constraints.ts` |
| Packet timeout (void transfers) | 5000 ms fixed | `ilp-connector/functions/schedule-timeout.ts` |
| PEERING_CHECK_INTERVAL / MINIMUM_PEERS | 1000 ms / 2 | `peer-protocol/maintain-peering-relationships.ts` |
| Peer message default timeout | 30000 ms | `peer-protocol/constants/timings.ts` |
| INTERNAL_PRECISION (FX) | 15 | `exchange/constants/internal-precision.ts` |
| Currency scales | USD 9, XRP 9 | `exchange/constants/currencies.ts` |
| VALID_REALMS | "test", "live" | `constants/general.ts` |
| SEED_PATH_SETTLEMENT_MODULE | "node/settlement-module" | `constants/seed-paths.ts` |

---

## 8. Database migrations relevant to this area

All in `database/migrations/`, listed in order by `index.ts`. Every table is `STRICT`.

| # | file | content |
|---|---|---|
| 0004 | `0004-create-settlement-schemes-table.ts` | `settlement_schemes(id TEXT PK NOT NULL, config TEXT NOT NULL)` |
| 0005 | `0005-create-nodes-table.ts` | `nodes(id TEXT PK, public_key BLOB, url TEXT, alias TEXT)`. Its rowid is referenced by `peers.node`. |
| 0006 | `0006-create-peers-table.ts` | `peers(node INTEGER PK NOT NULL, settlement_scheme_id TEXT NOT NULL)` |
| 0010 | `0010-add-settlement-state-column.ts` | `ALTER TABLE peers ADD COLUMN settlement_scheme_state TEXT NOT NULL DEFAULT "{}"`. The default literal uses double quotes, so SQLite treats it as a string fallback. |
| 0011 | `0011-create-accounts-table.ts` | `accounts(path TEXT PK NOT NULL, debits_posted INTEGER NOT NULL DEFAULT 0, credits_posted INTEGER NOT NULL DEFAULT 0)` |
| 0002/0003 → 0012 | `0012-change-payment-tables-for-open-payments.ts` | Recreates `outgoing_payment(id PK, destination, ledger TEXT, total_amount INTEGER, metadata TEXT)` and `incoming_payment(id PK, ledger TEXT, total_amount INTEGER, metadata TEXT)`. `ledger` is the owner LedgerId (`stub+usd`), and amounts are in that ledger's units. The original 0002/0003 versions used `subnet` columns. |

Every migration has a `down` that drops or reverts it. Migrations 0001 (scalar), 0007–0009 and 0013 are outside this area.

---

## 9. Doc vs. code divergences (`docs/implementation-notes/internal-ledger.md`)

- The doc's example ledger ID is `xrp` (e.g. `xrp:assets/settlement`). Code IDs are `xrpl-testnet+xrp` and similar.
- For a same-currency payment, the doc shows a second fee transfer: Dr `xrp:liabilities/peerA/interledger`, Cr `xrp:revenue/fees`. **Not implemented.** No fee is charged and no `liabilities/…` account exists.
- For a cross-currency payment, the doc shows 3 transfers including a fee to `revenue/fees`. The code uses 2 transfers and no fee.
- "Settlement from peer A" in the doc (Dr assets/settlement, Cr assets/interledger/peerA) matches the code's `reportIncomingSettlement`.
- `docs/implementation-notes/peer-state.md` describes termination, settle-on-terminate, the `advertise` flag, and the `request-peering`, `terminating` and `banned` states. **None of these are implemented** in the accounting or settlement code.
- `docs/implementation-notes/token-payment-protocol.md` is a design note only, with **no implementation** in the codebase. Its content:
  - Pay node A for a service, and get proof of dishonesty if A takes the money without performing.
  - Naive version (price fits in one ILP packet): A signs a quote that contains `executionCondition`, expiry and signature. The proof of dishonesty = the quote + the fulfillment + the signed publication that lacks the service.
  - Improved version: price in "tokens" issued by A, each purchasable with one ILP packet. Steps 1–15:
    1. B requests a quote. A returns a signed quote: description, price in tokens, expiry, signature.
    2. B probes the token price with an unfulfillable packet. A's reject carries the price ratio.
    3. For each token, B requests a signed token quote with a unique public key and a condition, then pays one packet. A fulfills.
    4. B redeems the tokens. A returns a signed acknowledgement.
    5. If there is no acknowledgement, B submits via witnesses W. Each witness returns a signed report: identity, timestamp, URL, request, response, signature.
  - Attack vectors discussed: head-in-the-sand, false double spend (A must show the spend tx signed by the token holder), "closed for today", and DoS (mitigated by generous deadlines and hourly retries).
  - No wire formats are defined. It is safe to treat this as out of scope or future work.

---

## 10. Open questions / known gaps (consolidated)

- **ACC-1** `liabilities/…` is documented but has no account types. Peer debt is represented as a negative balance on `assets/interledger/<peer>`.
- **ACC-2** `equity/suspense` is created but never used. It was meant to absorb mismatches between `assets/settlement` and the real on-ledger balance. Fees and rounding currently drift untracked.
- **ACC-3** `revenue/fees` and `expenses/fees` are never used. There is no connector fee or spread, and no node-registration fee booking.
- **ACC-4** `createAccount` overwrites an existing in-memory account. Pending counters reset, which corrupts state if pending transfers exist on that account.
- **ACC-5** The initial trust transfer of 1.1e9 is re-posted and persisted on every peer-actor start (restart or re-peer), so credit grows without bound. `cleanupPeer` does nothing; pending transfers are not voided on unpeer.
- **ACC-6** On every start each settlement module reports its full on-ledger balance as a new deposit. Stub reports 100 USD; XRPL reports the account balance. Because posted totals persist, `assets/settlement` and `equity/owner` inflate on each restart.
- **ACC-7** A cross-ledger prepare whose second transfer fails leaves the first transfer pending forever.
- **ACC-8** A same-account transfer, e.g. owner→owner, throws an AssertionError instead of returning a failure.
- **ACC-9** `equity/owner` has no limit, so the owner can spend beyond their balance. The owner ledger is hard-coded to `stub+usd` even on XRPL-only nodes, so owner payments to XRPL peers always go through FX (USD→XRP).
- **ACC-10** `removeSettlementScheme` truncates the in-memory list with `splice(1)`.
- **ACC-11** `ActiveSettlementSchemesSignal` updates are produced from the initial set, so multiple runtime changes are lost.
- **ACC-12** The `settlement_schemes.config` JSON is never passed to modules.
- **ACC-13** The `xrpl` mainnet module is not registered. Its memo type says "xrp-testnet", and `isSettlement` requires `settlementSchemeId === "xrpl-testnet"`, so it would treat every mainnet incoming settlement as a deposit.
- **ACC-14** `supportedVersions` is declared but no version negotiation exists.
- **ACC-15** `peerState` returned from `prepareSettlement` or `execute` is ignored. The peer state is never updated after peering.
- **ACC-16** Pending transfers and `PendingSettlementsMap` are memory-only. After a restart:
  - In-flight settlements are lost from the ledger: the pending debit disappears.
  - A late stream event for them makes `finalizeOutgoingSettlement` throw inside the XRPL event handler.
  - If `execute()` fails, nothing voids the pending transfer, because the XRPL module never calls `cancelOutgoingSettlement`. The amount stays locked until restart.
- **ACC-17** The settlement task handler is synchronous and fires async work, so the overlap policy has no effect. If XRPL `prepareSettlement` (autofill network I/O) takes longer than 4 s, the next tick computes the same unsettled balance, because the pending debit does not exist yet. That can produce a duplicate settlement.
- **ACC-18** The `peers` table is never written by the app. Runtime peerings do not survive restart.
- **ACC-19** No XRPL reserve handling. The reserve is counted as spendable and reported as owner equity.
- **ACC-20** On mainnet, account activation creates a `CreatedNode`, not a `ModifiedNode`. The handler ignores it, so the initial funding is never booked.
- **ACC-21** XRPL amounts are rounded up to whole drops on-ledger, while the sender books the unrounded amount. Transaction fees are never booked.
- **ACC-22** The XRPL stream handler does not check `meta.TransactionResult`. A failed `tec*` Payment that carries our memo still reduces our balance by the fee, and is treated as a successful outgoing settlement: the pending transfer is posted for the full amount, but the peer never receives anything. Incoming payments are attributed by MemoData only. The sender `Account` is not checked against `peerState.address`. That is safe for us, since we did receive the funds, but the attribution is spoofable.
- **ACC-23** The stub `handleSettlement` trusts the peer-claimed amount. Anyone peered can mint credit. This is acceptable only because stub is test-realm only.
- **ACC-24** `incomingCredit` is assumed equal to `outgoingCredit` (TODO in source). There is no protocol to learn the credit a peer extends to us, and no settlement request from creditor to debtor.
- **ACC-25** `getLedgerIdForSettlementScheme` throws for peers whose scheme is not loaded. That crashes the per-peer actors, the packet handling and the routing lookups for that peer.
- **ACC-26** Startup awaits all settlement module behaviors, so XRPL connect and faucet funding block node startup. A module failure (e.g. a realm mismatch) propagates through `LedgersActor`.
