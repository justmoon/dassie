# Settlement hubs

Peering rests on trust today. Settling is up to the node that owes, so a peer can always walk away with what it owes, and the credit a node extends bounds the loss ([own-fulfills](../decisions/own-fulfills.md)). A **settlement hub** is another way to peer, for nodes that choose it: a small ledger kept by a few validators and backed by an escrow on an underlying ledger such as the XRP Ledger. Two nodes that peer over a hub cannot walk away with what they owe each other: what one owes the other can be taken from funds the hub holds, without the debtor's help.

A hub moves trust, it does not remove it. Instead of trusting each peer up to its credit, a node trusts one set of validators with the balance it keeps in the hub. That set is known in advance, its members are chosen for their reputation, and one relationship covers every peer in the hub.

This is a design ([settlement-hubs](../decisions/settlement-hubs.md)); nothing in it is built yet. Durations and sizes in it are starting points, to be set from the deterministic simulation when the hub is built.

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
| **Sponsor** | The node that founds a hub. It has no powers once the hub runs. |

**What the validators can do.**

- With at most f of them faulty or malicious, the hub is safe and keeps running.
- Any f + 1 can halt it by refusing to take part.
- f + 1 that collude can sign two conflicting histories, each with a valid certificate. The two certificates name them: whoever signed both is proven to have misbehaved (§10).
- Taking funds from the escrow directly needs q of them, the escrow's signing quorum.

**How a node sees a hub.** The hub is a ledger a node does not control, like the XRP Ledger. Its balance there is an asset the node books, and the hub is a counterparty with its own exposure limit (§11). Nodes still keep their own books ([own-books](../decisions/own-books.md)); the hub is the one ledger two members agree on, as two XRPL users agree on the XRP Ledger.

## 2. Founding a hub

1. **The sponsor fixes the terms and picks n validators** that are all on its list of acceptable validators (§11), and pays each its setup fee over Dassie, in ordinary packets.
2. **Each validator makes three keys for the hub,** derived from its node seed under a path that names a random nonce the sponsor picked for this hub. The hub ID cannot name them: it is the hash of rules that contain these keys.
   - a **consensus key** (Ed25519), with which it signs votes, certificates and receipts;
   - an **escrow key**, an XRPL key whose account address becomes one entry of the escrow's signer list;
   - a **payout key**, an extended secp256k1 public key (BIP 32), from which anyone can derive the keys this validator signs a payout set with (§9), while only the validator can sign with them. With non-hardened derivation, one leaked child key and the public parent give away every child key, so the validator guards these as it guards its escrow key.

   It returns its acceptance, signed with its node key: the nonce, the hash of the terms, the three public keys, and its endpoint. An acceptance names no other validator, so one that does not accept, or fails before genesis, is replaced without touching the acceptances already given: the sponsor loses that validator's setup fee, not the others'. A validator that takes the fee and makes no keys has shown early that it is not one to trust.
3. **The rules are fixed** once n validators accepted: the terms and the acceptances; the hub ID is their hash. Every validator signs the final rules, which is the genesis certificate, so each sees the whole set before it commits to it. A validator signs at most one genesis per nonce, so its acceptance cannot found two hubs with different co-validators.
4. **The sponsor creates the escrow account** on the underlying ledger and, from its master key, sets it up as the rules say:
   - a signer list of the n escrow keys, each with weight 1, quorum q;
   - `RequireDest`, so a payment without a destination tag fails;
   - no regular key, then the master key disabled, last.
5. **The validators check the escrow** on their own XRPL servers before the first block: master key disabled, no regular key, the signer list exactly as the rules say, `RequireDest` set, and no other objects. A sponsor that kept a way in is caught here, and the hub never starts.
6. **The sponsor deposits** (§6), and the validators credit its account.
7. **The sponsor advertises** that it is a member, in its signed node info (§11). Other nodes that want to peer with it join the hub.

## 3. The ledger

**State.**

