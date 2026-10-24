# The node books only what ledgers its own UNL validated prove

**Areas:** [Checking the XRP Ledger's validators](../designs/xrpl-validation.md), [XRPL](../areas/xrpl.md#following-the-account), [Settlement hubs](../designs/settlement-hubs.md#6-deposits-and-withdrawals). A design, not built yet ([roadmap](../roadmap.md#checking-xrp-ledger-validators)); its proof servers are an open area of their own ([roadmap](../roadmap.md#proof-servers)).

## Facts

- An XRPL account takes its server's word on which ledgers are validated and what they hold. A server that lies can have a scheme book a settlement that never arrived, cancel one that applied (so the node pays again), or claim a testnet reset that writes off the balance. With real money, a server's operator could take a node's money that way.
- A ledger is validated when a quorum of a UNL signed its hash. A ledger's header holds the roots of two trees: its transactions with their metadata, and its state, every object under its key. A leaf's hash commits to its key, so a branch from a leaf to a root proves that leaf, and a branch that ends where a key would hang proves the key absent. A ledger object, the skip list, holds the hashes of earlier ledgers.
- rippled's public API serves headers, whole binary transaction sets, binary transactions, manifests and a live stream of validations, but no past validations and no branch of either tree. Without branches, a transaction is proven only by fetching its ledger's whole set, tens to hundreds of kilobytes on mainnet, and an object only through the transaction that last changed it, which shows a state as of that transaction, not that it is current; an issuer's account is changed so often that even that is impractical. rippled computes branches already, for the peers in its own network, and keeps recent validations in memory.
- Validator lists are published, signed by their publishers, over HTTPS. Mainnet has two publishers that rippled's default configuration names, and the testnet one of its own. Validators rotate their signing keys with manifests, and the lists' manifests lag behind: several mainnet validators' current keys are only in newer manifests.
- A validation signs no network ID. The mainnet and testnet UNLs share no key.
- An account's transactions take its sequence numbers one at a time, and a transaction applies only up to its last ledger.

## Decision

- **Nothing is booked, cancelled or written off on a server's word,** on either network ([the design](../designs/xrpl-validation.md#1-the-rule)). A ledger counts once a quorum of the node's own UNL signed its hash, and the node anchors only on one whose signatures are fresh; an older ledger once an anchor's skip list proves it; a transaction, and any state, once its branch reaches the root of a proven ledger; an absence only when proven.
- **Proofs from a patched rippled,** with three public methods: `validations` for a ledger, `tx_proof` for a transaction, and `state_proof` for objects, with proofs of absence ([the design](../designs/xrpl-validation.md#3-the-proof-api)). Their shapes are the contract the node is built against. Every answer proves itself, so a proof server is relied on to answer, never trusted. The project runs one public proof server per network to start, named in the defaults; an owner can name another patched server; and the patch is offered upstream, so that any rippled may serve proofs one day. There is no fallback to a stock server: an account whose server serves no proofs stalls, and says so.
- **State is proven directly,** by its branch in the state tree, at an anchor: the account's balance and sequence, its lines, their issuers' flags, the reserve, a pool. A transaction's absence is proven by its account's sequence at its last ledger, an account's by a proof of absence. Nothing is scanned, and nothing is derived from the transaction that last changed an object.
- **Anchors on demand.** The node keeps no validations subscription; it asks `validations` for the ledger it anchors on, and only then.
- **The UNL comes from signed lists** fetched over HTTPS from their publishers. Publisher keys and sites are built in per network, and the owner can replace them. The node verifies each list's signature, sequence and expiry, and the manifests in it, and takes a newer manifest from any source when it verifies under a listed master key. The proof server never relays a list. Publishers combine, and quorum is disabled while too many are unavailable, by rippled's rules. No negative UNL at launch: quorum counts the whole UNL.
- **When a list expires,** or its site cannot be reached past the expiry, the account stalls and says why. It never falls back to trusting its server.
- **Both networks verify.** The testnet schemes too, and a testnet reset counts only once the testnet's UNL validates the new chain ([ledger-resets](ledger-resets.md)).
- **A server that does not answer stalls the scheme.** Only a ledger older than the history every proof server keeps, which the server says it no longer has, is history the server does not have, as for a server without history today: logged as a warning and counted.
- **In the simulation,** validation signatures alone sit behind a narrow interface, so a simulated day does not spend its time on secp256k1; everything else is verified for real.

## Accepted

- **Availability rests on the proof servers.** With one per network at first, an outage of it stalls every node on that network that has no other: nothing settles, and nothing is booked wrong, until it returns or the owner names another. Hub validators that run the patch, and the patch upstream, add servers.
- **The project carries the patch** onto each rippled release until upstream takes it.
- **An anchor may be as old as the freshness bound allows.** A server can delay what came after it for that long, and show the peg guard a reading that old ([issued-assets](issued-assets.md)).
- **Beyond the history bound, a server can shortchange a peer** by saying it no longer has a ledger: a peer's settlement there becomes the owner's deposit. Only a node that was away longer than the proof servers keep history meets it, and it never costs the node.
- **Liveness follows the lists.** With mainnet's two publishers, either list expired or unreachable past its expiry stalls every proof, as it stalls rippled in its default configuration. So does a fifth of the UNL offline at once, which rippled rides out through its negative UNL.
- **A list site can serve an old list.** It can delay a node's view of a list update up to the expiry of the list the node holds, and give a node that has never seen a newer one an older list that has not expired; the kept highest sequence stops it going back further. rippled has the same exposure.
- **A clock minutes off stalls the account,** since it sees no validation as fresh.
- **The fee to offer stays a hint,** since it follows the load of the server's own queue; a lie costs at most the fee ceiling, and the fee booked is the proven transaction's.

## Rejected

- *Rebuilding transaction sets over a stock server's public API:* every ledger with a transaction of ours costs its whole set, state shows only as of the transaction that last changed it, so a server could pass an old state off as current, a busy issuer's account cannot practically be followed at all, and an absence takes a scan of every ledger the transaction could have applied in.
- *That rebuilding as a fallback, where no proof server answers:* a second path to the same guarantees, weaker in every case above, that runs rarely, so its faults would surface with real money; and any server could force the weaker path by refusing proofs.
- *State tied to the transaction that last changed it (`PreviousTxnID`), with a chain per object:* a tie proves a state as of its transaction, not that it is current, so a server could hide a clawback or a spend behind a real but old state. A state proof at an anchor is current by construction.
- *The validations stream,* always on or on demand: a mainnet server streams every validation it hears, gigabytes a day into each node, which a desktop node on a metered line should not pay for, and a node on demand must wait for live validations of a new ledger. `validations` gives exactly the anchor's.
- *Proofs from rippled's peer network* (`TMProofPathRequest`): every node would speak rippled's peer protocol and need to be let into its network, and servers serve those proofs only with ledger replay turned on. The patch serves the same branches over the API the account already speaks.
- *A proof service apart from rippled:* it would keep its own copy of the ledger, and a codebase that must follow every amendment; rippled holds the trees and computes the branches already.
- *Several servers that agree:* they may share an operator, and agreement is not validation. A quorum of the UNL is what the XRP Ledger means by validated.
- *Validator lists pinned in releases:* a list expires and validators rotate keys on their own schedule, so pinned nodes would stall until they updated, and a node that never updates would stall for good. Publishers' signed lists move with the network; their keys are what releases pin.
- *A static list of validators without a publisher:* the same, by the owner's hand.
- *Lists relayed by the proof server:* it could not forge one, but it could choose which unexpired list a new node starts from, and one server would stand behind both what the node proves and what it proves against. Fetched from the publishers, the lists do not depend on the proof server.
- *The server's network ID as the network:* validations do not sign it; the UNL binds the network.
- *Stopping beyond the history bound:* a node away longer than the proof servers keep history could never sync again.
- *Verifying only on mainnet:* the testnet, dev and the simulations would run code that real money does not, and the verifier would meet its first lying server with real money.
