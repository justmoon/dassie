# Each ledger has its own largest packet and default credit, fixed in the build

**Areas:** [Connector](../areas/connector.md), [Settlement](../areas/settlement.md)

## Facts

Every threshold of a link is counted in its largest packet ([thresholds-in-packets](thresholds-in-packets.md)), and the default credit sizes what one request to settle funds a hold by ([Settlement](../areas/settlement.md#credit)). Every ledger counts at scale 9, so one value for the whole node is worth a different amount in each asset, and a packet a conversion moves to another ledger is held to that same number in the other ledger's units. Nothing carries either value on the wire: a peer learns the largest packet only from an F08, and the credit is never announced. The XRP Ledger's own ledgers are owner books, which peer with nobody, so no thresholds are counted on them, and every move between a hub and its owner book is sized from the hub's scheme alone.

## Decision

- **A value for each ledger.** Each ledger has its largest packet (`Config::max_packet`, `crates/dassie-connector/src/config.rs`) and its default credit (`Policies::peer_credit`, `crates/dassie-node/src/node/mod.rs`), from a match over every ledger, so a new ledger has none until it is given them. Ledgers that should agree share a named constant: the testnet and mainnet ledgers of an asset, so the tests run on what real money does, and the dollar ledgers, which count the same dollar at par.
- **Fixed in the build.** The values are constants of a release, not converted at a rate, not stored and not an owner's setting. A node reads them as it starts; a release that changes one needs nothing of the peers, since each node keeps its own books and thresholds, and the owner's hub target is held to the new bounds when it is read.
- **Each threshold follows its ledger's.** A packet is held to its own ledger's largest packet, what a conversion makes of it to the outgoing ledger's, the money in flight on a method to its ledger's; a scheme counts its trigger, its least settlement, its holds and its hub's target in its ledger's.
- **The two stay apart.** The largest packet is what the owner is prepared to lose on a link; the credit sets how large and how frequent requests to settle are, a trade between the fees of a request and the money held. So they are set apart, at eleven packets of credit each for now, and the node refuses to start where a ledger's credit is below the least that survives a crash in that ledger's largest packet.

## Accepted

- The values drift from equal worth as prices move. A release corrects them; meanwhile the outgoing ledger's largest packet still refuses (F08) a packet that a conversion makes too large for the next link.
- A ledger whose largest packet differs from another of its asset is possible, and nothing requires them to agree.

## Rejected

- *One value converted at the current rate:* thresholds that move with the market, which [thresholds-in-packets](thresholds-in-packets.md) rejected for the credit. A price move of a tenth would move the hub's target a whole band, and the node would move funds and pay fees on prices alone. Settlement would stop while the rates are too old, where today only cross-currency packets do; a node of one asset would need rates; whoever moves a pool the rates read would move every node's thresholds; and a threshold could change between two steps that rely on it.
- *A value for each asset:* nothing needs ledgers of one asset to agree, and it would keep two hubs of one asset from differing where their fees do.
- *The credit as a fixed number of packets:* one value fewer, but a ledger whose requests cost more could not fund fewer, larger ones.
- *An owner or operator setting:* the credit is the node's policy, and a free value could leave a hub link that does not survive a crash. The owner sets the hub's target.
