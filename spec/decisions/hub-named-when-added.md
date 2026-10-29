# A hub scheme names its hub from the moment it is added

**Areas:** [Settlement hubs](../areas/hubs.md#in-the-node), [Settlement](../areas/settlement.md#schemes-and-ledgers), [design](../designs/settlement-hubs.md#11-choosing-a-hub). Leaving a hub that ended for another: [leaving-a-hub-that-ended](leaving-a-hub-that-ended.md); leaving one that runs is not built ([roadmap](../roadmap.md#settlement-hubs)).

## Facts

- An owner who adds a hub's scheme without rules of its own joins the default hub of its asset, whose rules the release carries ([In the node](../areas/hubs.md#in-the-node)). `dassie setup`, the wallet and the market maker's VM all add it so.
- A release founds a default hub again whenever what a hub's ID commits to changes: its protocol version, its encodings, the paths of its validators' keys ([hub-protocol-version](hub-protocol-version.md), [hub-keys-by-network](hub-keys-by-network.md)). The hub before it keeps running on validators that have not updated, and holds its members' money until they move it over.
- The node's account in a hub is that hub's: its key derives from the hub ID, its number, holds and operations are entries of that hub's ledger, and the books keep them as the account's state ([hub-keys-by-network](hub-keys-by-network.md)). They mean nothing in another hub, and another hub's account of the same number is someone else's.
- A scheme's config and its account's state outlive the release that wrote them.

## Decision

- **A hub's scheme added without rules is stored with the default hub's,** resolved when it is added: the node writes `{"rules": "…"}` as its config, so a stored hub scheme always names its hub. A later release with another default hub moves no node: it runs the scheme over the hub it was added with, or, when that hub no longer decodes, leaves it out and says so. An empty stored config names no hub, and the scheme is left out.
- **A scheme added without rules while no default hub of its asset is built in is refused,** with the reason, rather than stored for a default to fill in later.
- **The account's state names its hub,** and an account restores only a state of its own hub. A stored scheme whose config names another hub than its account's state is left out at start, with both hub IDs in the reason, and is added again only with its own hub's rules.

## Accepted

- **An owner whose default hub is founded again stays in the old one** until the owner moves. The old account's state stays with the books, so the scheme is added again only with the old hub's rules, until the node lets go of it: it does so when the owner leaves a hub that ended, once a quorum of its validators signed its payout set ([leaving-a-hub-that-ended](leaving-a-hub-that-ended.md)). Leaving a hub that runs needs its account closed first, which is not built ([roadmap](../roadmap.md#settlement-hubs)).
- **A state of the account from before it named its hub is refused.** No database this version opens keeps one: those of the early testnet are refused whole ([database-continuity](database-continuity.md)).

## Rejected

- *An empty config resolved at every start:* a release that founds its default hub again would move every node into the new hub silently, and restore the old hub's number, holds and operations into an account of the new one, signed with a key that is not the old account's.
- *An empty config resolved once, at the first start of a release, by a migration:* no database this version opens stores one, so a migration would carry a rule for no database.
- *Following a new default hub when the old one no longer decodes:* the old hub may still hold the node's money, whose state the books keep; moving is the owner's choice, made by leaving the old hub once it ended.