- **Accounts:** a number (assigned in order, never reused), the owner's public key, a balance, the reserves it locks (§7), a disbursal address on the underlying ledger (address and tag), the time of its last operation, and a sequence for the owner's operations.
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
| `redeem` | payee | Takes from a hold with the payer's latest signed claim (§4). |
| `claim` | payee | Takes from a hold with the payer's signed commitments and their fulfillments (§4). |
| `withdraw` | owner | Pays funds out to an address on the underlying ledger (§6). |
| `close` | owner | Pays out the account's balance and its account reserve, to an address on the underlying ledger as `withdraw` does (§6), or to another account. Refused while the account has holds, as payer or payee, or withdrawals pending. Paid to another account, it deletes the account at once; paid out on the underlying ledger, only once the payout settled. Until then the account is closing: it takes no credits and no holds as payee, a `transfer`, `pay` or `hold-open` to it fails, a deposit to its tag is paid back as one to a tag with no account (§6), and the idle fee does not apply. A payout that fails returns the funds to the account, less the fee; the account locks its account reserve again from them and is open as before. |
| `attest-deposit`, `attest-withdrawal` | validator | Reports what it saw on the underlying ledger (§6). |
| `reconfigure` | validators | Changes the validators or the rules (§8). |
| `set-endpoint` | validator | Changes the validator's own endpoint, at once. |
| `shutdown` | validators, or the rules | Ends the hub (§9). |

**Invariant,** asserted after every block: the balances and reserves of all accounts, holds, pending withdrawals, the validator accounts and the reserve account add up to no more than the escrow's balance less its reserves. Each validator also publishes the hub's total supply with its certificate, so anyone can compare it with the escrow's balance on the underlying ledger, and a member that sees a shortfall leaves.

**Receipts.** A member learns the outcome of an operation from a receipt: the operation's hash, the height of its block, its result, the block's certificate, and the path from the block's root to the result. Validators send a receipt to everyone the operation touches: its signer, and a hold's payee when the hold changes. A member that wants to know a balance asks; f + 1 matching signed answers at the same height include one from an honest validator.

**Time.** Everything that depends on time is judged by the block's timestamp, never by a validator's clock: a packet's expiry, a hold's notice period, idle fees, shutdown. Timestamps follow CometBFT's proposer-based timestamps: the leader stamps a block, and a validator votes for it only when the stamp is within the rules' precision of its own clock, allowing for the message delay. Members leave that much margin.

## 4. Peering over a hub

Two members peer with the hub as their settlement scheme. Each direction has its own hold: the payer A sets funds aside for the payee B, and **the hold is the credit B extends to A**. B is paid from the hold whether A cooperates or not, so the credit costs B nothing.

**Packets.**

- With every Prepare it sends B, A sends a **commitment**: its signature over the hub ID, the hold, a sequence (increasing per hold), the amount, the condition and an expiry: the Prepare's own expiry less the hub's margin M (below).
- B forwards the packet at once, as long as the packets in flight and what it has not yet taken from the hold fit in the hold. The hub is not involved.
- When the Fulfill comes back, A's reply to it ([own-fulfills](../decisions/own-fulfills.md)) carries a **claim**: A's signature over the hold, the total T it owes B on the hold, and the sequence W up to which T covers every fulfilled packet. B accepts a claim only when T is right by its own record of what it fulfilled.

**Taking from the hold.**

- `redeem (T, W)` pays B what the claim adds, less anything B already took with `claim` for packets up to W. W only grows, and T only grows.
- `claim` is for when A's claim does not come. B submits commitments above the last redeemed W together with their fulfillments; the hub checks that each fulfillment hashes to the commitment's condition and that the block is stamped before the commitment's expiry, then pays the amounts. Each hold remembers the sequences claimed this way until a `redeem` passes them, so nothing is paid twice. The table belongs to the hold, not the hub, and has room for at least as many as the claims B lets A leave missing (1024), so other members cannot fill it to make B's claims fail. Only a commitment's own expiry decides: once it passed, the packet can no longer be claimed, as in ILP.
- B redeems often enough that what it has not taken stays a small part of the hold, much as a node asks a peer to settle well before its credit runs out ([settlement](../areas/settlement.md)). Every redeem costs a fee, so B does not redeem after each packet.

**Why A is safe.** B is paid only for what A signed: a claim, or a commitment with its fulfillment in a block stamped before the commitment's expiry.

