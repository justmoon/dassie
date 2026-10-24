# Settlement hubs

Peering rests on trust today. Settling is up to the node that owes, so a peer can always walk away with what it owes, and the credit a node extends bounds the loss ([own-fulfills](../decisions/own-fulfills.md)). A **settlement hub** is another way to peer, for nodes that choose it: a small ledger kept by a few validators and backed by an escrow on an underlying ledger such as the XRP Ledger. Two nodes that peer over a hub cannot walk away with what they owe each other: what one owes the other can be taken from funds the hub holds, without the debtor's help.

A hub moves trust, it does not remove it. Instead of trusting each peer up to its credit, a node trusts one set of validators with the balance it keeps in the hub. That set is known in advance, its members are chosen for their reputation, and one relationship covers every peer in the hub.

**A hub is regional.** Its validators, and the members that peer over it, are within a few tens of milliseconds of each other, so a block is final in well under a quarter of a second and a packet over a hub keeps the connector's timing (§4). Nodes far apart peer over different hubs, or directly. Peering directly, with settlement up to the node that owes, stays; a hub both peers are in is preferred over it.

This is a design ([settlement-hubs](../decisions/settlement-hubs.md)). The rules, the ledger, consensus, the escrow, founding a hub, payout bundles and a member's side are built ([Settlement hubs](../areas/hubs.md)); the rest is not yet. Durations and sizes in it are starting points, to be set from the deterministic simulation when the hub is built.

## 1. Concepts

| Term | Meaning |
|---|---|
| **Hub** | A ledger, its validators and its escrow, identified by the hash of its rules. |
| **Validator** | A Dassie node that keeps a hub's ledger and co-signs its escrow. A hub has n = 3f + 1 of them, typically 4 or 7. |
| **Quorum** | q = 2f + 1 validators. |
| **Rules** | The hub's founding document: validators, asset, underlying ledger, fees, reserves, limits, timers. Everything but the validators is the **terms**. |
| **Member** | A node with an account in the hub. |
| **Escrow** | The account on the underlying ledger that holds every member's funds, controlled by the validators together. |
| **Hold** | Funds a payer sets aside in the hub for one payee, who can take from them with the payer's signed promises (§4). |
| **Sponsor** | The node that founds a hub. It has no powers once the hub runs, but for making the escrow again after a reset of a test network (§6). |

**What the validators can do.**

- With at most f of them faulty or malicious, the hub is safe and keeps running.
- Any f + 1 can halt it by refusing to take part.
- f + 1 that collude can sign two conflicting histories, each with a valid certificate. The two certificates name them: whoever signed both is proven to have misbehaved (§10).
- Taking funds from the escrow directly needs q of them, the escrow's signing quorum.

**How a node sees a hub.** The hub is a ledger a node does not control, like the XRP Ledger. Its balance there is an asset the node books, and the hub is a counterparty with its own exposure limit (§11). Nodes still keep their own books ([own-books](../decisions/own-books.md)); the hub is the one ledger two members agree on, as two XRPL users agree on the XRP Ledger.

## 2. Founding a hub

1. **The sponsor fixes the terms and picks n validators** that are all on its list of acceptable validators (§11), and asks each to accept them. A validator accepts only from sponsors its owner lists, and validates no more hubs than its own limit: nothing else stops a stranger from having it make keys and keep a hub it never wanted. Validating is something an owner turns on.
2. **Each validator makes three keys for the hub,** derived from its node seed under a path that names a random nonce the sponsor picked for this hub. The hub ID cannot name them: it is the hash of rules that contain these keys.
   - a **consensus key** (Ed25519), with which it signs votes, certificates and receipts;
   - an **escrow key**, an XRPL key whose account address becomes one entry of the escrow's signer list;
   - a **payout key**, an extended secp256k1 public key (BIP 32), from which anyone can derive the keys this validator signs a payout set with (§9), while only the validator can sign with them. With non-hardened derivation, one leaked child key and the public parent give away every child key, so the validator guards these as it guards its escrow key.

   It returns its acceptance, signed with its node key: the nonce, the hash of the terms, the three public keys, and its endpoint. An acceptance names no other validator, so one that does not accept, or fails before genesis, is replaced without touching the acceptances already given.
3. **The rules are fixed** once n validators accepted: the terms and the acceptances; the hub ID is their hash. Every validator signs the final rules, which is the genesis certificate, so each sees the whole set before it commits to it. A validator signs at most one genesis per nonce, so its acceptance cannot found two hubs with different co-validators.
4. **The sponsor creates the escrow account** on the underlying ledger, with a master key derived from its node seed and the nonce, and, from that key, sets it up as the rules say:
   - a signer list of the n escrow keys, each with weight 1, quorum q;
   - `RequireDest`, so a payment without a destination tag fails;
   - no regular key, then the master key disabled, last.
