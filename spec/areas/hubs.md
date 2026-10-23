# Settlement hubs

What is built of [settlement hubs](../designs/settlement-hubs.md): the hub's rules and its ledger, the deterministic function that takes a hub from one block to the next, Simplex consensus that orders its log, its escrow on the XRP Ledger, and a member's side: its account in the hub and the `hub-xrp-testnet` scheme on it. The node does not run either yet, nor the validator role ([roadmap](../roadmap.md#settlement-hubs)); the design describes them.

**Paths:** `crates/dassie-hub/`
**Read with:** [Settlement](settlement.md), [Encoding](encoding.md), [Identity](identity.md), [XRPL](xrpl.md), and for `member/` [Connector](connector.md#responses-and-expiry) and [Peer protocol](peer-protocol.md#message-set)

## The rules

A hub is named by the digest of its rules: the sponsor's terms and one signed acceptance per validator (`rules.rs`). An acceptance names the terms by their digest and no other validator, so a validator that drops out before genesis is replaced without touching the others' ([settlement-hubs](../decisions/settlement-hubs.md)).

`Terms::check` refuses terms whose values contradict what the hub's safety rests on: an account reserve too small to pay its account out at shutdown, fewer places for missing claims than a payee lets its payer leave missing, a hold notice no longer than a packet may be held, no places kept for deadlines, a vote window shorter than a validator's turn. The values themselves are starting points for the simulation.

## Digests and signatures

Everything is named by SHA-256 under a domain tag of its own (`digest.rs`), and every signature signs such a digest, so a signature made for one purpose never verifies for another. A member's operation is signed over the hub ID, its account number, the account's next sequence and the most fee it pays; a validator's over the hub ID, its index and a sequence that only grows; a payer's commitment and claim over the hub ID and the hold. Account and hold numbers are never reused, so nothing signed can be replayed in another hub, or twice in one. Weak Ed25519 keys are refused wherever a key is decoded, since under ZIP-215 they verify forgeries ([Identity](identity.md#node-key)).

All wire types are canonical OER with fixed-width integers ([Encoding](encoding.md)).

## The ledger

The ledger (`ledger/`) is a pure function of the blocks it executes: time is the block's timestamp, and nothing reads a clock or an RNG, so every validator that executes the same blocks reaches the same state, which the state's Merkle root lets anyone check.

- **Every drop is in exactly one place.** After every block the ledger asserts that what the escrow holds for the hub equals everything the hub owes: balances and reserves, holds and their reserves, pending withdrawals with their ledger fees, fees waiting for their voters, and the reserve account. That sum is the supply each validator publishes.
- **Bounded tables.** Accounts, holds, holds per account, withdrawals, pending and remembered deposits, claimed sequences per hold and operations per block each have a bound in the terms. An operation that would pass one fails; it never grows a table.
- **Refused or failed.** An operation that is not its signer's to make (unknown account, bad signature, wrong sequence) or cannot pay its fee is refused: it pays nothing and uses no sequence. One that is valid but cannot take effect fails: it pays its fee and uses its sequence, as a `tec` result does on the XRP Ledger. A failure changes nothing else.
- **Deadlines first.** A block keeps places for `redeem` and `claim`, which go first; a block that breaks the order or the bounds is not executed, and validators do not vote for it.
- **New accounts set a disbursal address first,** so every account the hub pays out at shutdown has somewhere to go. Validators' accounts, which receive their fees, are exempt and lock no reserve. A closing account does nothing more: it waits for its payout.
- **Nothing a block holds can stop the ledger.** A panic would happen in every validator at the same block, and the hub could never execute it, so whatever members and validators can put in a block is refused or fails before it reaches an assertion: terms that would let the ledger reach a state it cannot leave are refused by `Terms::check`, and new rules never shrink a table.

## Holds

A payee takes from a hold only what its payer signed (`ledger/holds.rs`):

- `redeem` pays what the payer's claim adds to the last redeemed total, less what `claim` already paid for sequences up to the claim's W. Totals and W only grow.
- `claim` pays commitments above the last redeemed W whose fulfillments hash to their conditions, in a block stamped before their expiry. A hold remembers each sequence claimed until a redeem passes it, so no packet is paid twice. A claim is all or nothing.
- A payer's reduction takes effect at the first block stamped after the hold's notice; a hold reduced to nothing closes and frees its reserve.
- A payee may release a hold toward it, which goes back to its payer at once. Only the holds an account pays into count toward its limit per account: anyone can open a hold toward an account, and it can release them, so no one can use up another's room.

## What validators attest

Deposits, withdrawal outcomes, the escrow's setup and resets of the test network each take effect once a quorum of validators attested the same thing, so up to f faulty validators can neither make one up nor hold one back (`ledger/escrow.rs`, `ledger/validator.rs`). Resets count only when a quorum saw the same new chain.

- **A deposit is credited once.** The hub remembers credited transactions back to a floor that rises as its table fills, and a deposit below the floor that is not pending already is not taken: it may have been credited and forgotten. One that is pending was never credited, and is attested whatever its ledger.
- **Each validator has its own share of the pending deposits,** so a faulty one that attests made-up deposits fills only its own share, and honest attestations still fit.
- **Deposits wait until a quorum attested the escrow,** at genesis and again after a reset.
- **A change of validators moves attestations to their validators' new indices** and drops those of validators that left, so they never count toward the new set's quorum; a withdrawal or deposit whose attestations make a quorum of the new set is settled at the end of the block. Votes start again. A pending deposit below the floor that no one attests any more is kept, so it can still be credited; such entries count toward the table, and each removed faulty validator can leave up to its share of them behind. A validator may change its vote, for a reset as for new rules. When the consensus or escrow keys change, the new set keeps the escrow's next sequence for the old set's change of the signer list, and no new rules are taken until it is made.
- **Validators' accounts are outside the bound on accounts,** which counts only accounts opened as members', so a change of validators never finds the table full; an account a validator joins with stays open, a payout that would have closed it a plain withdrawal.

A reset writes off every balance, reserve, hold and pending payment, and keeps the accounts, which lock their reserve again from what they next receive. Only the testnet's network ID allows it ([ledger-resets](../decisions/ledger-resets.md)).

## Consensus

`consensus/` is Simplex (Chan and Pass), without IO and generic over a block's payload, which the ledger fills and checks through the `Application` hooks. Commonware's implementation is the reference and the source of its scenarios; nothing depends on it.

- **What an honest validator signs.** It notarizes at most one block per view, and never one it nullified; it nullifies only its current view; it votes to finalize a block only if it neither nullified that view nor notarized another block in it, and only one block per height. So two conflicting votes, two certificates of one view, or two finalized blocks at one height name only validators that broke these rules: the evidence never blames an honest one.
- **Never two conflicting votes, even across a crash.** Before it sends a step's messages, the host persists the step's batch: the view, every vote it signed above its last finalized block, the heights it voted to finalize, the blocks it notarized and the certificates it holds above it, and whether it declared the hub stalled. Certificates are kept so that a hub whose validators all restart at once can still skip the views they had nullified. A validator that declared a stall never votes again.
- **Timing follows from the region.** Every duration derives from Δ, the region's one-way delay (`consensus/config.rs`): a timestamp is timely within twice Δ of a validator's clock, a block is proposed every interval and final three delays later, a silent leader's view is skipped after the interval and two delays. The hub's margin M is the precision, f skipped views, an interval and three delays. The values are never tuned for the simulation's sake.
- **Leaders** take turns by view, view mod n of the set running that view. A new set takes over from the view after the one that finalizes the block before its first height.
- **Catching up.** Peers keep the recent finalized blocks with their certificates (`RETAINED_BLOCKS`); a validator further behind takes the state from a later certified block (the ledger's root proves it), takes in any change of validators the skipped blocks scheduled, and continues from there. That a certificate is too far ahead to check is unverified, so the host limits how often it loads a state.
- **Room for blocks, per view.** A view holds its leader's first proposal and only blocks a verified certificate names beside it, and only blocks whose view is held and whose height one block per view since the last final block allows. So a faulty leader fills only the room of the views it leads, and can never crowd out an honest block.
- **Changes of validators are in the chain.** After each final block, the engine asks the ledger whether it scheduled a change, and takes it in before it finalizes the next, so every validator, however far behind, hands over at the same height.

The `hub` simulation runs 4 to 7 validators over a network with loss, duplication, slow links, partitions, crashes and clock skew, with up to f faulty ones (silent, withholding, proposing invalid payloads, equivocating) or, one run in ten, f + 1 colluding. It checks that honest validators never finalize different blocks at one height, that a fork always leaves evidence and that evidence never names an honest validator, and that blocks keep becoming final within production timings once the network heals.

## The escrow

`escrow/` holds what the hub does on the XRP Ledger, as functions of the rules and the ledger's state, so every validator builds the same transaction from the same block and signs it alone.

- **Keys** come from the node key under `node/hub/<nonce>/…`: a validator's consensus key, escrow key and BIP 32 payout key, and the sponsor's escrow master key. The master key is the sponsor's to keep: the same key makes the same escrow account again after a reset of the test network ([Identity](identity.md#seed-paths)).
- **Setup and check.** The sponsor funds the escrow, then sets its signer list, `RequireDest`, and disables its master key last. The validators take an escrow only with the master key disabled, no regular key, `RequireDest`, the signer list exactly the rules' (each escrow key weight 1, quorum q) and no other objects, all as of a validated ledger, the objects as of that ledger or later: before it, the master key could still have made one.
- **Nothing one party does can stop the escrow's sequence.** A transaction the XRP Ledger refuses as malformed never uses its sequence, and every escrow transaction after it would wait for ever. So `Rules::check` takes only acceptances whose escrow key is a key of the XRP Ledger and not the escrow's own (a signer list cannot name its account), and whose payout key is a secp256k1 point; and the ledger refuses the escrow itself and the zero account as a withdrawal's, a close's or a disbursal's destination.
- **What validators attest** from the escrow's transactions: a deposit is a successful Payment of XRP to the escrow with a destination tag, for what it delivered; for tag 0 the memo names the new account's key, and anything else attests no key, so the hub pays it back. An escrow transaction's outcome is its sequence and whether it succeeded or failed with a `tec`. The old validators' change of the signer list, once a quorum saw it apply, frees its kept sequence, and the reserve account pays its fee. A withdrawal's outcome counts only if the transaction at its sequence is its own Payment, delivering its amount.
- **The payout set** of a final block: in escrow sequence order with no gap, the withdrawals not yet settled and a pending change of signer list, as already signed; the selector, a signer list of every validator's payout key's child at the block's height (two 31-bit levels); and one payout per account with a disbursal address whose balance, reserves and the holds it paid into, less the fee, reach the least payout. Withdrawals and the selector are signed with escrow keys, payouts with the set's own keys. The first selector the XRP Ledger applies decides which set pays: every other set's payouts are refused for keys not on the list (`tefBAD_SIGNATURE`), without using a sequence.
- **The fee** of every escrow transaction is the rules' ledger fee, which must cover a quorum's signatures.

## Fees

A share of every fee goes to the reserve account; the rest waits in the pool of its block until the vote window has passed, and is then split equally among the validators whose votes for that block later blocks recorded (`ledger/fees.rs`). A block records each vote once, and only for blocks within the window. The time a validator's vote was last recorded also decides absence: a validator unrecorded for longer than the terms allow shuts the hub down.

## The member's side

`member/` is a member's side of a hub, without IO: its account as an [external asset](settlement.md#external-assets) and the `hub-xrp-testnet` settlement scheme on it, which the node books as every scheme ([Settlement](settlement.md#settlement-hubs)).

**What the account trusts, and when it reads.** The account's ledger is the hub's validators, each at its endpoint. It trusts a statement once f + 1 validators signed it alike: one is honest. It reads the operations that touch it in the order of the log, each once, and only up to a height a quorum covered:

- **The contract a validator keeps:** its answer to `Since` lists every executed operation that touches the account from the height asked, in order, and ends with the newest final block it covered; an answer too long for one message stops before a block. A `Final` is a validator's word that it sent everything up to it.
- **Coverage counts from the validator's own endpoint only.** A `Final` names no account, so one passed on from another member's answer would vouch for nothing; the node's driver authenticates each endpoint.
- **Why a quorum.** Once 2f + 1 validators covered a height, f + 1 honest ones sent every operation up to it, so each is agreed, and the account reads them, in order, up to that height. A faulty validator can neither make one up nor hide one. A payer decides a packet without a fulfillment only from a final block up to which it read everything ([below](#packets-promises-and-claims)).
- **Its own state** is told to the scheme only as of a block through which it read everything, and nothing touching it since, so the scheme checks its books against it without counting anything twice.

**What it submits.** One operation at a time, to every validator, again until one executes: claims and redeems before what waits, never before what is on its way. A settlement is sent only once the node executed it. An operation is signed with the account's sequence when it is sent; refused for its sequence, it is signed again with the one the account had as of that block or later. A sequence only ever goes up, so the refused one can never apply later. A new account sets its disbursal address before anything else, as the ledger requires.

**One scheme per account.** The node writes the two in one batch, and the scheme refuses a state that is not in step with its account's. When the scheme stops, the account reads nothing more until a restart, so neither misses what the other read.

### Packets, promises and claims

- **Amounts.** A commitment promises its packet in drops rounded up, so it never covers less than the packet. T is what the packets up to W add up to in internal units, rounded down to drops: what is less than a drop stays owed until there is more.
- **T covers W and nothing above.** The hub subtracts what `claim` paid from a redeem only up to its W, and remembers the rest, so a T that counted a packet above W would pay it twice. T counts every packet the hub paid with a claim, whatever the payer's books said: the hub overrules them.
- **The payee takes a commitment** only once an operation of the payer's on the hold that the hub executed verified under the key the payer gave at peering: the hub checks claims with the account's own key. It takes the packet if the hold covers it beyond what the payee has not taken, what is in flight, and a reduction due before the commitment expires.
- **The payee takes a claim** only if its T is what the payee fulfilled up to its W by its own record, and no packet up to W is still in flight: a redeem through such a packet would forbid claiming it on the hub. A claim that leaves out packets whose commitments expired before the payee could claim them is still taken: only the payer can pay those now, and the claim is worth more than none.
- **Missing claims.** A payer's claim for a packet is late a reply leg (the rules' allowance) after every packet below it was resolved, since only then can the payer cover it; and a margin before the commitment expires in any case, so the claim lands in a block stamped before. A claim the hub refused is tried again with each packet alone, a bounded number of times, each try costing a fee. Past 1024 packets its claims left uncovered, the payee takes no more of the payer's Prepares.
- **Redeeming** waits until the claims it took add two largest packets to what it took (`policy::redeem_threshold`): each redeem costs a fee, and what is not taken is lost if the hub fails.
- **The payer's waits.** A packet whose Reject upstream waits for the hub is decided at once by a claim the hub paid for it, and otherwise without a fulfillment once a final block, up to which the account read everything, is stamped at or after its commitment's expiry.

**What a peering costs beyond its hold** is booked as a loss on the peer (`Effect::PeerLoss`), as the design lists it ([Settlement hubs](../designs/settlement-hubs.md#13-how-it-fits-the-node)): the fee of every claim the payee had to make; a packet the hub paid that the payer had rejected upstream or forgotten; and a packet the payee had rejected and then claimed, which no honest payee does, so it also counts against the peering.

**Not built yet:** the node's driver for the validators' endpoints and its registry of the scheme; credit over a hub, which the node keeps at nothing beyond the hold; validators pushing statements, which the account asks for in the meantime; deposits, withdrawals and closing by the owner, and moving funds between the hub and the node's account; joining by being paid; and a booking of their own for a hold's reduction or release, which the scheme reconciles through suspense meanwhile.

## Decisions

- [settlement-hubs](../decisions/settlement-hubs.md): the hub as a whole.
- [tigerstyle](../decisions/tigerstyle.md): deterministic, bounded, asserted.
