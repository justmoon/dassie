# A hub finalizes again after any outage, in bounded memory

**Areas:** [Settlement hubs](../areas/hubs.md#consensus).

## Facts

- In Simplex a block is built on the highest notarized block, and becomes final through its own finalization or a later block's. While votes to finalize are lost (a partition that splits them, a slow link, a run of crashes), blocks keep being notarized and none becomes final.
- An engine must stay bounded in memory. One that holds a fixed number of views and blocks above the last final block, and drops the lowest past them, has dropped the chain the next block builds on after an outage of finality longer than that: no block can be verified, and the hub never finalizes again, though the network is healthy. The simulation showed it.
- A validator that was away while the chain grew lacks part of it, and others that let go of the views below the highest notarized one could no longer hand it over: the simulation showed such validators stuck, and with more than f of them the hub would be.
- A view may be both notarized and nullified: validators that notarized its block time out before they see the notarization, and nullify it too. A block may skip such a view, so the highest notarized block is not the only one built on: a leader that missed its notarization builds on a lower one, and validators vote for that. An engine that kept only the chain to the highest notarized block let go of the parent of a block it had voted for, and once every validator had, that block, notarized, could be neither finalized nor built on: a hub on the testnet stopped so for good.
- No honest validator may be left unable to finalize by asynchrony, however long, once the network heals.

## Decision

- **Keep what the next block needs, and what any block held builds on.** An honest leader that knows of the highest notarized view builds on the block it stands for, or later, so an engine keeps the views from the highest one that notarized a block that may extend the last final block (its floor) and their blocks. Below them, it keeps every block that a block those views notarized or finalized, or that it voted for, builds on, back to the last final block: a block that skips a view both notarized and nullified builds below the floor, and one held without its parent can never be finalized or built on. It lets go of everything else below. It persists what it keeps with the rest, so a restart keeps it too. A block built below the floor skips views it let go of, whose nullifications it can no longer check: it does not vote for it, and validators that kept them may.
- **Keep what a validator behind asks for, and ask for what is missing.** Below the floor, an engine keeps the views that certified a block it keeps. A validator that lacks a block asks for it by the view that certified it, at every step until it comes: the block the newest notarized view names, or the first one missing below it, never only the view that names a block it holds. A peer hands it what it keeps from that view down, then its views from its floor on, as much as a delivery holds, and it takes that below its own floor, as it takes a finalization of a block it keeps; a proposal above the heights it lets wait tells it that others finalized more, and it asks for that.
- **Bound the chain by height.** No block is notarized more than a fixed number of heights above the last final block. Past it, leaders repeat the last block, as at the end of an epoch ([epoch-handover](epoch-handover.md)): finalizing the repeat finalizes the chain. Every repeat builds on the block itself, through the view that notarized it or a repeat of it, so repeats never form a chain of their own.
- **Bound the failed views by time, and each view's wait by a constant.** While views fail in a row, more than a few rotations of leaders, each further view waits twice as long as the one before, up to the longest view, half a minute. The views held from the highest notarized one stay few however long the network cannot notarize: one per longest view for each view a live validator leads, and the views of up to f faulty leaders, which end at once, besides, so at most n / (n − f) per longest view. None is ever dropped while it is needed.
- **The stall fits around the longest view.** A validator declares the hub stalled after the rules' stall period without a final block, and the hub then ends. So the rules refuse a stall shorter than twice the recovery from the longest outage (the view in progress and those of up to f faulty leaders after it, each at most the longest view), and one longer than the outage whose views an engine holds, counting the views of faulty leaders that end at once: an outage that long is a stall anyway.
- **A run finalized at once is delivered whole.** A delivery carries the whole chain a repeat finalizes. A run that does not fit, by size, is handed over as its last block alone, and a validator behind it loads the state after it, as one further behind than peers keep.

## Accepted

- After a long run of failed views, the first view once the network heals may wait up to the longest view, half a minute, and each faulty leader's after it as long: recovery takes at most f + 1 of the longest views, well within the stall.
- The stall a hub's rules set is bounded on both sides, from a few minutes to a few hours: a hub that wants a longer one needs an engine that holds more views.
- A repeat costs a view, also mid-epoch, but only when finality has been lost for the whole bound.
- An application that lets go of states verifies again, per view, at most the chain that waits to be final.

## Rejected

- *Dropping the lowest views and blocks past a fixed bound:* the outage that matters is the one longer than the bound, and then the hub halts for good.
- *Keeping only the chain to the highest notarized block:* a block that skips a view both notarized and nullified builds below it, and once no validator holds its parent, the hub halts for good.
- *Raising the floor only to the highest notarized view not yet nullified:* a validator learns of a nullification late, or never, while others build past the view already, so it still lets go of a parent; and it changes nothing for a block it voted for.
- *Keeping every notarized block until it is final:* in an outage of finality, every view notarizes a repeat, and memory grows with the outage.
- *Holding everything until it is final:* memory grows with the outage, which nothing bounds.
- *A fixed limit on views, with no backoff:* views past it could not be entered, and the hub would halt in the same way.
- *Doubling with no cap:* after an outage of some length, the view in progress would wait about as long again, and a hub that could recover would reach its stall first and end.
- *Repeats that build on each other:* the proof of a block would grow with every split, and a long one would no longer fit in a delivery.