- **A learns every claim.** The validators send A the receipt of each `claim` on its holds, so A learns the fulfillment and fulfills upstream.
- **The hub overrules A's books.** When A did not take B's Fulfill, for being late say, and B then wins a claim for the packet, A's books follow the hub: the packet counts as fulfilled. The `hub` scheme specifies this with the claim.
- **A does not reject early.** When its outgoing packet times out at its expiry E, A does not reject upstream at once, as [connector-timing](../decisions/connector-timing.md) has a node do: B may still win a claim stamped before the commitment's expiry. A waits for a final block stamped at or after that expiry. Blocks become final in order and their stamps only grow, so that block settles it: if no claim for the packet came before it, A rejects; if one did, A fulfills. A peering over a hub is the one exception to [connector-timing](../decisions/connector-timing.md)'s timeout. B does not have to cheat for this to matter: its Fulfill only has to reach A after E.
- **The margin M** is the hub's timestamp precision plus the longest it takes to finalize a block with f validators faulty: f views skipped, each after its timeout, then a block. The rules state it. The commitment expires M before the Prepare, so the block that settles a packet is normally final by E.
- **The window.** After E, A still needs the receipt or the final block from a validator, and its Fulfill or Reject must then reach its upstream before the upstream's own expiry. B, on its side, needs time after its downstream answers to get a claim to the validators before the commitment's expiry. These legs cross the open network between members, not the hub's region. So a peering over a hub uses a wider window than the 1 s of [connector](../areas/connector.md): M plus those legs. A node works it out from the hub's rules and the delays it measures, and does not peer over a hub whose window it cannot afford.
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

**The hub's minimum for a new account** is the account reserve, which the account locks, plus a starting balance in the rules, sized to pay the fees of its first operations at the congestion cap (§7), so that a member paid into the hub needs no deposit before it can act. A new account must set its disbursal address before the hub accepts any other operation from it. Without one, its funds at shutdown stay in the escrow (§9).

## 6. Deposits and withdrawals

**Deposits.** A member pays the escrow on the underlying ledger with its account number as the destination tag; the rules keep account numbers below 2³², so they fit one. Each validator follows the escrow's transactions on its own XRPL servers, and attests a deposit once it sees it in a validated ledger:

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

**Why native multi-signing.** Each validator's signature stands alone, so signatures can be collected over days and from validators that are never online together, which shutdown needs (§9). A threshold key (FROST) would let the validators sign with one key, but its signing is interactive among a fixed group, and a share made for one group cannot be used with another.

## 7. Fees

- **Per operation.** Each operation pays the fee in the rules, from the signer's balance.
- **Reserves.** Each account locks an account reserve, and each hold a hold reserve from its payer's account, both set by the rules, as the XRP Ledger reserves funds for each account and each object it owns. A reserve is never spendable and never backs a hold. It prices what an account or a hold takes from the hub while it exists: a place in the bounded tables (§3), and for an account its payment in the payout set (§9). The rules set the account reserve at least at the least payout plus the payout fee, so that every account the idle fee has not drained is paid out at shutdown. A hold reserve is freed when its hold closes; the account reserve comes back with `close`, or in the payout set.
- **Idle fee.** An account with no operation for a period (in the rules) pays an idle fee each period, from its balance, then from its account reserve once the balance is gone, never from a hold or a hold reserve. An account with no balance, no account reserve left and no holds is deleted; its number is never used again.
- **Congestion.** The fee rises when blocks carry more operations than the target in the rules, and falls when they carry fewer, within a cap in the rules, the way Ethereum's EIP-1559 sets its base fee. What congestion adds goes to the reserve account, which pays the underlying ledger's fees and covers shortfalls, not to the validators: those who would set the fee do not profit from raising it.
- **Deadlines first.** `claim` and `redeem` have deadlines, so a share of every block (in the rules) is kept for them, and they go first. Ordinary operations cannot crowd them out; only other claims and redeems can, at the capped fee, which makes holding back someone's claim dear the longer it lasts. The rules size the space so that the hub's margin M holds as long as that space is not full.
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
- **Default hubs.** Dassie ships a default hub for each asset and underlying ledger, kept like the default list of validators. A node that peers over a hub uses the default for that asset unless its owner chose another.
- **What sponsors look for:** validators close to each other in the network, so the hub is fast, run by operators that are unlikely to collude. Nodes that consider joining check both.
- **Advertising.** A member says in its node info, in an `extension` entry ([peer protocol](../areas/peer-protocol.md)), which hubs it is in, with the hash of each hub's rules and its account, and whether it takes new peers over each. The rules themselves come from the validators.
- **Exposure.** A node keeps each hub's balance, reserves, holds and what it has not yet taken from its peers' holds under a limit the owner sets. It keeps the same sum, over all the hubs a validator operator takes part in, under a limit per operator: hubs that share validators fail together. When the underlying ledger is fast and cheap, it withdraws what is above what its peerings need, so that a failing hub costs it little.
- **Spread.** A hub's failure cuts off every peering over it at once, so a node keeps at least one peering outside any one hub.

