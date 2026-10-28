# A hub runs one version of the hub protocol, which its ID commits to

**Areas:** [Settlement hubs](../areas/hubs.md#the-rules), [design](../designs/settlement-hubs.md#2-founding-a-hub). Built: the version in the terms, refused unless this code runs it, kept by new rules, and pinned by golden vectors. Not built: moving a running hub to another version ([roadmap](../roadmap.md#settlement-hubs)).

## Facts

- Validators of one hub are different owners' nodes, which update to new releases each on its own schedule. A hub runs across releases.
- Every validator must reach the same state from the same blocks: the encodings, the digests, what the ledger does with a block and the rules of consensus are agreed between them. A release that changes any of them, run beside one that does not, splits the hub without a word: the two compute different state roots, and neither has a quorum.
- Members check what validators sign by the same encodings and digests.
- Nothing else commits a hub to how it runs: its rules name its asset, fees and timings, not the behavior of the code that reads them.

## Decision

- **A version of the hub protocol** (`rules::HUB_VERSION`) names what every validator of a hub must agree on: the hub's encodings and digests, what its ledger does with a block, and its consensus rules. It is not the release's version: a release that changes none of them keeps it, and one that changes any of them, however small, makes a new one.
- **The terms name it, first,** so the hub ID commits to it. A hub runs one version for its whole life: new rules keep it, as they keep the asset.
- **A node runs only the version its code runs.** Terms of another version neither decode nor check: a validator neither accepts nor signs the genesis of such a hub, its file of hubs is refused, a member does not join, and a stored hub of another version does not load. A node never runs a hub it would execute otherwise than its peers.
- **Golden vectors pin the version** (`testdata/hub/vectors.json`): rules and the ID, operations, promises, statements, a block with the state the ledger leaves after it, a vote, the genesis and its certificate, and the escrow's transactions. Once a release carries a version, a change that moves one of its vectors is a new version; until then no hub a release founded runs it, and its vectors may still move.

## Accepted

- A release that changes the protocol cannot run the hubs founded before it: they keep running on validators that have not updated, and are founded again under the new version, with their members moving their funds over. While no live network depends on hubs, that is cheap.
- A hub runs as long as its validators run a release of its version; a node that runs several hubs runs them all at its own version.

## Rejected

- *The release's version in the rules:* every release would found hubs anew, though most change nothing validators agree on, and two releases that agree would refuse each other.
- *Moving to a new version through `Reconfigure`, now:* a node judges new rules by its own version's checks, and one that does not run the new version cannot tell whether they are valid; validators of different releases would execute the same `Reconfigure` differently, which is the split this decision prevents. The room is left for an operation of its own, signed by a quorum, that moves a hub to version N at height h, which validators that run N follow and others stop at, once a hub must outlive a version.
- *Switches per feature in the rules:* each switch doubles what a validator must run identically, and nothing gains from running half of a version.
- *No version, with the vectors alone:* vectors catch a change in review, not a hub of mixed releases in production.
