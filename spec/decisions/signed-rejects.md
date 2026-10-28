# A payee signs a release with every Reject over a hub, and a payer disputes a claim of what it released

**Areas:** [Settlement hubs](../areas/hubs.md#holds), [design](../designs/settlement-hubs.md#4-peering-over-a-hub), [Peer protocol](../areas/peer-protocol.md#message-set), [Connector](../areas/connector.md#responses-and-expiry). Version 2 of the hub protocol ([hub-protocol-version](hub-protocol-version.md)).

## Facts

- A payer A signs a commitment with every Prepare to its payee B, which B can claim on the hub with the packet's fulfillment until the commitment expires, without A's help ([settlement-hubs](settlement-hubs.md)).
- The hub cannot see a Reject: it travels between A and B. In version 1, A rejects upstream at once when B rejects, and a claim B wins later for that packet is paid, then booked by A as a loss and as B's cheating.
- B learns a fulfillment without forwarding a packet when it is the receiver, or works with the receiver, who makes the condition. So B can reject packets, which frees room in the hold so A keeps sending, and claim them all before their commitments expire: up to the whole hold, in one burst, before A ends the peering. The design's "B is paid only for what A signed" does not hold for those packets.
- A redeem nets every claim against A's signed total: up to the claim's W, B is paid what A's total says, whatever it claimed. So the theft works only for a B that never redeems past what it rejected, one that is leaving anyway.
- An honest B never claims a packet it rejected: it waits for the hub on its own timeouts ([connector-timing](connector-timing.md)), so it never rejects a packet it might still have to pay for downstream.
- Rejects are frequent and carry the payment's own errors: STREAM's T04 and F99, and its probes of exchange rates. A Reject that waited for the hub would take up to the packet's expiry.

## Decision

- **B signs a release with every Reject over a hub:** its account key's signature over the hub, the hold, the commitment's sequence and its expiry, under a domain of its own. It goes with the Reject in a peer-protocol message of its own, `hubReject`. B signs one too for a Prepare it refuses before taking it, if A signed its commitment for A's hold toward B.
- **A release names one commitment.** Nothing keeps A from signing a second commitment under a sequence it used, so B remembers every sequence it released, across restarts, and takes no commitment of it while the released one could still be claimed; once that one expired, B forgets the release, and may take a new commitment of the sequence. The hub puts back only what a claim took for the commitment a release names, by its expiry, so a release B gave never takes back B's claim of another commitment.
- **A's Reject waits without one.** Over a hub that takes releases, a Reject without B's valid release of the packet's commitment does not stand: the node takes no Reject for it, and the packet waits for the hub as one that expired. So does a definite failure B answered with a status, since B read the commitment. A packet that never reached B is failed at once, as before.
- **Claims wait for the hold's notice.** What a `claim` takes leaves the hold and waits, all of a hold's together, until the hold's notice has passed since the last claim that added to it; then it is paid to B. The notice is the one a payer gives before it reduces a hold, for the same reason: a hold changes only after the other party had the time to answer.
- **A disputes with the release.** A `dispute` carries B's releases of commitments on one hold; what a claim took for each that still waits goes back to the hold, and the sequence stays claimed, for nothing, so it is never claimed again. A dispute is only of a sequence above the last redeemed W, since a redeem past it counted what the claim took as paid, and W only grows. It is a deadline operation, as a claim is.
- **A redeem leaves what waits alone,** and subtracts what `claim` took up to its W, paid or waiting, as in version 1: B is paid A's total whatever it claimed.
- **What a hold has waiting keeps it open:** a reduction waits for it, and B cannot release the hold. At a stall or the end of the hub, what waits goes back to A with the hold, as what B had not taken does.
- **The cheating still ends the peering.** A's fee for a dispute is a loss of its own cause (`DisputeFee`), booked against B as cheating; a claim A could not dispute in time, while it was away longer than the notice, is paid and booked as `Cheated`, as before.
- **Members learn the version from the validators.** From version 2 on, every answer to `Since` states the version, which a member takes from f + 1 alike. B sends releases, and A requires them, only once it knows the hub runs version 2.

## Accepted

- B's honest claims are paid a hold's notice later, a minute at the starting terms, and a later claim on the same hold moves the payout of what waits. Claims are the exception: they happen when A's own claims are late.
- A that is away longer than the hold's notice loses what it could not dispute, as it lost it at once before.
- Over a hub of version 2, a Reject from a peer whose release does not hold, or a failure the peer answered with a status, rejects upstream only once the hub decided the packet, at its expiry.
- A payee that already remembers as many releases as it keeps (`MAX_RELEASED`) rejects without one, and the payer waits for the hub.
- A member that read its holds anew learns what waits in a hold without its sequences: a payer then counts a claim it disputed as paid once the hold's claims are paid, which books B's cheating whether or not the dispute came first.

## Rejected

- *Waiting for the hub on every Reject:* every failed payment, and every rate probe, would take up to the packet's expiry, to guard against what only a cheating payee does.
- *Clawing back after the payout:* B can have spent or withdrawn what it was paid by the time A shows the release.
- *A sending its releases to the hub ahead of a claim:* an operation in consensus for every rejected packet, which [settlement-hubs](settlement-hubs.md) rejected for the same reason as reserving funds per packet.
- *A release that names the commitment alone, without B's memory:* A could send the identical commitment again while it can still be claimed, and a receiver that works with A could fulfill it the second time.
- *B's memory alone, by sequence:* B forgets a release once its commitment expired, and A could then sign a new commitment under the sequence, let B take and fulfill it, and dispute B's claim of it with the old release.
- *B remembering every release until a claim of A's passes it:* A holds back its claims, and B's memory would fill.
- *A penalty from B's balance on a disputed claim:* the dispute already makes the theft pointless, and B loses the peering either way; a penalty needs B to hold something, and a rule for how much.
- *A separate challenge window in the terms:* the hold's notice already is the time one party of a hold has to answer the other, and a term of its own would change the terms' layout.
- *A new version of the peer protocol, `dassie/3`, for the release:* every node would speak two versions for a release cycle. `hubReject` joins version 2 as the names' messages did: a node that does not know it answers 400, and it is sent only over a hub of version 2, whose members all run a release that knows it.
- *The release in the ILP Reject's data:* the connector forwards a peer's Reject upstream byte for byte, and STREAM's receivers put their own frames there.