5. **The validators check the escrow** on their own XRPL servers before the first block: master key disabled, no regular key, the signer list exactly as the rules say, `RequireDest` set, and no other objects. A sponsor that kept a way in is caught here, and the hub never starts. Each reads this from transactions it proved in ledgers it verified (§6), never from a server's account objects: the flags, the regular key and the owner count from the escrow's account root, tied to its transaction, and the account root's chain proven back, without a gap, to the Payment that created the account. Every transaction the escrow signs changes its account root, so that chain holds each `SignerListSet` and `SetRegularKey` it ever signed, and the signer list is the one the last `SignerListSet` in it made. A signer list tied on its own is not enough: a tie may show a real but old state, and an old list with only the validators' keys would hide a later one that adds the sponsor's. "No other objects" is an owner count of one, the signer list, which the tied account root states, since no server can prove that a list of objects is complete. The escrow is young at setup, so the chain is short. A tied root may still be older than the ledger's, but once it shows the master key disabled and no regular key, every later change needs a quorum of the signer list the chain proved, so a check that passed stays true.
6. **The sponsor deposits** (§6), and the validators credit its account.
7. **The sponsor advertises** that it is a member, in its signed node info (§11). Other nodes that want to peer with it join the hub.

**One network per escrow.** A transaction on the XRP Ledger testnet or mainnet names no network, so the same bytes would apply to an account of the same address on the other. An escrow's address derives from the sponsor's key and the hub's nonce, and a validator signs one genesis per nonce, so an escrow, and every key made for it, belongs to one network's hub: a payout set signed for a testnet hub can never pay out a mainnet escrow. The rules name the network, and a validator follows the escrow only on that network's ledger, against that network's validators.

## 3. The ledger

**State.**

- **Accounts:** a number (assigned in order, never reused), the owner's public key, a balance, the reserves it locks (§7), a disbursal address on the underlying ledger (address and tag), and a sequence for the owner's operations.
- **Holds:** a number, payer, payee, amount, whether it is closing and when, and what has been taken from it (§4).
- **Escrow:** the balance the hub has seen deposited and withdrawn, the reserves the account needs, and the escrow's next sequence on the underlying ledger.
- **Pending:** deposits seen by fewer than q validators, and withdrawals not yet settled on the underlying ledger.
- **Validator accounts,** which receive fees, and a **reserve account** (§7).

Every table has a limit in the rules; an operation that would exceed one fails, it never grows the table ([tigerstyle](../decisions/tigerstyle.md)).

**Operations.** A member signs each operation with its account key. Every signature covers the hub ID, the account number, the account's next sequence, and the most fee it pays. A signature therefore cannot be replayed in another hub, or twice in the same one: account numbers are never reused, and sequences only grow.

| Operation | By | Effect |
|---|---|---|
| `transfer` | owner | Moves funds to another account. |
| `pay` | owner | Like `transfer`, but creates the payee's account (§5). |
| `set-disbursal` | owner | Sets where the account's funds go at shutdown (§9). |
| `hold-open`, `hold-fund` | payer | Sets funds aside for a payee, or adds to a hold. Opening one locks a hold reserve (§7). |
| `hold-reduce` | payer | Takes funds back from a hold, after the hold's notice period. Reducing it to zero closes it and frees its hold reserve. |
| `hold-release` | payee | Gives a hold toward it back to its payer at once. Only holds an account pays into count toward its limit per account, so no one can use up another's room by opening holds toward it. |
| `redeem` | payee | Takes from a hold with the payer's latest signed claim (§4). |
| `claim` | payee | Takes from a hold with the payer's signed commitments and their fulfillments (§4). |
| `withdraw` | owner | Pays funds out to an address on the underlying ledger (§6). |
| `close` | owner | Pays out the account's balance and its account reserve, to an address on the underlying ledger as `withdraw` does (§6), or to another account. Refused while the account has holds, as payer or payee, or withdrawals pending. Paid to another account, it deletes the account at once; paid out on the underlying ledger, only once the payout settled. Until then the account is closing: it takes no credits and no holds as payee, a `transfer`, `pay` or `hold-open` to it fails, and a deposit to its tag is paid back as one to a tag with no account (§6). A payout that fails returns the funds to the account, less the fee; the account locks its account reserve again from them and is open as before. |
| `attest-deposit`, `attest-withdrawal` | validator | Reports what it saw on the underlying ledger (§6). |
| `reconfigure` | validators | Changes the validators or the rules (§8). |
| `set-endpoint` | validator | Changes the validator's own endpoint, at once. |
| `shutdown` | validators, or the rules | Ends the hub (§9). |
| `reset` | validators | Writes off what a reset of a test network took (§6). |

**Invariant,** asserted after every block: the balances and reserves of all accounts, holds, pending withdrawals, the validator accounts and the reserve account add up to no more than the escrow's balance less its reserves. Each validator also publishes the hub's total supply with its certificate, so anyone can compare it with the escrow's balance on the underlying ledger, and a member that sees a shortfall leaves.

