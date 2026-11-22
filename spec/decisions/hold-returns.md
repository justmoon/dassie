# What goes back to a hold's payer is booked as the settlement it undoes, under the settlement reference

**Areas:** [Settlement hubs](../areas/hubs.md#the-members-side), [Ledger](../areas/ledger.md#what-the-ledger-refuses-and-what-it-records)

## Facts

- Funding a hold over a hub is the payer's outgoing settlement and the payee's incoming one: the payer's books count the hold as paid to the peer in advance, the payee's as owed back to it ([Settlement hubs](../areas/hubs.md#in-the-node)).
- A payer's reduction, once it takes effect, and a payee's release give what is left in the hold back to its payer, with the hold's reserve on a release or a reduction that closes it. Our node submits neither, but a peer may, and so may another client with our key.
- Neither side booked it: the next state of the account reconciled the difference through suspense, as unexplained. The payer's owner then held the money twice, once as the gain and once in what its books still counted as paid to the peer, and its credit to the peer could follow the hold down only as far as those books left room. The simulation's check that every reconciliation is explained flagged it.
- The peer's account (`assets/interledger/<peer>`) may not be debited past its credits. Settlements are checked against that limit; reports (deposits, withdrawals, reconciliations, fees, losses) are recorded past it ([Ledger](../areas/ledger.md#what-the-ledger-refuses-and-what-it-records)). The node stops a scheme whose effects the books refuse, as a bug.
- The journal's kinds of reference are fixed by a check in its table. A new kind needs a migration that rebuilds the table, the journal whole, and a new case in the owner RPC's transfers.
- A payee takes a commitment only if the hold covers it beyond a reduction due before it expires ([Settlement hubs](../areas/hubs.md#packets-promises-and-claims)), so its books have the room to give back what a reduction takes. Each node keeps its own books, though, and they may drift ([own-books](own-books.md)).

## Decision

- **It is booked as that much of the settlement undone, on both sides** (`Effect::HoldReturned`): back to the payer, Dr `assets/settlement`, Cr the peer's `assets/interledger`, and its reserve as the reserve's, as it was booked when the hold opened; back from the payee, Dr the peer's `assets/interledger`, Cr `assets/settlement`.
- **Under the settlement reference,** posted at once, with the scheme's ID for it, which the hold and the block it went back in name, so it is booked once.
- **What the payee's books lack room for goes through suspense.** The node books of a hold given back to a peer only what the peer's account can take, as the effects before it in the batch leave it, and the rest as a reconciliation it cannot explain, which warns the owner, rather than stopping the scheme. A peer with no accounts on the ledger has it go through suspense whole.

## Accepted

- **The journal shows it as a settlement with the peer,** which it is in effect: the money that funded a hold, moving back. Nothing tells it from one in the journal but its ID.
- **A shortfall is a warning, and the simulation's check flags it,** as it should: it means the payee's books drifted from the hold.

## Rejected

- *A reference kind of its own, recorded as a report:* the exact record, and never a shortfall. But it needs a migration that rewrites the journal table on every node, and a new case in the owner RPC, for a booking the settlement reference carries. It stays possible later, if the journal needs to tell the two apart.
- *A reconciliation with a cause of its own:* the least change, and it keeps the check of unexplained reconciliations strict. But the peer's account would still count the hold as paid, so the payer's owner would hold the money twice, and its credit would not follow the hold down.
- *Booking it as the peer's incoming settlement* (`Effect::IncomingSettlement`): that also counts a settlement from the peer as an answer to our request to settle and as a payment for its standing as a receiver, which a reduction of our own hold is not.
