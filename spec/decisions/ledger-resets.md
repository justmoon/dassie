# A ledger below the newest seen is a reset only on the testnet

**Areas:** [XRPL](../areas/xrpl.md#following-the-account).

## Facts

- A public server address spreads its connections over several servers, and a server answers for "the validated ledger" from one that may be minutes old. After a reconnect, the scheme can be told about a ledger older than one it has already seen.
- Validated ledgers are final. Along one chain, ledger indexes and close times only grow.
- An account that does not exist as of an old ledger may exist now. Acting on that absence cancels settlements of ours that applied since, so the books show them as still owed, and the next settlement pays again.
- A testnet reset starts a new chain from a first ledger far below the old chain's, closed after it. Mainnet is never reset.
- The scheme runs only on the testnet ([real-money](real-money.md)), and a mainnet scheme is to come.
- The scheme trusts its server about which chain it is on: it does not check validators' signatures.

## Decision

- **Only what is newer.** The scheme acts only on what a server says as of a validated ledger at or past the newest it has seen, whether by the ledger stream, a server's state or what it booked. An answer from an older ledger, an absent account included, is not acted on, and is asked for again as ledgers close. An answer from the newest ledger itself counts, since it is final.
- **A reset is a lower ledger that closed later.** A server whose validated ledger is below the newest seen but closed after it is on a new chain, which a server behind cannot report. On the testnet, that is a reset: the scheme writes off what the books held, cancels what was in flight and follows the new chain. The newest ledger and its close time are kept with the scheme's state. A server's state carries its validated ledger's close time, so telling the two apart costs no request.
- **Only the testnet resets.** The gate is a function of the network ID of its own, not only the check that keeps the scheme on the testnet. On any other network, a lower ledger stops the scheme, and so does the absence of an account that held anything, with nothing written off. Devnet does not count, since the scheme cannot run there; it counts only once the scheme can.

## Accepted

- While its server is behind, the scheme stays syncing and settles nothing, until the server catches up or goes silent and the connection is made anew.
- A server that lies can still claim a reset on the testnet, by reporting a lower ledger with a later close time, and make the node write off its balance and cancel what is in flight. It could already make the scheme book anything it says. Checking validators' signatures would stop both; it is an [open area](../roadmap.md#checking-xrp-ledger-validators), not part of this decision.
- On the testnet, an account absent as of a ledger the scheme has not seen past is still written off, as when its owner deletes it.

## Rejected

- *Believe an absent account whenever the server calls its ledger validated:* a server behind calls an old ledger validated, which is how settlements that applied were cancelled.
- *The owner confirms a reset:* every testnet reset would stop every node until its owner acts, against zero configuration, and the owner can tell a reset from a server behind no better than the close times can.
- *A release names the new chain:* nodes could not settle after a testnet reset, which comes without notice, until a release named the new chain, and nodes that do not update would stay stuck.
- *Look up what is in flight before writing it off:* a server behind cannot find what applied after its ledger either, and after a real reset the old chain's history is gone.
