# Each validator set runs an epoch of its own, and a repeat finalizes an epoch's last block

**Areas:** [Settlement hubs](../areas/hubs.md#consensus), [design](../designs/settlement-hubs.md#8-changing-the-validators).

## Facts

- A new validator set takes over at a height H that the ledger fixes. The old set certifies no block at H or above, and the new set's first block builds on block H − 1, so H − 1 must be final before the new set can start.
- In Simplex, a notarized block becomes final through its own finalization, or as the ancestor of a later block that becomes final. A validator votes to finalize a block only if it has not nullified the block's view.
- When a partition splits the votes for block H − 1 after its notarization, some validators vote to finalize it and some, which timed out first, to nullify its view. If neither group reaches the quorum, H − 1 can be neither finalized nor skipped, and no later block of the old set may build on it. The simulation found exactly this: a hub that halted for good at a change of validators.
- Votes then named their view, and the view alone said which set they belonged to: the new set ran every view after the one that finalized H − 1. Several certificates can prove H − 1 final, in different views, and a validator learns of whichever reaches it first. So different validators would start the new set at different views, read one view's votes in different sets, and could build different first blocks.

## Decision

- **Epochs.** Each set runs an epoch of its own, numbered, with views from 1. Every block, vote and certificate names its epoch, and a vote's signature covers it, so votes of two sets never mix, and a validator in both signs nothing that conflicts.
- **The handover depends only on the block.** Once block H − 1 is final, by whatever certificate, the next epoch runs from view 1, and its first block builds on H − 1 at view 0. Every validator starts the new set at the same place, whichever certificate it learned of first.
- **A repeat finalizes the last block.** While H − 1 is notarized and not final, the old set's next leader proposes a repeat: a block of the same height, with H − 1 as its parent and no payload. Only the old set makes one, only of its epoch's last block, and the application never sees it. A repeat is notarized and finalized like any block, and a later repeat may build on an earlier one, so the split votes on H − 1 no longer block anything: the repeat's finalization proves H − 1 final.
- **The boundary is read from the chain.** The block that schedules the change can be notarized and not final while H − 2 and H − 1 are proposed, and its votes can split too. So whether a block is at or past H, and whether a repeat repeats the epoch's last block, is judged on the chain the block builds on: the application's state after its parent, final or not, names the change. A validator that has finalized less judges the same chain the same way, and the old set notarizes nothing at H or above. Finality is not judged by it: a finalization proves its block and every ancestor, so a validator applies the blocks it proves in order, learning of the change as it goes, and only then takes the repeats after them as the proof.
- **Proofs of the last block are either kind.** H − 1's own finalization or a repeat's, with the repeats between, proves the block in catch-up and in the epochs' last blocks each validator keeps.

## Accepted

- The application must keep the state after each block it found valid until that block is final or abandoned, and answer what change it names; the engine has it verify a notarized block it never saw proposed before it builds on it.

- A repeat costs a view, and only at the end of an epoch whose last block's votes split.
- A chain of repeats longer than a delivery's bound cannot be handed over in one delivery; it takes a run of splits at one boundary that long.
- Votes and blocks carry eight more bytes.

## Rejected

- *The boundary from what a validator has finalized:* the scheduling block can stay notarized and not final, by a split, for as long as the network is slow, and a larger notice only makes that less likely. Validators would then disagree on whether a block at H belongs to the old set, and the old set could certify it.
- *The change in the block's header,* for consensus to read: the change follows from the ledger's state, which the application keeps already; a header would be a second copy that the application must check anyway.

- *The new set starts from the view after H − 1's finalization:* which finalization a validator sees first decides that view, so validators disagree on which set runs the views between, and can build different first blocks on H − 1.
- *The new set builds on H − 1 once it is notarized:* two blocks at height H − 1 can be notarized, in different views, and the new set's validators would not agree which to build on.
- *Letting a validator that nullified a view finalize it at a boundary:* a finalization and a nullification of one view could then both form, and a block that skips a final one could become final too.
- *Filler blocks of the old set past H − 1, to finalize it as an ancestor:* the ledger would see blocks at heights the new set should certify, and the handover height would follow consensus instead of the ledger.