## 12. Incentives and risks

- **Validators risk nothing but their reputation.** A deposit they post as a bond would sit in the escrow they control, and colluding validators would take it with the rest. While a hub thrives, their future fees and their standing outweigh what stealing would bring; as a hub shrinks, or its validators plan to stop, that balance can turn. Members limit what they keep in a hub (§11), and prefer hubs whose escrow is small against the fees they earn. A validator that serves many hubs loses its standing in all of them if it cheats in one.
- **Fees that make honesty pay.** Taking an escrow that holds E needs q validators, and every one of them; however they split it, the one with the smallest share gains at most E/q. A multi-signed payment names its signers, so a theft costs each of them its place on the list and the fees of every hub it validates. A hub whose fees pay its validators R a year pays each about R/n (§7). Value a validator's fees over the years it expects to run with an annuity factor A, which is 1/r at a discount rate r for a validator that means to run for good. When its co-validators are the same in all its hubs, they can take every escrow at once, and honesty pays while R > (n/q) · E / A. When a theft can reach one hub only, a validator in h hubs like this one loses h hubs' fees for one escrow, and honesty pays while R > (n/q) · E / (h · A). This is a starting point for fees in the rules, not a bound honesty needs: known operators, their jurisdiction, the risk of prosecution and their standing keep validators honest below it. A member works the margin out from public data: the supply published with each certificate (§3) and the fees in the log.
- **The default list is the root of trust.** Whoever keeps it decides whom nodes trust, far more than any sponsor does. Validators hold others' funds together, which may make them custodians, or money transmitters, under some laws.
- **Fees are not the validators' to set.** The rules fix them, with caps, and only a reconfiguration with notice changes them (§7). A member's cost of leaving (a withdrawal fee, the underlying ledger's fee, and finding new peers) is what a hub could charge above a fair price, and the caps keep it from doing so.
- **Validators are paid for taking part,** not for being listed (§7).
- **Volume gathers.** Volume in networks follows a power law, so one hub per asset, likely the default, may carry most of it. That is accepted: the fewer the hubs, the more peers one deposit reaches. A hub is bounded already by its asset and by the region its validators share for low latency.
- **Small hubs multiply the validators' work.** Higher fees when a hub fills up push new members to other hubs, and each validator serves more of them. The setup fee pays for that.
- **Funds can wait.** A hub that loses more than f validators stops, and pays out only once q validators have signed one payout set, though not at the same time (§9).
- **Privacy.** The validators see every member's balance and holds, and the packets of disputes (§4).

## 13. How it fits the node

- **A new crate, `dassie-hub`:** the ledger, its rules and operations, Simplex, the payout set, and their wire types, all without IO. Validators are Dassie nodes that run it.
- **A new settlement scheme, `hub`,** in the node's registry of schemes ([settlement](../areas/settlement.md)): a member's side of a hub, which peers, opens and funds holds, signs commitments and claims, redeems and claims, deposits and withdraws.
- **The peer protocol** gains the commitment on a Prepare and the claim in the reply to a Fulfill, for peerings over a hub.
- **Credit.** A hold is the credit its payee extends, so a peer over a hub needs no slow start ([earned-trust](../decisions/earned-trust.md)).
- **What a peering over a hub can still cost**, beyond its holds, is for the work on hubs to bound ([roadmap](../roadmap.md#settlement-hubs)):
  - the hub fees of every `claim` that its missing or late claims made us submit: the 1024 missing at once bound the claims, not the fees over time, and a payer could route through us to itself to make us pay them for ever;
  - packets we paid on the hub but learned of too late to fulfill upstream (§4);
  - what it owed before its account existed (§5).
- **The simulation** runs validators as nodes with the fake XRP Ledger ([testing](../areas/testing.md)), with:
  - up to f validators that equivocate, hold back operations or stay silent, and f + 1 that collude, whose evidence must appear;
  - payers that send no claims, and payees that claim twice, or late, just before a commitment's expiry;
  - shutdowns and stalls where validators sign payout sets at different times and for different blocks, some after others are gone, and the escrow pays out exactly the balances of one final block.
- **Real money** stays off until [real-money](../decisions/real-money.md)'s conditions are met.
