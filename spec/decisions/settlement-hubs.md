# Settlement hubs

**Areas:** [Settlement hubs](../designs/settlement-hubs.md), [Settlement](../areas/settlement.md), [Peer protocol](../areas/peer-protocol.md). A design, partly built ([Settlement hubs](../areas/hubs.md), [roadmap](../roadmap.md#settlement-hubs)).

## Facts

Settling directly is up to the node that owes, so peering needs trust, which the credit a node extends only bounds. Owners would rather trust a few well-known validators once than every peer.

That trust matters most with real money. A hub's validators learn of deposits and withdrawals from XRP Ledger servers, which can lie unless the validators check the ledger's own validators against proofs. The XRP Ledger's testnet and mainnet share addresses, and a transaction names no network.

## Decision

Peers settle over settlement hubs, and only over them ([hubs-only](hubs-only.md)). [The design](../designs/settlement-hubs.md) describes them:

- **A hub** is a ledger kept by n = 3f + 1 validators from a default list, backed by an escrow account on the XRP Ledger that their signer list controls with quorum 2f + 1. Anyone can found one; a node uses a hub only when it accepts all of its validators.
- **Holds, not a hub operation per packet.** A payer sets funds aside for each payee, signs a commitment with each Prepare and a cumulative claim with its reply to each Fulfill, and the payee takes from the hold with either. The hub sees settlements and disputes, never the packet path.
- **One ordered log** for every operation, with Simplex consensus, built without IO as [tigerstyle](tigerstyle.md) asks.
- **Native multi-signing** for the escrow, each validator signing alone.
- **Payout even from a stalled hub,** through payout sets that hand the escrow to keys made for that set alone, so the XRP Ledger applies one set and refuses the rest.
- **A hub's validators are regional,** within a few tens of milliseconds of each other; its members may be anywhere ([hubs-only](hubs-only.md)). The rules state the hub's margin and an allowance for the legs between members, and a node peers over a hub only when both fit in the window its connector already keeps, so a peering over a hub keeps the connector's timing but for the payer's Reject. A member whose legs are slower than the allowance books what it learns of too late.
- **The hold is the credit,** all of it and nothing more, and does not grow with earnings. Settling on request funds the hold.
- **Hubs only.** Every hub two nodes share is a settlement method of their peering ([settlement-methods](settlement-methods.md)), and nodes that share none do not peer; nothing settles directly ([hubs-only](hubs-only.md)). On the test network, a node joins the default hub of each asset unless its owner chose another. On mainnet, a node joins a hub only once its owner turns hubs on, at first: the default list would otherwise steer owners' real money into an escrow its validators jointly control, before any mainnet hub has a record. A node is in one hub per asset and underlying ledger, and may be in hubs of several assets. When a hub fails, its method becomes unavailable on every peering over it, with no fallback to settling directly.
- **A node moves its own funds** between its account on the underlying ledger and the hub, to stay between a floor its holds need and a ceiling its owner sets, both counted in largest packets.
- **Fees are rules,** flat, with caps, and **accounts and holds lock reserves,** as on the XRP Ledger, which price their place in the hub and pay each account out at shutdown.
- **The assets are XRP and a dollar token,** on the testnet and on mainnet, where hubs are part of the launch ([real-money](real-money.md)): `hub-xrpl-testnet-xrp` and `hub-xrpl-testnet-dsusd`, `hub-xrpl-xrp` and `hub-xrpl-rlusd` ([hub-scheme-names](hub-scheme-names.md), [token-hubs](token-hubs.md)). The default list of validators is kept like the default bootstrap node list, and so is a **default hub** for each asset and ledger, which owners may override. Each network has its own entries: a validator's record on the testnet vouches for nothing on mainnet, and mainnet's default hub comes with mainnet's default list of validators.
- **Validators attest only what they verified,** each against the XRP Ledger's validators itself ([verified-ledgers](verified-ledgers.md)): a deposit or a withdrawal's outcome once it is a transaction proven in a ledger the network's UNL validated, and the escrow's setup once its state is proven at such a ledger. A server's word attests nothing. A validator may run the patched rippled that serves the proofs, which adds a proof server for every node.
- **A hub runs one version of the hub protocol,** which its terms name and its ID commits to, so validators on different releases never execute its blocks differently ([hub-protocol-version](hub-protocol-version.md)).
- **Validators are reached at endpoints of their own,** which their acceptances name and the ledger keeps. A validator may one day be a role of its own, run apart from a Dassie core node, by an operator that runs no node at all; so a change to how validators are found keeps them reachable at endpoints of their own, never only as nodes of the network that other nodes resolve.
- **Validators accept the terms, not the set,** so one that fails while a hub is founded is replaced alone. A validator accepts only from sponsors its owner lists.
- **The hub's messages prove themselves** and travel over a QUIC stream kind of their own. Commitments go in one message with their Prepare, and a claim after the byte that accepts a Fulfill.
- **A reset of the test network** is written off in the hub, which makes its escrow again at the same address and keeps its ID. A validator attests one only once the testnet's UNL validated the new chain ([ledger-resets](ledger-resets.md)). Mainnet is never reset.

Hubs hold real money only once [real-money](real-money.md)'s conditions are met.

It will affect: the hub settlement schemes, commitments and claims in the peer protocol, the hubs a node's link state offers, the testing campaign, the payer's Reject over a hub ([connector-timing](connector-timing.md)), the reply to a Fulfill ([own-fulfills](own-fulfills.md)), and starting credit (a hold is credit, so no slow start).

## Accepted

- A hub that loses more than f validators pays out only once q of them have signed, which may take until enough return: the XRP Ledger has no way to make a transaction or a change of signers valid only after a time.
- A stall pays out with no notice period for claims, so payees lose what they had not taken from holds, from a block that may be older than the last one any member saw.
- Volume may gather in one hub per asset. The fewer the hubs, the more peers one deposit reaches, and a hub is bounded already by its asset.
- Mainnet hubs grow slower while owners turn them on themselves, and a mainnet node peers with no one until its owner does.
- Validators hold others' real money together on mainnet, which may make them, and whoever keeps the default list, custodians or money transmitters under some laws.
- A hub's failure makes its method unavailable on every peering over it at once; its members are cut off from each other in its asset until they are in a working hub again ([hubs-only](hubs-only.md)).
- What a payer's missing claims make its payee pay in `claim` fees has no bound yet, so the simulation does not hold a peering over a hub to [own-books](own-books.md)'s bound on what a peer can cost; it measures that cost instead.
- A node's limit is per hub. Hubs that share validators fail together, so a limit per validator operator comes with being in several hubs of one asset.
- A member whose legs to its peers are slower than the hub's allowance learns of some packets too late, and books the loss.
- Abandoned accounts keep their place in the hub's tables, which their reserves paid for, and holding back someone's claim costs only the fees of the claims that crowd it out.

## Rejected

- *Hubs beside direct settlement, or direct settlement as a fallback when a hub fails:* see [hubs-only](hubs-only.md).
- *Reserving funds in the hub for every batch of packets:* a consensus round on every hop, funds locked by every rejected packet until it expires, and every packet's condition shown to the validators.
- *Payments without consensus* (FastPay, Astro): a hold is spent by two parties, its payee before the expiry and its payer after, and near the expiry neither may get a quorum.
- *A setup fee paid atomically against the validator's keys:* a validator that takes the fee and makes no keys has shown early that it is not one to trust; the sponsor loses one fee and picks another.
- *Acceptances over rules that name the validators:* replacing one validator would void every other acceptance, and the setup fees paid for them.
- *An idle fee without reserves:* a place in the bounded tables would cost only a balance its owner can spend again, and an account's payment in the payout set could cost more than it holds.
- *A window of its own for a peering over a hub,* from delays the node measures: a peer could widen it by answering slowly, and a hub spread far enough to need one is too slow to peer over.
- *Credit beyond the hold,* growing with earnings: it brings back the trust the hub is there to remove.
- *Commitments and claims as settlement messages:* those are sent once, with nothing that ties them to their packet, so a payee could see a Prepare before its commitment.
- *The hub's messages inside the peer protocol's envelope:* they prove themselves already, and the envelope is for nodes that know each other.
- *Validators named by their node key alone, found as any peer is:* simpler, with one place for an address, but it would make every validator a core node of the network, and keep out validators run apart from one.
- *Fees that follow congestion, an idle fee and a setup fee, from the start:* each answers a problem no running hub has shown yet; they wait on the [roadmap](../roadmap.md#settlement-hubs), and reserves already price a place in the tables.
- *Founding with any sponsor:* without a setup fee, nothing would stop a stranger from making validators keep hubs they never wanted.
- *Joining mainnet's default hub by default, as on the testnet:* zero configuration and faster liquidity, but the project's default list would place owners' real money with validators before any mainnet hub has a record. Exposure is bounded by the floor and the ceiling either way; this is revisited once mainnet hubs have run.
- *One default list of validators for both networks:* an operator's testnet record would vouch for its mainnet hub.
- *A new hub after each reset of the test network:* nodes could not use it until a release named it, as [ledger-resets](ledger-resets.md) rejects for the XRP scheme.
- *A threshold key (FROST) for the escrow:* its signing is interactive within a fixed group, so a validator that returns later cannot add its share.
- *A recovery signer* that can move the escrow without the validators: without a time lock, it weakens the escrow at all times.
- *Paying out only after a committed shutdown:* a hub that lost more than f validators could never pay out.
- *Chaining a payout set's transactions by `AccountTxnID`:* the hash of a multi-signed transaction depends on which signatures were put together.
- *AlephBFT, HotStuff-2 or Jolteon, Mysticeti:* slower, or built for many validators. *Depending on Commonware or Malachite:* async runtimes of their own, or alpha and unaudited; Commonware's Simplex is the reference instead.
