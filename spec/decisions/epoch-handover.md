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
- **Proofs of the last block are either kind.** H − 1's own finalization or a repeat's, with the repeats between, proves the block in catch-up and in the epochs' last blocks each validator keeps.

## Accepted

- A repeat costs a view, and only at the end of an epoch whose last block's votes split.
- A chain of repeats longer than a delivery's bound cannot be handed over in one delivery; it takes a run of splits at one boundary that long.
- Votes and blocks carry eight more bytes.

## Rejected

- *The new set starts from the view after H − 1's finalization:* which finalization a validator sees first decides that view, so validators disagree on which set runs the views between, and can build different first blocks on H − 1.
- *The new set builds on H − 1 once it is notarized:* two blocks at height H − 1 can be notarized, in different views, and the new set's validators would not agree which to build on.
- *Letting a validator that nullified a view finalize it at a boundary:* a finalization and a nullification of one view could then both form, and a block that skips a final one could become final too.
- *Filler blocks of the old set past H − 1, to finalize it as an ancestor:* the ledger would see blocks at heights the new set should certify, and the handover height would follow consensus instead of the ledger.
