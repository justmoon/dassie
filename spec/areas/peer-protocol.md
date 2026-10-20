# Peer protocol

Dassie nodes talk to each other with the peer protocol v1: OER messages sent as HTTPS `POST /peer` requests, authenticated by a MAC from the two nodes' static keys. This document covers what the protocol guarantees, what a node trusts from whom, and what must stay compatible. What the messages are used for (the node table, membership, peering, routing) is in [Network](network.md).

**Paths:** `crates/dassie-peer/src/wire/` (layouts), `crates/dassie-peer/src/auth/` (MACs, replay), `crates/dassie-peer/src/node/inbound.rs` (handlers and statuses), `crates/dassie-node/src/http/peer.rs`, `crates/dassie-node/src/peer_client.rs`, `crates/dassie-node-core/src/address.rs`
**Read with:** [Network](network.md), [Identity](identity.md), [Encoding](encoding.md), [Settlement](settlement.md), [Connector](connector.md)

## Versions

- v1 keeps v0's message set, OER layouts, OIDs, media type and `POST /peer`, and adds sender authentication, replay protection, authenticated responses and new messages. The envelope's version byte is `1`; anything else is refused, so v1 and v0 nodes do not interoperate ([D2](../decisions/D02-peer-protocol-v0-or-v1.md)).
- A link state is signed under a new OID (`…1399.1.3`), so a node info signed under v0 is never a v1 link state.
- Some fields and replies exist only for nodes of 0.1, and must not change while such nodes may run:
  - `credit` in `peeringRequest` and its acceptance is sent as 0 and ignored: a node knows only the credit it extends, never the credit extended to it ([D23](../decisions/D23-each-node-books-its-own-fulfills.md)). A later protocol version drops it.
  - The reply to a Fulfill is always `01` ("accepted"). Nodes of 0.1 read it as a verdict on whether the Fulfill counted; "accepted" tells them to keep their own books, as every node does.
  - A Fulfill or Reject is taken from any node we hold a settlement channel with, peered or not, since nodes of 0.1 ask for verdicts after a peering ended.
