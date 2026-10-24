# A ledger below the newest seen is a reset only on the testnet, and only once its UNL validates it

**Areas:** [XRPL](../areas/xrpl.md#following-the-account), [Checking the XRP Ledger's validators](../designs/xrpl-validation.md#7-resets-under-a-unl). Built: only what is newer than the anchor, a reset only once the testnet's UNL validated it, a lower ledger that closed later or a higher one off the kept chain, told by the kept hash and the skip lists, a higher ledger that closed earlier ignored as an older chain on the testnet and a fork elsewhere, and the network's constant as the reset gate. Waiting for what anchors hold to be proven ([roadmap](../roadmap.md#checking-xrp-ledger-validators)): an account gone only when proven.

## Facts

- A public server address spreads its connections over several servers, and a server answers for "the validated ledger" from one that may be minutes old. After a reconnect, the scheme can be told about a ledger older than one it has already seen.
- Validated ledgers are final. Along one chain, ledger indexes and close times only grow.
- An account that does not exist as of an old ledger may exist now. Acting on that absence cancels settlements of ours that applied since, so the books show them as still owed, and the next settlement pays again.
- A testnet reset starts a new chain from a first ledger far below the old chain's, closed after it. Mainnet is never reset.
- A validation signs a ledger's hash, not its network, and the testnet's UNL shares no key with mainnet's ([verified-ledgers](verified-ledgers.md)). A server's network ID and its validated ledger are its own claims; an account's absence can be proven at a validated ledger.

## Decision

- **Only what is newer.** The scheme acts only on what a server says as of a validated ledger at or past the newest it has seen, whether by the ledger stream, a server's state or what it booked. An answer from an older ledger, an absent account included, is not acted on, and is asked for again as ledgers close. An answer from the newest ledger itself counts, since it is final.
- **A reset is a lower ledger that closed later, which the UNL validated.** A validated ledger below the newest seen but closed after it is on a new chain, which a server behind cannot report. On the testnet, that is a reset: the scheme writes off what the books held, cancels what was in flight and follows the new chain. The newest ledger, its close time and its hash are kept with the scheme's state. The ledger counts only as an anchor: a ledger the testnet's UNL signed, with signatures fresh by the node's clock; a server that shows a lower ledger is only a hint to anchor.
- **A higher ledger off the kept chain is a reset too,** once the validators are checked: a node down across a reset may meet the new chain already past its newest ledger. The kept anchor keeps the hash of the newest flag ledger below it, and an anchor whose skip lists do not hold that hash is on another chain ([the design](../designs/xrpl-validation.md#7-resets-under-a-unl)).
- **Along one chain both index and close time grow,** so a validated ledger with a higher index but an earlier close time is not on the kept chain. On the testnet it is on an older chain and is ignored, and the close times are compared before ancestry is judged, since judged by ancestry it would be off the kept chain and a reset. Validations of the old chain replayed after a reset never flip the scheme back.
- **Only the testnet resets.** Whether a network resets is a constant of the account's network, never what a server reports, and a gate of its own besides the check that keeps the account on its network. On any other network, a lower validated ledger that closed later stops the scheme, and so does a higher one that closed earlier or is off the kept chain: with no older chain to come from, it is a fork. The proven absence of an account that held anything stops it too. Nothing is written off. Devnet does not count, since the scheme cannot run there; it counts only once the scheme can.
- **An account is gone** only when a proof of absence shows its root missing from an anchor's state. A server's word that it is absent makes the scheme wait while the books hold anything of it.

## Accepted

- While its server is behind, or withholds the new chain's validations, the scheme stays syncing and settles nothing, until the server catches up or goes silent and the connection is made anew.
- A reset that also gives the testnet new validator keys is seen only once the node has fetched the new validator list, which it does when quorum is lost.
- A node whose clock runs minutes ahead sees no validation as fresh, and sees no reset until its clock is right. One whose clock runs behind can take an older anchor for fresh, but never one older than its newest, so its view never goes back.
- Until what anchors hold is proven, a server that lies can still make the scheme book what it says of the account; it can no longer claim a reset.
- Until an absence is proven, on the testnet, an account absent as of a ledger the scheme has not seen past is still written off, as when its owner deletes it.

## Rejected

- *Believe an absent account whenever the server calls its ledger validated:* a server behind calls an old ledger validated, which is how settlements that applied were cancelled.
- *The server's lower ledger with a later close time, on its word:* a server that lies could then make every testnet node write off its balance.
- *The server's network ID as the reset gate:* a server's claim; the UNL binds a ledger to its network.
- *The owner confirms a reset:* every testnet reset would stop every node until its owner acts, against zero configuration, and the owner can tell a reset from a server behind no better than the close times can.
- *A release names the new chain:* nodes could not settle after a testnet reset, which comes without notice, until a release named the new chain, and nodes that do not update would stay stuck.
- *Ignore a higher ledger that closed earlier on every network:* on a network that never resets, the UNL validated it on a chain the scheme is not on, which is a fork, and ignoring it would hide one.
- *Look up what is in flight before writing it off:* a server behind cannot find what applied after its ledger either, and after a real reset the old chain's history is gone.
