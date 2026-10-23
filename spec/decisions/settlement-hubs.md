# Settlement hubs

**Areas:** [Settlement hubs](../designs/settlement-hubs.md), [Settlement](../areas/settlement.md), [Peer protocol](../areas/peer-protocol.md). A design, not built yet ([roadmap](../roadmap.md)).

## Facts

Settling is up to the node that owes, so peering needs trust, which the credit a node extends only bounds. Some owners would rather trust a few well-known validators once than every peer.

## Decision

Add settlement hubs as another way to peer, beside bilateral peering, which stays. [The design](../designs/settlement-hubs.md) describes them:

- **A hub** is a ledger kept by n = 3f + 1 validators from a default list, backed by an escrow account on the XRP Ledger that their signer list controls with quorum 2f + 1. Anyone can found one; a node uses a hub only when it accepts all of its validators.
- **Holds, not a hub operation per packet.** A payer sets funds aside for each payee, signs a commitment with each Prepare and a cumulative claim with its reply to each Fulfill, and the payee takes from the hold with either. The hub sees settlements and disputes, never the packet path.
- **One ordered log** for every operation, with Simplex consensus, built without IO as [tigerstyle](tigerstyle.md) asks.
- **Native multi-signing** for the escrow, each validator signing alone.
- **Payout even from a stalled hub,** through payout sets that hand the escrow to keys made for that set alone, so the XRP Ledger applies one set and refuses the rest.
- **Hubs are regional.** Validators and the members that peer over a hub are within a few tens of milliseconds of each other. The rules state the hub's margin and an allowance for the legs between members, and a node peers over a hub only when both fit in the window its connector already keeps, so a peering over a hub keeps the connector's timing but for the payer's Reject.
- **The hold is the credit,** all of it and nothing more, and does not grow with earnings. Settling on request funds the hold.
- **Hub or direct.** Two nodes in the same hub peer over it; otherwise they settle directly, which stays. On the test network, a node joins the default hub unless its owner turned hubs off, or its own round trip to the validators exceeds the rules' allowance. A node is in at most one hub per asset and underlying ledger for now. A node whose hub fails peers again directly.
- **A node moves its own funds** between its account on the underlying ledger and the hub, to stay between a floor its holds need and a ceiling its owner sets, both counted in largest packets.
- **Fees are rules,** flat, with caps, and **accounts and holds lock reserves,** as on the XRP Ledger, which price their place in the hub and pay each account out at shutdown.
- **The asset is XRP** at first. The default list of validators is kept like the default bootstrap node list, and so is a **default hub** for each asset and ledger, which owners may override.
- **Validators accept the terms, not the set,** so one that fails while a hub is founded is replaced alone. A validator accepts only from sponsors its owner lists.
- **The hub's messages prove themselves** and travel over a QUIC stream kind of their own. Commitments go in one message with their Prepare, and a claim after the byte that accepts a Fulfill.
- **A reset of the test network** is written off in the hub, which makes its escrow again at the same address and keeps its ID.

Hubs hold real money only once [real-money](real-money.md)'s conditions are met.

It will affect: the `hub-xrp-testnet` settlement scheme, commitments and claims in the peer protocol, a node-info extension, the testing campaign, the payer's Reject over a hub ([connector-timing](connector-timing.md)), the reply to a Fulfill ([own-fulfills](own-fulfills.md)), and starting credit ([earned-trust](earned-trust.md): a hold is credit, so no slow start).

## Accepted

- A hub that loses more than f validators pays out only once q of them have signed, which may take until enough return: the XRP Ledger has no way to make a transaction or a change of signers valid only after a time.
- A stall pays out with no notice period for claims, so payees lose what they had not taken from holds, from a block that may be older than the last one any member saw.
- Volume may gather in one hub per asset. The fewer the hubs, the more peers one deposit reaches, and a hub is bounded already by its asset and the region its validators share.
- Nodes far from every hub settle directly, as before.
- While a hub runs, a node in it keeps no peering outside it, so a hub's failure cuts off every peering over it at once, until its nodes have peered again directly.
- What a payer's missing claims make its payee pay in `claim` fees has no bound yet, so the simulation does not hold a peering over a hub to [own-books](own-books.md)'s bound on what a peer can cost; it measures that cost instead.
- Until a node can be in several hubs, its limit is on the one hub it is in. Hubs that share validators fail together, so a limit per validator operator comes with being in several.
- A member whose legs to its peers are slower than the hub's allowance learns of some packets too late, and books the loss.
- Abandoned accounts keep their place in the hub's tables, which their reserves paid for, and holding back someone's claim costs only the fees of the claims that crowd it out.

## Rejected

- *Reserving funds in the hub for every batch of packets:* a consensus round on every hop, funds locked by every rejected packet until it expires, and every packet's condition shown to the validators.
- *Payments without consensus* (FastPay, Astro): a hold is spent by two parties, its payee before the expiry and its payer after, and near the expiry neither may get a quorum.
- *A setup fee paid atomically against the validator's keys:* a validator that takes the fee and makes no keys has shown early that it is not one to trust; the sponsor loses one fee and picks another.
- *Acceptances over rules that name the validators:* replacing one validator would void every other acceptance, and the setup fees paid for them.
- *An idle fee without reserves:* a place in the bounded tables would cost only a balance its owner can spend again, and an account's payment in the payout set could cost more than it holds.
- *A window of its own for a peering over a hub,* from delays the node measures: a peer could widen it by answering slowly, and a hub spread far enough to need one is too slow to peer over.
- *Credit beyond the hold,* growing with earnings: it brings back the trust the hub is there to remove.
- *Commitments and claims as settlement messages:* those are sent once, with nothing that ties them to their packet, so a payee could see a Prepare before its commitment.
- *The hub's messages inside the peer protocol's envelope:* they prove themselves already, and the envelope is for nodes that know each other.
- *Fees that follow congestion, an idle fee and a setup fee, from the start:* each answers a problem no running hub has shown yet; they wait on the [roadmap](../roadmap.md#settlement-hubs), and reserves already price a place in the tables.
- *Founding with any sponsor:* without a setup fee, nothing would stop a stranger from making validators keep hubs they never wanted.
- *A new hub after each reset of the test network:* nodes could not use it until a release named it, as [ledger-resets](ledger-resets.md) rejects for the XRP scheme.
- *A threshold key (FROST) for the escrow:* its signing is interactive within a fixed group, so a validator that returns later cannot add its share.
- *A recovery signer* that can move the escrow without the validators: without a time lock, it weakens the escrow at all times.
- *Paying out only after a committed shutdown:* a hub that lost more than f validators could never pay out.
- *Chaining a payout set's transactions by `AccountTxnID`:* the hash of a multi-signed transaction depends on which signatures were put together.
- *AlephBFT, HotStuff-2 or Jolteon, Mysticeti:* slower, or built for many validators. *Depending on Commonware or Malachite:* async runtimes of their own, or alpha and unaudited; Commonware's Simplex is the reference instead.
