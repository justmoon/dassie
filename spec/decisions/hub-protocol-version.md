# A hub runs one version of the hub protocol at a time, and its validators move it to a newer one

**Areas:** [Settlement hubs](../areas/hubs.md#the-rules), [design](../designs/settlement-hubs.md#2-founding-a-hub). Built: the version in the terms, a release that runs several versions, golden vectors for each, and the vote that moves a running hub to a newer one.

## Facts

- Validators of one hub are different owners' nodes, which update to new releases each on its own schedule. A hub runs across releases.
- Every validator must reach the same state from the same blocks: the encodings, the digests, what the ledger does with a block and the rules of consensus are agreed between them. A release that changes any of them, run beside one that does not, splits the hub without a word: the two compute different state roots, and neither has a quorum.
- Members check what validators sign by the same encodings and digests, and read the operations of every block that touches them.
- Nothing else commits a hub to how it runs: its rules name its asset, fees and timings, not the behavior of the code that reads them.
- A hub holds its members' funds in an escrow its validators control. Founding it again under a new version means every member moves its funds over, and the hub's ID, which members' configurations name, changes.

## Decision

- **A version of the hub protocol** names what every validator of a hub must agree on: the hub's encodings and digests, what its ledger does with a block, its consensus rules, and how its validators exchange its messages ([batched-validator-messages](batched-validator-messages.md)). It is not the release's version: a release that changes none of them keeps it, and one that changes any of them, however small, makes a new one.
- **The terms name the version a hub was founded under, first,** so the hub ID commits to it. New rules name the version the hub runs then.
- **A release runs a range of versions** (`rules::OLDEST_HUB_VERSION` to `rules::HUB_VERSION`), each exactly as that version says, and founds hubs under the newest. Terms of a version outside it neither decode nor check: a validator neither accepts nor signs the genesis of such a hub, its file of hubs is refused, a member does not join, and a stored hub of another version does not load.
- **Validators move a running hub to a newer version with a vote of its own,** `upgrade`, which every version counts alike, whether its code runs the version voted for or not, so validators of different releases agree on the vote. Once a quorum votes for one version, the hub runs it from the first block after the notice for new rules, before that block's operations; the hub keeps its rules and its ID, and its state names the version it runs. One move waits at a time, never beside new rules waiting, which name the version the hub runs before it; and a change of validators starts the votes afresh, as every vote.
- **A validator whose code does not run the version stops at that block,** as at a final block it cannot follow: it votes no more and signs the payout set of the last block it executed. A block of a hub of one version holds no operation a newer version brought, so a validator of an older release is never handed one before the move.
- **A validator votes only where its owner says,** naming the version in its validator file (`hubVersion`), and only for a version its release runs.
- **Members learn the version from the validators:** from version 2 on, every answer to `Since` states it, and a member takes it from f + 1 alike. One whose release does not run it stops using the hub.
- **Golden vectors pin each version** (`testdata/hub/vectors-v<version>.json`): rules and the ID, operations, promises, statements, a block with the state the ledger leaves after it, a vote, the genesis and its certificate, the escrow's transactions, and what each version adds. Once a release carries a version, a change that moves one of its vectors is a new version; until then no hub a release founded runs it, and its vectors may still move.
- **A hub moves in this order.** A release that runs the new version reaches every validator and every member first. The validators' owners then name the version in their files, and the move takes effect after the notice for new rules, which members that do not accept it can leave in.

## Accepted

- A validator of a release from before the `upgrade` vote cannot read a block that holds one, and stops at the first, though no quorum agrees yet: a faulty validator's vote alone does it. A member of such a release reads no validator's votes, and is cut off only once the hub runs the new version and states it. The order above keeps either from happening to anyone who updated.
- A release keeps the code of every version it runs. The oldest goes once no hub runs it.
- A version that changes consensus needs the engine to switch at the block the move takes effect in, as epochs hand over; version 2 changes the ledger alone.
- A version that changes only how validators exchange messages, as version 3 does, still cuts off a hub's members on releases that do not run it, though nothing changes for them: a member stops using a hub whose version its release does not run.

## Rejected

- *The release's version in the rules:* every release would found hubs anew, though most change nothing validators agree on, and two releases that agree would refuse each other.
- *Founding every hub again under each new version:* every member moves its funds over, and the hub's ID, which members' configurations name, changes.
- *Moving to a new version through `Reconfigure`:* new rules need a new acceptance from every validator, signed over the new terms, which nothing in the node gathers; and rules that name a version a validator's release does not run would not even decode, so validators of different releases would execute the same `Reconfigure` differently.
- *Switches per feature in the rules:* each switch doubles what a validator must run identically, and nothing gains from running half of a version.
- *No version, with the vectors alone:* vectors catch a change in review, not a hub of mixed releases in production.
