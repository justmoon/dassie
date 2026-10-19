# Peer protocol

Dassie nodes talk to each other with the peer protocol v1: OER messages sent as HTTPS `POST /peer` requests, each authenticated with a MAC from the two nodes' static keys. This document is the protocol's wire format and the rules for sending and answering each message. What the messages are used for (the node table, membership, peering, routing) is in [Network](network.md).

**Paths:** `crates/dassie-peer/src/wire/`, `crates/dassie-peer/src/auth/`, `crates/dassie-peer/src/node/inbound.rs`, `crates/dassie-node/src/http/peer.rs`, `crates/dassie-node/src/peer_client.rs`
**Read with:** [Network](network.md), [Identity](identity.md), [Encoding](encoding.md), [Settlement](settlement.md), [Connector](connector.md)

## Versions

- This is version 1. The envelope's `version` byte is `1`. A request with any other version (such as `0` from an original node) is refused with 400 `unsupported version`.
- v1 keeps v0's message set, OER layouts, OIDs, media type and `POST /peer`, and adds sender authentication, replay protection, authenticated responses, new messages and stricter limits. v1 nodes do not interoperate with v0 nodes ([D2](../decisions/D02-peer-protocol-v0-or-v1.md)).
- A message whose sender equals the recipient is refused with 400.

## Transport

A request is `POST {url}/peer`, where `{url}` is the recipient's URL from its link state or the bootstrap list ([Network](network.md)).

| Header | Value |
|---|---|
| `Accept` (request) | exactly `application/dassie-message` |
| `Content-Type` (request) | exactly `application/dassie-message` |
| `Content-Type` (200 response) | `application/dassie-peer-response` |
| `Content-Type` (other responses) | `text/plain; charset=utf-8` |

Header values are compared as exact strings; there is no negotiation.

### Statuses

The HTTP status means something (NET-11):

