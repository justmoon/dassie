# A validator checks an escrow, and a member looks up a peer's account, when what it waits for happens; the poll stays as the fallback

**Areas:** [Settlement hubs](../areas/hubs.md#the-validator-in-the-node), [Peer protocol](../areas/peer-protocol.md#settlement-offers)

## Facts

- A hub takes no deposit until a quorum of its validators checked its escrow ([Settlement hubs](../areas/hubs.md#the-escrow)). A validator checked it at once when it began to follow the escrow, and then every `ESCROW_CHECK_EVERY_MS` (30 s). The hub runs before its sponsor sets the escrow up, so the first check finds it not set up, and the next comes up to 30 s after the setup applied. In the hub end-to-end test, the setup applied about 4 s after the first check, and the hub waited about 26 s more for the next.
- The validators follow the escrow's transactions anyway, each proven against a ledger its UNL validated. Every step of the sponsor's setup is a transaction of the escrow's own account, and once the master key is disabled no transaction of the escrow's goes without the validators' signatures. So the transaction that completes a setup is always one the watcher proves.
- What a check proves is the escrow's state at the newest anchor. A second check at the same anchor proves the same state.
- A member peered with a newcomer before the newcomer's own deposit opened its account learns of the account only by asking the hub. It asked at peering, after a failed `pay`, and then every `LOOK_UP_EVERY_MS` (60 s). Until it knew, the two disagreed about whether they promise ([Settlement hubs](../areas/hubs.md#the-members-side)), and the newcomer refused its Prepares (T04) for up to a minute.
- The newcomer names its account in its link state's offer as soon as it has one ([Peer protocol](../areas/peer-protocol.md#settlement-offers)), and its link state reaches its peers within seconds. A member sees only the operations that touch its own account, so it never sees another's account open in the hub's blocks.
- A sponsor learns whether a quorum checked the escrow only by asking: the founding stream answers requests and pushes nothing ([hub-founding](hub-founding.md)). Whether it knows changes nothing for the hub, which takes deposits as soon as a quorum's attestations are final.

## Decision

- **A validator checks the escrow as soon as one of the escrow's own transactions is proven to apply,** while no quorum has checked it, and **when a founding asks** whether a quorum checked it, since its sponsor asks only once it saw its setup in a validated ledger. Each check goes at an anchor newer than the last check's, so neither event checks more than once per anchor. The check every `ESCROW_CHECK_EVERY_MS` stays, for what no transaction of the escrow's own changes, such as a token issuer dropping a transfer fee.
- **A member looks up a peer without an account as soon as the peer's link state names one in the hub,** where the state it held before named none or another. The peer protocol tells the node so (`HubAccountAdvertised`), and the hub's scheme asks the hub at its account's next turn. The number the peer names is never taken; only f + 1 validators' answer is. The lookup every `LOOK_UP_EVERY_MS` stays, for a state that never arrives or a restart that held the new one already.
- **The sponsor keeps asking every `STATUS_EVERY_MS`.** Its first ask after the setup has the validators check at once, so it waits at most one more interval for the answer.

## Accepted

- **A check that fails on the way.** A setup spread over several ledgers is checked after each, and each failed check says in the validator's log that the escrow is not set up as its rules say, as the first check always did. A sponsor submits its whole setup at once, so this is rare.
- **A founding asks for a check every time it asks for the status,** every five seconds while no quorum checked the escrow, for as long as the founding lasts. The asking costs a validator one state proof per anchor at most, and only a sponsor its owner lists can ask.
- **A peer can make us ask the hub.** Each link state that names another account costs one lookup. The peer protocol already limits how many link states a node takes from each subject, a scheme asks for a bounded number of keys at once (`MAX_LOOKUPS`), and the rest wait for the account's next turns, so many peers naming accounts at once cause no storm.

## Rejected

- *Shorter polls:* each still waits half an interval on average, and dev and test timings follow the production rule ([engineering](../engineering.md)), so a shorter dev interval would only hide the wait.
- *Taking the account the peer names:* a peer could name another's account, and our promises would name a hold the hub would never let it claim. Only the hub says which account a key has.
- *A validator holding a founding's status request until the escrow is checked,* so the sponsor learns at once: it would hold one of the validator's few places for founding requests, which other foundings need, and it speeds up only what the sponsor's owner sees.
- *Watching the hub's blocks for accounts opened:* a member reads only what touches its own account. Reading every block would cost each member the whole hub's traffic, and the account still has to be matched to a key by a lookup.
