# A hub finalizes again after any outage, in bounded memory

**Areas:** [Settlement hubs](../areas/hubs.md#consensus).

## Facts

- In Simplex a block is built on the highest notarized block, and becomes final through its own finalization or a later block's. While votes to finalize are lost (a partition that splits them, a slow link, a run of crashes), blocks keep being notarized and none becomes final.
- An engine must stay bounded in memory. One that holds a fixed number of views and blocks above the last final block, and drops the lowest past them, has dropped the chain the next block builds on after an outage of finality longer than that: no block can be verified, and the hub never finalizes again, though the network is healthy. The simulation showed it.
- A validator that was away while the chain grew lacks part of it, and others that let go of the views below the highest notarized one could no longer hand it over: the simulation showed such validators stuck, and with more than f of them the hub would be.
- No honest validator may be left unable to finalize by asynchrony, however long, once the network heals.

## Decision

- **Keep what the next block needs, and nothing below it.** A block is built only on the block the highest notarized view stands for, or later, so an engine keeps the views from the highest one that notarized a block that may extend the last final block, the blocks of those views, and the chain from the last final block to that block, and lets go of everything below. It persists the chain with the rest, so a restart keeps it too.
- **Keep what a validator behind on the chain asks for.** Below that view, an engine keeps the views that certified a block of the chain. A validator that lacks part of the chain asks for it by the view that certified it, and takes it below its own floor, as it takes a finalization of a block of its chain; a proposal above the heights it lets wait tells it that others finalized more, and it asks for that.
- **Bound the chain by height.** No block is notarized more than a fixed number of heights above the last final block. Past it, leaders repeat the last block, as at the end of an epoch ([epoch-handover](epoch-handover.md)): finalizing the repeat finalizes the chain. Every repeat builds on the block itself, through the view that notarized it or a repeat of it, so repeats never form a chain of their own.
- **Bound the failed views by time.** While views fail in a row, more than a few rotations of leaders, each further view waits twice as long as the one before. The views held from the highest notarized one stay few, however long the network cannot notarize, and none is ever dropped while it is needed.
- **A run finalized at once is delivered whole.** A delivery carries the whole chain a repeat finalizes. A run that does not fit, by size, is handed over as its last block alone, and a validator behind it loads the state after it, as one further behind than peers keep.

## Accepted

- After a long run of failed views, the first view once the network heals may wait about as long as views had already failed past the threshold: recovery takes at most about the time lost.
- A repeat costs a view, also mid-epoch, but only when finality has been lost for the whole bound.
- An application that lets go of states verifies again, per view, at most the chain that waits to be final.

## Rejected

- *Dropping the lowest views and blocks past a fixed bound:* the outage that matters is the one longer than the bound, and then the hub halts for good.
- *Holding everything until it is final:* memory grows with the outage, which nothing bounds.
- *A fixed limit on views, with no backoff:* views past it could not be entered, and the hub would halt in the same way.
- *Repeats that build on each other:* the proof of a block would grow with every split, and a long one would no longer fit in a delivery.
