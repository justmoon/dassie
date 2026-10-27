# Real money

**Areas:** [XRPL](../areas/xrpl.md#real-money), [Node](../areas/node.md), [Settlement hubs](../designs/settlement-hubs.md), [Roadmap](../roadmap.md#real-money).

## Facts

- On the test network a bug, a lying peer or a lying server costs nothing real. On mainnet it costs owners their money.
- What a peer can cost a node is bounded by the credit it extends plus what is in flight ([own-books](own-books.md)), but only while the node's books are right about what arrived on the ledger. A node that takes its server's word books what the server says, so the server's operator could have it credit XRP that never arrived, or cancel a settlement that applied and pay it again.
- A dollar token is booked at par ([issued-assets](issued-assets.md)); a node that converts at par while the token is off its peg can be drained.
- A node can check what a server says only against proofs, which a stock rippled does not serve ([verified-ledgers](verified-ledgers.md)).
- The XRP Ledger's testnet and mainnet share transaction formats and addresses, and a transaction names no network. A testnet wallet's keys are not mainnet's, but a node pointed at a mainnet server would read mainnet as its own.
- Some owners would rather trust a few validators once than each peer ([settlement-hubs](settlement-hubs.md)), and on mainnet that trust is worth the most.

## Decision

The `live` realm stays refused until all of these hold:

- peer messages are authenticated ([peer-protocol-v1](peer-protocol-v1.md));
- the ledger is durable and reconciles with the underlying ledger;
- the XRPL schemes handle reserves, fees and transaction results;
- releases are signed ([release-integrity](release-integrity.md));
- **the node checks the XRP Ledger's validators itself,** on both networks, against a UNL of its own, with transactions and state proven by branches against validated ledgers ([verified-ledgers](verified-ledgers.md));
- **proof servers run:** a public rippled patched to serve those proofs, for each network, named in the node's defaults ([verified-ledgers](verified-ledgers.md));
- **mainnet schemes exist for XRP and RLUSD,** each with a memo type of its own, and a ledger runs only in its node's realm ([mainnet-schemes](mainnet-schemes.md)). There is no DSUSD on mainnet;
- **the dollar token's scheme pauses while its pool prices it off its peg** ([issued-assets](issued-assets.md));
- **settlement hubs run on mainnet:** a mainnet hub scheme, whose validators observe the escrow through proofs, and a default mainnet hub ([settlement-hubs](settlement-hubs.md));
- there has been an external review;
- a mainnet bootstrap node list exists.

**Each network's account refuses another network's server.** Today the testnet account stops unless its server reports the testnet's network ID, so an owner who points it at a mainnet server gets a failed scheme, not mainnet XRP signed for with a testnet wallet. With the validators checked, the UNL is what binds an account to its network, and the network ID stays the first, cheap refusal, as today: a server on another network gets nothing booked either way.

## Accepted

- **Launch, then observe.** Incentive problems no running network has shown yet (hub fees that follow congestion, idle and setup fees) are not solved before the launch. Their starting values are in the code; running nodes show which need more.
- **What verifying ledgers leaves:** a proof server that is down stalls the nodes that rely on it, an anchor may be as old as the freshness bound allows, and beyond the history the proof servers keep a server can move a peer's settlement into the owner's suspense ([verified-ledgers](verified-ledgers.md)).

## Rejected

- *Mainnet with the server trusted, and the validators checked later:* real money is what makes lying worth a server operator's while.
- *Launching on stock servers before the proof servers run:* a node could then prove nothing, so it would either trust its server or never settle.
- *Hubs after the launch:* owners who would rather trust a few validators than each peer could not, on the network where that trust matters.
- *A dollar of the project's own on mainnet,* as DSUSD is on the testnet: the project would issue real money ([mainnet-schemes](mainnet-schemes.md)).
