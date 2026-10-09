# 3. Peer network: identity, peer protocol, membership, peering, routing, BTP

Source citations refer to the original implementation at commit `e962ab78` on `main` (see [the index](README.md#sources)).

Scope: node identity and crypto, the peer message protocol (wire format, transport, authentication), the node table, link-state propagation, network membership (bootstrap node lists, majority lists, registration), the peering lifecycle, heartbeats, uplink addresses, routing, and the BTP server (ILP clients). Every section cites the source files it comes from. Paths are relative to `packages/app-dassie/src/` unless they start with `packages/` or `docs/`.

Notation:
- `||` means byte concatenation.
- `LEN(x)` is an OER length determinant (see §4.2).
- Hex bytes are written as `0x..`.
- "Open question / known gap" (abbreviated **NET-n**) marks behavior that looks incomplete, TODO, buggy, or inconsistent with the docs. All of them are collected in §19.

---

## 0. Source map

| Area | Files |
|---|---|
| Wire schemas | `peer-protocol/peer-schema.ts`, `constants/object-identifiers.ts` |
| Transport (client) | `peer-protocol/functions/send-peer-message.ts`, `peer-protocol/utils/is-connection-refused-error.ts` |
| Transport (server) | `peer-protocol/register-peer-http-handler.ts`, `packages/lib-http-server/src/{asserts.ts,middlewares/parse-body.ts,constants.ts,failures/*}` |
| Authentication | `peer-protocol/functions/{generate-message-authentication,authenticate-peer-message}.ts`, `peer-protocol/utils/calculate-message-hmac.ts`, `peer-protocol/stores/{incoming,outgoing}-session-keys.ts`, `peer-protocol/computed/x25519-private-key.ts`, `peer-protocol/constants/anonymous-messages.ts` |
| Dispatch | `peer-protocol/functions/handle-peer-message.ts`, `peer-protocol/handlers/*.ts` |
| Node table | `peer-protocol/stores/node-table.ts`, `peer-protocol/functions/modify-node-table.ts`, `peer-protocol/computed/{peers,registered-node-list}.ts`, `peer-protocol/utils/parse-link-state-entries.ts` |
| Link state | `peer-protocol/{maintain-own-node-table-entry,broadcast-state-updates,forward-link-state-update,send-heartbeats,refresh-node-state}.ts`, `peer-protocol/functions/verify-link-state.ts`, `peer-protocol/constants/timings.ts` |
| Membership | `peer-protocol/{poll-node-list-hashes,download-node-lists,add-majority-nodes}.ts`, `peer-protocol/computed/{majority-node-list,serialized-node-list,node-list-hash}.ts`, `peer-protocol/signals/*`, `constants/bootstrap-nodes.ts`, `config/environment-config.ts` |
| Registration | `peer-protocol/handlers/registration.ts`, `registration-client/**` |
| Peering | `peer-protocol/maintain-peering-relationships.ts`, `peer-protocol/handlers/{peering-request,peering-info-request,settlement,settlement-message}.ts`, `peer-protocol/create-peer-ledger-entries.ts`, `ledgers/types/settlement-scheme-module.ts` |
| Uplink | `peer-protocol/receive-address-from-uplink.ts`, `peer-protocol/functions/query-uplink-address.ts`, `peer-protocol/stores/uplink-addresses.ts`, `ildcp-server/handle-ildcp-requests.ts`, `ilp-connector/functions/{send-link-local-packet,get-endpoint-ilp-address}.ts` |
| Routing | `routing/**`, `local-ilp/route-local-endpoints.ts`, `ilp-connector/functions/calculate-prepare-packet-outcome.ts` |
| Identity / crypto | `crypto/**`, `ilp-connector/utils/calculate-node-id.ts`, `ilp-connector/computed/{node-id,node-ilp-address}.ts`, `config/database-config.ts`, `config/computed/ilp-allocation-scheme.ts`, `packages/lib-x509/src/ed25519.ts`, `constants/seed-paths.ts`, `packages/gui-dassie/src/utils/{signer,authentication}.ts` |
| Persistence | `peer-protocol/tables/*.ts`, `peer-protocol/persist-node-table.ts`, `database/migrations/{0005,0006,0010,0013}-*.ts`, `database/schema.ts`, `packages/app-dev/src/utils/prefill-database.ts` |
| BTP | `btp-server/**`, `ilp-connector/senders/send-btp-packets.ts`, `packages/lib-protocol-utils/src/btp.ts` |
| Startup | `daemon.ts`, `peer-protocol/index.ts`, `routing/index.ts` |
| Docs | `docs/implementation-notes/{network-membership,node-discovery,peer-state,peer-to-peer-communication}.md` |

---

## 1. Architecture and startup order

The peer layer is a set of reactive "actors" (lib-reactive). An actor **re-runs from scratch** when any signal it read with `readAndTrack` changes. Before re-running, its cleanups run and its scheduled tasks are cancelled. Several behaviors below (for example "heartbeat sent immediately") follow from this re-run semantic. A Rust reimplementation can use explicit event handlers, but it must reproduce the trigger conditions listed in each section.

Scheduled tasks (`sig.task({handler, interval, jitter?})`, `packages/lib-reactive/src/internal/scheduled-task.ts`):
- **Fixed-rate scheduling.** On each tick the next timer is scheduled first, with `delay = interval + Math.random() * (jitter ?? 0)`, and then the handler runs.
- **Default overlap policy `skip`.** If the previous async execution is still running, the tick is skipped.
- `task.execute()` runs the handler immediately. `task.schedule()` arms the timer. Handler errors are caught and logged.

Startup (`daemon.ts`):
1. `DaemonActor` runs LocalRpcServer, HttpServer, then awaits AcmeCertificateManager, then TrpcServer, AuthenticationFeature, then `StartNodeIdentityDependentServicesActor`.
2. `StartNodeIdentityDependentServicesActor` does nothing until a node identity (`config.dassieKey`) exists. Once it does, it runs: Accounting, BtpServer, IldcpServer, IlpHttp, LocalIlp, await ExchangeRates, await Ledgers, await OpenPaymentsServer, PublicApiServer, **await PeerProtocolActor**, then **RoutingActor**.

`PeerProtocolActor` (`peer-protocol/index.ts`) has a strict order (`await` marks a step that waits for the actor's first execution):
1. `PersistNodeTableActor` loads the node table from the DB. It must run before any derived signal (Peers, RegisteredNodeList) is first instantiated, because those signals snapshot the table on creation and then follow only *actions*, not raw writes.
2. `RegisterPeerHttpHandlerActor` mounts `POST /peer`.
3. await `MaintainOwnNodeTableEntryActor` creates and signs our own link state.
4. `MaintainPeeringRelationshipsActor` (auto-peering, every 1 s).
5. await `SendHeartbeatsActor`.
6. `ForwardLinkStateUpdateActor` (every 200 ms).
7. await `PollNodeListHashesActor`. The first poll completes before continuing.
8. await `DownloadNodeListsActor`. The first download round completes before continuing.
9. `AddMajorityNodesActor`.
10. `BroadcastStateUpdatesActor`.
11. await `RefreshNodeStateActor`. The first refresh completes.
12. `runMap(ReceiveAddressFromUplinkActor)`, one instance per peer.
13. `runMap(CreatePeerLedgerEntriesActor)`, one instance per peer.

`RoutingActor` then runs `CalculateRoutesActor`.

---

## 2. Node identity and cryptography

### 2.1 Node private key

Sources: `config/database-config.ts`, `crypto/computed/node-private-key.ts`, `packages/lib-x509/src/ed25519.ts`, `authentication/http-routes/setup.ts`, `packages/gui-dassie/src/utils/{signer,authentication}.ts`.

- **Algorithm.** The node key is an Ed25519 key. Its 32-byte seed is the "raw private key".
- **Storage.** It is stored in the DB scalar `config.dassie_key` (TEXT) as **PEM PKCS#8**: the tag `-----BEGIN PRIVATE KEY-----`, and DER = `302E020100300506032B657004220420 || seed32` (48 bytes).
- **Parsing.** `parseEd25519Key` rejects:
  - encrypted keys;
  - the wrong PEM tag;
  - a DER length other than 48 (private) or 44 (public);
  - a preamble mismatch.

  The public-key form is `PUBLIC KEY` with DER `302A300506032B65700321 || 00 || pub32` (44 bytes). lib-x509's constant omits the `00` byte, so the 33-byte "key" it returns starts with `00`. This only matters if public PEM keys are ever parsed; the peer layer never does.
- **Creation via `/api/setup`.** The browser (GUI) derives the key and sends it to `/api/setup`:
  - `masterSeed = bip39.mnemonicToSeed(mnemonic)` (64 bytes, empty passphrase).
  - `nodeKey = HMAC-SHA256(key = masterSeed, msg = "node")`.

  The server stores the key only if both of these hold:
  - `setupAuthorizationToken` matches;
  - `loginAuthorizationSignature == hex(HMAC-SHA256(nodeKey, "login"))`.
- **Seed paths** (`crypto/utils/seed-paths.ts`, `constants/seed-paths.ts`). `getPrivateSeedAtPath(nodeKey, path)` works as follows:
  - `"node"` returns `nodeKey` itself.
  - `"node/a/b/..."` returns the iterated HMAC: `k0 = nodeKey`, then `k_{i+1} = HMAC-SHA256(key = k_i, msg = utf8(segment_i))` over the segments after `node/`.
  - Any path not starting with `node/` throws on the server. The client computes the same iteration starting from `masterSeed` with the full path, which is equivalent.
  - Named paths: `node`, `node/login`, `dev/session` (client/dev only), `node/settlement-module`. Settlement modules get entropy at `node/settlement-module/<settlementSchemeId>/<path>` (`ledgers/manage-settlement-scheme-instances.ts`).
- **Public key.** `NodePublicKeySignal = ed25519.getPublicKey(seed)` (`crypto/computed/node-public-key.ts`). It uses `@noble/ed25519` v3 with SHA-512 from node:crypto (`crypto/ed25519.ts`).
- **Signing.** `signWithDassieKey(data) = Ed25519.sign(data, seed)`. This is pure Ed25519 (RFC 8032, no prehash, no context) (`crypto/functions/sign-with-dassie-key.ts`).
- **Verification.** `@noble/ed25519` `verifyAsync` uses the default **ZIP-215** rules (`{zip215: true}`): non-canonical encodings of points with y in [0, 2^256) are accepted and cofactored verification is used. A strict RFC 8032 verifier can reject some signatures that Dassie accepts. Only adversarially crafted signatures are affected; for honest signatures the two are interoperable.

### 2.2 Node ID

Source: `ilp-connector/utils/calculate-node-id.ts`.

```
input  = OER(SEQUENCE { versionPrefix OBJECT IDENTIFIER (constant 1.3.6.1.4.1.54392.5.1399.2.1),
                        nodePublicKey OCTET STRING (variable) })
       = 0x0d 2b 06 01 04 01 83 a8 78 05 8a 77 02 01 || 0x20 || publicKey32       (47 bytes for a 32-byte key)
digest = SHA-256(input)
b64    = base64url(digest) without padding        (43 chars)
nodeId = "d" || strip_trailing_'A'(b64[0..32])
```

Properties:
- The ID is `"d"` followed by up to 32 base64url characters (`[A-Za-z0-9_-]`). Trailing `A` characters of the 32-character prefix are removed with the regex `/A+$/`, so the length is 1 to 33.
- The wire schema allows a length of 2 to 45.
- Verified test vectors (default bootstrap nodes):
  - pubkey `QBe5HLbdYFtIpjFTIu_Lz8lkoUJ3fmAvFHfCb9CVwIM` (base64url) gives `dNKbwOMCw0DTToPM4tyyVASWxM4NP3MKW`.
  - pubkey `tmS1PB42DEjYi-Ua4RNguhweId9YjT9Kml3uq3iZSuQ` gives `dyrjUANXiJ5S6JXxwo7jdKKJjdLtK129U`.
- Extra vector (seed `01`×32): pub `8a88e3dd7409f195fd52db2d3cba5d72ca6709bf1d94121bf3748801b40f6f5c`, ID `dagvMuaJff2H5DrnJw8llmK-WZ83WrSzY`.

**Identity binding.** A node ID is valid for a link state iff `calculateNodeId(linkState.publicKey) == linkState.nodeId` (`verify-link-state.ts`).

### 2.3 Object identifiers

Source: `constants/object-identifiers.ts`.

- Prefix: `1.3.6.1.4.1.54392.5.1399`.
- `dassiePeerProtocolId` = prefix.1.
- `dassiePeerSignedHelloId` = prefix.1.1 (unused).
- `dassiePeerNodeInfoId` = prefix.1.2. OER: `0d 2b 06 01 04 01 83 a8 78 05 8a 77 01 02`.
- `dassieNodeFingerprintId` = prefix.2.
- `dassieNodeFingerprintV1Id` = prefix.2.1. OER: `0d 2b 06 01 04 01 83 a8 78 05 8a 77 02 01`.

### 2.4 X25519 keys used for peer message authentication

Sources: `peer-protocol/computed/x25519-private-key.ts`, `peer-protocol/functions/generate-message-authentication.ts`.

- **Static X25519 private key of a node.** `ed25519.utils.toMontgomerySecret(seed)` = `clamp(SHA-512(seed)[0..32])`. This is the same as libsodium `crypto_sign_ed25519_sk_to_curve25519`.
- **X25519 public key of a peer.** `ed25519.utils.toMontgomery(edPublicKey)` = birational map `u = (1+y)/(1-y) mod p`. This is the same as libsodium `crypto_sign_ed25519_pk_to_curve25519`.
- Vector: seed `03`×32 gives Ed pub `ed4928c6...37d1` and X25519 pub `75e270df2952c57ba8367ba8618c178f9fe50db2799d304e74e918d985686146`.

---

## 3. ILP addressing of nodes

Sources: `config/computed/ilp-allocation-scheme.ts`, `ilp-connector/computed/node-ilp-address.ts`, `ilp-connector/types/ilp-address.ts`, `ilp-connector/functions/get-endpoint-ilp-address.ts`.

- **Allocation scheme.** `"test"` if `config.realm === "test"` (the default realm is `"test"`), otherwise `"g"`.
- **Node address.** `<scheme>.das.<nodeId>`, for example `test.das.dNKbwOMCw0DTToPM4tyyVASWxM4NP3MKW`. This is the address regardless of registration status; the node always assumes it.
- **Address assigned to a peer endpoint**, i.e. what ILDCP hands to a peer:
  - if the peer is in our *majority node list* (§11.4): `<scheme>.das.<peerId>`;
  - otherwise: `<ourNodeAddress>.<peerId>`, a child or "uplink" address.
- **Other endpoint types:**
  - ILDCP: `peer.config`.
  - BTP client: `<ourNodeAddress>.<connectionId>` is used as the *source* address. The *route* registered is `<ourNodeAddress>.<nanoid(6)>` (see **NET-25**).
  - Local endpoint: `<ourNodeAddress>.<localIlpAddressPart>`.
  - ILP-over-HTTP: the literal `test.not-implemented` (TODO).
- **Link-local destination.** `peer.config` is the ILDCP destination (`packages/lib-protocol-ildcp/src/address.ts`).

---

## 4. OER encoding rules used by the peer protocol (lib-oer subset)

Sources: `packages/lib-oer/src/*`. Only the features used here are listed. None of the peer sequences use OPTIONAL fields or extensions, so **no sequence has a preamble byte**: a SEQUENCE is the concatenation of its fields in declaration order.

### 4.1 Choice tags

- `x.tag(n)` defaults to the **context** class.
- A tag byte is `0x80 | n` for n < 63. For n ≥ 63 it is `0xBF` followed by base-128 n.
- So the tags used here encode as `0x80`, `0x81`, ... `0x89`.
- A choice encodes as tag byte(s) followed by the encoding of the alternative.
- An unknown tag is a parse failure.

### 4.2 Length determinant

- If length ≤ 127: one byte, the length itself.
- Otherwise: `0x80 | n` followed by n big-endian bytes, minimal (n ≤ 4 on serialize).
- The parser rejects non-minimal long forms unless "allowNoncanonical" is set. It is never set in the peer layer.
- The parser rejects lengths with more than 6 length octets.

### 4.3 Types

| Type | Encoding |
|---|---|
| `uint8Number()` | 1 byte |
| `uint32Number()` | 4 bytes big-endian |
| `uint64Bigint()` | 8 bytes big-endian (unsigned) |
| `boolean()` | 1 byte. Serialize: `0xFF` = true, `0x00` = false. Parse rejects other values (canonical mode). |
| `empty()` | 0 bytes |
| `octetString()` (unbounded) | `LEN(n) \|\| bytes` |
| `octetString(N)` (fixed) | N raw bytes, no length. **Parse does not bounds-check** (slice may be short); see **OER-q1**. |
| `ia5String([a,b]).from(alphabet)` | `LEN(n) \|\| ASCII bytes`. Parse: every byte must be in the alphabet; char count must be in [a,b]. **Serialize checks only the length, not the alphabet.** |
| `visibleString()` | `LEN \|\| bytes`. Allowed bytes at parse are 0x20..**0x7D** (the code uses `fill(true,0x20,0x7e)`, end-exclusive, so `~` 0x7E is rejected). See **NET-30**. |
| `utf8String()` | `LEN \|\| UTF-8 bytes`, no character filter. Length bounds are counted in UTF-16 code units of the decoded string. |
| `objectIdentifier()` | `LEN \|\| base-128 subidentifiers` (first = 40·a+b). Here it is only used as `.constant(...)`: the exact pre-serialized bytes are emitted and matched byte-for-byte on parse. |
| `sequenceOf(T)` | quantity as OER *variable-length unsigned integer*: `LEN(k) \|\| k-byte big-endian minimal` (zero = `01 00`, two = `01 02`), followed by the elements |
| `captured(T)` | transparent: encodes exactly like T. On parse it also yields the raw `bytes` of T. On serialize, raw `{bytes}` may be given and are copied verbatim (not validated). |
| `octetString().containing(T)` | `LEN(n) \|\| encoding(T)`. On serialize a raw `Uint8Array` may be given instead of a T value. On parse, T is parsed starting at the content start **using the whole outer buffer as the bound**; there is no check that T consumed exactly n bytes (**OER-q2**). |
| `.refine(pred)` | extra predicate. All refinements in the peer schema are `() => true`. |

### 4.4 Top-level parse

- `schema.parse(bytes)` fails if any bytes remain after the top-level value ("additional bytes present"). This makes `empty()` responses require an **empty body**.
- Nested `octetString().containing` regions are not checked this way (**OER-q2**).

### 4.5 String alphabets used

- `nodeIdSchema` = `ia5String([2, 45]).from("a-z" + "A-Z" + "0-9" + "-_")`.
- `settlementSchemeIdSchema` = `ia5String([2, 128]).from("a-z" + "0-9" + "-")`. Known values: `"stub"`, `"xrpl"`, `"xrpl-testnet"` (`peer-protocol/types/settlement-scheme-id.ts`).
  - Note: the settlement-module docs mention `+` separators, which this alphabet would reject (**NET-31**).

---

## 5. Wire schemas

Source: `peer-protocol/peer-schema.ts`.

### 5.1 Building blocks

```
NodeInfoEntry ::= CHOICE {
  neighbor          [0] SEQUENCE { nodeId NodeId },                    -- 0x80 || LEN || ascii
  settlementScheme  [1] SEQUENCE { settlementSchemeId SettlementSchemeId } -- 0x81 || LEN || ascii
}

PeerNodeInfo ::= SEQUENCE {                 -- "link state" / "node info"; the SIGNED bytes
  type       OBJECT IDENTIFIER (constant 1.3.6.1.4.1.54392.5.1399.1.2),  -- 14 fixed bytes 0d2b0601040183a878058a770102
  nodeId     NodeId,                        -- ia5String [2..45]
  sequence   UINT64,                        -- 8 bytes BE; = ms timestamp of creation (see §10.1)
  url        VisibleString,                 -- node's public HTTPS base URL, e.g. "https://host[:port]"
  alias      UTF8String,                    -- unverified display name
  publicKey  OCTET STRING,                  -- variable length (32 for Ed25519); length-prefixed
  entries    SEQUENCE OF NodeInfoEntry
}

SignaturePackage ::= CHOICE { ed25519 [0] OCTET STRING (SIZE(64)) }   -- 0x80 || 64 bytes

SignedPeerNodeInfo ::= SEQUENCE {
  signed     OCTET STRING CONTAINING PeerNodeInfo,   -- LEN || PeerNodeInfo bytes
  signature  SignaturePackage
}
-- Signature = Ed25519.sign(nodeKey, PeerNodeInfo bytes)  (the CONTENTS of `signed`, without its length prefix)

PeerInterledgerPacket ::= SEQUENCE {
  requestId  UINT32,                        -- 4 bytes BE
  packet     OCTET STRING                   -- serialized ILP packet (Prepare/Fulfill/Reject)
}
```

Entry ordering when producing our own link state: all `neighbor` entries first, in PeersSignal set iteration order (filtered, §10.1), then all `settlementScheme` entries in ActiveSettlementSchemes set order. Receivers do not depend on the order.

### 5.2 Message content (request bodies)

```
PeerMessageContent ::= CHOICE {
  peeringRequest      [0] SEQUENCE { settlementSchemeId SettlementSchemeId,
                                     nodeInfo OCTET STRING CONTAINING SignedPeerNodeInfo,
                                     settlementSchemeData OCTET STRING },
  linkStateUpdate     [1] SignedPeerNodeInfo,                          -- no extra wrapper
  interledgerPacket   [2] SEQUENCE { signed OCTET STRING CONTAINING PeerInterledgerPacket },  -- "signed" is a misnomer: nothing is signed
  linkStateRequest    [3] SEQUENCE { nodeIds SEQUENCE OF NodeId },
  settlement          [4] SEQUENCE { settlementSchemeId SettlementSchemeId, amount UINT64,
                                     settlementSchemeData OCTET STRING },
  settlementMessage   [5] SEQUENCE { settlementSchemeId SettlementSchemeId, message OCTET STRING },
  nodeListHashRequest [6] SEQUENCE {},                                  -- encodes as just 0x86
  nodeListRequest     [7] SEQUENCE {},                                  -- encodes as just 0x87
  registration        [8] SEQUENCE { nodeInfo SignedPeerNodeInfo },     -- NOT octet-wrapped (contrast peeringRequest)
  peeringInfoRequest  [9] SEQUENCE { settlementSchemeId SettlementSchemeId }
}
```

Tag byte table:

| Message | Tag byte |
|---|---|
| peeringRequest | 0x80 |
| linkStateUpdate | 0x81 |
| interledgerPacket | 0x82 |
| linkStateRequest | 0x83 |
| settlement | 0x84 |
| settlementMessage | 0x85 |
| nodeListHashRequest | 0x86 |
| nodeListRequest | 0x87 |
| registration | 0x88 |
| peeringInfoRequest | 0x89 |

Note the asymmetry:
- In `peeringRequest`, the node info is wrapped in an extra OCTET STRING (length-prefixed).
- In `registration` and `linkStateUpdate`, the SignedPeerNodeInfo appears inline.

### 5.3 Envelope (HTTP request body)

```
PeerMessage ::= SEQUENCE {
  version         UINT8,                    -- must be 0
  sender          NodeId,                   -- claimed sender node ID (NOT cryptographically bound, see §7)
  authentication  CHOICE {
     NONE                         [0] NULL/empty,                      -- 0x80
     ED25519_X25519_HMAC-SHA256   [1] SEQUENCE { sessionPublicKey OCTET STRING (SIZE(32)),
                                                 messageAuthenticationCode OCTET STRING (SIZE(32)) }  -- 0x81 || 32 || 32
  },
  content         PeerMessageContent        -- raw choice encoding, runs to end of body
}
```

Byte layout: `00 || LEN(sender) || sender || (80 | 81 || spk32 || mac32) || contentTag || contentBody`.

### 5.4 Responses (HTTP response bodies)

The response type is selected by the *request* content type. There is no envelope and no tag (`peerMessageResponse` in `peer-schema.ts`).

| Request | Response schema | Encoding |
|---|---|---|
| peeringRequest | `SEQUENCE { accepted BOOLEAN, data OCTET STRING }` | `00/FF \|\| LEN \|\| data` |
| linkStateUpdate | empty | 0 bytes |
| interledgerPacket | empty | 0 bytes. The ILP response travels as a *separate* interledgerPacket message in the reverse direction. |
| linkStateRequest | `SEQUENCE OF CHOICE { notFound [0] empty, found [1] SignedPeerNodeInfo }` | quantity, then per element `80` or `81 \|\| SignedPeerNodeInfo` |
| settlement | empty | 0 bytes |
| settlementMessage | empty | 0 bytes |
| nodeListHashRequest | `SEQUENCE { hash OCTET STRING (SIZE(32)) }` | 32 raw bytes |
| nodeListRequest | `SEQUENCE OF NodeId` | quantity, then `LEN \|\| id` repeated |
| registration | empty | 0 bytes |
| peeringInfoRequest | `SEQUENCE { settlementSchemeData OCTET STRING }` | `LEN \|\| data` |

### 5.5 Test vectors

These were generated with an independent encoder written from this spec. They match lib-oer's rules. The node-ID derivation was cross-checked against the real bootstrap config (§2.2). The vectors are not produced by Dassie itself; regenerate them from the real code if byte-exact confirmation is needed.

Node A (seed `01`×32, pub `8a88e3dd...6f5c`, ID `dagvMuaJff2H5DrnJw8llmK-WZ83WrSzY`). Node B (seed `03`×32, pub `ed4928c6...37d1`, ID `dRLajGQUsQnlsol-B7F0Uf_og05hZo4C3`).

PeerNodeInfo of A, with fields:
- sequence = 1700000000000 (`0000018bcfe56800`);
- url `https://a.example`;
- alias `Node A`;
- entries `[neighbor B, settlementScheme "stub"]`.

```
0d2b0601040183a878058a770102                      OID (constant)
21 646167764d75614a666632483544726e4a77386c6c6d4b2d575a38335772537a59   nodeId (33 chars)
0000018bcfe56800                                  sequence
11 68747470733a2f2f612e6578616d706c65             url
06 4e6f64652041                                   alias
20 8a88e3dd7409f195fd52db2d3cba5d72ca6709bf1d94121bf3748801b40f6f5c   publicKey
0102                                              entries quantity = 2
80 21 64524c616a47515573516e6c736f6c2d4237463055665f6f673035685a6f344333   neighbor B
81 04 73747562                                    settlementScheme "stub"
```

- Signature (Ed25519, deterministic): `49e26e533d7bb1dd47d7b7cb88c91e6ca7d82af0c3bae27938d5d2d4cca8f93d5af8664241e2377bb3324a08c62310875259ac1ae96a729c01e00b2fe11e8d05`.
- SignedPeerNodeInfo: `819d` (LEN=157, long form) `|| PeerNodeInfo || 80 || signature`.
- Envelope `nodeListHashRequest` from A, no auth: `00 21 <A id ascii> 80 86`, i.e. `0021646167764d75614a666632483544726e4a77386c6c6d4b2d575a38335772537a598086`.
- Envelope `linkStateUpdate` from A, no auth: `00 21 <A id> 80 81 819d<...signedPeerNodeInfo...>`.

Authenticated `interledgerPacket` A to B (session X25519 private `02`×32 → session pub `ce8d3ad1ccb633ec7b70c17814a5c76ecd029685050d344745ba05870e587d59`):
- **content.** requestId=42, packet=`0c` (a dummy byte, not a valid ILP packet) gives `82 06 0000002a 01 0c`.
- **Shared secret.** `X25519(sessionPriv, toMontgomery(B.edPub)) = X25519(clampSHA512(B.seed), sessionPub) = 6b43e8f2fc80e7891620dba266ebfd61a3cd1a37515f29c039c5883952d6d32d`.
- **MAC.** `HMAC-SHA256(sharedSecret, content)` = `079027d1da0e0e782c531156d8fc6ca5cf61ad89a58ab61ee7ab85046d81403f`.
- **Envelope.** `00 21<A id> 81 ce8d...7d59 0790...403f 82060000002a010c`.

Node list examples:
- [A, B] sorted with `localeCompare` → `0102 21<A> 21<B>`, SHA-256 `8a42b105c9fb1d5c4f15296f245b23f8674b8aae2b438c77c3fa026ac0ee4b50`.
- Empty list `0100` → SHA-256 `47dc540c94ceb704a23875c11273e16bb0b8a87aed84de911f2133568115f254`.

---

## 6. Transport

Sources: `peer-protocol/functions/send-peer-message.ts`, `peer-protocol/register-peer-http-handler.ts`, `peer-protocol/constants/content-type.ts`, lib-http-server.

### 6.1 Server side: `POST /peer` on the node's HTTPS router

The router is `HttpsRouter`, the same HTTPS listener as the web UI. Its base URL is `config.url = "https://" + hostname + (httpsPort==443 ? "" : ":" + httpsPort)`. The default httpsPort is 443.

Processing pipeline, in order:
1. **Accept header.** It must equal exactly `application/dassie-message` (string equality, no negotiation). Otherwise **406** with text body `Not Acceptable, expected application/dassie-message`.
2. **Content-Type header.** It must equal exactly `application/dassie-message`. Otherwise **415**.
3. **Body size.** The body is read fully; above **2 MiB** (`DEFAULT_MAX_BODY_SIZE = 2*1024*1024`) the response is **413**.
4. **Parse.** The body is parsed as `PeerMessage` (canonical, no trailing bytes). On failure the response is **400** `Invalid HTTP request body`.
5. **Version.** If `version !== 0`, the response is **400** `Bad Request, unsupported version`.
6. **Authentication.** `senderNode = nodeTable.get(body.sender)` and `isAuthenticated = authenticatePeerMessage(body, senderNode)` (§7.3).
7. **Anonymous check.** If `!isAuthenticated` and the content type is not in `ALLOW_ANONYMOUS_USAGE`, the response is **401** `Incoming Dassie message is not authenticated`.
8. **Dispatch.** Emit `IncomingPeerMessageTopic` with `{message, authenticated, peerState: senderNode?.peerState}`, then dispatch to the handler (§8).
9. **Response serialization.** The handler result is serialized with `peerMessageResponse[type]`. Response: **200**, `Content-Type: application/dassie-peer-response` (note: different from the request type), and the binary body.
10. **Exceptions.** If the handler or serializer throws, lib-http-server returns **500** with an "Internal Server Error" body (JSON or HTML depending on Accept; here plain text).

`ALLOW_ANONYMOUS_USAGE` = `peeringRequest`, `linkStateRequest`, `linkStateUpdate`, `nodeListHashRequest`, `nodeListRequest`, `registration`, `peeringInfoRequest`. Messages that therefore **require authentication**: `interledgerPacket`, `settlement`, `settlementMessage`.

### 6.2 Client side: `sendPeerMessage({destination, message, asUint8Array?, timeout?})`

1. **Serialize content.** `serializedMessage = asUint8Array ?? peerMessageContent.serialize(message)`.
2. **Emit topic.** Emit `OutgoingPeerMessageTopic` (used for debugging and observability).
3. **Resolve contact info**, first match wins:
   - **Self.** `destination == ownNodeId` resolves to `internal` (loopback, §6.3).
   - **Node table.** If the node table has `linkState` for the destination: `url = linkState.url`, `publicKey = linkState.publicKey`.
   - **Bootstrap config.** If the destination is a configured bootstrap node: `url = config.url`, `publicKey = base64url-decode(config.publicKey)`.
   - **Otherwise.** Log `no url known for destination` and return `undefined`; nothing is sent.
4. **Authentication.** `authentication = generateMessageAuthentication(serializedMessage, type, destination, publicKey)` (§7.2).
5. **Envelope.** `envelope = PeerMessage{version: 0, sender: ownNodeId, authentication, content: {bytes: serializedMessage}}`.
6. **HTTP request.** `fetch(url + "/peer", {method: "POST", body, headers: {accept: "application/dassie-message", "content-type": "application/dassie-message"}})`. Notes:
   - There is no normalization of a trailing slash in `url`.
   - The timeout uses an AbortController: `timeout ?? DEFAULT_NODE_COMMUNICATION_TIMEOUT` (**30 000 ms**). No caller passes a custom timeout.
   - `NODE_LIST_HASH_TIMEOUT` (5 s) is defined but unused (**NET-14**).
7. **Response handling.** The client **does not check the HTTP status code** (**NET-11**). It reads the body and parses it with `peerMessageResponse[type]`:
   - On parse failure it logs and returns `undefined`.
   - Error responses (text bodies) therefore surface as parse failures. An empty-bodied error would parse successfully as `empty()`.
8. **Exceptions** (network errors, aborts) are caught; the function returns `undefined`. "Connection refused" (TypeError whose cause is an AggregateError of ECONNREFUSED errors) gets a dedicated log message.
9. **Retries.** There are none. Every caller treats `undefined` as failure and relies on periodic tasks to try again later.
10. **TLS.** Node's default `fetch` verifies certificates, so peers need publicly valid HTTPS certs. Dev uses a local CA.

### 6.3 Internal loopback

When the destination is self, the client:
- parses the envelope;
- builds the event `{message, authenticated: true, peerState: {id: "none"}}`;
- emits `IncomingPeerMessageTopic`;
- calls the handler directly and parses the result like an HTTP response.

This happens in practice because `BroadcastStateUpdatesActor` sends our own link state to every node-table entry, including ourselves. It also happens if we are one of our own bootstrap nodes (polls, refresh oracles).

---

## 7. Message authentication

Sources: `peer-protocol/functions/generate-message-authentication.ts`, `authenticate-peer-message.ts`, `utils/calculate-message-hmac.ts`, `stores/*-session-keys.ts`.

### 7.1 Scheme `ED25519_X25519_HMAC-SHA256`

This is an ephemeral-static Diffie-Hellman, followed by an HMAC over the content bytes:

```
sender:   (sk_e, pk_e) = X25519 keypair, random (x25519.utils.randomSecretKey())
          shared = X25519(sk_e, toMontgomery(recipientEd25519PublicKey))
          mac    = HMAC-SHA256(key = shared (32 bytes), msg = contentBytes)
          send authentication = [1]{ sessionPublicKey = pk_e, messageAuthenticationCode = mac }
recipient: shared = X25519(toMontgomerySecret(ownEd25519Seed), pk_e)
           valid iff mac == HMAC-SHA256(shared, contentBytes)
```

`contentBytes` is the exact OER encoding of `PeerMessageContent`, including its choice tag byte. It is captured from the received bytes on the server. The MAC does **not** cover `version`, `sender`, or the authentication field itself.

### 7.2 Sender rules (`generateMessageAuthentication`)

- For types in `ALLOW_ANONYMOUS_USAGE`, it returns `NONE` (`0x80`). Anonymous-capable messages are *never* authenticated, even to peers.
- Otherwise it uses the per-destination cache `OutgoingSessionKeysStore: Map<NodeId, {sessionPublicKey, sharedSecret, createdAt}>`.
  - A new ephemeral key is generated if there is no entry, or if `createdAt + SESSION_KEY_EXPIRY < now`.
  - `SESSION_KEY_EXPIRY = 7 days` (604 800 000 ms).
  - The same session key is therefore reused for all messages to one destination for up to 7 days.
- The cache is memory-only and is lost on restart.

### 7.3 Recipient rules (`authenticatePeerMessage(message, senderNode)`)

The message counts as authenticated (`true`) only if all of these hold:
1. `authentication.type != NONE`;
2. `senderNode` (looked up by the **claimed `sender` field**) exists **and** has a `linkState`. The linkState contents are not used, only its presence.
3. `senderNode.peerState.id == "peered"`;
4. the MAC verifies with `shared` taken from `IncomingSessionKeysStore[base64(sessionPublicKey)]` (standard base64 with padding) or computed fresh.

On successful verification with a freshly computed secret, the entry `{sharedSecret, createdAt}` is cached under that key. The cache is never evicted or expired (**NET-9**).

The MAC comparison is a plain early-exit loop, not constant-time (**NET-10**).

### 7.4 Security properties (important for reimplementers)

**Known gap (NET-8).** The scheme only proves that the sender knows the recipient's *public* key, and the HMAC binds only the content.
- The claimed `sender` is not bound to any key of the sender. Any party that can reach `/peer` can forge `interledgerPacket`, `settlement`, and `settlementMessage` messages "from" any currently-peered node of the recipient, provided it knows that node's ID. It can generate its own ephemeral key and compute a valid MAC.
- There is no nonce or timestamp, so captured messages can be replayed.
- TLS protects confidentiality in transit, but HTTPS client authentication is not used.

A wire-compatible reimplementation must accept this scheme to interoperate. A secure redesign would need a static-static DH (sender's Montgomery static key) or an Ed25519 signature by the sender, which would change the wire format.

---

## 8. Message handlers (receiver semantics) and their senders

Source: `peer-protocol/handlers/*.ts`, dispatch in `functions/handle-peer-message.ts`. Every handler receives `{message, authenticated, peerState}`, where `peerState` is the sender's state as seen *before* handling.

### 8.1 `peeringRequest` [0] (anonymous allowed)

Receiver (`handlers/peering-request.ts`):
1. `nodeId = nodeInfo.signed.value.nodeId`, taken from the embedded node info and **not** from `sender`.
2. If `settlementSchemeId` is not in our `ActiveSettlementSchemesSignal`, respond `{accepted: false, data: empty}`.
3. If `nodeId` is not already in our node table, respond `{accepted: false, data: empty}`. A node must already be known to us (majority, registered, or previously added) to peer.
4. `result = settlementModule(settlementSchemeId).acceptPeeringRequest({peerId: nodeId, data: settlementSchemeData})`. If the module is missing or returns `false`, respond `{accepted: false, data: empty}`.
5. Accept:
   - `addNode(nodeId)` (no-op, the node exists).
   - `processLinkState({linkState: nodeInfo.signed.value, linkStateBytes: nodeInfo.signed.bytes, retransmit: "never"})`.
   - `updateNode(nodeId, {peerState: {id: "peered", lastSeen: now, settlementSchemeId, settlementSchemeState: result.peerState}})`.
6. Respond `{accepted: true, data: result.peeringResponseData}`.

Known problems in this handler:
- **No signature verification** of the embedded node info (**NET-3**). There is also no check that `sender == nodeId`.
- **Bug (NET-4).** `linkStateBytes` is set to `nodeInfo.signed.bytes`, which is the *unsigned PeerNodeInfo* bytes. Elsewhere `lastUpdate` is always the full *SignedPeerNodeInfo*. The effect occurs only if this link state's sequence is newer than what we hold, because processLinkState ignores older or equal sequences. If it does take effect, then:
  - our `lastUpdate` for that node becomes unsigned bytes;
  - a later `linkStateRequest` from a third party gets `found` with bytes that do not parse as SignedPeerNodeInfo, so the whole response fails to parse at the requester;
  - heartbeat and forwarding are unaffected because retransmit is `never`.

  A reimplementation should store the full SignedPeerNodeInfo and verify it.

Sender: see the auto-peering procedure (§12.2).

### 8.2 `linkStateUpdate` [1] (anonymous allowed)

Receiver (`handlers/link-state-update.ts`):
1. `verifyLinkState` (§10.4). If invalid, log and respond empty.
2. `processLinkState({linkState: signed.value, linkStateBytes: <entire SignedPeerNodeInfo bytes>, retransmit: "scheduled"})` (§10.2).
3. Respond with an empty body.

The `sender` and authentication fields are ignored.

Senders:
- heartbeats (§10.5);
- broadcast of own updates (§10.6);
- forwarding (§10.3).

### 8.3 `interledgerPacket` [2] (authentication required)

Receiver (`handlers/interledger-packet.ts`):
1. Discard with a warning, and return an empty 200, if `!authenticated` or the sender's `peerState.id != "peered"`. The first check is unreachable in practice because the HTTP layer already returned 401.
2. `ledgerId = settlementModule(peerState.settlementSchemeId).ledger`.
3. Build the source endpoint: `{type: "peer", nodeId: sender, accountPath: "<ledgerId>:assets/interledger/<sender>"}`.
4. `processPacket({sourceEndpointInfo, serializedPacket: content.signed.packet, parsedPacket: parseIlpPacket(packet), requestId: content.signed.requestId})`.

**NET-27.** `parseIlpPacket` throws on a malformed packet, which yields HTTP 500.

Notes:
- The response is always empty. Prepare and Fulfill/Reject are each separate one-way messages.
- They are correlated by `requestId` scoped per peer: the connector key is `peer:<nodeId>#<requestId>` (`ilp-connector/functions/process-{prepare,fulfill}-packet.ts`).
- The outgoing requestId for a forwarded Prepare is `Math.trunc(Math.random() * 0xffffffff)`, a uint32.
- Fulfill/Reject are sent back with the *incoming* Prepare's requestId (`ilp-connector/senders/send-peer-packets.ts`).

Sender: `SendPeerPackets.sendPrepare` and `sendResult` wrap the ILP bytes in `{signed: {requestId, packet}}`.

### 8.4 `linkStateRequest` [3] (anonymous allowed)

Receiver (`handlers/link-state-request.ts`): for each requested nodeId, in order:
- `found{bytes = nodeTable[id].linkState.lastUpdate}` if we hold a link state;
- otherwise `notFound`.

There is no limit on the number of IDs; the response is bounded only by the 2 MB request limit. Responses are served from cache verbatim; no re-verification happens.

Sender: `RefreshNodeStateActor` (§10.7). It always requests exactly one ID.

### 8.5 `settlement` [4] (authentication required)

Receiver (`handlers/settlement.ts`):
1. If there is no settlement module for `settlementSchemeId`, respond empty. The settlement is silently ignored.
2. `peerState = nodeTable[sender].peerState`. If not peered, **throw**, which yields HTTP 500.
3. `await module.handleSettlement({peerId: sender, amount, settlementSchemeData, peerState: peerState.settlementSchemeState})`.
4. Respond empty.

Sender: `ledgers/send-outgoing-settlements.ts` sends `{settlementSchemeId, amount, settlementSchemeData: message}` after `prepareSettlement`, before `execute()`. See [5 §4.6](05-accounting-and-settlement.md).

### 8.6 `settlementMessage` [5] (authentication required)

Receiver: if the module exists, `module.handleMessage.tell({peerId: sender, message})` (fire-and-forget). Respond empty.

Sender: the settlement module host method `sendMessage({peerId, message})` (`ledgers/manage-settlement-scheme-instances.ts`).

### 8.7 `nodeListHashRequest` [6] (anonymous allowed)

Receiver: respond `{hash: SHA-256(serializedNodeList)}` (§11.5).

Sender: `PollNodeListHashesActor` (§11.2).

### 8.8 `nodeListRequest` [7] (anonymous allowed)

Receiver: respond with the cached serialized node list bytes (§11.5).

Sender: `DownloadNodeListsActor` (§11.3).

### 8.9 `registration` [8] (anonymous allowed)

Receiver (`handlers/registration.ts`):
1. `verifyLinkState(content.nodeInfo)`. If invalid, log and respond empty. The client never learns of the failure.
2. `addNode(nodeId)`.
3. `registerNode(nodeId)`:
   - `registrationState = {id: "registered", registeredAt: existing.registeredAt ?? now, renewedAt: now}`.
4. `processLinkState({..., retransmit: "never"})`.
5. Respond empty.

There is no fee, no rate limit, no proof-of-anything, and no sender check (**NET-17**). Registration state is not persisted (**NET-1**).

Sender: `RegisterWithNode` (§11.6), triggered manually from the UI.

### 8.10 `peeringInfoRequest` [9] (anonymous allowed)

Receiver:
- If the module for `settlementSchemeId` is missing, **throw** "Settlement scheme not found", which yields 500.
- Otherwise respond `{settlementSchemeData: (await module.getPeeringInfo()).data}`.

Sender: auto-peering, step 1 (§12.2).

---

## 9. Node table

Sources: `peer-protocol/stores/node-table.ts`, `computed/peers.ts`, `computed/registered-node-list.ts`.

In-memory `Map<NodeId, NodeTableEntry>` (immutable updates):

```
NodeTableEntry {
  nodeId: NodeId
  linkState: LinkState | undefined
  peerState: PeerState
  registrationState: RegistrationState
}
LinkState {
  lastUpdate: bytes            -- full SignedPeerNodeInfo of the newest accepted state (see NET-4)
  sequence: u64
  url: string
  alias: string
  publicKey: bytes
  neighbors: NodeId[]          -- from neighbor entries, in order
  settlementSchemes: SettlementSchemeId[]
  updateReceivedCounter: number       -- copies of the current sequence received (gossip suppression)
  scheduledRetransmitTime: number     -- epoch ms, or +Infinity
}
PeerState = { id: "none" }
          | { id: "peered", lastSeen: epoch ms, settlementSchemeId, settlementSchemeState: object (JSON-serializable) }
RegistrationState = { id: "none" } | { id: "registered", registeredAt: Date, renewedAt: Date }
```

The type `DiscoverySource = "self" | "registration" | "majority" | "client"` is declared but unused (**NET-29**).

Actions:
- `addNode(id)`: inserts `{linkState: undefined, peerState: none, registrationState: none}` if absent; no-op otherwise.
- `updateNode(id, partial)`: shallow merge; **throws if the node is absent**.
- `registerNode(id)`: as described in §8.9; throws if the node is absent.

There is **no removal action**: nodes are never deleted from the table (**NET-18**).

Derived signals:
- **`PeersSignal`** (`Set<NodeId>` of peered nodes).
  - Initialized from the table at first use.
  - On an `updateNode` action whose partial contains `peerState`, the node is added if `peerState.id == "peered"` and removed otherwise.
- **`RegisteredNodeListSignal`** (`Set<NodeId>`).
  - Initialized from the table.
  - Grows on `registerNode`.
  - Never shrinks.

Debug RPC (`peer-protocol/rpc-routers/network.ts`): `network.getNodes` and `network.subscribeToNodeTableStore` (protected; used by the UI).

---

## 10. Link state: generation, processing, propagation, refresh

### 10.1 Own link state

Source: `peer-protocol/maintain-own-node-table-entry.ts`.

**Trigger.** The actor re-runs whenever any of these change:
- `PeersSignal`;
- `ActiveSettlementSchemesSignal`;
- `MajorityNodeListSignal`;
- `NodeIdSignal`;
- `NodePublicKeySignal`;
- `DatabaseConfigStore` (any config field).

On each run:
- `peerIds = [...peers].filter(id => majorityNodeList.has(id))`. Only peers that are majority (globally registered) nodes are advertised as neighbors (**NET-21**).
- A new link state is produced iff any of these holds:
  - there is no current own link state;
  - `peerIds` differs from `old.neighbors` (as a set, compared by length and membership);
  - the settlement scheme set differs from `old.settlementSchemes`;
  - `old.sequence < now - LINK_STATE_MAX_UPDATE_INTERVAL` (1 h).
- Changes of `url` or `alias` alone do **not** produce a new link state (**NET-6**). Neither does the passage of time without a re-run: there is **no timer**, so the "1 hour keep-alive refresh" fires only if the actor happens to re-run (**NET-5**).
- `sequence = max(now_ms, old.sequence + 1)`. The sequence doubles as a timestamp (used for staleness in §10.7).
- The link state is serialized as `PeerNodeInfo{nodeId, sequence, url: config.url, alias: config.alias, publicKey, entries}`, signed with Ed25519, and wrapped as SignedPeerNodeInfo.
- If our own node is not in the table yet, `addNode(ownId)` runs first. Then `processLinkState({..., linkStateBytes: SignedPeerNodeInfo, retransmit: "immediately"})`.

After a restart, our own link state is not persisted, so the sequence restarts at `now`. If the clock went backwards, other nodes ignore our updates until the clock passes the old sequence (**NET-7**).

### 10.2 `processLinkState({linkStateBytes, linkState, retransmit})`

Source: `functions/modify-node-table.ts`.

1. `node = nodeTable.get(linkState.nodeId)`. If absent, **return; the state is discarded**. Link states from unknown nodes are ignored, so nodes enter the table only via majority lists, registration, peering, or as self.
2. If `sequence == node.linkState?.sequence` and `retransmit == "scheduled"`, increment `updateReceivedCounter`.
3. If `sequence <= (node.linkState?.sequence ?? 0)`, return.
4. Otherwise replace the link state:
   - `lastUpdate`, `sequence`, `publicKey`, `url`, `alias`;
   - `neighbors` and `settlementSchemes` parsed from entries;
   - `updateReceivedCounter = 1`;
   - `scheduledRetransmitTime = now + delay(retransmit)`.

   The delay depends on `retransmit`:
   - `immediately` → 0;
   - `scheduled` → `ceil(random() * LINK_STATE_MAX_UPDATE_RETRANSMIT_DELAY)`, i.e. 1..500 ms;
   - `never` → +Infinity.

`processLinkState` does not verify signatures; callers must call `verifyLinkState` first. The peering-request handler does not (**NET-3**). It also does not check that the public key matches a previously known key for that node ID; the node-ID binding (§2.2) covers this implicitly.

### 10.3 Forwarding (flooding with gossip suppression)

Source: `forward-link-state-update.ts`.

- Constants: `COUNTER_THRESHOLD = 3`, `MAX_RETRANSMIT_CHECK_INTERVAL = 200 ms`.
- A task runs every 200 ms. For every node-table entry with a linkState, where `updateReceivedCounter < 3` and `scheduledRetransmitTime < now`:
  1. Set `scheduledRetransmitTime = +Infinity`, so the update is retransmitted at most once.
  2. Serialize `linkStateUpdate{bytes: lastUpdate}` once.
  3. Send it to **every current peer except the subject node itself**. The peer the update came from is **not** excluded.
- Effective algorithm:
  - A newly received state is rebroadcast to all peers after a random 1–500 ms delay (+ up to 200 ms polling granularity).
  - It is suppressed if 3 or more copies (including the first) of the same sequence arrived via `linkStateUpdate` before the timer fired.
  - Our own new state is retransmitted on the next tick, with no delay.

### 10.4 `verifyLinkState({linkStateSignedBytes, linkState, signature})`

Source: `functions/verify-link-state.ts`.

1. Assert that the signature type is `ed25519` (the only alternative).
2. If `calculateNodeId(linkState.publicKey) != linkState.nodeId`, fail with "invalid node ID".
3. `Ed25519.verify(signature, linkStateSignedBytes /* PeerNodeInfo bytes */, publicKey)` (ZIP-215). Failure → "invalid signature".

Notes:
- No check is made that `publicKey` is 32 bytes. Noble throws on a wrong length, which inside an HTTP handler yields 500 (**NET-28**).
- No freshness check is made (sequence vs. clock).
- In the refresh flow, no check is made that the returned nodeId equals the requested one.

### 10.5 Heartbeats

Source: `send-heartbeats.ts`.

- `MIN_HEARTBEAT_INTERVAL = 5000 ms`, `MAX_HEARTBEAT_INTERVAL = 20 000 ms`. The task has `interval = 5000` and `jitter = 15 000`, so the delay is uniform in [5 s, 20 s).
- **Heartbeat payload.** A `linkStateUpdate` carrying our current own `lastUpdate`, sent to every peer (fire-and-forget).
- **Re-run trigger.** The actor re-runs (and sends a heartbeat round immediately via `task.execute()`) whenever `PeersSignal` changes or **our own node-table entry object** changes. The entry changes on new own states, on every `updateReceivedCounter` increment (for example echoes of our own state from peers), and when `scheduledRetransmitTime` is set to Infinity.
- **Receiving side.** A heartbeat is processed like any linkStateUpdate: it only increments `updateReceivedCounter`. **`peerState.lastSeen` is never updated** (it is set only at peering time). **There is no liveness detection or peer timeout** (**NET-12**).

### 10.6 Broadcast of own state to all known nodes

Source: `broadcast-state-updates.ts`.

- The actor re-runs when our own `linkState.lastUpdate` changes (tracked by reference).
- It sends `linkStateUpdate{lastUpdate}` to **every node ID in the node table**:
  - including ourselves (loopback);
  - including nodes without a known URL (these are dropped with a "no url known" warning).
- This is a direct, unrelayed push to all known nodes, in addition to peer flooding. With N known nodes, every own update costs N HTTP requests (**NET-20**).

### 10.7 Refreshing stale or unknown node states

Source: `refresh-node-state.ts`.

- `NODE_REFRESH_INTERVAL = 1000 ms`, `INDIRECT_QUERY_PROBABILITY = 0.2`, `NODE_STATE_STALE_TIMEOUT = 3 h` (10 800 000 ms).
- At start, assert that at least 1 bootstrap node is configured (fatal otherwise).
- Each tick:
  1. Partition the node table, excluding ourselves:
     - `current` = nodes whose `linkState.sequence > now - 3h` (the sequence is interpreted as an ms timestamp);
     - `old` = all others (no linkState, or stale).
  2. If `old` is empty, stop.
  3. Pick a uniformly random `nodeId ∈ old`.
  4. Pick the oracle:
     - default `oracle = nodeId` (ask the node itself, at the URL from its last known state);
     - if we have no linkState for it, **or** with probability 0.2, use instead `oracle = random(current ∪ bootstrapNodeIds)`. The bootstrap list may duplicate entries in `current` and may include ourselves.
  5. Send `linkStateRequest{nodeIds: [nodeId]}`. If the response is `found`, verify it, then `processLinkState(..., retransmit: "never")`.
- Consequences:
  - One node is refreshed per second, at most.
  - A node whose link state is never re-issued (see **NET-5**) stays "old" forever and is queried continuously.
  - The docs' "prefer faster/more up-to-date oracles" is not implemented.

---

## 11. Network membership: BNL, node lists, majority, registration

### 11.1 Bootstrap Node List (BNL)

Sources: `constants/bootstrap-nodes.ts`, `config/environment-config.ts`.

- The env var `DASSIE_BOOTSTRAP_NODES` holds JSON of the form `[{id: NodeId (2..45 chars, alphabet not enforced), url: string, publicKey: base64url Ed25519 pubkey}]`.
- The default is `DEFAULT_BOOTSTRAP_NODES`:
  - `dNKbwOMCw0DTToPM4tyyVASWxM4NP3MKW`, `https://test1.dassie.xyz`, `QBe5HLbdYFtIpjFTIu_Lz8lkoUJ3fmAvFHfCb9CVwIM`;
  - `dyrjUANXiJ5S6JXxwo7jdKKJjdLtK129U`, `https://test2.dassie.xyz`, `tmS1PB42DEjYi-Ua4RNguhweId9YjT9Kml3uq3iZSuQ`.
- The config's `id` vs `publicKey` consistency is **not verified** (**NET-19**).
- The config's public key and URL are used only as contact info fallback (§6.2).
- Bootstrap nodes are **not** automatically added to the node table. They enter it only if they appear on the majority list (§11.4).

### 11.2 Polling node-list hashes

Source: `poll-node-list-hashes.ts`.

- Interval `NODE_LIST_HASH_POLLING_INTERVAL`: **1000 ms in dev** (`import.meta.env.DEV`), **60 000 ms in prod**. The first run happens immediately at startup and is awaited.
- Each run sends `nodeListHashRequest` to all bootstrap nodes, with concurrency 5 (`NODE_LIST_POLLING_CONCURRENCY`).
- For each non-undefined response, set `BootstrapNodeListHashesSignal[bnId] = hash`. Failures leave the previous value in place.

### 11.3 Downloading node lists

Source: `download-node-lists.ts`.

- Same interval as §11.2, but an independent task. The first run is awaited at startup.
- Each run:
  - `changed` = bootstrap nodes whose polled hash differs from the hash stored with their downloaded list (an absent list counts as the empty hash).
  - Send `nodeListRequest` to each, concurrency 5.
  - On success, store `BootstrapNodeListsSignal[bnId] = {hash: <polled hash>, entries: <ids>}`.
- The stored hash is the *polled* one, not one recomputed from the downloaded list. The list is never verified against the hash. Lists of bootstrap nodes that stop responding are kept forever (**NET-15**).

### 11.4 Majority node list

Sources: `computed/majority-node-list.ts`, `registration-client/constants/threshold.ts`.

- `MAJORITY_THRESHOLD = 1/2`.
- `N = number of configured bootstrap nodes`. This counts all configured nodes, including those whose lists were never downloaded.
- `votes(id)` = number of configured bootstrap nodes whose downloaded list contains `id`.
- `majority = { id : votes(id)/N > 1/2 }`. This is a strict majority: N=1 needs 1, N=2 needs 2, N=3 needs 2, N=4 needs 3. The docs say "> N/2 + 1" (**NET-16**).
- Set-compared, so the signal is updated only when the set membership changes.

`AddMajorityNodesActor` calls `addNode` for every majority ID not yet in the table. It never removes nodes, even after they lose majority (**NET-18**).

Uses of the majority list:
- Neighbors advertised in our own link state (§10.1).
- Address assignment for peers via ILDCP (§3, §12.4).
- Our served node list (§11.5).

### 11.5 Serving our node list ("Known Node List")

Sources: `computed/serialized-node-list.ts`, `computed/node-list-hash.ts`.

- `list = sort_localeCompare(majoritySet ∪ registeredSet)`, then serialized as `SEQUENCE OF NodeId`. This is `peerMessageResponse.nodeListRequest` with `{value: list}`.
- The hash is `SHA-256(serialized list bytes)`.
- Sort order: JavaScript `String.prototype.localeCompare` with the runtime default locale (ICU collation, roughly case-insensitive with punctuation weights). This is not byte order.
  - Interop note: peers compare our hash only against hashes previously received from us, never recompute it. Exact ordering is therefore not interop-critical. A reimplementation should still sort deterministically to avoid spurious hash changes.
  - To stay bit-identical with existing nodes' hashes, use ICU root collation (**NET-32**).

### 11.6 Registration (client)

Sources: `registration-client/functions/register-with-node.ts`, `rpc-router/registration-client.ts`, `computed/registration-status.ts`, `constants/threshold.ts`.

- **Manual only.** The UI calls the RPC `registrationClient.registerWithNode({nodeId})`. The RPC rejects the request when:
  - there is no downloaded bootstrap list for `nodeId` ("not found in bootstrap lists");
  - our ID is already on that list ("already registered");
  - `nodeId` is not in our node table.
- If accepted, it sends `registration{nodeInfo: {bytes: ownLinkState.lastUpdate}}` (raw SignedPeerNodeInfo) and ignores the empty response.
- **Status signal** (RPC `registrationClient.subscribeRegistrationStatus`):
  - `nodes = [{nodeId, hostname: URL(bootstrap.url).hostname, registered: list.entries.includes(ourId)}]` for every bootstrap node with a downloaded list;
  - `registrationRatio = count(registered) / nodes.length`. This is NaN if no lists have been downloaded. The denominator is the number of *downloaded* lists, unlike the majority computation, which uses configured nodes.
  - `threshold = TARGET_REGISTRATION_THRESHOLD = 4/5`. Used by the UI only.
- There is no automatic registration, renewal, fee quote, or payment. "Pay the cheapest 75%" from the docs is not implemented (**NET-17**).

### 11.7 How a new node becomes globally routable (as implemented)

1. The operator registers with the bootstrap nodes via the UI. Each bootstrap node records us as `registered` and adds us to its served list.
2. Every node re-polls hashes every 60 s and re-downloads changed lists. Once a strict majority of its configured bootstrap nodes list us, we are in its majority set, so it `addNode`s us.
3. It then learns our link state via `refreshNodeState` (it has no link state for us, so it asks a current node or bootstrap node, §10.7), or via our broadcast/flooding. Our broadcast reaches only nodes already in *our* table (§10.6), and receivers drop states of unknown nodes (§10.2).
4. Our peers that see us in their majority set also start advertising us as a neighbor (§10.1), which makes us routable (§13).

---

## 12. Peering lifecycle and peer state machine

### 12.1 States as implemented

The `PeerState` states are `none` and `peered` (§9). Transitions:

| From | Event | To | Where |
|---|---|---|---|
| (absent) / `none` | we accept an inbound `peeringRequest` | `peered` (lastSeen = now, scheme, state = module.acceptPeeringRequest().peerState) | `handlers/peering-request.ts` |
| `none` | our outbound peering request is accepted and `finalizePeeringRequest` resolves | `peered` (lastSeen = now, scheme, state = finalize().peerState) | `maintain-peering-relationships.ts` |
| any | node restart | `peered` with `lastSeen = 0` if a `peers` DB row exists; otherwise `none` | `persist-node-table.ts` |
| `peered` | — | *no transition out exists* | — |

The docs' states `request-peering`, `terminating`, and `banned`, the `advertise` flag, termination messages, flapping control, and liveness review are **not implemented** (**NET-13**). A peered node stays peered for the life of the process. A peer that restarts loses runtime-established peerings, because they are not persisted (**NET-1**). The other side still believes it is peered, so asymmetric state results. Inbound peering requests from an already-peered node are accepted again, overwriting the state.

### 12.2 Auto-peering

Source: `maintain-peering-relationships.ts`.

- `PEERING_CHECK_INTERVAL = 1000 ms`, `MINIMUM_PEERS = 2`. The task's overlap policy is `skip`, so at most one attempt is in flight.
- Each tick:
  1. If `|peers| >= 2`, do nothing.
  2. Build candidates: node-table entries that satisfy all of the following:
     - have a linkState;
     - `nodeId != ours`;
     - `peerState.id == "none"`;
     - at least one of their advertised `settlementSchemes` is in our active scheme set.

     `commonSettlementScheme` = the **first** scheme in the candidate's advertised order that we also support.
  3. `randomNode` = uniformly random candidate. If there are none, stop.
  4. Require our own linkState to exist (otherwise warn and stop), and the settlement module actor for the scheme to exist.
  5. Send `peeringInfoRequest{settlementSchemeId}` to the candidate. If the response is undefined, warn and stop.
  6. `data = module.createPeeringRequest({peerId, peeringInfo: resp.settlementSchemeData}).data`.
  7. Send `peeringRequest{nodeInfo: ownLinkState.lastUpdate (raw SignedPeerNodeInfo bytes, octet-wrapped), settlementSchemeId, settlementSchemeData: data}`.
  8. If the response is undefined or `accepted == false`, stop. The candidate will simply be retried randomly later; there is no backoff or blacklist (**NET-22**).
  9. `final = module.finalizePeeringRequest({peerId, peeringInfo, data: resp.data})`.
  10. `updateNode(peerId, {peerState: {id: "peered", lastSeen: now, settlementSchemeId, settlementSchemeState: final.peerState}})`.
- There is no preference for majority nodes, centrality, reputation, or features. The docs' selection criteria are not implemented. Peers are not limited above the minimum: inbound requests are always accepted if the scheme matches and the node is known.

### 12.3 Effects of becoming peered

These are reactions to `PeersSignal`:
- **`CreatePeerLedgerEntriesActor`** (one per peer, `create-peer-ledger-entries.ts`):
  - `ledgerId = settlementModule(peerState.settlementSchemeId).ledger`;
  - `initializePeer` creates accounts `<ledgerId>:assets/interledger/<peerId>` (limit `debits_must_not_exceed_credits`) and `<ledgerId>:contra/trust/<peerId>` (limit `credits_must_not_exceed_debits`) (`accounting/functions/manage-peer.ts`);
  - cleanup on removal is a no-op TODO.
- **`ReceiveAddressFromUplinkActor`** (one per peer): ILDCP query, see §12.4.
- **Own link state.** The peer is advertised as a neighbor if it is in the majority set (§10.1).
- **Heartbeats.** The heartbeat actor re-runs and sends immediately (§10.5).

### 12.4 Uplink address (ILDCP over the peer link)

Sources: `peer-protocol/functions/query-uplink-address.ts`, `ilp-connector/functions/send-link-local-packet.ts`, `ildcp-server/handle-ildcp-requests.ts`.

**Requesting side.** For each new peer, the node sends a link-local ILP Prepare *directly to that peer*, bypassing routing (via `predeterminedOutcome`). The Prepare has:
- `amount = 0`;
- `destination = "peer.config"`;
- `executionCondition = ILDCP_CONDITION`, the bytes `66687aadf862bd776c8fc18b8e9f8e20089714856ee233b3902a591d0d5f2925`, i.e. SHA-256 of 32 zero bytes;
- `expiresAt = now + MAXIMUM_HOLD_TIME (30 s)`;
- empty data.

It travels as an `interledgerPacket` peer message (authenticated).

**Serving side.** The peer routes `peer.config` to its fixed ILDCP route and replies with a Fulfill:
- `fulfillment = 32 zero bytes`;
- `data = OER SEQUENCE { address IA5String, assetScale UINT8, assetCode IA5String }`, with `address = getEndpointIlpAddress(source peer endpoint)` (§3);
- **`assetScale = 9`, `assetCode = "XRP"`, hardcoded** (**NET-24**).

**Result.** On success, `UplinkAddressesStore[peerId] = address`; on Reject or parse failure, the entry is deleted. **Nothing reads `UplinkAddressesStore`** (**NET-23**). The node always uses `<scheme>.das.<ownId>` as its address.

### 12.5 Settlement scheme module contract used by peering

Source: `ledgers/types/settlement-scheme-module.ts`.

- `getPeeringInfo() → {data}`: answered to `peeringInfoRequest`.
- `createPeeringRequest({peerId, peeringInfo}) → {data}`: put in `peeringRequest.settlementSchemeData`.
- `acceptPeeringRequest({peerId, data}) → {peeringResponseData, peerState} | false`.
- `finalizePeeringRequest({peerId, peeringInfo, data}) → {peerState}`.
- `handleSettlement({amount, peerId, settlementSchemeData, peerState})`.
- `handleMessage({peerId, message})`.
- Host side: `sendMessage({peerId, message})` → peer message `settlementMessage`.

The `stub` scheme returns empty data and `{}` peer state everywhere. The settlement module's `supportedVersions` is declared ("used to reject peering when versions don't overlap") but is not exchanged on the wire (**NET-26**).

---

## 13. Routing

### 13.1 Routing table

Sources: `routing/signals/routing-table.ts`, `routing/utils/prefix-map.ts`.

- `RoutingTableSignal` holds a mutable `PrefixMap<IlpAddress, RoutingInfo>`, and its comparator always returns "changed".
- `RoutingInfo` is one of:
  - `{type: "fixed", destination: EndpointInfo}`;
  - `{type: "peer", firstHopOptions: NodeId[], distance: number}`.

**PrefixMap semantics:**
- `sortedKeys` is ordered by length descending, then lexicographically descending (JavaScript `>` on UTF-16).
- `lookup(addr)` resolution:
  - if `get(addr)` is truthy, return that exact entry;
  - otherwise return the first key in `sortedKeys` such that `addr.startsWith(key)`;
  - if there is none, return `undefined`.
- This is **plain string-prefix matching, not segment-aware**: a key `test.das.dAB` would match `test.das.dABC` (**NET-33**). ILP convention is segment-aware (`key == addr` or `addr.startsWith(key + ".")`).

**Writers of the table:**
- `CalculateRoutesActor`: `peer` routes for `<scheme>.das.<nodeId>`.
- `HandleIldcpRequestsActor`: fixed `peer.config` → ILDCP endpoint.
- `RouteLocalEndpointsActor`: fixed `<nodeAddr>.<localPart>` → local endpoint.
- BTP server: fixed `<nodeAddr>.<nanoid6>` → BTP connection. This is supposed to happen on the websocket "open" event, but see **NET-25**.

### 13.2 Route calculation

Source: `routing/calculate-routes.ts`.

**Trigger.** The actor re-runs on any change of `IlpAllocationSchemeSignal`, `NodeIdSignal`, or the **entire node table**. This includes every counter or timer change, so recomputation is frequent.

**Algorithm.** The code comment calls it an "Even-Shiloach tree"; in practice it is a plain BFS:

```
info[own] = {level 0, parents []}; queue = [ownEntry]
while queue not empty:
  cur = queue.pop_front()
  for n in cur.linkState?.neighbors ?? []:          # DIRECTED edges, as advertised by cur
    if n in info:
      if info[n].level > info[cur].level+1: info[n] = {level: cur+1, parents: [cur]}   # never happens in BFS
      elif info[n].level == info[cur].level+1: info[n].parents.push(cur)               # equal-cost alternative
    else:
      info[n] = {level: cur+1, parents: [cur]}
      if n in nodeTable: queue.push(nodeTable[n])    # nodes not in table get a route but are not expanded
for (id, i) in info:
  parents = set(i.parents); L = i.level
  repeat (L-2) times: parents = ∪_{p∈parents} info[p].parents   # climb to level-1 ancestors
  firstHops = (i.level == 1) ? [id] : [...parents]
  routingTable.set(`${scheme}.das.${id}`, {type: "peer", firstHopOptions: firstHops, distance: i.level})
cleanup (on re-run): delete every `${scheme}.das.${id}` that this run inserted
```

Properties:
- Shortest-hop routing on the graph of advertised neighbor edges. Edges are taken from each node's *own* claim and are **not** checked for reciprocity.
- Our own first hops come from our own link state's neighbors. These are only majority peers (§10.1), so **non-majority peers (child nodes) receive no route** (**NET-21**).
- `firstHopOptions` contains *all* level-1 ancestors on any shortest path. The order is the BFS discovery order, which depends on neighbor ordering in link states.
- **Own node.** Our own node gets an entry `<scheme>.das.<ownId>` with `firstHopOptions: []` and distance 0 (**NET-34**).
- **Missing parent entries.** If a node referenced as a parent has no entry, `info[p].parents ?? []` is used.

### 13.3 Route resolution (next-hop selection)

Source: `routing/functions/resolve-ilp-address.ts`.

`resolveIlpAddress(dest)`:
- `info = routingTable.lookup(dest)`. If none, return `NoRouteFoundFailure`; the connector maps it to ILP Reject `F02 Unreachable` "No route found for destination" (`ilp-connector/functions/calculate-prepare-packet-outcome.ts`).
- If `fixed`, return its destination.
- If `peer`:
  - `nextHop = firstHopOptions[0]`. Always the first option: no load balancing, failover, or liquidity awareness.
  - If `nextHop`'s peerState is not `peered`, **throw** `"Next hop node is not actually peered <id>"`. This also happens for the own-node entry with empty first hops, where nextHop is undefined (**NET-34**).
  - Otherwise return `{type: "peer", nodeId: nextHop, accountPath: "<ledgerId(peer's scheme)>:assets/interledger/<nextHop>"}`.

### 13.4 Link-state propagation feeding routing

Routing uses only `nodeTable[*].linkState.neighbors`. Its inputs come from §10:
- own state;
- received updates (flooding plus direct broadcast);
- refresh queries.

There is no aging-out of link states: stale states remain in routing indefinitely, because nothing removes nodes or link states (**NET-18**).

---

## 14. BTP server (ILP clients over WebSocket)

Sources: `btp-server/register-btp-http-upgrade.ts`, `ilp-connector/senders/send-btp-packets.ts`, `packages/lib-protocol-utils/src/btp.ts`.

The BTP server is not part of node-to-node peering. It is the client-facing ILP access protocol, specified here because it shares the routing table and address scheme. Its packet handling is in [4 §4.8](04-interledger.md) and its schemas in [4 §10](04-interledger.md).

### 14.1 Connection setup

- WebSocket upgrade on `GET /btp` (HTTPS WebSocket router).
- `connectionId` is an incrementing integer starting at 0, not persisted.
- `localIlpAddressPart = nanoid(6)`, regenerated until `<nodeAddr>.<part>` is unused in the routing table.

### 14.2 Message schemas

These are OER schemas, not the BTP 2.0 ASN.1 with a `subprotocolData` envelope field. The field is commented out:

```
BtpEnvelope ::= SEQUENCE { messageType UINT8, requestId UINT32, message OCTET STRING }
BtpMessage  ::= SEQUENCE { protocolData ProtocolData }
BtpTransfer ::= SEQUENCE { amount UINT64, protocolData ProtocolData }
BtpError    ::= SEQUENCE { code IA5String(SIZE(3)), name IA5String, triggeredAt IA5String,
                           data OCTET STRING (SIZE(0..8192)), protocolData ProtocolData }
ProtocolData ::= SEQUENCE OF SEQUENCE { protocolName IA5String, contentType UINT8, data OCTET STRING }
```

- Message types: Response=1, Error=2, Message=6, Transfer=7.
- Content types: 0 = application/octet-stream, 1 = text/plain;utf8, 2 = application/json.

The real BTP 2.0 is `envelope{type, requestId, data}` where `data` contains `protocolData` (a sequence-of with a var-uint count). The OER here matches that byte layout because a SEQUENCE OF quantity is an OER var-uint.

### 14.3 Authentication

The first message must be:
- a **Message** (type 6) whose `protocolData[0]` is `{protocolName: "auth", contentType: 0, data: empty}`;
- containing an entry `auth_token` with contentType 1 and a UTF-8 token that exists in `BtpTokensStore` (DB table `btp_tokens`).

Anything else closes the socket. On success the server replies with a Response (type 1), the same requestId, and empty protocolData.

### 14.4 After authentication

- **Message / Transfer / Response:** the first `protocolData` entry named `ilp` is handed to `processPacket` with `source {type: "btp", connectionId}` and `requestId = envelope.requestId`. Transfer `amount` is ignored.
- **Auth again** on an authenticated connection: the socket is closed.
- **Error (type 2):** ignored silently.
- **Non-binary frames:** the socket is closed.

### 14.5 Outgoing packets

- A Prepare is sent as a Message (type 6) with `[{ilp, 0, packet}]`, `requestId = outgoingRequestId`.
- A Fulfill/Reject is sent as a Response (type 1) with the incoming requestId.

### 14.6 Close

On close, the route is deleted and the socket is forgotten.

---

## 15. Persistence

Sources: `database/migrations/*`, `peer-protocol/tables/*`, `peer-protocol/persist-node-table.ts`.

```sql
-- 0005
CREATE TABLE nodes (id TEXT NOT NULL PRIMARY KEY, public_key BLOB NOT NULL, url TEXT NOT NULL, alias TEXT NOT NULL) STRICT
-- 0006 (+0010)
CREATE TABLE peers (node INTEGER PRIMARY KEY NOT NULL, settlement_scheme_id TEXT NOT NULL) STRICT
ALTER TABLE peers ADD COLUMN settlement_scheme_state TEXT NOT NULL DEFAULT "{}"   -- JSON
-- 0013
CREATE TABLE registrations (node INTEGER PRIMARY KEY NOT NULL, registered_at INTEGER NOT NULL, renewed_at INTEGER NOT NULL) STRICT  -- ms epoch
```

- `peers.node` and `registrations.node` reference `nodes.rowid` (the implicit rowid, not `id`). No FK constraint is declared.
- The SQLite application ID is `0x1DA53B81` (`database/schema.ts`).

**Load** (once, at PeerProtocol start): `SELECT nodes.id, peers.settlement_scheme_id, peers.settlement_scheme_state, registrations.registered_at, registrations.renewed_at FROM nodes LEFT JOIN peers ON nodes.rowid = peers.node LEFT JOIN registrations ON nodes.rowid = registrations.node`. For each row:
- `linkState = undefined`. The persisted `public_key`, `url`, and `alias` are **ignored**.
- `peerState = peered{lastSeen: 0, settlementSchemeId, settlementSchemeState: JSON.parse(state)}` if both peers columns are non-null. Otherwise `none`. Non-object JSON throws.
- `registrationState = registered{registeredAt, renewedAt}` if both timestamps are truthy. Otherwise `none`.

**Write-back: not implemented.** The `addNode`, `updateNode`, and `registerNode` watchers are empty `// TODO` stubs (**NET-1**). Only the dev tool `app-dev/src/utils/prefill-database.ts` writes these tables:
- it pre-registers configured nodes at bootstrap nodes;
- it pre-peers configured peers with their settlement state.

**Consequence of the ignored columns (NET-2).** Loaded peers have no linkState until refreshed. Until then:
- they cannot authenticate to us (§7.3 requires a linkState);
- we cannot send to them unless they are bootstrap nodes (no URL, §6.2).

Our own link state, session keys, bootstrap lists, and uplink addresses are memory-only.

---

## 16. Constants

| Name | Value | Source |
|---|---|---|
| Peer message content type (request + Accept) | `application/dassie-message` | `peer-protocol/constants/content-type.ts` |
| Peer response content type | `application/dassie-peer-response` | `register-peer-http-handler.ts` |
| Peer endpoint path | `POST {url}/peer` | `send-peer-message.ts`, `register-peer-http-handler.ts` |
| Envelope version | `0` | same |
| Max request body | 2 097 152 bytes | `packages/lib-http-server/src/constants.ts` |
| `SESSION_KEY_EXPIRY` (outgoing) | 7 d = 604 800 000 ms | `stores/outgoing-session-keys.ts` |
| `MAJORITY_THRESHOLD` | 1/2 (strict >) | `registration-client/constants/threshold.ts` |
| `TARGET_REGISTRATION_THRESHOLD` | 4/5 (UI only) | same |
| `MINIMUM_PEERS` | 2 | `maintain-peering-relationships.ts` |
| `COUNTER_THRESHOLD` (gossip suppression) | 3 | `forward-link-state-update.ts` |
| `INDIRECT_QUERY_PROBABILITY` | 0.2 | `refresh-node-state.ts` |
| `NODE_LIST_POLLING_CONCURRENCY` | 5 (both poll and download) | `poll-node-list-hashes.ts`, `download-node-lists.ts` |
| `MAXIMUM_HOLD_TIME` (ILDCP expiry) | 30 000 ms | `ilp-connector/constants/expiry-constraints.ts` |
| ILDCP response asset | scale 9, code "XRP" (hardcoded) | `ildcp-server/handle-ildcp-requests.ts` |
| OIDs | see §2.3 | `constants/object-identifiers.ts` |
| Default bootstrap nodes | see §11.1 | `constants/bootstrap-nodes.ts` |
| Node ID prefix / length | `"d"` + ≤32 base64url chars, trailing `A`s stripped | `calculate-node-id.ts` |
| NodeId wire constraint | IA5 [2..45], `[A-Za-z0-9_-]` | `peer-schema.ts` |
| SettlementSchemeId wire constraint | IA5 [2..128], `[a-z0-9-]` | `peer-schema.ts` |
| Signature | Ed25519, 64 bytes | `peer-schema.ts` |
| Session pubkey / MAC | 32 / 32 bytes | `peer-schema.ts` |

## 17. Timers

| Timer | Value | Behavior | Source |
|---|---|---|---|
| `LINK_STATE_MAX_UPDATE_INTERVAL` | 3 600 000 ms (1 h) | Own state is re-issued if older, but only checked on actor re-run (NET-5) | `constants/timings.ts`, `maintain-own-node-table-entry.ts` |
| `LINK_STATE_MAX_UPDATE_RETRANSMIT_DELAY` | 500 ms | Random forward delay `ceil(rand*500)` for received updates | `timings.ts`, `modify-node-table.ts` |
| `MAX_RETRANSMIT_CHECK_INTERVAL` | 200 ms | Forwarder poll period | `forward-link-state-update.ts` |
| `NODE_LIST_HASH_POLLING_INTERVAL` | 1 000 ms dev / 60 000 ms prod | Hash poll and list download periods | `timings.ts` |
| `NODE_LIST_HASH_TIMEOUT` | 5 000 ms | **Unused** (NET-14) | `timings.ts` |
| `DEFAULT_NODE_COMMUNICATION_TIMEOUT` | 30 000 ms | HTTP abort for every peer message | `timings.ts`, `send-peer-message.ts` |
| `NODE_STATE_STALE_TIMEOUT` | 10 800 000 ms (3 h) | Link state older than this (by sequence-as-timestamp) is refreshed | `timings.ts`, `refresh-node-state.ts` |
| `NODE_REFRESH_INTERVAL` | 1 000 ms | One stale node queried per tick | `refresh-node-state.ts` |
| `MIN_HEARTBEAT_INTERVAL` / `MAX_HEARTBEAT_INTERVAL` | 5 000 / 20 000 ms | Heartbeat period uniform in [5 s, 20 s); immediate on re-run | `timings.ts`, `send-heartbeats.ts` |
| `PEERING_CHECK_INTERVAL` | 1 000 ms | Auto-peering attempt period | `maintain-peering-relationships.ts` |
| `SESSION_KEY_EXPIRY` | 7 d | Outgoing ephemeral key rotation | `stores/outgoing-session-keys.ts` |

Task scheduling rules (all of the above):
- the next timer is armed at tick start;
- overlap policy `skip`;
- actors that `await task.execute()` run once immediately at startup.

---

## 18. Documentation vs. implementation

| Doc claim (`docs/implementation-notes/*`) | Implementation |
|---|---|
| KNL entry via paid registration; fee grows with KNL size; new nodes pay the cheapest 75% of BNL | Registration is free and manual, and the UI target is 80%. There is no fee quote message. |
| Node added when on > N/2 + 1 KNLs (node-discovery) / "majority" (network-membership) | `votes/N > 1/2` (§11.4) |
| Registered-only nodes removed if not on a majority after a long delay; inactive nodes removed after a long timeout | No removal at all (NET-18) |
| Discovery end criteria: all nodes up to date, timeout, or ≥75% BNL lists and ≥50% KNL states | Startup awaits only one round each of hash poll, download, and refresh. No criteria. |
| Status queries prefer fast, up-to-date oracles | Uniform random choice with a 20% indirect probability |
| Peer states `request-peering`, `peered`, `terminating`, `banned`; advertise flag; termination; flapping; liveness | Only `none` and `peered`. No exit (NET-13). |
| Peering target "verifies requester via peer data endpoint" before accepting | Not done. Acceptance depends only on scheme, known node, and module. |
| Private UDP API with AES-128-GCM datagrams, session IDs, replay cache, MAX_PACKET_TIMEOUT wait after restart | Not implemented. All traffic is HTTPS POST `/peer` with HMAC authentication (§6, §7). |
| Node properties: agent, features, web URL | Not in `PeerNodeInfo`. Only nodeId, sequence, url, alias, publicKey, neighbors, settlement schemes. |

---

## 19. Open questions / known gaps

Every item is a behavior of the original code. For each one the reimplementation must choose: reproduce it (wire-compat) or fix it (semantics).

- **NET-1** Node-table persistence write-back is TODO (`persist-node-table.ts`). Peers and registrations established at runtime are lost on restart. Only the dev prefill writes the tables.
- **NET-2** Persisted `nodes.public_key/url/alias` are ignored on load. Loaded peers have `linkState = undefined`, so they cannot authenticate to us or be contacted until a refresh succeeds. `lastSeen` is set to 0.
- **NET-3** `peeringRequest` handler: no signature verification of the embedded node info, and no check that `sender == nodeInfo.nodeId`. An unauthenticated party can make us peer with (and store a forged link state for) any known node ID, if the settlement module accepts. The stub always accepts.
- **NET-4** `peeringRequest` handler stores `nodeInfo.signed.bytes` (unsigned PeerNodeInfo) as `lastUpdate` instead of the SignedPeerNodeInfo. This poisons later `linkStateRequest` responses for that node.
- **NET-5** Own link state has no periodic refresh timer. The 1-hour re-issue happens only if the actor re-runs for another reason. Otherwise, after 3 h every other node considers us stale and queries us every refresh cycle forever.
- **NET-6** Changing `url`/`alias` does not re-issue the link state. Separately, `config.url` is computed once at load, and `setHostname` does not update it (`config/database-config.ts`), so the URL is stale until restart.
- **NET-7** Sequence = wall-clock ms and is not persisted. After a clock rollback or restart with a lower clock, new own states are ignored by others until the clock catches up.
- **NET-8** Authentication does not authenticate the sender (ephemeral-static DH only). The MAC excludes `sender`/`version`. There is no replay protection (§7.4).
- **NET-9** `IncomingSessionKeysStore` grows without bound, with no expiry or eviction. Entries are added only after a valid MAC. Because of NET-8, though, any party can produce a valid MAC (by claiming to be any peered node that has a link state) and so add an entry per fresh ephemeral key. This is a memory DoS vector.
- **NET-10** MAC comparison is not constant-time.
- **NET-11** The client ignores HTTP status codes. Error responses become parse failures, or are silently accepted for `empty()` responses with an empty body.
- **NET-12** Heartbeats do not update `lastSeen`, and there is no liveness detection. `lastSeen` is write-only.
- **NET-13** No peering termination, banning, advertise flag, or flapping control, despite the docs.
- **NET-14** `NODE_LIST_HASH_TIMEOUT` (5 s) is unused. Polls use the 30 s default, and startup awaits the first poll round, so an unreachable bootstrap node delays startup by up to 30 s per round.
- **NET-15** Downloaded node lists are stored with the *polled* hash and never verified against it. Lists from bootstrap nodes that go offline are kept forever and keep voting.
- **NET-16** The majority rule in code is strict `> N/2`; the node-discovery doc says `> N/2 + 1`. With the default N=2, both bootstrap nodes must list a node.
- **NET-17** The registration anti-Sybil fee is not implemented: no price query message, no payment. Registration is accepted from anyone with a valid self-signed link state.
- **NET-18** Nodes and link states are never removed: no removal action, no expiry from majority loss or inactivity. Routing can therefore use stale edges indefinitely.
- **NET-19** The bootstrap config's `id` is not checked against `publicKey`.
- **NET-20** `BroadcastStateUpdatesActor` sends every own update to every known node directly (O(N) HTTP requests per update, including self), in addition to peer flooding.
- **NET-21** Only *majority* peers are advertised as neighbors and get routes. Child/uplink peers (non-registered nodes using `<ourAddr>.<peerId>`) get an ILDCP address but no route entry. Packets to them match our own `<scheme>.das.<ownId>` route with empty first hops, which throws (NET-34). Child-node addressing is effectively non-functional.
- **NET-22** Auto-peering has no backoff or blacklist for failing candidates. It stops initiating once `|peers| ≥ 2`, and it does not consider majority status or reachability when choosing a candidate.
- **NET-23** `UplinkAddressesStore` is written but never read.
- **NET-24** The ILDCP response hardcodes `assetScale 9`, `assetCode "XRP"` regardless of the peer's settlement scheme or ledger.
- **NET-25** BTP: the route `<nodeAddr>.<nanoid>` is registered in the websocket `"open"` handler. With `ws` server sockets (`handleUpgrade` callback) the socket is already open and `"open"` is likely never emitted, so the route would never be added; verify. Separately, the BTP source address (`<nodeAddr>.<connectionId>`) differs from the routed address (`<nodeAddr>.<nanoid>`).
- **NET-26** The settlement module `supportedVersions` is never negotiated on the wire.
- **NET-27** Handler exceptions return HTTP 500, which the sender sees as a parse failure. Cases:
  - `parseIlpPacket` throws on malformed ILP;
  - settlement from an unpeered node;
  - `peeringInfoRequest` for an unknown scheme;
  - `updateNode` on a missing node.
- **NET-28** `verifyLinkState` does not validate the public key length; noble throws, which returns 500.
- **NET-29** `DiscoverySource` type is declared but unused. The node table does not record how a node was discovered (except registration).
- **NET-30** lib-oer `visibleString` rejects `~` (0x7E) on parse (off-by-one in `fill`), but serialize does not check. A node URL containing `~` produces link states that every other node rejects.
- **NET-31** The settlement scheme ID alphabet `[a-z0-9-]` conflicts with the documented `+`-separated naming convention.
- **NET-32** The node list sort uses locale-dependent `localeCompare`. The hash is stable per runtime but not specified.
- **NET-33** PrefixMap is not segment-aware (plain `startsWith`).
- **NET-34** The route for our own node address has `firstHopOptions: []`. Resolving any address that falls through to it throws instead of returning a Reject. Local delivery depends on longer fixed routes (`<nodeAddr>.<localPart>`).
- **NET-35** `ForwardLinkStateUpdate` does not exclude the peer the update was received from. Combined with the heartbeat re-run on own-entry changes (§10.5), echoes cause extra immediate heartbeat rounds.
- **NET-36** `RefreshNodeState` does not verify that the returned link state's nodeId is the one requested. This is harmless (it is signed) but surprising.
- **OER-q1** Fixed-length `octetString(N)` parse does not bounds-check (silently short slice).
- **OER-q2** `octetString().containing(T)` does not check that T exactly fills the octet string, and T may read past it.
- **OER-q3** OID serializer bug (`let currentOffset = (offset = lengthOfLengthPrefix)` writes at an absolute offset). It is harmless here because OIDs are only used via `.constant()`, which pre-serializes standalone.

---

## 20. Compatibility surface

This section lists what an implementation must match to interoperate with original Dassie nodes. [Chapter 7 §2](07-rewrite.md#2-compatibility-contract) decides how much of it the rewrite keeps. The proposal is to keep the identity derivation, and to version the peer protocol rather than stay byte-compatible with v0.

Interoperating with v0 nodes requires matching exactly:
- the OER layouts in §5, including tags 0x80–0x89, the asymmetric octet-wrapping of node info in `peeringRequest`, and length determinants;
- the signature input = PeerNodeInfo bytes;
- node-ID derivation (§2.2);
- the OIDs;
- the HMAC input = content bytes (tag included);
- X25519 conversions (§2.4);
- the HTTP path, method, and both header values;
- response body types per request (§5.4);
- `version = 0`;
- the anonymous-allowed set (senders must use `NONE` for those types, or a strict server may differ; Dassie servers accept either);
- the ILDCP request/response format over `interledgerPacket`;
- per-peer `requestId` uint32 correlation.

Should match behaviorally for network health:
- sequence = ms timestamp, strictly increasing;
- staleness 3 h;
- majority `> N/2`;
- served node list = majority ∪ registered;
- flood forwarding with suppression after 3 copies;
- heartbeats every 5–20 s.

Free to improve, with no wire impact:
- persistence;
- liveness detection;
- removal policies;
- verification in the peering handler (NET-3/4);
- the timer-based 1 h refresh (NET-5);
- route selection beyond `firstHopOptions[0]`;
- segment-aware prefix matching;
- HTTP status checking;
- constant-time MAC compare;
- bounded session-key cache.
