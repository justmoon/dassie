# The node books only what ledgers its own UNL validated prove

**Areas:** [Checking the XRP Ledger's validators](../designs/xrpl-validation.md), [XRPL](../areas/xrpl.md#following-the-account), [Settlement hubs](../designs/settlement-hubs.md#6-deposits-and-withdrawals). A design, not built yet ([roadmap](../roadmap.md#checking-xrp-ledger-validators)).

## Facts

- An XRPL account takes its server's word on which ledgers are validated and what they hold. A server that lies can have a scheme book a settlement that never arrived, cancel one that applied (so the node pays again), or claim a testnet reset that writes off the balance. With real money, a server's operator could take a node's money that way.
- A ledger is validated when a quorum of a UNL signed its hash. Each header names its parent's hash, so one validated header proves every ledger below it. A ledger's transactions and their metadata are the leaves of a tree whose root is in the header.
- rippled's public API serves headers, whole binary transaction sets, binary transactions, manifests and a live stream of validations, but no past validations and no state proofs: no public method proves that an account or a line holds what a server says it holds now. rippled's peer protocol does (`TMProofPathRequest`), but speaking it means joining the peer network.
- Validator lists are published, signed by their publishers, over HTTPS. Mainnet has two publishers that rippled's default configuration names, and the testnet one of its own. Validators rotate their signing keys with manifests, and the lists' manifests lag behind: several mainnet validators' current keys are only in newer manifests.
- A validation signs no network ID. The mainnet and testnet UNLs share no key.
- Every ledger object that changes names the transaction that last changed it (`PreviousTxnID`), and that transaction's metadata holds the object's fields. An issuer's account is threaded by every trust line made or deleted, mostly with no fields: tying it means walking back thousands of steps.
- Most mainnet ledgers' transaction sets are tens to hundreds of kilobytes. Public servers throttle a few heavy requests at once per address.

## Decision

- **Nothing is booked, cancelled or written off on a server's word,** on either network ([the design](../designs/xrpl-validation.md#1-the-rule)). A ledger counts once a quorum of the node's own UNL signed its hash with fresh signing times; a transaction once `verify(root, key, leaf, branch)` accepts it against a proven header; state only through its `PreviousTxnID` transaction; an absence only when proven.
- **Proofs over the public API, built around branches.** For now the node builds the branch itself, from the whole binary transaction set of a ledger, fetched only for ledgers that hold a transaction the node must prove, its account's or one a tie names, or an absence it must prove. A source from rippled's peer network may replace the builder later, behind the same gate, and give real state proofs.
- **State through its transaction.** An account's balance and sequence, a line, a pool's balances count only when they equal the fields a verified transaction's metadata gave them. The account also keeps the chain of transactions that touched each of its objects, so a server cannot drop one from the middle of its history.
- **The UNL comes from signed lists** fetched over HTTPS. Publisher keys and list sites are built in per network, and the owner can replace them. The node verifies each list's signature, sequence and expiry, and the manifests in it, and takes newer manifests from any source under the listed master key. It combines publishers, and disables quorum when too many are unavailable, by rippled's rules. No negative UNL at launch: quorum counts the whole UNL.
- **When a list expires or its site cannot be reached past the expiry,** the account stalls and says why. It never falls back to trusting its server.
- **Both networks verify.** The testnet schemes too, and a testnet reset counts only once the testnet's UNL validates the new chain ([ledger-resets](ledger-resets.md)).
- **Hints stay hints:** fees, an issuer's flags and `submit` results steer what the node does and never reach the books. The reserve does reach them, so the account takes the higher of the server's value and a floor per network.
- **What cannot be proven within the bounds** (a set too large, a gap too deep, history that neither the server nor the next default server will serve) is history the server does not have, as for a server without history today: logged as a warning and counted.
- **Validations come on demand:** the account subscribes to the validations stream while it needs a validated ledger, and leaves it after.
- **In the simulation,** validation signatures alone sit behind a narrow interface, so a simulated day does not spend minutes on secp256k1; everything else is verified for real.

## Accepted

- **A tie proves a state as of its transaction, not that it is current.** A server can return an old state and hide what came after. The cursor sits at the tie and hidden transactions surface through the chain, in order. Beyond the bounds they become history that does not reach back: a peer's hidden settlement is booked as the owner's deposit, and a settlement of ours stays pending, never cancelled. A stale tie can also overstate: an issuer's clawback, which changes only our line, or a spend by another instance of the same wallet, stays hidden until a later proven transaction on that object surfaces it, and meanwhile the owner's books hold funds the ledger does not, so Payments against them fail, each a booked fee. A stale tie never credits a peer, cancels a settlement or writes anything off. Peer-network state proofs would end this.
- **A token issuer's flags are read unverified.** Both a global freeze and a transfer fee can only stop payments, and the ledger enforces them whatever the node believes. A lying server can pause the scheme, which a silent server can too, or make it try Payments that fail, each a proven failure whose fee is booked, with backoff. Our own line's freezes are tied, as any line's state is.
- **Liveness follows the lists.** With mainnet's two publishers, either list expired or unreachable past its expiry stalls every proof, as it stalls rippled in its default configuration. So does a fifth of the UNL offline at once, which rippled rides out through its negative UNL.
- **A cancelled settlement waits longer.** A transaction past its last ledger is cancelled only once every ledger it could have applied in was read; a `tem` from `submit`, which only a bug in our codec produces, costs a window of ledgers instead of none.
- **More traffic per settlement:** one transaction set per ledger with a transaction of ours, a set per ledger of each absence scan, and a header walk where the ring of validated ledgers does not reach. A node that waits on validations sees each booking up to a ledger later.
- **The peg guard's traffic:** each reading ties the pool's two balances to the swap that last changed them, someone else's transaction, so a busy mainnet pool costs a mainnet transaction set per freshness period.
- **A list site can serve an old list.** It can delay a node's view of a list update up to the expiry of the list the node holds, and give a node that has never seen a newer one an older list that has not expired; the persisted highest sequence stops it going back further. rippled has the same exposure.
- **A clock minutes off stalls the account,** since it sees no validation as fresh.
- **When a network lowers its reserve,** nodes hold back the old floor until a release lowers it. **When it raises its reserve above the floor,** a lying server can overstate what is spendable down to the floor until a release raises it, at the cost of failed Payments' fees.
- **What cannot be proven can shortchange the peer** whose settlement was hidden in it, never the node. A server can hide one on purpose, by refusing to serve what proves it, where no other server is tried: when the account has only one, such as one the owner named.
- **A server can withhold the newest records** on our objects for as long as the account stays on it, while relaying current validations, so a peer's settlement goes unbooked and the peer is withdrawn or ended. It costs that peering, never the node's funds.

## Rejected

- *A source from rippled's peer network now:* it gives real state proofs, but it means speaking the peer protocol and being let into the peer network, for a first release. It stays the later source behind the same gate.
- *A proof service of our own,* which serves branches and state proofs from a node the project runs: it would make one operator the root of every node's trust, which the UNL is there to avoid.
- *Several servers that agree:* they may share an operator, and agreement is not validation. A quorum of the UNL is what the XRP Ledger means by validated.
- *Validator lists pinned in releases:* a list expires and validators rotate keys on their own schedule, so pinned nodes would stall until they updated, and a node that never updates would stall for good. Publishers' signed lists move with the network; their keys are what releases pin.
- *A static list of validators without a publisher:* the same, by the owner's hand.
- *The server's network ID as the network:* validations do not sign it; the UNL binds the network.
- *Stopping on what cannot be proven:* anyone who can fill one ledger, for a few XRP of escalated fees, could stall a node's scheme.
- *Verifying only on mainnet:* the testnet, dev and the simulations would run code that real money does not, and the verifier would meet its first lying server with real money.
- *Byte-equal rebuilds of tied objects:* they need a template per object type for fields the node never reads; comparing the fields it reads is enough.
- *Always subscribed to validations:* simplest, but a mainnet server streams every validation it hears, gigabytes a day into each node, which a desktop node on a metered line should not pay for.
