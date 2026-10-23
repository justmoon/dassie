# Settlement hubs

What is built of [settlement hubs](../designs/settlement-hubs.md): the hub's rules and its ledger, the deterministic function that takes a hub from one block to the next. Consensus, the escrow on the XRP Ledger, the member's scheme and the node's validator role are not built yet ([roadmap](../roadmap.md#settlement-hubs)); the design describes them.

**Paths:** `crates/dassie-hub/`
**Read with:** [Settlement](settlement.md), [Encoding](encoding.md), [Identity](identity.md)

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
- **New accounts set a disbursal address first,** so every account the hub pays out at shutdown has somewhere to go. Validators' accounts, which receive their fees, are exempt and lock no reserve.

## Holds

A payee takes from a hold only what its payer signed (`ledger/holds.rs`):

- `redeem` pays what the payer's claim adds to the last redeemed total, less what `claim` already paid for sequences up to the claim's W. Totals and W only grow.
- `claim` pays commitments above the last redeemed W whose fulfillments hash to their conditions, in a block stamped before their expiry. A hold remembers each sequence claimed until a redeem passes it, so no packet is paid twice. A claim is all or nothing.
- A payer's reduction takes effect at the first block stamped after the hold's notice; a hold reduced to nothing closes and frees its reserve.

## What validators attest

Deposits, withdrawal outcomes, the escrow's setup and resets of the test network each take effect once a quorum of validators attested the same thing, so up to f faulty validators can neither make one up nor hold one back (`ledger/escrow.rs`, `ledger/validator.rs`). A deposit is credited once: the hub remembers credited transactions back to a floor that rises as its table fills, and takes no attestation below it. Deposits wait until a quorum attested the escrow, at genesis and again after a reset.

A reset writes off every balance, reserve, hold and pending payment, and keeps the accounts, which lock their reserve again from what they next receive. Only the testnet's network ID allows it ([ledger-resets](../decisions/ledger-resets.md)).

## Fees

A share of every fee goes to the reserve account; the rest waits in the pool of its block until the vote window has passed, and is then split equally among the validators whose votes for that block later blocks recorded (`ledger/fees.rs`). A block records each vote once, and only for blocks within the window. The time a validator's vote was last recorded also decides absence: a validator unrecorded for longer than the terms allow shuts the hub down.

## Decisions

- [settlement-hubs](../decisions/settlement-hubs.md): the hub as a whole.
- [tigerstyle](../decisions/tigerstyle.md): deterministic, bounded, asserted.