| Status | Meaning |
|---|---|
| 200 | Handled. The body is a `PeerResponse`. |
| 400 | Malformed, a version other than 1, a sender equal to the recipient, a link state that fails verification, or a settlement out of order or that the books cannot take. |
| 401 | Authentication is missing where it is required, uses an unsupported scheme, fails, or is a replay. |
| 406 | `Accept` is not `application/dassie-message`. |
| 408 | The body did not arrive within 10 s. |
| 409 | The message needs a state the recipient does not hold: a peering (`interledgerPacket`, `settlementMessage`, `settlementRequest`, `peeringTermination`), a settlement channel (`settlement`) or an active scheme (`peeringInfoRequest`); or a `registration` the recipient will not take (it is no bootstrap node, the host or network is taken, the URL did not answer). |
| 413 | The body is over 64 KiB. |
| 415 | `Content-Type` is not `application/dassie-message`. |
| 429 | Rate limited. |
| 503 | The node is busy (too many requests waiting, or no place for a new sender's replay state), its table or a bounded set is full, or its peer protocol is not running yet. |

- Every non-200 response has a short `text/plain` body for people.
- Senders treat any status other than 200 as a failure.
- 400, 401, 409 and 503 mean the message was not acted upon.

### Inbound limits

The HTTP layer checks, in order: `Accept` (406), `Content-Type` (415), the client's rate (429), whether the peer protocol runs and a place is free (503), then reads the body (413, 408). The peer node decides everything else, including the status, and answers only once what the request changed is on disk.

- At most 256 inbound peer requests wait on the node at once; more get 503.
- Requests without authentication, and requests answered 400, 401, 409 or 429, are charged to their client address (an IPv4 address or an IPv6 /56): 20 at once, refilled at one per second. A client with none left gets 429. At most 1024 clients are tracked; when the table is full, the client seen least recently loses its place. Nodes authenticate every request, so this rations tools and failures, not peers.

## Encoding

Messages are OER (X.696) as [Encoding](encoding.md) describes it. In this protocol:

- No SEQUENCE has OPTIONAL fields or extensions, so none has a preamble: a SEQUENCE is its fields in order.
- A CHOICE is its tag byte, `0x80 | n` (context class), then the alternative. An unknown tag is malformed.
- `UINT8`, `UINT16`, `UINT32` and `UINT64` are 1, 2, 4 and 8 bytes, big-endian.
- `OCTET STRING` without a fixed size is `LEN ‖ bytes`. With a fixed size (`SIZE(n)` and no range) it is `n` raw bytes, unless noted as length-prefixed.
- `SEQUENCE OF` starts with its count as an OER quantity (`01 00` for zero, `01 02` for two).
- `OCTET STRING CONTAINING T` is `LEN ‖ T`, and `T` must fill it exactly.
- Every message, response body and contained value must be read to its last byte: trailing bytes are malformed.
- `‖` is concatenation; `LEN` is an OER length determinant.

### Building blocks

```
NodeId             ::= IA5String (SIZE(2..45)) FROM ("a".."z" | "A".."Z" | "0".."9" | "-" | "_")
SettlementSchemeId ::= IA5String (SIZE(2..64)) FROM ("a".."z" | "0".."9" | "-")
```

- Node IDs are derived from Ed25519 keys as [Identity](identity.md) describes; computed ones are at most 33 characters. Node IDs compare by their bytes.
- Known scheme IDs are `stub` and `xrpl-testnet`. v1 allows 64 characters, down from v0's 128.

## Envelope

A request body is:

```
PeerMessage ::= SEQUENCE {
  version        UINT8,                     -- 1
  sender         NodeId,
  authentication Authentication,
  content        PeerMessageContent }       -- raw CHOICE, runs to the end of the body

Authentication ::= CHOICE {
  none         [0] NULL,                    -- 0x80
  hmacSha256V1 [1] SEQUENCE {               -- 0x81, then 96 fixed bytes
    senderKey   OCTET STRING (SIZE(32)),    -- Ed25519 public key; NodeId(senderKey) MUST equal sender
    sessionId   OCTET STRING (SIZE(16)),
    counter     UINT64,                     -- 0, 1, 2, ... within the session
    timestampMs UINT64,
    mac         OCTET STRING (SIZE(32)) },
  reserved     [2] OCTET STRING }           -- 0x82 ‖ LEN ‖ bytes; for a future X-Wing scheme; answered 401
```

Byte layout: `01 ‖ LEN ‖ sender ‖ (80 | 81 ‖ senderKey ‖ sessionId ‖ counter ‖ timestampMs ‖ mac) ‖ content`. An empty `content` is malformed.

- `senderKey` must be a valid point, not of small order, and must hash to `sender`; otherwise 401.
- The `reserved` tag is parsed so that it can be refused with 401 `unsupported authentication`. It is kept for a scheme built on the X-Wing hybrid KEM; signatures and node IDs stay Ed25519 ([D2](../decisions/D02-peer-protocol-v0-or-v1.md)). Neither that scheme nor hybrid post-quantum TLS is built ([roadmap](../roadmap.md)).

## Responses

A 200 response body is:

```
PeerResponse ::= SEQUENCE {
  authentication CHOICE {
    none         [0] NULL,                       -- 0x80
    hmacSha256V1 [1] OCTET STRING (SIZE(32)) },  -- 0x81 ‖ respMac
  body }                                         -- the body for the request's kind, runs to the end
```

- A response is authenticated if and only if its request was. A sender refuses a response to an authenticated request that has no MAC or a wrong one, as if the request had failed.
- The body has no tag of its own: its type follows from the request's kind. The table is in [the message set](#message-set).
- A 200 response whose `Content-Type` is not `application/dassie-peer-response` is not a peer's answer: the sender treats it as a failure.

## Keys and MACs

The sender is S and the recipient is R. `pub X` is X's Ed25519 public key, and `x25519` is the conversion of an Ed25519 key to X25519 ([Identity](identity.md)): for a secret key, `clamp(SHA-512(seed)[0..32])`; for a public key, the birational map `u = (1 + y) / (1 − y)`.

```
dh      = X25519(x25519(S), x25519(pub R))   -- = X25519(x25519(R), x25519(pub S)); all-zero is refused
K_pair  = HKDF-SHA256-Extract(salt = "dassie peer v1", ikm = dh)
K_req   = HKDF-SHA256-Expand(K_pair, "request"  ‖ pub S ‖ pub R, 32)
K_resp  = HKDF-SHA256-Expand(K_pair, "response" ‖ pub S ‖ pub R, 32)
mac     = HMAC-SHA256(K_req, 0x01 ‖ LEN ‖ sender ‖ LEN ‖ recipient ‖ sessionId ‖ counter ‖ timestampMs ‖ content)
respMac = HMAC-SHA256(K_resp, mac ‖ body)
```

- `content` is the exact bytes of `PeerMessageContent`, tag included. `LEN ‖ id` is the OER encoding of a node ID (one length byte). `0x01` is the version. `counter` and `timestampMs` are 8 bytes big-endian.
- `body` is the response body after the authentication field.
- S and R always name the request's sender and recipient, so a response is keyed by the same pair in the same order. The labels and the ordered keys make each MAC valid in one direction only: a message cannot be reflected back to its sender.
- There is no ephemeral key: a scheme that only MACs gains nothing from one, and it would let anyone force an X25519 operation per request ([D2](../decisions/D02-peer-protocol-v0-or-v1.md)).
- MACs are compared in constant time (NET-10).
- Vector: S with seed `01`×32, R with seed `03`×32, `sessionId` `05`×16, `counter` 7, `timestampMs` 1700000000123, content `nodeListRequest` (`87`) give `mac = 7253c8ea5323692f8b6b7a15cefe82e79a7b6cb77cc49ef480bef76b44920e55` (`crates/dassie-peer/tests/auth.rs`).

### Sessions

- A sender uses one session per destination, with a random 16-byte `sessionId`, and starts a new one every hour. Its `counter` starts at 0 and goes up by one per message.
- A sender keeps sessions for at most 1024 destinations; the oldest goes first.
- Derived keys are cached per node, at most 1024, least recently used first. A key of a node that sent us a request is added only after its MAC verified (NET-9).

## Which messages need authentication

| Message | Authentication |
|---|---|
| `peeringRequest`, `interledgerPacket`, `settlement`, `settlementMessage`, `registration`, `peeringTermination`, `settlementRequest` | required: 401 without it |
| `linkStateUpdate`, `linkStateRequest`, `nodeListHashRequest`, `nodeListRequest`, `peeringInfoRequest` | optional |

- A v1 node authenticates every request it sends, since it only sends to nodes whose key it knows (from a link state or the bootstrap list).
- An unauthenticated request is answered without a MAC; tools may use it for the optional kinds.
- An authenticated `linkStateUpdate` whose sender is the subject of the state counts as a heartbeat ([Network](network.md)).
- For `peeringRequest` and `registration`, the embedded signed node info must verify and name the sender (NET-3, NET-4).

## Replay protection

For each sender, the recipient accepts an authenticated message only if:

- `|timestampMs − now| ≤ 60 s` (otherwise 401 `the timestamp is too far from now`);
- `timestampMs` is above the sender's floor;
- its `(sessionId, counter)` pair is new.

Otherwise it is a replay: 401 `replayed`.

- The recipient keeps at most two sessions per sender. Each holds the highest counter seen and a 1024-counter sliding window; counters below the window are refused.
- A message of a session the recipient does not hold takes a free place, or displaces the session used longest ago. The sender's floor then rises to the newest timestamp of the displaced session.
- Checking changes nothing; the state is recorded only once the MAC has verified.
- **Restarts.** For peers, the newest accepted timestamp is persisted in the same transaction as the message's effects. After a restart, a peer's floor is the later of that timestamp and the start time; every other sender's floor is the start time.
- A message's effects only become visible after the transaction that records it, so a message lost in a crash can be accepted once more, but never twice.
- **Bounds.** Peers, bootstrap nodes and nodes we hold a settlement channel with ([Network](network.md#settlement-channels)) always keep their replay state. Every other sender shares 1024 places, so a flood of new identities cannot crowd out the nodes that matter. Once the places are full, a new sender takes the place of one whose newest message is older than the 60 s window (forgetting it opens nothing); if there is none, it gets 503.

## Link states

A node's link state ("node info") is its signed description of itself: its URL, alias, key, the peers it advertises and the schemes it settles with. How it is issued, flooded and refreshed is in [Network](network.md).

```
PeerNodeInfo ::= SEQUENCE {                   -- the signed bytes
  type       OBJECT IDENTIFIER (1.3.6.1.4.1.54392.5.1399.1.3),
                                              -- constant 0d2b0601040183a878058a770103
  nodeId     NodeId,
  sequence   UINT64,                          -- ms timestamp of issue, strictly increasing
  url        VisibleString (SIZE(0..256)),    -- LEN ‖ ASCII 0x20..0x7E
  alias      UTF8String (SIZE(0..64)),        -- LEN ‖ UTF-8, at most 64 bytes, unverified display name
  publicKey  OCTET STRING,                    -- length-prefixed, exactly 32 bytes (Ed25519)
  entries    SEQUENCE (SIZE(0..64)) OF NodeInfoEntry }

NodeInfoEntry ::= CHOICE {
  neighbor         [0] SEQUENCE { nodeId NodeId },                        -- 0x80 ‖ LEN ‖ id
  settlementScheme [1] SEQUENCE { settlementSchemeId SettlementSchemeId }, -- 0x81 ‖ LEN ‖ id
  extension        [2] SEQUENCE { type UINT16,                            -- 0x82 ‖ type ‖ LEN ‖ value
                                  value OCTET STRING (SIZE(0..2048)) } }

SignaturePackage ::= CHOICE { ed25519 [0] OCTET STRING (SIZE(64)) }       -- 0x80 ‖ 64 bytes

SignedPeerNodeInfo ::= SEQUENCE {
  signed    OCTET STRING CONTAINING PeerNodeInfo,   -- LEN ‖ PeerNodeInfo, at most 4 KiB inside
  signature SignaturePackage }
```

- The signature is pure Ed25519 (RFC 8032, no prehash, no context) over the `PeerNodeInfo` bytes, without their length prefix.
- **New OID.** v0 used `1.3.6.1.4.1.54392.5.1399.1.2`; a node info signed under v0 is not a v1 link state.
- **Extensions.** Unknown extension types are kept, since the signature covers them, and otherwise ignored. A future X-Wing public key is type 1.
- **Limits:** `url` at most 256 bytes, `alias` at most 64 bytes, at most 64 entries, the signed `PeerNodeInfo` at most 4 KiB, an extension's value at most 2048 bytes.
- A node's own entries are its advertised neighbors first, in node-ID order, then its schemes in its order of preference. Receivers do not depend on the order.

### Verification

A link state verifies when, in this order:

1. its `url` has the URL syntax below;
2. its `publicKey` is a valid Ed25519 point and not of small order (under ZIP-215 one signature verifies every message under such a key);
3. its `nodeId` is the node ID of `publicKey` ([Identity](identity.md));
4. its signature verifies under ZIP-215 rules (`ed25519-zebra`), as the original's verifier did. ZIP-215 is kept on purpose: being stricter could split the network on edge-case signatures.

**URL syntax** is part of verification, so all nodes agree on it: `https://`, then a lowercase hostname or an IP literal, then an optional port, and nothing else (NET-30).

- No userinfo, path, query, fragment or trailing slash.
- A hostname is at most 253 bytes of dot-separated labels, each 1 to 63 bytes of `a-z`, `0-9` and `-`, not starting or ending with `-`. A name of only digits and dots is read as an IPv4 literal.
- An IPv6 literal is in brackets and lowercase.
- A port is 1 to 65535, without leading zeros.
- The OER alphabet of `url` accepts `~` (0x7E), which v0 refused; the URL syntax then decides.

A node whose own URL or alias does not fit (too long, or not a valid node URL) publishes no URL, and so no link state, or an empty alias.

## Message set

```
PeerMessageContent ::= CHOICE {
  peeringRequest      [0]  PeeringRequest,
  linkStateUpdate     [1]  SignedPeerNodeInfo,          -- inline, no wrapper
  interledgerPacket   [2]  SEQUENCE { signed OCTET STRING CONTAINING PeerInterledgerPacket },
  linkStateRequest    [3]  SEQUENCE { nodeIds SEQUENCE (SIZE(0..16)) OF NodeId },
  settlement          [4]  Settlement,
  settlementMessage   [5]  SEQUENCE { settlementSchemeId SettlementSchemeId, message OCTET STRING },
  nodeListHashRequest [6]  SEQUENCE {},                 -- just 0x86
  nodeListRequest     [7]  SEQUENCE {},                 -- just 0x87
  registration        [8]  SEQUENCE { nodeInfo SignedPeerNodeInfo },   -- inline, not octet-wrapped
  peeringInfoRequest  [9]  SEQUENCE { settlementSchemeId SettlementSchemeId },
  peeringTermination  [10] SEQUENCE { reason UINT8 },
  settlementRequest   [11] SEQUENCE { settlementSchemeId SettlementSchemeId } }
```

| Message | Tag | Auth | Response body |
|---|---|---|---|
| `peeringRequest` | `0x80` | required | `PeeringResponse` |
| `linkStateUpdate` | `0x81` | optional | empty |
| `interledgerPacket` | `0x82` | required | one byte `01` for a Fulfill, else empty |
| `linkStateRequest` | `0x83` | optional | `LinkStateResponse` |
| `settlement` | `0x84` | required | empty |
| `settlementMessage` | `0x85` | required | empty |
| `nodeListHashRequest` | `0x86` | optional | 32 bytes: SHA-256 of the served list |
| `nodeListRequest` | `0x87` | optional | `NodeList` |
| `registration` | `0x88` | required | `RegistrationResponse` |
| `peeringInfoRequest` | `0x89` | optional | `SEQUENCE { settlementSchemeData OCTET STRING }` |
| `peeringTermination` | `0x8A` | required | empty |
| `settlementRequest` | `0x8B` | required | empty |

Note the asymmetry kept from v0: in `peeringRequest` the signed node info is wrapped in an extra OCTET STRING; in `registration` and `linkStateUpdate` it is inline.

Below, "peered" means the recipient holds a peering with the sender that is `peered` or `terminating`; "active" means `peered` only ([Network](network.md)).

### peeringRequest [0]

```
PeeringRequest ::= SEQUENCE {
  settlementSchemeId   SettlementSchemeId,
  nodeInfo             OCTET STRING CONTAINING SignedPeerNodeInfo,
  settlementSchemeData OCTET STRING,
  versions             SEQUENCE (SIZE(1..16)) OF UINT16,   -- the scheme versions the sender supports (NET-26)
  credit               UINT64 }                            -- sent as 0 and ignored

PeeringResponse ::= CHOICE {
  accepted [0] SEQUENCE { version UINT16, credit UINT64, data OCTET STRING },  -- credit sent as 0, ignored
  refused  [1] SEQUENCE { reason UINT8 } }
```

| Reason | Meaning |
|---|---|
| 0 | unspecified (also: already peered on another scheme); unknown values read as 0 |
| 1 | unknown node: the requester is not in the responder's table |
| 2 | unsupported scheme |
| 3 | no common version |
| 4 | capacity: no inbound slot, no peer slot, or no room for a settlement channel |
| 5 | concurrent request: both asked at once, and the responder's own request wins |
| 6 | refused by the scheme |
| 7 | unreachable: the probe of the requester's URL failed |
| 8 | banned |
| 9 | terminating: the responder is ending, or an owner ended in the last 24 h, its peering with the requester |

- `credit` in both directions is sent as 0 and ignored: a node knows only the credit it extends, never the credit extended to it ([D23](../decisions/D23-each-node-books-its-own-fulfills.md)). A later protocol version drops the field.
- The embedded node info must name the sender (400 `the link state is not the sender's`), verify (400 `invalid link state`), and have a sequence at most 10 min ahead of the responder's clock (400).
- `version` in `accepted` is the highest version both sides list. The requester refuses an answer naming a version it does not speak.
- A refusal is a 200 with `refused`; 400, 401 and 503 are for messages not acted upon.
- The responder probes the requester's URL before accepting, and records the peering before it answers. The order of the checks, the probe, repeats and crossed requests are in [Network](network.md).
- Sender: auto-peering, after a `peeringInfoRequest` for the same scheme.

### linkStateUpdate [1]

- The content is a `SignedPeerNodeInfo`, about any node. The receiver takes it in as [Network](network.md) describes: an older or equal sequence is dropped before its signature is checked; a state of a node not in its table, from the future, or over its subject's rate is dropped.
- A state that is new and fails verification gets 400 `invalid link state`. Anything else gets 200 with an empty body, whether the state was taken or dropped.
- Authenticated, from the subject of the state, it is a heartbeat: the sender's peering counts as heard from. If the receiver holds no peering with the sender, is not asking for one, and the state lists it as a neighbor, it answers that one-sided peering with a `peeringTermination` of reason 3, at most once per 2 min ([Network](network.md#one-sided-peerings)).
- Senders: heartbeats, flooding, and the push of a node's new state to its bootstrap nodes.

### interledgerPacket [2]

```
PeerInterledgerPacket ::= SEQUENCE {
  requestId UINT32,          -- correlates a Prepare with its Fulfill or Reject, per peer
  packet    OCTET STRING }   -- an ILP packet: Prepare (type 12), Fulfill (13) or Reject (14)
```

`0x82 ‖ LEN ‖ requestId ‖ LEN ‖ packet`. The field name `signed` is v0's; nothing is signed.

- A Prepare and its Fulfill or Reject are separate one-way messages. The Fulfill or Reject goes back with the Prepare's `requestId`. Request IDs are per peer.
- **A Prepare** needs a peering: 409 `not peered` without one, and 409 `the peering is ending` while it is terminating, so no new packets enter a peering being ended.
- **A Fulfill or Reject** is taken from any node we hold a settlement channel with, peered or not, since nodes of 0.1 ask for verdicts after the peering ended. Otherwise 409.
- **The reply to a Fulfill** has a one-byte body, always `01` (`accepted`). Nodes of 0.1 read it as a verdict on whether the Fulfill counted; `accepted` tells them to keep their own books, as every node does ([D23](../decisions/D23-each-node-books-its-own-fulfills.md)). The reply to a Prepare or Reject is empty.
- The packet is handed to the connector, which decides what to do with it ([Connector](connector.md)).

**Sending packets.**

- A Prepare is sent with the timeout of its own expiry, at most 30 s.
- A Prepare that surely did not reach its peer is rejected at once with T01 `the peer could not be reached`. "Surely" means: refused before sending, a connection that failed, or 400, 401, 409 or 503 in reply. One that may have reached it (a timeout after sending, a broken response, another status) is left to expire.
- A Fulfill or Reject for a peer is sent again until a 200 arrives or 30 s have passed: each try may take the time that is left, and a failed one is tried again 1 s later. A 400, 401 or 409 in reply ends the retries. A copy that arrives twice is dropped by the peer's connector.
- A Reject that never arrives has the effect of expiry. A Fulfill stays booked whether it arrives or not ([D23](../decisions/D23-each-node-books-its-own-fulfills.md)).
- Responses are sent only while the peering exists (`peered` or `terminating`); a peering being ended waits for them ([Network](network.md#termination)).

### linkStateRequest [3]

```
LinkStateResponse ::= SEQUENCE (SIZE(0..16)) OF CHOICE {
  notFound [0] NULL,                  -- 0x80
  found    [1] SignedPeerNodeInfo }   -- 0x81 ‖ the stored bytes
```

- A request carries at most 16 node IDs; more is malformed.
- The answer is in the order asked, each ID once (a repeated ID is answered at its first place only).
- `found` carries the stored `SignedPeerNodeInfo` bytes as received; they are not verified again.
- The requester takes only states of nodes it asked about (NET-36), and verifies them.
- Senders: refresh, anti-entropy sweep, and probes of a requester's URL ([Network](network.md)).

### settlement [4]

```
Settlement ::= SEQUENCE {
  settlementSchemeId   SettlementSchemeId,
  amount               UINT64,          -- in the scheme ledger's units
  settlementSchemeData OCTET STRING,
  sequence             UINT64 }         -- per sender and recipient: increasing, never reset
```

It is accepted from any authenticated node we hold a settlement channel with, peered or not, so a final settlement that arrives after the peering ended is still credited. `sequence` belongs to the pair of nodes, not to a peering; the sender sends one at a time, in order, again until a 200 arrives, across restarts. The answers:

| Case | Answer |
|---|---|
| No settlement channel with the sender | 409 `no accounts with the sender` |
| `sequence` at or below the highest applied from the sender | 200, not applied again |
| `sequence` is the next one, and the books can take it | applied, then 200 |
| `sequence` is the next one, and the books cannot take it | 400 `the settlement cannot be booked`, nothing changes |
| any other `sequence` | 400 `settlement out of order` |

The books are checked before the scheme sees the settlement. Delivery, booking, and when and how much a node settles are in [Settlement](settlement.md#settlement-channels).

### settlementMessage [5]

- A scheme's own message to its counterpart on a peer (for example an address exchange). It needs a peering: 409 otherwise.
- It is passed to the scheme and answered with an empty 200. It is sent once; schemes do not depend on it arriving.

### nodeListHashRequest [6] and nodeListRequest [7]

```
NodeList ::= SEQUENCE (SIZE(0..4096)) OF NodeId   -- sorted by bytes, no duplicates
```

- `nodeListRequest` is answered with the node's served list; `nodeListHashRequest` with the SHA-256 of that list's encoded bytes, 32 raw bytes.
- The served list is the node's unexpired registrations and itself, sorted by bytes (NET-32). What it is for is in [Network](network.md).
- A downloaded list is authentic because its response is MACed under the bootstrap node's configured key. Its hash is computed from the bytes received (NET-15).

### registration [8]

```
RegistrationResponse ::= SEQUENCE { expiresAtMs UINT64 }   -- 8 bytes; v0 answers with an empty body
```

- Only a node that is one of its own configured bootstrap nodes takes registrations; others answer 409 `not a bootstrap node`.
- The node info must name the sender (400), verify (400), and be at most 10 min ahead (400).
- Refusals: 503 when 1024 registrations are held, or the table has no room for the node; 409 when another node is registered at the same URL host and port, or when 8 registrations already came from the same /24 or /48; 429 when the global bucket for new registrations is empty, or a probe of that node or host is in flight, or probes are rate-limited; 503 when 32 requests already wait on probes.
- A new registration, or one at a new URL, waits on a probe of the URL: 409 `not reachable at its URL` if it fails. A renewal from the probed URL is answered at once.
- The rules and numbers are in [Network](network.md).

### peeringInfoRequest [9]

- Answered with `SEQUENCE { settlementSchemeData OCTET STRING }`: the scheme's peering info, which the requester's scheme uses to build its `peeringRequest`.
- 409 `the scheme is not active` if the scheme is not active here (NET-27).

### peeringTermination [10]

```
reason: 0 unspecified, 1 by the owner (or a ban), 2 unresponsive,
        3 not peered (the sender holds no peering with the recipient),
        4 short of room (the recipient's room in the sender's books stayed short of a
          packet, without a settlement); unknown values read as 0
```

- From a peer whose peering is `peered`: the recipient begins terminating too, and answers 200.
- From a peer whose peering is already `terminating`: both ended it at once; each message tells the other. 200.
- Otherwise (no peering, or only our own request pending, which is dropped): 409 `not peered`.
- The sender sends it again until a 200 or a 409 arrives, or 2 min have passed ([Network](network.md)).

### settlementRequest [11]

- The sender asks the recipient to settle on the named scheme, leaving the amount to it ([D23](../decisions/D23-each-node-books-its-own-fulfills.md)).
- 409 `not peered on that scheme` unless the recipient holds a `peered` peering with the sender on that scheme; a peering being ended settles in full at its end instead. The response is empty.
- When a node asks, and what the asked node pays, are in [Settlement](settlement.md#asking-a-peer-to-settle).

## Outbound requests

- Requests go only to `https` URLs. The node never follows redirects: a 3xx is a failure.
- Requests to private, loopback, link-local or similar addresses are refused, both for IP literals and after DNS resolution, unless the operator allows them with `DASSIE_PEER_ALLOW_PRIVATE` (for dev and tests).
  - An IPv6 literal is read as the HTTP client reads it, ending at its bracket; one the guard cannot read counts as private.
  - With private addresses allowed, `localhost` and names under it resolve to loopback (RFC 6761), so dev nodes reach each other by name on any system.
- Certificates are checked against the platform's roots, plus those in `DASSIE_PEER_CA_FILE` (dev and tests).
- Connecting may take 10 s. A response body may be at most 256 KiB.
- At most 128 requests are in flight, and at most 16 to one host. ILP packets do not count, so their responses always get through. A request over the limit fails as not delivered.
- **Timeouts:** 30 s; 5 s for node-list hash polls and probes; a Prepare's own expiry if sooner.
- **Failures** are of two kinds: *not delivered* (refused before sending, or the connection failed), and *unknown* (a timeout after sending, or a broken response). Only the first, or a 400, 401, 409 or 503 status, says the request surely had no effect.
- The node keeps at most 8192 of its own requests in flight; more are dropped.
- The same client makes the node's other outbound requests, to other wallets for Open Payments and to exchange-rate sources, under the same address rules and in-flight limits, each with its own timeout and response size ([Payments](payments.md), [Connector](connector.md#sources)).

## Test vectors

- v0 layouts that v1 keeps: `crates/dassie-oer/tests/data/vectors.json` (key `peer`), generated from the original by `testdata/original/` ([3 §5.5](../original/03-peer-network.md)). v1's `PeerNodeInfo` is the v0 vector with the OID's last byte `03`.
- Keys and node IDs: `crates/dassie-identity/tests/data/keys.json`.
- v1 encodings and MACs: `crates/dassie-peer/tests/wire.rs` and `crates/dassie-peer/tests/auth.rs` (the pinned MAC above).

## Decisions

- [D2](../decisions/D02-peer-protocol-v0-or-v1.md): v1 instead of v0; static-static X25519 MACs without an ephemeral key, direction labels, the sender key in the envelope, the replay window, authenticated responses, the reserved post-quantum tag.
- [D4](../decisions/D04-network-membership-economics.md): authenticated node lists (NET-15) and the limits on free registration.
- [D12](../decisions/D12-peering-lifecycle-and-liveness.md): `peeringTermination` and its reasons.
- [D23](../decisions/D23-each-node-books-its-own-fulfills.md): `settlementRequest`, `credit` sent as 0, the Fulfill reply always `accepted`.

## Original

- [3 §4–§5](../original/03-peer-network.md): v0's OER subset, building blocks, message contents and response bodies.
- [3 §6–§7](../original/03-peer-network.md): v0's transport and its ephemeral-static authentication (NET-8).
- [3 §8](../original/03-peer-network.md): v0's handlers.
- [3 §19–§20](../original/03-peer-network.md): the gaps (NET-n, OER-q) and the v0 compatibility surface.
