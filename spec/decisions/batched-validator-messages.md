# Hub validators send each other their messages in batches, once their hub's version says so

**Areas:** [Peer protocol](../areas/peer-protocol.md#the-batch-stream), [Settlement hubs](../areas/hubs.md#the-validator-in-the-node).

## Facts

- A hub proposes a block every interval, a tenth of a second, and each validator sends each other validator several messages for each: its votes, the certificates it put together, operations it passes on. A step often sends more than one message to the same validator.
- A node may validate several hubs, often with the same validators: the testnet's XRP hub and its dollar hub run on the same four nodes, and mainnet's will too.
- Each message went on a stream of its own, and took one of the few places a node keeps for validators' requests until its hub's validator had written the step it took it in. A node that validates several hubs fills those places in bursts, and is answered busy.
- A node of an older release resets a stream of a kind it does not know, and its sender sees that as it sees any stream reset: nothing tells it that the node cannot read the kind.
- Validators of different owners update to new releases each on its own schedule, and a hub's version of the hub protocol is how its validators agree on what changes between releases ([hub-protocol-version](hub-protocol-version.md)).

## Decision

- **A validator sends another the messages it has waiting for it, of every hub the two validate, in one batch,** on a stream kind of its own, rather than one stream each. While fewer than two batches are on their way to a node, a message goes at once, alone; those that come meanwhile go together in the next. So a busy validator sends fewer, fuller streams, and an idle one waits for nothing.
- **Each message is answered on its own, as its answer comes,** as the hub's stream would answer it. One slow hub, catching up or writing a large state, holds up no other hub's answers.
- **A batch is read only from a validator of a hub the node validates, and each message only from a validator of its own hub,** so a batch never carries a member's request and never takes a member's place.
- **Validators batch only for hubs of the hub protocol's version 3 or later.** A release that reads batches takes them for any hub it validates, whatever its version, so a hub moves to version 3 once its validators run such a release, as any move goes. Version 3 changes nothing a ledger executes.
- **A node waits once per validator node before it sends again to one it could not reach,** for every hub it validates: a node that is down is down for all of them, and one that answers in any hub is up.

## Accepted

- A hub's version now also names how its validators exchange its messages, not only what they agree on. Moving a hub to version 3 cuts off its members on releases that do not run it, though nothing changes for members, so the release reaches members first, as any move asks.
- Batching for one hub waits until that hub moved, though its validators may run a release that reads batches long before.
- A batch holds a place among validators' requests for as long as its slowest message's step takes, as one message did.

## Rejected

- *A new stream kind, falling back to one stream per message when a node resets it:* no version and no vote, but a reset says nothing about why, so a node that is merely busy or restarting looks like one of an older release, and the first batch to an old node is lost each time the memory of it runs out.
- *A new stream kind every node uses at once, from a release on:* the least code, but validators on mixed releases lose every message to each other until all have updated.
- *A new version of the peer protocol (its ALPN):* the batch stream is a node's business with other validators, not with its peers, and a new ALPN changes every connection for the sake of a few.
- *Answering a batch once every message is answered:* the slowest hub's step would hold up every other hub's answers, past the sender's wait.
- *One consensus and ledger for all the assets a validator set holds:* it saves the same costs and more, but an issuer's freeze or clawback of a token would reach the other assets' accounts, every asset's escrow would still need its own account and sequence, and one hub's fault would halt all of them.