- The answer to `settlementRequest` was empty before it carried the amount on its way ([D28](../decisions/D28-a-node-that-ends-a-peering-collects-first-and-pays-nothing.md)). An empty answer is still read, as saying nothing of the amount, so older nodes need no new version.
- **Post-quantum.** Authentication tag `[2]` is parsed only to be refused: it is reserved for a scheme on the X-Wing hybrid KEM, whose key would travel as link-state extension type 1. Neither is built ([roadmap](../roadmap.md#smaller-open-points)).

The layouts are pinned by golden vectors ([Test vectors](#test-vectors)); a change to any of them is a new protocol version.

## Transport

Requests and responses use fixed media types, compared as exact strings, with no negotiation. The HTTP layer checks the headers, the client's rate and the body's size; the peer node decides everything else, and answers only once what the request changed is on disk.

- Unauthenticated requests, and requests that fail, are rate-limited per client address. Nodes authenticate every request, so this rations tools and failures, not peers.
- The number of requests waiting on the node is bounded; past it the node answers 503.

### Statuses

The status is part of the protocol, and senders depend on one property of it: **400, 401, 409 and 503 mean the message was not acted upon.** Only these, or a request that surely never left, let a sender treat a message as undelivered: the connector rejects a Prepare at once with T01 on them, and leaves every other failure to expiry ([Connector](connector.md#responses-and-expiry)). A peering refusal is therefore a 200 with `refused`, since the responder did act on the request. The statuses and their causes are in `crates/dassie-peer/src/node/inbound.rs`.

## Encoding

Messages are canonical OER as [Encoding](encoding.md) describes it, read strictly: an unknown CHOICE tag, a value that does not fill its container, or trailing bytes are malformed. The layouts are in `crates/dassie-peer/src/wire/`.

A 200 response body has no tag of its own: its type follows from the request's kind.

## Authentication

### Which messages need authentication

- Every message that changes state, or moves money, must be authenticated: peering, packets, settlements, scheme messages, registration, termination and settlement requests. Without it they get 401.
- Read-only gossip (link states, node lists, peering info) may be sent without it, so tools can query a node. A v1 node still authenticates every request it sends, since it only sends to nodes whose key it knows.
- A response is authenticated if and only if its request was. A sender refuses a response to an authenticated request that has no MAC or a wrong one, as if the request had failed. Authenticated responses are what make node lists, peering answers and registrations authentic end to end, not only by TLS.
- An authenticated message from a peer, or an authenticated response to one of our requests, marks the peer as heard from ([Network](network.md#advertised-peers)).

### Keys and MACs

Keys come from a static-static X25519 between the two nodes' Ed25519 keys, converted as [Identity](identity.md) describes, and HKDF-SHA256. The derivation is in `crates/dassie-peer/src/auth/keys.rs` and pinned by a MAC vector in `crates/dassie-peer/tests/auth.rs`. Its properties:

- **The sender's key travels in the envelope** and must hash to the claimed node ID, be a valid point and not of small order ([Identity](identity.md)). So a node can check a sender it does not know yet, such as a registrant. An all-zero shared secret is refused.
- **Direction binding.** Request and response keys are expanded under different labels, with the sender's and recipient's public keys in that order. A request MAC is valid from S to R only, so a message cannot be reflected back to its sender, and a response is keyed by the request's pair in the same order.
- **What a MAC covers.** A request MAC covers the version, sender, recipient, session, counter, timestamp and the content's exact bytes. A response MAC covers the request's MAC and the body, so a response is bound to its request.
- **No ephemeral key.** For a scheme that only MACs, an ephemeral adds neither forward secrecy nor protection against key-compromise impersonation, and it would let anyone force an X25519 operation per request ([D2](../decisions/D02-peer-protocol-v0-or-v1.md)).
- Derived keys are cached per node, bounded; a key of a node that sent us a request is cached only after its MAC verified, so forged traffic cannot evict the keys of real peers.

### Replay protection

A sender keeps one session per destination, with a random ID and a counter, and starts a new one periodically. A recipient accepts an authenticated message only if its timestamp is within a window around its clock, above the sender's floor, and its `(sessionId, counter)` pair is new (a sliding window per session). The guarantees:

- **Checking changes nothing.** Replay state is recorded only once the MAC has verified, so forged messages cannot advance or poison it.
- **Evicting a session opens nothing.** The recipient keeps a few sessions per sender; a new one displaces the session used longest ago, and the sender's floor rises to that session's newest timestamp.
- **Across restarts, a message is accepted at most once.** For every sender whose replay state is kept for good (below), the newest accepted timestamp is persisted in the same transaction as the message's effects. After a restart, such a sender's floor is the later of that timestamp and the start time: the start time alone would let a message from a sender whose clock runs ahead through again. Every other sender's floor is the start time. That keeps the promise, since a stranger's request with effects makes it a sender kept for good in the same transaction (a registration, a peering), and a link state from a stranger is signed data that anyone may relay. A request that only reads moves no persisted floor, since a replayed read is harmless; a peer's read does, since its replay would mark the peer as heard from. A floor is kept until it is stale, even when its sender stops mattering, and goes once nothing it protects can arrive: when the sender is forgotten, or, if it sent again in between and took a place in the pool, when that place goes. Raising every sender's floor after a restart by the timestamp window instead would refuse every registration, settlement and Fulfill from a node that is not a peer for that long. A message lost in a crash, whose effects were never committed, can be accepted once more, but never twice.
- **Floods of identities cannot crowd out the nodes that matter.** Peers, bootstrap nodes, nodes we hold a settlement channel with ([Network](network.md#settlement-channels)) and nodes registered with us ([Network](network.md#registration)) keep their replay state for good, so a registered node's renewal gets through however long it was quiet. A node that stops mattering, such as one whose registration lapsed, keeps its state outside the pool until every message accepted from it is stale, and is then forgotten. Every other sender, a stranger, shares a bounded pool; a newcomer may take only the place of a sender whose newest message is already outside the timestamp window, so forgetting it opens nothing. If there is none, the newcomer gets 503.
- **One network holds only its share of the pool.** Strangers from one /24 (IPv4) or /48 (IPv6) of source addresses, grouped as for the registration cap ([Network](network.md#registration)), hold a bounded number of places; once a network has its share, a newcomer from it may take only the place of an idle sender from the same network. An IPv4 client seen by a dual-stack listener counts in its IPv4 network. Loopback is no network where private peers are allowed, as for the limits per client ([HTTP](http.md)), since dev and the tests run many nodes there. The bounds are `AuthConfig`'s (`crates/dassie-peer/src/auth/mod.rs`).
- **A request that only reads leaves nothing in the pool.** Requests for link states, node lists and peering info (`Kind::is_read_only`) change nothing at the recipient, and their answers are public, signed data or the recipient's own, which anyone may ask for unauthenticated. From a stranger, such a request is checked against the timestamp window and the sender's state, if it has any, and is not recorded, nor is its key cached: a replay gets the replayer only what asking would. Otherwise new identities could fill the pool, or keep a place fresh, with requests that are answered 200, which the HTTP layer does not charge ([Transport](#transport)). From a node whose state is kept, it is recorded like any other, so a replay cannot mark a peer as heard from.

## Link states

A node's link state ("node info") is its signed description of itself: its URL, alias, key, the peers it advertises and the schemes it settles with. How it is issued, spread and refreshed is in [Network](network.md#link-states). The signature is pure Ed25519 over the signed bytes as received.

- **Extensions** of unknown type are kept, since the signature covers them, and otherwise ignored. That is how later versions add fields.
- The alias is an unverified display name.

### Verification

A link state verifies when its URL has the node-URL syntax, its key is a valid Ed25519 point not of small order, its node ID is that key's, and its signature verifies. All nodes must agree on which states verify, or the network splits on them:

- **Signatures follow ZIP-215** (`ed25519-zebra`), as the original's verifier did. Being stricter could split the network on edge-case signatures. Small-order keys are refused because under ZIP-215 one signature verifies every message under such a key.
- **URL syntax is part of verification** for the same reason: an HTTPS origin with a lowercase hostname or IP literal and an optional port, nothing else (`crates/dassie-peer/src/wire/url.rs`). A node whose own URL does not fit publishes no link state.

## Message set

What a node trusts, and the rules a sender and a recipient rely on, by message. The handlers are in `crates/dassie-peer/src/node/inbound.rs`; "peered" means a peering that is `peered` or `terminating`, "active" means `peered` ([Network](network.md#lifecycle)).

- **Link states** (`linkStateUpdate`, `linkStateRequest`) are trusted by their signature, from anyone. A requester takes only the states of nodes it asked about, so a responder cannot stuff its table. An answer carries stored states as received, without verifying them again; the requester verifies.
- **Embedded node info** in `peeringRequest` and `registration` must verify and name the sender, and may not be far ahead of the recipient's clock.
- **Node lists** are trusted only from a configured bootstrap node, by the response's MAC under its configured key ([Network](network.md#membership)). A list's hash is computed over the bytes received.
- **`peeringRequest`** carries the scheme versions the sender speaks; an acceptance names the highest common one, and a requester refuses one it does not speak. The responder probes the requester's URL and records the peering before it answers ([Network](network.md#peering)).
- **`interledgerPacket`.** A Prepare and its Fulfill or Reject are separate one-way messages, correlated by a request ID per peer. A Prepare needs an active peering, so no new packet enters a peering being ended. A Fulfill or Reject is sent again, for a bounded time, until a 200 arrives, or a 400, 401 or 409, which says it will never be taken; a copy that arrives twice is dropped by the peer's connector. A Reject that never arrives has the effect of expiry; a Fulfill stays booked whether it arrives or not ([D23](../decisions/D23-each-node-books-its-own-fulfills.md)). The packet itself is the connector's ([Connector](connector.md)).
- **`settlement`** is accepted from any node we hold a settlement channel with, peered or not, so a settlement that arrives after the peering ended is still credited. Its sequence belongs to the pair of nodes, not to a peering, and never resets. The sender sends one at a time, in order, again until a 200 arrives, across restarts, with a backoff. One whose last try got no answer also goes at once when the node next answers any request of ours, since it is reachable again, rather than waiting out the backoff. That early try is made once per silence, so a node that never answers it gets it at most twice as often as the backoff alone would send it; a 401 keeps it (a node that just started refuses messages dated before its start, and the early copy is dated anew), and any other answer gives it up, since a refusal or a busy node is not helped by going early. The recipient answers a sequence it already applied with 200 and does nothing; it applies only the next one, and only if its books can take it, checked before the scheme sees it (otherwise 400 and nothing changes). So each settlement is credited exactly once ([Settlement](settlement.md#settlement-channels)).
- **`settlementMessage`** is a scheme's own message to its counterpart, sent once; schemes do not depend on it arriving.
- **`settlementRequest`** asks an active peer to settle on a scheme, leaving the amount to it ([Settlement](settlement.md#asking-a-peer-to-settle)). Its answer is what the peer then has on its way to the asker, in the scheme ledger's units, as eight bytes ([Settlement](settlement.md#settling-on-request)). A node ending the peering, and not yet saying so, answers 0 ([Network](network.md#termination)).
- **`peeringTermination`** is sent until a 200 or a 409 arrives, or the termination times out. A 409 means the recipient holds no peering with us, so there is nothing left to end ([Network](network.md#termination)).

## Outbound requests

Peer requests, and the node's other outbound requests (Open Payments to other wallets, exchange-rate sources: [Payments](payments.md), [Connector](connector.md#sources)), go through one client in `crates/dassie-node/src/peer_client.rs`, with one set of rules. A peer or a registrant chooses the URL we connect to, so:

- Requests go only to `https` URLs, and redirects are never followed.
- Private, loopback, link-local and similar addresses are refused, both as IP literals and after DNS resolution (`crates/dassie-node-core/src/address.rs`). An IPv6 literal is read as the HTTP client reads it, and one the guard cannot read counts as private, so the guard and the client cannot disagree. `DASSIE_PEER_ALLOW_PRIVATE` lifts this for dev and tests.
- In-flight requests are bounded, in total and per host. ILP packets do not count, so their responses always get through.
- **Failures** are of two kinds: *not delivered* (refused before sending, or the connection failed) and *unknown* (a timeout after sending, or a broken response). Only the first, or a status that means not acted upon ([Statuses](#statuses)), says a request surely had no effect.

## Test vectors

- v0 layouts that v1 keeps: `crates/dassie-oer/tests/data/vectors.json` (key `peer`), generated from the original by `testdata/original/`. v1's `PeerNodeInfo` is the v0 vector with the OID's last byte `03`.
- Keys and node IDs: `crates/dassie-identity/tests/data/keys.json`.
- v1 encodings and MACs: `crates/dassie-peer/tests/wire.rs` and `crates/dassie-peer/tests/auth.rs`.

## Decisions

- [D2](../decisions/D02-peer-protocol-v0-or-v1.md): v1 instead of v0; static-static X25519 MACs without an ephemeral key, direction binding, the sender key in the envelope, replay protection across restarts, authenticated responses, the reserved post-quantum tag.
- [D4](../decisions/D04-network-membership-economics.md): authenticated node lists and the limits on free registration.
- [D12](../decisions/D12-peering-lifecycle-and-liveness.md): `peeringTermination`.
- [D23](../decisions/D23-each-node-books-its-own-fulfills.md): `settlementRequest`, `credit` sent as 0, the Fulfill reply always "accepted".
