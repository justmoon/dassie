# The sponsor founds a hub over a stream of its own, and a quorum's signatures of the genesis found it

**Areas:** [Settlement hubs](../areas/hubs.md#founding-a-hub), [Peer protocol](../areas/peer-protocol.md#the-hubs-stream), [design](../designs/settlement-hubs.md#2-founding-a-hub).

## Facts

- A hub's ID is the digest of its rules, which hold each validator's acceptance, so nothing can be addressed to the hub before every validator accepted: the hub's stream names a hub first.
- A validator accepts only from sponsors its owner lists ([settlement-hubs](settlement-hubs.md)). The QUIC connection proves the asker's node key, which is the sponsor's key in the terms.
- A validator's keys for a hub derive from the hub's nonce, so a validator that signed two genesis blocks under one nonce would run two hubs, with different co-validators, on the same keys.
- Nothing is at stake until the escrow exists: the sponsor sets it up only after the hub runs, and no member deposits before a quorum checked it.

## Decision

- **A stream kind of its own.** A sponsor's requests (propose the terms, sign the genesis, run the certified hub, say whether the escrow is checked) travel on a QUIC stream kind of their own, one request and its answer per stream, pinned to the validator's node key. The validator takes the connection's proved key as the asker, and answers only the sponsor the terms name, and only if its owner lists it.
- **The sponsor drives.** It asks each validator in turn, persists where its founding stands after every answer that changed it, and asks again after a wait that doubles. Validators never talk to each other while a hub is founded.
- **The genesis certificate** is a quorum's consensus-key signatures of the genesis block, under a domain tag of its own. A validator signs one genesis per nonce, the same one again as often as asked, and runs the hub a certificate founds, its own signature in it or not, as long as it signed no other genesis under the nonce and the rules carry its own acceptance as it makes it.
- **Places.** Each hub a validator runs, each genesis it signed and each acceptance younger than a day holds one of its places, its bound on hubs: a sponsor that stops halfway frees an acceptance's place by itself, and a signed genesis's once its hub runs.
- **The escrow last.** Once a quorum runs the hub, the sponsor funds the escrow with an owner's payment from its XRP Ledger testnet account, sets up what is missing of it, the master key disabled last, and asks the validators until f + 1 say a quorum checked it.

## Accepted

- A sponsor that vanishes after a quorum signed, and before every validator holds the certificate, leaves those without it holding a place for the signed genesis: the founding is persisted, so a sponsor that comes back hands the certificate on.
- The validators that were down while a quorum founded the hub start it whenever the sponsor reaches them; until then the hub runs on the others.
- A validator whose owner drops a sponsor from its list keeps running the hubs it founded with it.

## Rejected

- *Founding messages on the hub's stream under a placeholder ID:* the stream's bounds and queues are per hub, read before anything else; a hub that does not exist yet would need a route of its own anyway.
- *Validators passing the certificate among themselves:* it would need a validator to talk to nodes it never agreed to found a hub with before the rules are fixed, and saves nothing while the escrow does not exist.
- *Signing the genesis with the node key:* the consensus key is the hub's own, and the acceptance already ties it to the node key.
- *Running the hub on the validator's own signature:* a validator would run a hub no quorum committed to, which never starts.