**Receipts.** A member learns the outcome of an operation from a receipt: the operation's hash, the height of its block, its result, the block's certificate, and the path from the block's root to the result. Validators send a receipt to everyone the operation touches: its signer, and a hold's payee when the hold changes. A member they cannot reach, behind a network that takes no connections or on the HTTPS fallback, asks for its receipts instead, as often as its deferred Rejects (§4) need. A member that wants to know a balance asks; f + 1 matching signed answers at the same height include one from an honest validator.

**Time.** Everything that depends on time is judged by the block's timestamp, never by a validator's clock: a packet's expiry, a hold's notice period, shutdown. Timestamps follow CometBFT's proposer-based timestamps: the leader stamps a block, and a validator votes for it only when the stamp is within the rules' precision of its own clock, allowing for the message delay. Members leave that much margin.

## 4. Peering over a hub

Two members peer with the hub as their settlement scheme. Each direction has its own hold: the payer A sets funds aside for the payee B, and **the hold is the credit B extends to A**, all of it and nothing more. B is paid from the hold whether A cooperates or not, so the credit costs B nothing, and B extends nothing beyond it.

**Settling is funding the hold.** Redeeming takes from the hold and from what B has not yet taken alike, so it leaves B's room for A as it was; only A's funds grow it. Settlement stays on request ([settlement](../areas/settlement.md#settling-on-request)): when B's room for A runs low, B asks, and A answers with a `hold-fund` of the amount it would otherwise have paid. A payer opens a hold at the least credit a peering needs, and keeps it there by answering requests.

**Packets.**

- With every Prepare it sends B, A sends a **commitment**: its signature over the hub ID, the hold, a sequence (increasing per hold), the amount, the condition and an expiry: the Prepare's own expiry less the hub's margin M (below). It travels in the same message as the Prepare, a message of its own in the peer protocol, so B never sees a Prepare without its commitment.
- B forwards the packet at once, as long as the packets in flight and what it has not yet taken from the hold fit in the hold. The hub is not involved.
- When the Fulfill comes back, A's reply to it ([own-fulfills](../decisions/own-fulfills.md)) carries a **claim**, after the byte that accepts it: A's signature over the hold, the total T it owes B on the hold, and the sequence W up to which T covers every fulfilled packet. B accepts a claim only when T is right by its own record of what it fulfilled.

**Taking from the hold.**

- `redeem (T, W)` pays B what the claim adds, less anything B already took with `claim` for packets up to W. W only grows, and T only grows.
- `claim` is for when A's claim does not come. B submits commitments above the last redeemed W together with their fulfillments; the hub checks that each fulfillment hashes to the commitment's condition and that the block is stamped before the commitment's expiry, then pays the amounts. Each hold remembers the sequences claimed this way until a `redeem` passes them, so nothing is paid twice. The table belongs to the hold, not the hub, and has room for at least as many as the claims B lets A leave missing (1024), so other members cannot fill it to make B's claims fail. Only a commitment's own expiry decides: once it passed, the packet can no longer be claimed, as in ILP.
- B redeems often enough that what it has not taken stays a small part of the hold, much as a node asks a peer to settle well before its credit runs out ([settlement](../areas/settlement.md)). Every redeem costs a fee, so B does not redeem after each packet.

**Why A is safe.** B is paid only for what A signed: a claim, or a commitment with its fulfillment in a block stamped before the commitment's expiry.

- **A learns every claim.** The validators send A the receipt of each `claim` on its holds, so A learns the fulfillment and fulfills upstream.
- **The hub overrules A's books.** When A did not take B's Fulfill, for being late say, and B then wins a claim for the packet, A's books follow the hub: the packet counts as fulfilled. The `hub` scheme specifies this with the claim.
- **A does not reject early.** When its outgoing packet times out at its expiry E, A does not reject upstream at once, as [connector-timing](../decisions/connector-timing.md) has a node do: B may still win a claim stamped before the commitment's expiry. A waits for a final block stamped at or after that expiry. Blocks become final in order and their stamps only grow, so that block settles it: if no claim for the packet came before it, A rejects; if one did, A fulfills. A peering over a hub is the one exception to [connector-timing](../decisions/connector-timing.md)'s timeout. B does not have to cheat for this to matter: its Fulfill only has to reach A after E.
- **The margin M** is the hub's timestamp precision plus the longest it takes to finalize a block with f validators faulty: f views skipped, each after its timeout, then a block. The rules state it. The commitment expires M before the Prepare, so the block that settles a packet is normally final by E.
- **The window.** After E, A still needs the receipt or the final block from a validator, and its Fulfill or Reject must then reach its upstream before the upstream's own expiry. B, on its side, needs time after its downstream answers to get a claim to the validators before the commitment's expiry. The rules state M and an allowance for these legs between members, which a regional hub keeps small. A node peers over a hub only when M plus the allowance fits in the window the connector keeps between an incoming and an outgoing expiry ([connector](../areas/connector.md)), so a peering over a hub needs no window of its own. The window is fixed by the rules, not measured: a peer cannot widen it by answering slowly, and a member whose legs are slower than the allowance learns of some packets too late, a cost it books (§13).
- **Outside the fault model,** with more than f faulty validators or a network slower than the rules assume, A can still learn of a claim too late to fulfill upstream. That loss is at most what A had in flight to B (§13).

**Why B is safe.** B never forwards more than the hold covers. A hold cannot shrink before its notice period, which the rules make longer than any packet can be held (30 s, [connector-timing](../decisions/connector-timing.md)), so B learns of a reduction before it matters, and accepts no Prepare that expires after the hold would shrink. If A stops sending claims, B claims on the hub before each commitment's expiry, which may take most of the window, so B submits a claim as soon as a claim from A is late, and stops taking Prepares from A when too many are missing (1024).

**Liquidity.** A's funds are set aside up to the size of its holds, not only for the seconds a packet is in flight. A hold only has to cover what is in flight plus what B has not yet taken, and resizing it is one operation. What B redeems lands in its own balance, from which it funds its holds toward other peers, so the same escrowed funds serve many peerings.

**What the validators see:** balances, holds, redeemed totals and withdrawals. Conditions and amounts of single packets only reach them in a dispute. ILP uses the same condition on every hop, so routing everything through hubs would let their validators follow a payment.

## 5. Joining

A node joins a hub in one of two ways.

- **By deposit** (§6): a deposit with destination tag 0 and the new account's public key as its memo creates the account, which locks the account reserve (§7) from it and keeps the rest as its balance. A deposit below the hub's minimum for a new account is paid back to its sender, less a fee.
- **By being paid.** A peers with B over a hub, and B has no account yet. Who trusts whom at first follows [earned-trust](../decisions/earned-trust.md):
  - **A asked to peer,** so B extends it nothing. A submits `pay` before it sends any packet, which creates B's account with the key B gave in the peering and pays it the hub's minimum for a new account. From then on the peering works as in §4.
  - **B asked to peer,** so B extends A its starting credit: A sends packets on it, which that credit bounds, and submits `pay` once it owes B the hub's minimum for a new account.

**The hub's minimum for a new account** is the account reserve, which the account locks, plus a starting balance in the rules, sized to pay the fees of its first operations (§7), so that a member paid into the hub needs no deposit before it can act. A new account must set its disbursal address before the hub accepts any other operation from it. Without one, its funds at shutdown stay in the escrow (§9).

## 6. Deposits and withdrawals

**Deposits.** A member pays the escrow on the underlying ledger with its account number as the destination tag; the rules keep account numbers below 2³², so they fit one. Each validator follows the escrow's transactions on its own XRPL servers, and attests a deposit once it has proven it in a validated ledger: a transaction counts once it is in the transaction tree of a ledger whose header a quorum of the network's UNL signed, or that chains back to one, with the UNL the validator keeps for the network the rules name ([checking the validators](xrpl-validation.md)). A server's word that a ledger is validated attests nothing. The deposit must be:

- the result is `tesSUCCESS`, and the transaction is a Payment to the escrow with a destination tag;
- the amount is the ledger's `delivered_amount`, never `Amount`: a partial payment can name a large `Amount` and deliver almost nothing;
- the asset is the hub's;
- the transaction's hash was not credited before.

Once q validators attested the same deposit, the hub credits the account. A deposit to a tag that has no account, or whose account is closing (§3), is paid back to its sender, less a fee.

**Withdrawals.**

1. `withdraw` takes the amount and the fee from the owner's balance and gives the withdrawal the escrow's next sequence.
2. Each validator builds the same transaction from the block: a Payment with that sequence, the fee the rules set, the destination and tag, and no partial-payment flag. It signs it alone, with its escrow key, and sends its signature to the other validators and the owner.
3. Anyone with q signatures submits the transaction.
4. Each validator attests the outcome. Once q did, the hub burns the amount, or, when the transaction failed (a `tec` result, which still uses the sequence), returns it to the owner, less the fee.

**The underlying ledger's rules apply.**

- Paying a new XRPL account needs at least the base reserve (1 XRP), so the hub's least withdrawal is at least that.
- The escrow's own reserves never back a balance.
- The asset is XRP. An issued currency is backed by its issuer, which can freeze or claw back the escrow's balance.

**A reset of a test network** takes the escrow with everything else. Validators tell a reset from a server that is behind as the XRP scheme does ([ledger-resets](../decisions/ledger-resets.md)): a validator attests one only once the testnet's UNL validated the new chain, so no server can make it attest a reset. On the test network, once q validators attested the reset, whatever ledger of the new chain each saw it at, the next block contains `reset`: one faulty validator, or one lying server, cannot wipe the hub. For a while after, the hub takes no attestation of a reset, so votes that arrive late for this one never add up to another. It writes off every balance, hold, reserve and pending withdrawal, and keeps the accounts, each of which locks its account reserve again from what it next receives. The sponsor makes the escrow again from the same derived key, so at the same address, and sets it up as at founding; the validators check it as before the first block (§2), and the hub goes on under the same ID, so no release has to name a new hub. Until that check, the hub takes no deposits and says so, and members send none: the sponsor's derived key controls the address until it is disabled. Members write off their hub balance as the XRP scheme writes off its own. On any other network, mainnet included, the escrow's proven absence stops the hub, and a server's word that it is absent makes the validator wait.

**Why native multi-signing.** Each validator's signature stands alone, so signatures can be collected over days and from validators that are never online together, which shutdown needs (§9). A threshold key (FROST) would let the validators sign with one key, but its signing is interactive among a fixed group, and a share made for one group cannot be used with another.

## 7. Fees

- **Per operation.** Each operation pays the fee in the rules, from the signer's balance. The fee is flat: it does not follow how full blocks are ([roadmap](../roadmap.md#settlement-hubs)). A share of it, in the rules, goes to the reserve account, which pays the underlying ledger's fees for the hub's own transactions (signer lists, the selector of §9) and covers shortfalls.
- **Reserves.** Each account locks an account reserve, and each hold a hold reserve from its payer's account, both set by the rules, as the XRP Ledger reserves funds for each account and each object it owns. A reserve is never spendable and never backs a hold. It prices what an account or a hold takes from the hub while it exists: a place in the bounded tables (§3), and for an account its payment in the payout set (§9). The rules set the account reserve at least at the least payout plus the payout fee, so that every account is paid out at shutdown. A hold reserve is freed when its hold closes; the account reserve comes back with `close`, or in the payout set.
- **Abandoned accounts** keep their place in the tables, which their account reserve paid for. Nothing charges them for idling yet ([roadmap](../roadmap.md#settlement-hubs)).
- **Deadlines first.** `claim` and `redeem` have deadlines, so a share of every block (in the rules) is kept for them, and they go first. Ordinary operations cannot crowd them out; only other claims and redeems can, each paying its fee. The rules size the space so that the hub's margin M holds as long as that space is not full.
- **Who gets fees.** Different validators can hold certificates of one block signed by different q, so the fee split cannot follow certificates. Instead, each block records every vote for the last K blocks (K in the rules, at least n, since a slow validator may only have its votes recorded in its own blocks) that its proposer received and no block has recorded yet, and a proposer records its own. A vote that comes later than that is not counted. The fees of a block go, in equal parts, to the validators whose votes for it are recorded. That is part of the log, the same for every validator. A validator that does not take part earns nothing; one that is live but slow still has its votes recorded, by later proposers or in its own blocks. Over the period that decides shutdown (days) it proposes many; if leaders are picked at random it may miss some fees, but it still counts as live.
- **Changes.** Fees change only through `reconfigure` (§8), within caps in the rules, and with notice longer than it takes to leave.

## 8. Changing the validators

The set of validators changes; a hub whose set cannot change dies with its first lost validator.

- **Endpoints.** A validator changes its endpoint with `set-endpoint`, signed by itself, at once. A Fedimint federation lost a guardian for good when its domain was taken and no guardian could change its address.
- **Members of the set, keys and rules.** `reconfigure` commits like any operation, then takes effect after the rules' notice period. During the notice, members that do not accept the new set leave, and pay no withdrawal fee.
- **Taking effect.** The old set signs the escrow's new signer list (`SignerListSet`, with the next escrow sequence), new validators load the state from the last certified block, and the new set takes over at a height.

## 9. Shutdown

A hub ends in one of two ways: a **shutdown** the log commits, which is orderly, or a **stall**, when the hub can no longer commit anything. Both end in the same kind of payout: a set of transactions on the underlying ledger that each validator signs alone, whenever it can.

**Shutdown.** A hub shuts down:

- when no vote of a validator has been recorded (§7) for a period in the rules (days), and it has not been replaced: the next block must contain `shutdown`. This happens while q validators are still live;
- when q validators vote for it.

After `shutdown` the hub takes no new holds, transfers or withdrawals, and for one notice period payees may still redeem and claim. Then the last block fixes the final balances and contains the hash of the payout set.

**Stall.** A hub that lost more than f validators commits nothing more. When a validator has seen no block become final for a period in the rules, it declares the hub stalled, tells the other validators and the members, and **never votes in the hub again**. Its last state is then the latest final block it knows of. A validator that was only cut off alone declares too, and is from then on gone for a hub that carries on without it, which replaces it (§8) or shuts down.

- **The period** is set from the simulation, long enough that the partitions between validators a hub should survive end first: a partition that outlasts it makes every validator declare, and ends a hub that would have recovered, with no notice period for payees.
- **Passing views around.** For a period in the rules before it signs, and for as long as it is up after, a validator sends the highest certified block it knows of to the other validators and the members, and asks for theirs. A certificate proves itself, so a validator that learns of a higher final block takes it, with the state after it (below), and signs the payout set for that block too. It never signs one for a lower block than it already has.
- **Who holds the proof.** A member credited in a block holds that block's receipt and certificate (§3). The members who would lose by a payout from an earlier state are those who hold the proof that a later one exists, and they have every reason to pass it on.
- **What a stall costs.** There is no notice period: holds go back to their payers, so payees lose what they had not yet taken. If a set for an earlier block wins (below), what happened after that block is lost too. That happens only if q validators signed it before they heard of the later block: a set that ever reached q signatures stays valid, and whoever prefers it can submit it first. Waiting before signing makes this unlikely, not impossible. Both fall under each member's exposure limit (§11).
- **Why the hub cannot go on afterwards.** A payout set needs q signatures, so at least f + 1 honest validators that declared a stall signed it. Those never vote again, and the remaining 2f validators are fewer than q, so no block can become final after a stall payout could complete.

**The state.** Every block carries the root of a Merkle tree over the state after it. A validator that lacks a block's state takes it from anyone that kept it, a validator or a member, and checks it against the certified root.

**The payout set** for a final block is computed from the state after it and from public data, and is the same for every validator, so any validator can build it alone:

1. every withdrawal that has an escrow sequence but is not yet settled, as already signed;
2. the **selector**: a `SignerListSet` that replaces the escrow's signers with keys made for this set alone, each with weight 1, quorum q. Every validator of the current set has one entry, including those that are gone: its key is the non-hardened child of its payout key (§2) at the block's height, split into two levels of 31 bits so any height fits. Anyone can derive every entry from the payout keys; only each validator can sign with its own. Signer entries need not be funded accounts;
3. one Payment per account whose balance and reserves come to more than the least payout and that has a disbursal address, in account order, less the payout fee, signed with the set's own keys. Holds go back to their payers first, and validator accounts are paid like any other. What remains (balances too small to pay, accounts without an address, payments that fail) stays in the escrow.

All of them have consecutive sequences, the fee in the rules, and no last ledger, so they stay valid until they are applied.

**The underlying ledger breaks ties.** Validators may have signed sets for different blocks. The first selector the XRP Ledger applies decides which set pays out:

- every other selector needs the same sequence, or an earlier one, and can no longer apply;
- the ledger checks signatures against the signer list in place when a transaction applies, so every payment of another set is now signed by keys that are not on the list, and is refused without being applied (`tefBAD_SIGNATURE`). It uses no sequence, so it cannot take the place of the winning set's payments;
- where one set has a withdrawal at the sequence of another set's selector, either the withdrawal applies, and the other set can never pay out, or the selector does, and its set pays that member from the balance it had before the withdrawal.

Two sets can therefore never be mixed, which would pay out more than the escrow holds. After a shutdown there is only one set, since consensus certifies at most one last block; the selector costs one transaction there too, and the rules are the same.

**Signing it.** A validator signs every transaction of a set at once, and publishes one bundle: the hub ID, the block with its certificate, and its signatures. It sends the bundle to every member and every validator, and to anyone who asks. Members keep every bundle they receive, and every `reconfigure` block with its certificate.

- Once q validators signed one set, anyone assembles its transactions and submits them in order of sequence. Each member has reason to: its own payment is among them.
- **A validator that comes back later,** after others are gone for good, gets from any member the highest certified block, the state after it or its set, and every `reconfigure` block since the last set of validators it knew, with their certificates. It checks each certificate against the set before it, starting from the set it knew, never from a set that a block it was handed names; then it checks the block's certificate and the state's root, builds the set and signs. No other validator needs to be up at the same time, so the payout completes with signatures collected over days, from validators that are never online together.
- **Funds wait for q signatures.** A hub that lost more than f validators for good pays out only once q of them have signed a set, which may take until enough return. On the XRP Ledger nothing can make a transaction or a change of signers valid only after a time, so there is no way to pass the escrow to fewer validators or to someone else.

## 10. Consensus

**One ordered log for every operation.** The validators run a Byzantine fault tolerant consensus protocol and execute the log in order.

- **Simplex** (Chan and Pass, 2023): a block is final three message delays after it is proposed, and a slow or faulty leader's view is skipped after its timeout.
- **Certificates** are q Ed25519 votes of a block with a bitmap of who signed, a few hundred bytes for n ≤ 7. Which q depends on the order votes arrived in, so who took part is read from the votes blocks record (§7), not from certificates.
- **Members submit to every validator,** so a faulty leader cannot hold back an operation for more than one view.
- **Evidence.** Two conflicting certificates for one height name at least f + 1 validators that signed both. They are published, and those validators are dropped from the default list (§11).
- **Built for Dassie:** a state machine without IO, with time and randomness passed in, bounded and asserted, as [tigerstyle](../decisions/tigerstyle.md) asks. Commonware's implementation of Simplex is the reference and a source of test cases.

**Why not without consensus.** Payments where only an account's owner spends from it need no consensus: broadcasting each payment to the validators, each signing the first it sees per sequence, is enough (FastPay, Astro). A hold is spent by two parties, though: by its payee's claim before the expiry, and by its payer after it. Near the expiry, the validators' votes can split so that neither side gets q of them, and the hold stays stuck until something decides it, which is consensus again. Mints that depend on what each validator saw on the underlying ledger, fees that depend on time, reconfiguration and shutdown need an order too. With 4–7 validators in one region, consensus costs a few milliseconds.

## 11. Choosing a hub

- **The acceptable validators.** Dassie ships a default list of acceptable validators, kept by the same people and in the same way as the default bootstrap node list ([bootstrap-network](../decisions/bootstrap-network.md)). Each entry names a node, its operator and its jurisdiction, and an operator has one entry. A node's owner may change the list. A node uses a hub only when every one of its validators is on its list, the way an XRP Ledger server trusts validators from its UNL. An operator earns its entry by its record: running Dassie nodes, and validating hubs that owners chose from lists of their own.
- **Default hubs.** Dassie ships a default hub for each asset and underlying ledger, kept like the default list of validators, with its genesis rules built in: one on the testnet, and one on mainnet once a default list of mainnet validators exists. That list is kept with the mainnet bootstrap node list, and has entries of its own: an operator's testnet entry vouches for nothing on mainnet. Until it exists, no mainnet hub is built in, and `hub-xrp` is not offered. A node that peers over a hub uses the default for that asset unless its owner chose another. A node is in at most one hub per asset and underlying ledger; being in several waits for the [roadmap](../roadmap.md#settlement-hubs).
- **Hub or direct.** Two nodes in the same hub peer over it; otherwise they settle directly. On the test network, a node joins the default hub unless its owner turned hubs off, or unless it is too far from it: before it joins, a node measures its own round trip to the validators, which no peer can lengthen, and joins only when a quorum of them answer within the rules' allowance. On mainnet, a node joins only once its owner turns hubs on, at first, and then under the same check of distance ([settlement-hubs](../decisions/settlement-hubs.md)). A node far from every hub settles directly.
- **What sponsors look for:** validators close to each other in the network, so the hub is fast, run by operators that are unlikely to collude. Nodes that consider joining check both.
- **Advertising.** A member says in its node info, in an `extension` entry ([peer protocol](../areas/peer-protocol.md)), which hub it is in, with the hash of the hub's rules and its account, and whether it takes new peers over it. The rules themselves come from the validators.
- **Exposure.** A node keeps its hub balance, reserves, holds and what it has not yet taken from its peers' holds under a ceiling the owner sets, and above a floor of what its holds need. Between them, the node moves funds itself: it deposits from its own account on the underlying ledger when it falls below the floor, and withdraws what is above the ceiling, so that a failing hub costs it little. Both are counted in largest packets ([settlement](../areas/settlement.md#positions-and-thresholds)), and far enough apart that every deposit or withdrawal, each with its fees, moves many packets' worth. A move is the owner's, as between any two of its ledgers: it leaves the owner's funds on the ledger of the network's XRP scheme (`xrpl-testnet`, or `xrpl` on mainnet) when the payment applies, and joins them on the hub's ledger when the hub credits it. In between, the owner sees it as the move on its way. Hubs that share validators fail together, so a limit per validator operator, over all its hubs, comes with being in several.
- **When a hub fails.** A hub's failure cuts off every peering over it at once. A node whose hub stalls or shuts down ends its peerings over it and peers again directly, so it is cut off until those are made, not for good. It keeps no peering outside the hub while the hub runs.

## 12. Incentives and risks

- **Validators risk nothing but their reputation.** A deposit they post as a bond would sit in the escrow they control, and colluding validators would take it with the rest. While a hub thrives, their future fees and their standing outweigh what stealing would bring; as a hub shrinks, or its validators plan to stop, that balance can turn. Members limit what they keep in a hub (§11), and prefer hubs whose escrow is small against the fees they earn. A validator that serves many hubs loses its standing in all of them if it cheats in one.
- **Fees that make honesty pay.** Taking an escrow that holds E needs q validators, and every one of them; however they split it, the one with the smallest share gains at most E/q. A multi-signed payment names its signers, so a theft costs each of them its place on the list and the fees of every hub it validates. A hub whose fees pay its validators R a year pays each about R/n (§7). Value a validator's fees over the years it expects to run with an annuity factor A, which is 1/r at a discount rate r for a validator that means to run for good. When its co-validators are the same in all its hubs, they can take every escrow at once, and honesty pays while R > (n/q) · E / A. When a theft can reach one hub only, a validator in h hubs like this one loses h hubs' fees for one escrow, and honesty pays while R > (n/q) · E / (h · A). This is a starting point for fees in the rules, not a bound honesty needs: known operators, their jurisdiction, the risk of prosecution and their standing keep validators honest below it. A member works the margin out from public data: the supply published with each certificate (§3) and the fees in the log.
- **The default list is the root of trust.** Whoever keeps it decides whom nodes trust, far more than any sponsor does. Validators hold others' funds together, which may make them custodians, or money transmitters, under some laws.
- **Fees are not the validators' to set.** The rules fix them, with caps, and only a reconfiguration with notice changes them (§7). A member's cost of leaving (a withdrawal fee, the underlying ledger's fee, and finding new peers) is what a hub could charge above a fair price, and the caps keep it from doing so.
- **Validators are paid for taking part,** not for being listed (§7).
- **Volume gathers.** Volume in networks follows a power law, so one hub per asset, likely the default, may carry most of it. That is accepted: the fewer the hubs, the more peers one deposit reaches. A hub is bounded already by its asset and by the region its validators share for low latency.
- **Small hubs multiply the validators' work.** Each validator serves more of them, and founding one costs it keys and a check of the escrow before it earns anything. A setup fee would pay for that ([roadmap](../roadmap.md#settlement-hubs)); until then, a validator founds hubs only with sponsors its owner chose (§2).
- **Funds can wait.** A hub that loses more than f validators stops, and pays out only once q validators have signed one payout set, though not at the same time (§9).
- **Privacy.** Anyone sees every member's balance and holds, and every operation on them, with the packets of disputes (§4): a validator answers whoever asks, since its answers prove themselves and a member need not be known to it.

## 13. How it fits the node

- **A new crate, `dassie-hub`:** the ledger, its rules and operations, Simplex, the payout set, and their wire types, all without IO. Validators are Dassie nodes that run it.
- **A new settlement scheme for each asset and underlying ledger,** in the node's registry of schemes ([settlement](../areas/settlement.md)), with a ledger of its own in the node's books and the hub's ID in its configuration: a member's side of a hub, which peers, opens and funds holds, signs commitments and claims, redeems and claims, deposits and withdraws. `hub-xrp-testnet` is on the XRP Ledger testnet; **`hub-xrp`** is on mainnet, books on a ledger of its own in the live realm, `hub-xrp+xrp`, and runs, like every live ledger, only in a node of the live realm ([mainnet-schemes](../decisions/mainnet-schemes.md)). Its rules name mainnet's network, and its members pay out at the hub's end, and move funds, to and from the node's mainnet account.
- **The peer protocol** gains a message that carries a Prepare with its commitment, and a claim after the byte that accepts a Fulfill, for peerings over a hub. Both peers chose the hub, so both understand them, and no new protocol version is needed.
- **Talking to the hub.** Validators and members exchange the hub's messages over a stream kind of their own on the node's QUIC endpoint. Every message proves itself (votes and certificates by consensus keys, operations by account keys, receipts by certificates), so it needs no peer protocol envelope, and a member need not be known to the validators. Members fall back to HTTPS as they do for the peer protocol; validators do not.
- **Credit.** A hold is the credit its payee extends, so a peer over a hub needs no slow start ([earned-trust](../decisions/earned-trust.md)), and its credit does not grow with earnings. A node that wants more room with a peer puts more into its hold; doing that with what it earned waits for the [roadmap](../roadmap.md#settlement-hubs).
- **The connector** keeps its timing for a peering over a hub, but for one exception: when a packet to a peer over a hub times out, the Reject upstream waits for the hub (§4).
- **What a peering over a hub can still cost**, beyond its holds, is booked as a loss on that peer, and the simulation measures it. Nothing bounds it further until running hubs show a bound is needed:
  - the hub fees of every `claim` that its missing or late claims made us submit: the 1024 missing at once bound the claims, not the fees over time, and a payer could route through us to itself to make us pay them for ever;
  - packets we paid on the hub but learned of too late to fulfill upstream (§4);
  - what it owed before its account existed (§5).
- **The simulation** runs validators as nodes with the fake XRP Ledger ([testing](../areas/testing.md)), with:
  - up to f validators that equivocate, hold back operations or stay silent, and f + 1 that collude, whose evidence must appear;
  - payers that send no claims, and payees that claim twice, or late, just before a commitment's expiry;
  - shutdowns and stalls where validators sign payout sets at different times and for different blocks, some after others are gone, and the escrow pays out exactly the balances of one final block.
- **Real money.** Hubs on mainnet are part of the launch, and hold real money only once [real-money](../decisions/real-money.md)'s conditions are met: among them, `hub-xrp` with a default mainnet hub, and validators that observe the escrow through ledgers they verified.
