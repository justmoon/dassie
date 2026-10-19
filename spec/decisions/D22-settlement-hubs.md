# D22. Settlement hubs

**Status:** active, decided 2026-10-17. A design, not built yet ([roadmap](../roadmap.md)).
**Areas:** [Settlement hubs](../designs/settlement-hubs.md), [Settlement](../areas/settlement.md), [Peer protocol](../areas/peer-protocol.md)

## Facts

Settling is up to the node that owes, so peering needs trust, which the credit a node extends only bounds. Some owners would rather trust a few well-known validators once than every peer.

## Decision

Add settlement hubs as another way to peer, beside bilateral peering, which stays. [The design](../designs/settlement-hubs.md) describes them:

- **A hub** is a ledger kept by n = 3f + 1 validators from a default list, backed by an escrow account on the XRP Ledger that their signer list controls with quorum 2f + 1. Anyone can found one; a node uses a hub only when it accepts all of its validators.
- **Holds, not a hub operation per packet.** A payer sets funds aside for each payee, signs a commitment with each Prepare and a cumulative claim with its reply to each Fulfill, and the payee takes from the hold with either. The hub sees settlements and disputes, never the packet path.
- **One ordered log** for every operation, with Simplex consensus, built without IO as [D16](D16-engineering-style-pragmatic-tigerstyle-and-deterministic-simulation.md) asks.
- **Native multi-signing** for the escrow, each validator signing alone.
- **Payout even from a stalled hub,** through payout sets that hand the escrow to keys made for that set alone, so the XRP Ledger applies one set and refuses the rest.
- **Fees are rules,** with caps; **the asset is XRP** at first; the default list of validators is kept like the default bootstrap node list.

Hubs hold real money only once [D8](D08-real-money.md)'s conditions are met.

It will affect: the `hub` settlement scheme, commitments and claims in the peer protocol, a node-info extension, the testing campaign, the payer's Reject over a hub ([D13](D13-connector-timing-and-packet-limits.md)), the reply to a Fulfill ([D23](D23-each-node-books-its-own-fulfills.md)), and starting credit ([D20](D20-trust-follows-what-a-peer-earned-us.md): a hold is credit, so no slow start).

## Accepted

- A hub that loses more than f validators pays out only once q of them have signed, which may take until enough return: the XRP Ledger has no way to make a transaction or a change of signers valid only after a time.
- A stall pays out with no notice period for claims, so payees lose what they had not taken from holds, from a block that may be older than the last one any member saw.

## Rejected

- *Reserving funds in the hub for every batch of packets:* a consensus round on every hop, funds locked by every rejected packet until it expires, and every packet's condition shown to the validators.
- *Payments without consensus* (FastPay, Astro): a hold is spent by two parties, its payee before the expiry and its payer after, and near the expiry neither may get a quorum.
- *A threshold key (FROST) for the escrow:* its signing is interactive within a fixed group, so a validator that returns later cannot add its share.
- *A recovery signer* that can move the escrow without the validators: without a time lock, it weakens the escrow at all times.
- *Paying out only after a committed shutdown:* a hub that lost more than f validators could never pay out.
- *Chaining a payout set's transactions by `AccountTxnID`:* the hash of a multi-signed transaction depends on which signatures were put together.
- *AlephBFT, HotStuff-2 or Jolteon, Mysticeti:* slower, or built for many validators. *Depending on Commonware or Malachite:* async runtimes of their own, or alpha and unaudited; Commonware's Simplex is the reference instead.
