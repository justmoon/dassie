# 4. Interledger: packets, connector, ILDCP, ILP-over-HTTP, STREAM, Open Payments, OER

Source citations refer to the original implementation at commit `e962ab78` on `main` (see [the index](README.md#sources)).

Scope: ILP connector pipeline, ILP-over-HTTP, ILDCP, local ILP endpoints, Open Payments / SPSP, exchange rates,
`lib-protocol-ilp`, `lib-protocol-ildcp`, `lib-protocol-stream`, `lib-protocol-utils` (BTP schemas), `lib-oer`,
and constants in `packages/app-dassie/src/constants/**`.

All paths are relative to the repo root (`packages/...`). "AD" = `packages/app-dassie/src`.
Statements marked **Deviation** differ from the Interledger RFCs or the JS reference implementations.
Statements marked **Open question / known gap** are TODOs, bugs, or unfinished work that a reimplementation has to decide about.

---

## 0. Source map and runtime wiring

| Area | Files |
|---|---|
| Connector core | `AD/ilp-connector/functions/*.ts` (process-packet, process-prepare-packet, calculate-prepare-packet-outcome, process-fulfill-packet, process-reject-packet, schedule-timeout, trigger-early-rejection, trigger-late-rejection, send-packet, send-link-local-packet, get-endpoint-account-path, get-endpoint-ilp-address) |
| Senders (per endpoint type) | `AD/ilp-connector/senders/send-{peer,btp,ildcp,local,ilp-http}-packets.ts` |
| Failures | `AD/ilp-connector/failures/*.ts` |
| Constants | `AD/ilp-connector/constants/expiry-constraints.ts`, `max-packet-amount.ts` |
| State | `AD/ilp-connector/values/pending-packets-map.ts`, topics in `AD/ilp-connector/topics/*` |
| Node identity / address | `AD/ilp-connector/computed/node-id.ts`, `node-ilp-address.ts`, `utils/calculate-node-id.ts` |
| ILP-over-HTTP | `AD/ilp-http/**` |
| ILDCP server | `AD/ildcp-server/**` |
| Local endpoints | `AD/local-ilp/**` |
| Open Payments / SPSP | `AD/open-payments/**`, `AD/utils/resolve-payment-pointer.ts`, `AD/rpc-server/routers/payment.ts` |
| Exchange | `AD/exchange/**` |
| Libraries | `packages/lib-protocol-ilp/src`, `lib-protocol-ildcp/src`, `lib-protocol-stream/src`, `lib-protocol-utils/src/btp.ts`, `lib-oer/src` |

Packet entry points into the connector (all call `ProcessPacket`, `AD/ilp-connector/functions/process-packet.ts`):
- Peer protocol message `interledgerPacket` (`AD/peer-protocol/handlers/interledger-packet.ts`)
- BTP WebSocket `/btp` (`AD/btp-server/register-btp-http-upgrade.ts`)
- ILP-over-HTTP `POST /ilp` and `POST /ilp/callback` (`AD/ilp-http/*`)
- ILDCP server responses (`AD/ildcp-server/handle-ildcp-requests.ts`)
- Local endpoints (`AD/local-ilp/functions/create-local-endpoint.ts`, `AD/ilp-connector/senders/send-local-packets.ts`)
- Link-local sends (`AD/ilp-connector/functions/send-link-local-packet.ts`), used by `AD/peer-protocol/functions/query-uplink-address.ts`

There is no `index.ts` actor for the connector; it is a set of reactor-scoped functions instantiated on first `reactor.use()`.

Startup order (`AD/daemon.ts`, `StartNodeIdentityDependentServicesActor`, only runs once the node identity is configured):
1. `AccountingActor` 2. `BtpServerActor` 3. `IldcpServerActor` 4. `IlpHttpActor` 5. `LocalIlpActor`
6. **`await ExchangeRatesActor`**: blocks until the exchange-rate HTTP fetch completes (see §5)
7. `await LedgersActor` 8. **`await OpenPaymentsServerActor`**: starts the STREAM server, which performs an ILDCP request over a local endpoint
9. `PublicApiServerActor` 10. `await PeerProtocolActor` 11. `RoutingActor`

**Open question / known gap:** if the exchange-rate fetch fails (network error or schema mismatch), the actor throws, so the remaining services (ledgers, open payments, peer protocol, routing) never start. There is no retry.

---

## 1. ILP packet format (`packages/lib-protocol-ilp/src`)

### 1.1 Envelope
`ilpPacketSchema` (`schema.ts`) is an OER SEQUENCE `{ type: UInt8, data: open type }`. The open type is a variable-length OCTET STRING (length prefix, then body). The body schema is selected by `type`:

| type | name | body schema |
|---|---|---|
| 12 | Prepare | `ilpPrepareSchema` |
| 13 | Fulfill | `ilpFulfillSchema` |
| 14 | Reject | `ilpRejectSchema` |

An unknown type gives a ParseFailure ("Unknown value in identifier field"). Bytes left over inside the open type are rejected ("extra bytes inside of open type"). Bytes after the end of the packet are rejected ("non-canonical encoding - additional bytes present..."). Both rules come from the default `allowNoncanonical=false`.

### 1.2 Prepare (type 12)
| field | OER type | constraint |
|---|---|---|
| amount | UInt64, fixed 8 bytes, big-endian | 0..2^64-1 |
| expiresAt | IA5String, **fixed 17 chars, no length prefix** | format below |
| executionCondition | OCTET STRING fixed 32 | |
| destination | IA5String var (`ilpAddressSchema` = 1..1023 chars) | |
| data | OCTET STRING var | 0..32767 bytes |

### 1.3 Fulfill (type 13)
`fulfillment`: OCTET STRING fixed 32. `data`: var OCTET STRING 0..32767.

### 1.4 Reject (type 14)
| field | type |
|---|---|
| code | IA5String fixed 3 |
| triggeredBy | IA5String var 1..1023 (`ilpAddressSchema`) |
| message | UTF8String var, max **8191 characters**. The underlying octet range is 0..32764 bytes; see §2.5 for how characters are counted |
| data | var OCTET STRING 0..32767 |

**Deviation:** `triggeredBy` requires at least 1 char. ILPv4 senders that emit an empty `triggeredBy` produce packets that Dassie cannot parse (parse throws). ILPv4 limits `message` to 8191 *bytes*; Dassie counts JS UTF-16 code units.
**Deviation:** ILP addresses are not validated against the ILPv4 address grammar (scheme prefix, allowed characters `[a-zA-Z0-9_~-]`, segment rules). Any 7-bit ASCII (0x00-0x7F) of length 1..1023 is accepted. The serializer does not check characters at all (§2.5).

### 1.5 API semantics
- `parseIlpPacket(bytes)` **throws** `Error("Failed to parse ILP packet", {cause})` on failure, and returns `{type, data}`.
- `serializeIlpPacket(packet)` **throws** on failure.
- Types: `IlpPreparePacket`, `IlpFulfillPacket`, `IlpRejectPacket`, `IlpPacket` (tagged union `{type, data}`), `IlpResponsePacket` (Fulfill | Reject).
- `IlpEndpoint` (`io.ts`) = `{ sendPacket(prepare) => Promise<Fulfill|Reject>, handlePackets(handler) => unregister() }`. This is the abstraction used by local endpoints and by `lib-protocol-stream`.

### 1.6 Interledger timestamp (`date.ts`)
- Format: `YYYYMMDDHHmmssSSS` in UTC, exactly 17 ASCII digits, with no separators and no `Z`. `INTERLEDGER_TIME_LENGTH = 17`.
- `timestampToInterledgerTime(ms)`: zero-pads each component. Milliseconds = `(ms/1000).toFixed(3).slice(2,5)`.
- `interledgerTimeToTimestamp(s)`: **throws** if the length is not 17. Otherwise it returns `Date.UTC(...)` of the numeric slices. Non-digit characters give `NaN` and do not throw.
- Example: `20171223012140549` = 2017-12-23T01:21:40.549Z.

### 1.7 Error codes (`errors.ts`)
`IlpErrorCode` constants (F = final, T = temporary, R = relative):

| code | constant | human readable (`humanReadableIlpErrors`) |
|---|---|---|
| F00 | F00_BAD_REQUEST | Bad Request |
| F01 | F01_INVALID_PACKET | Invalid Packet |
| F02 | F02_UNREACHABLE | **"Unavailable"** (Deviation: RFC name is "Unreachable") |
| F03 | F03_INVALID_AMOUNT | Invalid Amount |
| F04 | F04_INSUFFICIENT_DESTINATION_AMOUNT | Insufficient Destination Amount |
| F05 | F05_WRONG_CONDITION | Wrong Condition |
| F06 | F06_UNEXPECTED_PAYMENT | Unexpected Payment |
| F07 | F07_CANNOT_RECEIVE | Cannot Receive |
| F08 | F08_AMOUNT_TOO_LARGE | Amount Too Large |
| F99 | F99_APPLICATION_ERROR | Application Error |
| T00 | T00_INTERNAL_ERROR | Internal Error |
| T01 | T01_PEER_UNREACHABLE | Peer Unreachable |
| T02 | T02_PEER_BUSY | Peer Busy |
| T03 | T03_CONNECTOR_BUSY | Connector Busy |
| T04 | T04_INSUFFICIENT_LIQUIDITY | Insufficient Liquidity |
| T05 | T05_RATE_LIMITED | Rate Limited |
| T99 | T99_APPLICATION_ERROR | Application Error |
| R00 | R00_TRANSFER_TIMED_OUT | Transfer Timed Out |
| R01 | R01_INSUFFICIENT_SOURCE_AMOUNT | Insufficient Source Amount |
| R02 | R02_INSUFFICIENT_TIMEOUT | Insufficient Timeout |
| R99 | R99_APPLICATION_ERROR | Application Error |

`getHumanReadableIlpError(code)` returns "Unknown Error" for unknown codes.

### 1.8 F08 data (`amount-too-large.ts`)
`amountTooLargeDataSchema = SEQUENCE { receivedAmount UInt64 (8 bytes), maximumAmount UInt64 (8 bytes) }`, 16 bytes total, matching RFC. Exposed as `serializeAmountTooLargeData` and `parseAmountTooLargeData`. **Note:** the Dassie connector itself sends F08 rejects with **empty data** (§4.5).

### 1.9 Test vectors (`lib-protocol-ilp/src/test/fixtures/packets.ts`)
- Prepare {amount 107, expiresAt "20171223012140549", condition b64 `dOETbcccnl8oO+yDRhy/EmHEAU9y1I+N1lRToLhOfeE=`, destination "example.alice", data b64 `XbND/cQYmPbfQgIykTncJC3Q9VioEbRrKJGP2rN8bLA=`} →
  `DGgAAAAAAAAAazIwMTcxMjIzMDEyMTQwNTQ5dOETbcccnl8oO+yDRhy/EmHEAU9y1I+N1lRToLhOfeENZXhhbXBsZS5hbGljZSBds0P9xBiY9t9CAjKROdwkLdD1WKgRtGsokY/as3xssA==`
  (0x0c, length 0x68=104, 8-byte amount, 17-byte time, 32-byte condition, 0x0d+"example.alice", 0x20+32 data)
- Fulfill {fulfillment `w4ZrSHSczxE7LhXCXSQH+/wUR2/nKWuxvxvNnm5BZlA=`, data `Zz/r14ozso4cDbFMmgYlGgX6gx7U7ZHrzRUOcknC5gA=`} →
  `DUHDhmtIdJzPETsuFcJdJAf7/BRHb+cpa7G/G82ebkFmUCBnP+vXijOyjhwNsUyaBiUaBfqDHtTtkevNFQ5yScLmAA==`
- Reject {code F01, triggeredBy "example.us.bob", message "missing destination. ledger=example.us.", data `AAAABBBB`} →
  `DkFGMDEOZXhhbXBsZS51cy5ib2InbWlzc2luZyBkZXN0aW5hdGlvbi4gbGVkZ2VyPWV4YW1wbGUudXMuBgAAAAQQQQ==`

---

## 2. OER library (`packages/lib-oer/src`)

The library is a zod-like schema builder for X.696 Octet Encoding Rules. Values are **big-endian**. Parsing returns `{success:true, value, length}` or a `ParseFailure`. Serialization returns a `Uint8Array` or a `SerializeFailure`. `parseOrThrow` and `serializeOrThrow` throw `ParseError` / `SerializeError`. A `ParseFailure` message includes a hex excerpt of ±20 bytes around the offset (`PARSE_ERROR_CONTEXT_BYTES = 20`).

Top-level `parse(input, offset=0, {allowNoncanonical=false})`: unless `allowNoncanonical` is set, the encoding must consume exactly `input.length - offset` bytes. Trailing bytes are a failure.

### 2.1 Length determinant (`utils/length-prefix.ts`)
- Short form: one byte 0x00-0x7F = length.
- Long form: `0x80 | n`, followed by n big-endian length bytes.
- Parse rules:
  - n > 6 → failure.
  - The value is accumulated with 32-bit JS ops (`(length << 8 | b) >>> 0`). **Open question / known gap:** n = 5 or 6 overflows and truncates to 32 bits.
  - Bounds: there must be enough bytes for the prefix and for the content it announces. This holds in both forms: short form also checks `offset + length + 1 <= buffer length`.
  - Canonical checks, skipped when `allowNoncanonical`:
    (a) long form with length <= 0x7F is rejected, except in "enumeration mode";
    (b) rejected if `length < 2^((n-1)*8) - 1`. **Bug:** this is off by one. For example `0x82 0x00 0xFF` (255 in 2 bytes) is accepted even though it is non-minimal.
- Serialize: length > 0x7F uses the long form with the minimal byte count. More than 4 length bytes → failure ("length is too large"). Lengths must be safe unsigned integers.

### 2.2 Integers
`integerAsBigint(range?)` (`integer-as-bigint.ts`) chooses an encoding from the range:
- Both bounds given and min >= 0:
  - max <= 255 → UInt8 fixed 1 byte
  - <= 65535 → UInt16 (2 bytes)
  - <= 2^32-1 → UInt32 (4 bytes)
  - <= 2^64-1 → UInt64 (8 bytes)
  - larger → variable unsigned
- Both bounds given and min < 0: the code checks `min <= INT8_MIN && max <= INT8_MAX` → Int8, then the same pattern for Int16/32/64. **Bug (Deviation from X.696 §10):** the comparison should be `min >= INT_MIN`. As written, only ranges whose minimum is <= the type minimum select that type. For example [-10, 10] falls through every case and becomes a *variable-length signed* integer instead of Int8. The predefined `int8Bigint` / `int16Bigint` / ... helpers use exactly the type bounds and are therefore correct.
- Otherwise: min defined and >= 0 → **variable unsigned**; else → **variable signed**.
- Fixed: value range checked on parse and serialize. Out of range → failure.
- Variable unsigned: length determinant + minimal big-endian bytes. 0 encodes as `01 00`. Parse rejects length 0. **Deviation:** parse does *not* reject non-minimal (leading 0x00) encodings.
- Variable signed: length determinant + two's complement minimal bytes (`signedBigintByteLength`). Parse rejects length 0. Non-minimal input is accepted.
- Helpers: `uint8Bigint` .. `uint64Bigint`, `int8Bigint` .. `int64Bigint`. Constants: `UINT8_MAX` .. `UINT64_MAX`, `INT8_MIN`/`MAX` .. `INT64_MIN`/`MAX` as bigint.

`integerAsNumber(range)` (`integer-as-number.ts`): both bounds are required, otherwise it throws. Only fixed 8/16/32-bit encodings are supported; anything larger throws "only supports up to 32 bits". It has the same signed-range bug: signed ranges that are not the exact type range throw TypeError. Helpers: `uint8Number`, `uint16Number`, `uint32Number`, `int8Number`, `int16Number`, `int32Number`.

### 2.3 Octet strings (`octet-string.ts`)
- `octetString(n)` with min == max → fixed, no length prefix. **Bug:** parse has *no bounds check*. It slices whatever is available and reports `length = n`. A truncated input is only caught by the top-level trailing-bytes or length check, or not at all when nested.
- `octetString([min,max])` or `octetString()` → length determinant + bytes. Size range is checked on parse and serialize.
- `.containing(subType)` (OCTET STRING CONTAINING): parses the outer octet string, then parses the subtype at the content offset. **It does not require the subtype to consume the whole content.** Serialization accepts either a raw `Uint8Array` (passed through) or a value for the subtype.
- Serialization also accepts a pre-built `Serializer` (function with `.size`) as the value.

### 2.4 Boolean, Empty, Enumerated
- `boolean()`: 1 byte. Serialize writes 0xFF for true and 0x00 for false. Parse: any non-zero value is true. Values other than 0x00/0xFF are rejected unless `allowNoncanonical`.
- `empty()`: zero bytes.
- `enumerated({name: number})`: parse handles the short form (byte < 0x80) and the long form (via the length-determinant parser in "enumeration mode"). **Bugs:** serialize always writes one raw byte, so values > 127 are mis-encoded and negative values are unsupported. Long-form parse also applies the "enough bytes follow" check to the value.

### 2.5 Strings (`string.ts`)
Strings are built on octet strings. The length range is in *characters*. For UTF-8 the octet range is `[min, max*4]`.
- `utf8String(range)`: TextDecoder("utf8"). Character length = **JS string `.length` (UTF-16 code units)**, checked after decoding.
- `ia5String(range)`: filter allows bytes 0x00-0x7F, so bytes >= 0x80 are rejected on parse. Decoded with TextDecoder("ascii").
- `visibleString(range)`: filter `fill(true, 0x20, 0x7e)`. **Bug:** the end index is exclusive, so 0x7E `~` is *rejected* (VisibleString is 0x20-0x7E). Used for the peer `url` field (`AD/peer-protocol/peer-schema.ts`), so a URL containing `~` fails to parse.
- `numericString(range)`: filter `fill(true,0x30,0x39)` plus 0x20. **Bug:** `9` (0x39) is rejected.
- `printableString(range)`: letters + digits + `" '()+,-./:=?"`.
- `.from(alphabet)`: restricts to a custom ASCII alphabet (code points > 127 throw at schema build time).
- **Bug / Deviation:** serialization never applies the character filter and always UTF-8-encodes. An `ia5String` can therefore emit non-ASCII bytes that its own parser rejects. Serialize checks length in UTF-16 code units.

### 2.6 SEQUENCE (`sequence.ts`)
- Preamble present iff the sequence is extensible or has OPTIONAL/DEFAULT root fields. Layout: bit 7 of byte 0 = extension bit (if extensible), then one presence bit per optional root field in declaration order, MSB first, padded to whole bytes.
- **Bug:** the "unused preamble bits must be zero" check uses `byte << (24 + bitLen%8)`. When the preamble bit count is an exact multiple of 8 it requires the *entire* last byte to be 0, which rejects valid encodings with 8, 16, ... preamble bits.
- `.optional()` → absent means `undefined`. `.default(v)` → absent means `v`. On serialize, a field is omitted iff its key is not in the input object (`key in input`). **Deviation:** CANONICAL-OER requires omitting fields equal to their DEFAULT; the library does not (that test is skipped).
- `.constant(v)` (`OerConstant`): pre-serializes `v`; parse requires a byte-exact match; the field is not required in serialize input.
- `.refine(predicate)`: parse fails if the predicate fails.
- `.unique()`: marks an identifier field for information-object sets.
- Extensions (`.extensible()`, `.extend({...})`): serialize writes the extension bit, a length-prefixed presence bitmap `[unusedBits, bits...]`, then each present extension as a length-prefixed open type. **Bug:** parse reads the presence bits from `uint8Array[offset + floor(i/8)]`, i.e. from the start of the *sequence* (the preamble), not from the bitmap. For the first extension this happens to work because the preamble's extension bit is set. The README lists extensions, optional fields and defaults as unsupported or limited. No protocol in this scope uses extensions or optional fields.
- Information object sets (`information-object/*.ts`): `defineClass({idField: type, payload: openType})` and `defineObjectSet(class, [{...}])`. In a sequence, the identifier field must come before the open-type field. Parsing the identifier selects the object, and an unknown identifier fails ("Unknown value in identifier field"). The open type is encoded as a length-prefixed octet string; its inner value must fill it exactly unless `allowNoncanonical`.
- Note: the information-object state map is per-schema-instance mutable state and is cleared at the start of each sequence parse/serialize. Implementation detail; not wire-relevant.

### 2.7 SEQUENCE OF (`sequence-of.ts`)
Quantity = variable-length unsigned integer (length determinant + minimal big-endian bytes; e.g. 0 elements = `01 00`), followed by the elements. This matches X.696.

### 2.8 CHOICE (`choice.ts`) and tags (`utils/tag.ts`)
- Tag byte: bits 7-6 = class (00 universal, 01 application, 10 context, 11 private). Bits 5-0 = tag number if < 63; otherwise 0x3F followed by the base-128 tag number.
- Automatic tagging (no option has an explicit `.tag()`): context tags 0, 1, 2, ... in declaration order. If any option is explicitly tagged, all options must be. Default class for `.tag(n)` is context. Duplicate tags throw at schema build time. Maximum tag value 0x3FFFFFFF (30 bits).
- Encoding: tag, then the alternative's encoding with no length wrapper. Parsed value is `{type: key, value}`.

### 2.9 Other types
- `objectIdentifier()`: length determinant + base-128 subidentifiers. The first subidentifier is `40*a + b`; on parse, a value >= 80 decodes as `2.(x-80)`. The 0x80 padding byte is rejected.
  **Bug:** the serializer writes subidentifiers starting at absolute offset `lengthOfLengthPrefix` (`let currentOffset = (offset = lengthOfLengthPrefix)`) instead of `offset + lengthOfLengthPrefix`. It is only correct when the OID is at buffer offset 0 or wrapped in `.constant()` (pre-serialized standalone). The only production use is `.constant(...)`, so it works in practice.
- `bitstring(n)`: fixed, ceil(n/8) bytes, MSB first. `bitstring(n,{variableLength:true})`: length + unused-bits byte + bytes. **Bug:** serialize writes `unusedBits = 8 - n%8`, which gives 8 instead of 0 when n is a multiple of 8, and its own parser then rejects it (>7). The value is a boolean array.
- `realAsNumber()` / `float32Number()` / `float64Number()`: raw IEEE-754 binary32/binary64, 4 or 8 bytes. Only base 2 is supported. A narrow mantissa/exponent range selects 32-bit, otherwise 64-bit.
- `captured(subType)`: parses the subtype and also returns the exact consumed `bytes`. Serializes from `{value}` or verbatim `{bytes}`. Used to keep signed blobs byte-exact.

---

## 3. Node identity and ILP addressing

### 3.1 Node ID (`AD/ilp-connector/utils/calculate-node-id.ts`)
```
content = OER SEQUENCE { versionPrefix OID constant 1.3.6.1.4.1.54392.5.1399.2.1, nodePublicKey OCTET STRING (var) }
        = 0d 2b0601040183a878058a770201  ||  <len=0x20> || <32-byte Ed25519 public key>
nodeId  = "d" + base64url(SHA-256(content))[0:32] with trailing "A" characters stripped
```
The ID has variable length, at most 33 chars, and always starts with `d`. Verified against the bootstrap nodes:
public key `QBe5HLbdYFtIpjFTIu_Lz8lkoUJ3fmAvFHfCb9CVwIM` (base64url) → `dNKbwOMCw0DTToPM4tyyVASWxM4NP3MKW`;
`tmS1PB42DEjYi-Ua4RNguhweId9YjT9Kml3uq3iZSuQ` → `dyrjUANXiJ5S6JXxwo7jdKKJjdLtK129U`.

### 3.2 Node ILP address (`AD/ilp-connector/computed/node-ilp-address.ts`, `AD/config/computed/ilp-allocation-scheme.ts`)
`{scheme}.das.{nodeId}`. scheme = `test` if config realm is `"test"` (the default realm), otherwise `g` (realm `"live"`).

### 3.3 Endpoint types (`EndpointInfo` union, `AD/ilp-connector/functions/send-packet.ts`)
| type | fields | unique id (`utils/get-unique-endpoint-id.ts`) | ILP address of the endpoint (`get-endpoint-ilp-address.ts`, used for ILDCP replies and logs) | ledger account path (`get-endpoint-account-path.ts`) |
|---|---|---|---|---|
| `peer` | nodeId, accountPath | `peer:{nodeId}` | `{scheme}.das.{nodeId}` if the node is in the majority node list, else `{ownAddress}.{nodeId}` | `{ledgerId}:assets/interledger/{nodeId}` (ledgerId from the peer's settlement scheme) |
| `ildcp` | — | `ildcp` | `peer.config` | `"unreachable"` (sentinel; ILDCP must not carry money) |
| `btp` | connectionId (number) | `btp:{connectionId}` | `{ownAddress}.{connectionId}` | `{ownerLedgerId}:equity/owner` |
| `local` | hint, localIlpAddressPart | `local:{part}` | `{ownAddress}.{part}` | `{ownerLedgerId}:equity/owner` |
| `http` | id | `http:{id}` | `test.not-implemented` (TODO) | `{ownerLedgerId}:equity/owner` |

Owner ledger ID is hard-coded to `stub+usd` (`AD/accounting/signals/owner-ledger-id.ts`).

**Open question / known gap (BTP addressing mismatch):** the BTP server registers the route `{ownAddress}.{nanoid(6)}` per connection (`AD/btp-server/register-btp-http-upgrade.ts`), but ILDCP tells a BTP client its address is `{ownAddress}.{connectionId}`. A BTP client therefore cannot receive packets at the address it learns.

### 3.4 Routing interaction (`AD/routing/**`, specified in [3 §13](03-peer-network.md))
- `RoutingTableSignal` is a `PrefixMap<IlpAddress, RoutingInfo>`. Lookup returns the longest prefix via plain `startsWith`, with **no segment-boundary check**: `test.das.dabc` matches the key `test.das.dab`. An exact match is checked first.
- `RoutingInfo` is either `{type:"fixed", destination: EndpointInfo}` (ILDCP `peer.config`, local endpoints, BTP connections) or `{type:"peer", firstHopOptions: NodeId[], distance}` (from link-state routing).
- `ResolveIlpAddress`: no match → `NoRouteFoundFailure`, which the connector maps to F02. A `peer` route takes `firstHopOptions[0]`. If that node is not in state `peered` it **throws** `Error("Next hop node is not actually peered ...")` instead of rejecting.
- **Open question / known gap:** route calculation inserts the node's *own* address `{scheme}.das.{ownId}` with an empty `firstHopOptions`. A packet for `{ownAddress}.something` that matches no local endpoint or BTP route therefore resolves to that entry, and `ResolveIlpAddress` throws. No F02 reject is sent; upstream gets R00 after the 5 s internal timeout.

---

## 4. ILP connector pipeline (`AD/ilp-connector/**`)

### 4.1 ProcessPacket
`processPacket({sourceEndpointInfo, serializedPacket, parsedPacket, requestId})` dispatches on `parsedPacket.type` to the Prepare, Fulfill or Reject handler.
`requestId: number | string`. Peers and BTP use uint32 numbers. Local endpoints use nanoid strings. ILP-over-HTTP uses the header string for `/ilp` and `Number(header)` for `/ilp/callback`.

Callers parse incoming bytes with `parseIlpPacket`, which **throws**. **Deviation:** a malformed ILP packet never gets an F01 reject. `InvalidPacketIlpFailure` (F01) exists but is never constructed. The error propagates to the transport: the BTP message handler on authenticated connections is not try/caught, peer-message handling and HTTP handlers behave as their framework does (HTTP → 500).

### 4.2 Prepare processing (`process-prepare-packet.ts`)
1. `outgoingRequestId = Math.trunc(Math.random() * 0xFFFFFFFF)`, a uniform integer in [0, 2^32-2]. **No uniqueness check:** a collision silently overwrites the pending entry.
2. Build a `PreparedIlpPacketEvent` {sourceEndpointInfo, serializedPacket (incoming bytes), parsedPacket (incoming prepare data), incomingRequestId, outgoingRequestId, pendingTransfers (mutable array), timeoutAbort (AbortController)}.
3. `outcome = predeterminedOutcome ?? calculatePreparePacketOutcome(...)` (§4.3).
4. Failure → `triggerEarlyRejection` (§4.5) and stop. The packet is not placed in the pending map.
5. Success:
   - `pendingPacketsMap.set("{uniqueId(destination)}#{outgoingRequestId}", event)`.
   - `scheduleTimeout` (§4.6).
   - Append the created ledger transfers to `pendingTransfers`.
   - Build the outgoing Prepare: `amount = outgoingAmount`, `expiresAt = outgoingExpiry`, and `executionCondition`, `destination`, `data` **copied unchanged**. Serialize it.
   - `sendPacket` to the destination sender (§4.8).
   - Emit `PreparedIlpPacketTopic` (consumed only by the debug UI).

### 4.3 Outcome calculation (`calculate-prepare-packet-outcome.ts`), in this exact order
1. **Route:** `resolveIlpAddress(destination)`. Failure → **F02** "No route found for destination".
2. **Expiry parse:** `interledgerTimeToTimestamp(expiresAt)`. The code checks for a Failure return, but the function only ever throws (length is guaranteed 17 by the schema). Non-digit input gives NaN, which passes the next check, and the outgoing packet serialization then throws.
3. **Timeout check:** `now = clock.now()`, `delta = incomingExpiry - now - ILP_MESSAGE_WINDOW(1000)`. If `delta < 1000` → **R02** "Insufficient timeout". Equivalently, at least 2000 ms must remain until expiry.
4. **Outgoing expiry:** `now + min(delta, MAXIMUM_HOLD_TIME=30000)`, i.e. the incoming expiry minus 1 s, capped at 30 s from now.
5. **Amount handling, only if `amount > 0`.** Zero-amount packets skip conversion and ledger entirely, so `outgoingAmount = 0`.
   - `amount > MAX_PACKET_AMOUNT (100_000_000)` → **F08** "Packet amount exceeds maximum allowed amount". The limit is in incoming (source-ledger) units regardless of currency; at scale 9 that is 0.1 unit. **Deviation:** F08 `data` is empty (RFC: SHOULD contain receivedAmount/maximumAmount).
   - `outgoingAmount = calculateOutgoingAmount(srcAccountPath, dstAccountPath, amount)` (§5).
   - `applyPacketPrepareToLedger(src, dst, in, out)` (`AD/accounting/functions/apply-interledger-packet.ts`):
     - same ledger: one pending transfer debit `src` → credit `dst`, amount `in`. Asserts `in === out`.
     - different ledgers: two pending transfers: debit `src` → credit `{srcLedger}:revenue/fx` (amount `in`), and debit `{dstLedger}:expenses/fx` → credit `dst` (amount `out`).
   - Each transfer goes through `ledgerStore.createTransfer`:
     - `InvalidAccountFailure` → **T00** "Missing internal ledger account"
     - `ExceedsCreditsFailure` / `ExceedsDebitsFailure` → **T04** "Insufficient liquidity"
   - **Bug:** if the 2nd transfer fails after the 1st succeeded, the 1st pending transfer is never voided (pending balance leak).
   - **Edge case:** `createTransfer` *asserts* (throws) when debit and credit are the same account or on different ledgers. A non-zero packet between two owner-equity endpoints (local↔local, local↔BTP, BTP↔HTTP, ...) maps both sides to `stub+usd:equity/owner` and throws inside the pipeline, so no response is sent.
   - **Edge case:** a non-zero packet to or from the ILDCP endpoint uses the account path `"unreachable"`, which has no colon. `getLedgerIdFromPath` asserts and throws.
6. Return `{destinationEndpointInfo, outgoingAmount, outgoingExpiry, transfers}`.

Failure objects are preallocated singletons with fixed messages (listed above).

### 4.4 Fulfill / Reject processing (`process-fulfill-packet.ts`, `process-reject-packet.ts`)
- Key: `"{uniqueId(sourceEndpointInfo)}#{requestId}"`, where the source is the endpoint the response came from (= the prepare's destination). Not found → warn and **drop** (no further action).
- Fulfill: `SHA-256(fulfillment)` must equal the stored *incoming* prepare's `executionCondition`. On mismatch, warn and **drop**: the entry stays pending and the 5 s timeout later rejects upstream with R00. **Deviation:** common connector behaviour is to turn an invalid fulfillment into a reject immediately (e.g. F05 / wrong condition). Dassie relies on the timeout instead.
- On match (fulfill) or any reject:
  1. delete the pending entry
  2. abort the timeout
  3. `postPendingTransfer` (fulfill) or `voidPendingTransfer` (reject) for each transfer
  4. forward the **original serialized response bytes** (no re-serialization, no modification of reject `triggeredBy`/data) to `prepare.sourceEndpointInfo`
  5. emit `ResolvedIlpPacketTopic`
- **Deviation:** the response is not checked against the packet's expiry. A fulfill that arrives before the 5 s internal timeout is honoured even if the outgoing `expiresAt` has passed, and one that arrives after the timeout is dropped even though the outgoing `expiresAt` may still be in the future.

### 4.5 Rejections
- **Early rejection** (`trigger-early-rejection.ts`, used for outcome failures): Reject `{code: failure.errorCode, triggeredBy: ownNodeIlpAddress, message: failure.message, data: empty}` is sent via `sendPacket` to `prepare.sourceEndpointInfo`, using the incoming requestId. No pending entry or ledger effects exist at this point.
- **Late rejection** (`trigger-late-rejection.ts`, used on timeout): builds Reject `{code, triggeredBy: ownAddress, message, data: empty}` and feeds it into `processRejectPacket` as if it came from the downstream endpoint (`sourceEndpointInfo = destination`, `requestId = outgoingRequestId`). This voids the transfers and forwards the reject upstream.

### 4.6 Timeout (`schedule-timeout.ts`)
- **Fixed 5000 ms** after forwarding (`delayWithAbortSignal(clock, 5000, abortSignal)`), independent of `expiresAt`. On expiry → late rejection **R00** "Packet timed out".
- **Open question / known gap:** the outgoing expiry can be up to 30 s, but Dassie gives up after 5 s. If the next hop fulfills between 5 s and the expiry, Dassie has already voided and rejected upstream. The fulfill is then dropped ("did not match any pending request"), which can lose money to the next hop.

### 4.7 Pending state
`PendingPacketsMap = Map<"{endpointUniqueId}#{requestId}", PreparedIlpPacketEvent>` is in-memory only. It is not persisted, so in-flight packets are lost on restart.

### 4.8 Senders (`senders/*.ts`; `SendPacket` picks the sender by `destinationEndpointInfo.type`)
A parameter object with a `prepare` property is a result (Fulfill/Reject). Otherwise it is a prepare.

**peer** (`send-peer-packets.ts`)
- Prepare and result are both sent with `sendPeerMessage` (fire-and-forget) as a peer message `interledgerPacket`: `{ signed: OCTET STRING CONTAINING { requestId UInt32, packet OCTET STRING } }` (`AD/peer-protocol/peer-schema.ts`, choice tag 2). The peer-message response type is `empty()`.
- Prepare carries `outgoingRequestId`. Result carries the incoming requestId (asserted to be a number).
- Incoming: `HandleInterledgerPacket` drops the message if it is unauthenticated or the sender is not in state `peered`. The account path comes from the peer's settlement scheme → ledger ID.

**btp** (`send-btp-packets.ts`)
- Prepare: BTP envelope `{messageType: 6 (MESSAGE), requestId: outgoingRequestId, message: BtpMessage{protocolData:[{protocolName:"ilp", contentType:0, data: ILP bytes}]}}`. The amount==0 and amount>0 branches are identical: **BTP TRANSFER (7) is never used for sending**.
- Result: `{messageType: 1 (RESPONSE), requestId: incomingRequestId (asserted number), message: BtpMessage{ilp protocolData}}`.
- Incoming (`register-btp-http-upgrade.ts`): on MESSAGE, TRANSFER or RESPONSE, the first protocolData entry named `ilp` is processed with `requestId = envelope.requestId`. TRANSFER's amount field is ignored. ERROR (2) is ignored.

**ildcp** (`send-ildcp-packets.ts`)
- Prepare → `HandleIldcpRequestsActor.handle({sourceIlpAddress: getEndpointIlpAddress(source), requestId: outgoingRequestId})`.
- Result → throws (ILDCP never sends prepares).

**local** (`send-local-packets.ts`)
- Prepare: looks up the endpoint's `prepareCallback`. If absent it throws inside `tell`, which logs the error; no reject is sent and the packet times out with R00 (TODO in code). Otherwise it awaits `callback(prepare.data)` and feeds the result into `processPacket` as coming from that local endpoint with `requestId = outgoingRequestId`.
- Result: resolves the outstanding request's deferred (`OutstandingRequestsStore[incomingRequestId]`) with the parsed packet. Unknown id → throws "Invalid request ID".

**http** (`send-ilp-http-packets.ts`)
- Prepare → `sendAsyncPrepare` to the **hard-coded URL `http://localhost:3002`** ("TODO: Select URL based on id").
- Result → looks up `IncomingRequestIdMap[incomingRequestId]` (throws if missing) and POSTs to its callback URL. See §8.

### 4.9 Link-local packets (`send-link-local-packet.ts`)
`sendLinkLocalPacket({packet, peerId})` exists only for zero-amount packets (amount != 0 → throws).
- Registers a nanoid requestId in `OutstandingRequestsStore`.
- Source endpoint is the pseudo-local endpoint `{type:"local", hint:"Link Local Sender", localIlpAddressPart:"link-local"}`. This endpoint is not routable.
- Calls `processPreparePacket` with a predetermined outcome that skips routing, the expiry check and the ledger: destination `{type:"peer", nodeId: peerId, accountPath:"invalid"}`, `outgoingAmount 0`, `outgoingExpiry = packet.expiresAt` (unchanged), no transfers.
- Returns the response packet promise. Used by `QueryUplinkAddress` to send ILDCP to a peer: `{amount 0, destination "peer.config", executionCondition ILDCP_CONDITION, expiresAt = Date.now() + 30000, data empty}`. Uses wall-clock `Date.now()`, not the reactor clock.

### 4.10 Topics
- `PreparedIlpPacketTopic`: emitted after forwarding.
- `ResolvedIlpPacketTopic`: emitted after a response is forwarded.

The only consumer is the debug RPC (`AD/rpc-server/routers/debug.ts`).

---

## 5. Exchange rates and amount conversion (`AD/exchange/**`)

### 5.1 Currencies and ledgers
- `CURRENCIES` (`exchange/constants/currencies.ts`): `USD {code "USD", scale 9}` and `XRP {code "XRP", scale 9}`.
- `LEDGERS` (`AD/accounting/constants/ledgers.ts`): `stub+usd → USD`, `xrpl+xrp → XRP`, `xrpl-testnet+xrp → XRP`.

### 5.2 Rate loading (`load-exchange-rates.ts`)
- `GET {exchangeRateUrl}`. Default `https://api.coinbase.com/v2/exchange-rates`. Configurable only through the DB scalar `config.exchange_rate_url`; there is no setter action in `DatabaseConfigStore` (`AD/config/database-config.ts`).
- Response schema (zod): `{ data: { currency: string, rates: Record<string,string> } }`. A rate is "units of X per 1 base-currency unit" (Coinbase semantics). Any extra fields are ignored.
- Normalization: `rate_X = BigInt(Math.round(Number(rateString) * 10^15))`. `INTERNAL_PRECISION = 15`. The intermediate is a JS double, so precision is limited to ~15-16 significant digits.
- Stored in `ExchangeRatesSignal` = `{baseCurrency, rates}`. Initial value `{baseCurrency:"USD", rates:{}}`.
- Loaded **once** when the actor starts. It re-runs only if `exchangeRateUrl` changes, which never happens at runtime. **No periodic refresh, no staleness handling, no retry.**
- **No spread or fee is applied anywhere** in the exchange code; conversion is mid-market. "Spread" does not exist in this codebase.

### 5.3 Conversion (`functions/convert.ts`, `calculate-outgoing-amount.ts`)
- If source and destination currency are the same (same currency *object*, i.e. same code), `outgoing = incoming`.
- Otherwise, with integer (truncating) BigInt division in each step:
```
P = INTERNAL_PRECISION = 15
base   = floor( amount * 10^(2P - fromScale) / rate[from] )
target = floor( base * rate[to] / 10^(2P - toScale) )
```
- A missing rate for either currency **throws** `Error("No rate available for currency X")` inside the prepare pipeline. No reject is sent; upstream times out.
- **Open question:** with the Coinbase base currency USD, `rates["USD"]` must also be present for USD conversions to work. Whether the API includes the base currency in `rates` is not checked.
- **Open question / known gap:** `BigInt(10 ** k)` is computed through a JS double, which is exact only for k <= 22. With `2P - scale` that requires scale >= 8. All current currencies use scale 9 (k = 21), so it is exact today. A reimplementation should use exact integer powers.
- `convertCurrencyAmounts.toBaseFrom` / `.fromBaseTo` are also used by `AD/accounting/computed/total-owner-balance.ts`.

---

## 6. ILDCP

### 6.1 Library (`packages/lib-protocol-ildcp/src`)
- `ILDCP_ADDRESS = "peer.config"`.
- `ILDCP_CONDITION` = SHA-256(32 zero bytes) = `66687aadf862bd776c8fc18b8e9f8e20089714856ee233b3902a591d0d5f2925` (base64 `Zmh6rfhivXdsj8GLjp+OIAiXFIVu4jOzkCpZHQ1fKSU=`). The fulfillment is 32 zero bytes.
- Response schema `ildcpResponseSchema = SEQUENCE { address IA5String var (unbounded), assetScale UInt8, assetCode IA5String var }`, matching RFC 31 (address, scale, code).
  - `parseIldcpResponse` returns the value or a `ParseFailure`.
  - `serializeIldcpResponse` throws on failure.
- Fixture: `{address "test.alice", assetScale 9, assetCode "XRP"}` ↔ `CnRlc3QuYWxpY2UJA1hSUA==`.
- No request schema: an ILDCP request is a Prepare with amount 0, destination `peer.config`, `ILDCP_CONDITION`, and empty data.

### 6.2 Server (`AD/ildcp-server/handle-ildcp-requests.ts`)
- On start, inserts the fixed route `peer.config → {type:"ildcp"}` and removes it on cleanup. The route matches by prefix, so `peer.config.*` also hits it.
- Requests go through the full prepare pipeline (§4.3): route, expiry (needs >= 2 s left), amount 0 → no ledger.
- `handle({sourceIlpAddress, requestId})` replies with Fulfill `{fulfillment: 32 zero bytes, data: ILDCP{address: sourceIlpAddress, assetScale: 9, assetCode: "XRP"}}`. The reply is injected into `processPacket` as coming from the ILDCP endpoint with the outgoing requestId. The connector verifies `SHA-256(0^32) == prepare.executionCondition`, so a request with any other condition is dropped and gets R00 after 5 s. The request's data and amount are not inspected.
- **Open question / known gap:** asset code and scale are hard-coded to XRP/9 for every client, including local endpoints whose ledger is `stub+usd` (USD) and peers on USD ledgers. They are inconsistent with the wallet endpoint (USD/9) and incoming payments (USD/2) in §9.
- Address handed out = `getEndpointIlpAddress(source)` (§3.3).

### 6.3 Client use
- Peer uplink address query (§4.9).
- STREAM servers and clients (`lib-protocol-stream/src/server/query-ildcp.ts`): Prepare with expiry `now + 30000` and the ILDCP condition. Non-fulfill or parse failure → `ConfigurationDetectionFailure`.
- The upstream `ilp-protocol-stream` (used by Open Payments) also performs ILDCP through the plugin.

---

## 7. Local ILP endpoints (`AD/local-ilp/**`)

`CreateLocalEndpoint({scope, hint})` returns a `LocalIlpEndpoint` (`IlpEndpoint` plus `id`):
- `id` = `nanoid()` (21 chars, alphabet `A-Za-z0-9_-`), regenerated until unique among local endpoints. Registered in `LocalEndpointsStore` as `{type:"local", hint, localIlpAddressPart:id}` and unregistered when `scope` is disposed.
- `RouteLocalEndpointsActor` keeps the fixed routes `{ownAddress}.{id} → local endpoint` in the routing table, adding and removing them on store changes and on cleanup. The endpoint's ILP address is `{ownAddress}.{id}`.
- `sendPacket(prepare)`:
  1. allocate a unique nanoid requestId in `OutstandingRequestsStore`
  2. `processPacket` as a Prepare from this endpoint
  3. return a promise resolved by the connector's local `sendResult`
  - **No timeout of its own:** it relies on the connector's 5 s R00 when the packet was forwarded. An early-reject path also resolves it.
- `handlePackets(handler)`: at most one handler (a second one throws). Returns an unregister function.

Store shapes:
- `LocalEndpointsStore: Map<localIlpAddressPart, {type, hint, localIlpAddressPart, prepareCallback?}>`
- `OutstandingRequestsStore: Map<requestId, resolve>`

Both are in-memory.

---

## 8. ILP-over-HTTP (`AD/ilp-http/**`)

Constant: `ILP_OVER_HTTP_CONTENT_TYPE = "application/octet-stream"`. Routes are registered on the HTTPS router (`HttpsRouter`). Max body 2 MiB (`DEFAULT_MAX_BODY_SIZE`, else 413).

### 8.1 Inbound `POST /ilp` (`register-ilp-http-handler.ts`)
- `Accept` header must be **exactly** `application/octet-stream`, else 406 "Not Acceptable, expected application/octet-stream".
- `Content-Type` must be **exactly** `application/octet-stream`, else 415.
- `Prefer` must be exactly `respond-async`, else 400 "This implementation only supports asynchronous mode, expected 'Prefer: respond-async'". **Deviation:** the synchronous mode (default in RFC 35) is not supported.
- `Request-Id` is required (400 "Missing required header Request-Id"); its format (UUID) is not validated.
- `Callback-Url` is required (400 "Missing required header Callback-Url").
- **No authentication** ("TODO: Authentication, manage ILP-HTTP connections"). Source endpoint = `{type:"http", id:"TODO"}` for all clients, so every HTTP client shares one endpoint identity.
- Stores `IncomingRequestIdMap[requestId] = {requestId, callbackUrl}`. Entries are never removed (leak).
- `processPacket(...)` with `requestId` = header string. The body is parsed with `parseIlpPacket`, which throws on bad input → 500.
- Response: **202**, empty body, `Content-Type: application/octet-stream`.
- The result (fulfill/reject) is later POSTed by `sendAsyncResult` to `callbackUrl` with headers `accept: application/octet-stream`, `content-type: application/octet-stream`, `request-id: <original Request-Id>`, body = ILP response bytes. The callback's HTTP response is ignored, with no retry.

### 8.2 Inbound `POST /ilp/callback` (`register-ilp-http-callback-handler.ts`)
- Same exact Accept and Content-Type assertions. `Request-Id` is required. No authentication.
- `processPacket` with source `{type:"http", id:"TODO"}` and `requestId = Number(header)`.
- Response: 200, empty body, content type octet-stream.

### 8.3 Outbound prepare (`functions/send-async-prepare.ts`)
POST to the hard-coded `http://localhost:3002` with headers:
- `accept: application/octet-stream`
- `content-type: application/octet-stream`
- `prefer: respond-async`
- `request-id: String(outgoingRequestId)` (a decimal integer, not a UUID)
- `callback-url: https://{hostname}/ilp/callback}`. **Bug:** the trailing `}` is a stray character.
- `authorization: Bearer test-USD` (hard-coded)

The HTTP response status is ignored.

**Open question / known gap:** nothing ever creates a route to an `http` endpoint, so outbound ILP-over-HTTP is effectively dead code. The `http` endpoint's ILP address is `test.not-implemented`.

---

## 9. Open Payments, SPSP and payment pointers (`AD/open-payments/**`)

**Important:** the application uses the **upstream `ilp-protocol-stream` npm package (pinned `2.7.2-alpha.2`)** for receiving and sending, not Dassie's own `lib-protocol-stream`. The latter is used only by the website "stream toy" (`packages/app-website/src/components/stream-toy/**`) and its own tests.

### 9.1 Plugin adapter (`functions/create-plugin.ts`)
Creates an ilp-plugin-compatible object on top of a local endpoint (hint `"plugin"`):
- `connect` / `disconnect` / `isConnected` toggle a boolean.
- `sendData(buf)`: throws if not connected. Parses the buffer, requires a Prepare (else throws), calls `localEndpoint.sendPacket`, and returns the serialized response.
- `registerDataHandler(h)`: allows only one handler. Wraps `h` as the endpoint handler: serialize the prepare, call `h`, parse the result; a Prepare result throws.
- `deregisterDataHandler`: throws if no handler is registered.

### 9.2 STREAM server (`stream-server.ts`)
- `createServer({plugin})` from `ilp-protocol-stream`. **No `serverSecret` is supplied**, so the upstream library generates a random one per process. All previously issued credentials become invalid after a restart.
- The server address comes from ILDCP through the local endpoint: `{ownAddress}.{localEndpointNanoid}`. Credentials take the form `{serverAddress}.{token}`.
- On each new stream: `setReceiveMax(Infinity)`. Money events are **only logged**. Nothing is credited to incoming payments; the connector ledger is the only accounting (owner equity).

### 9.3 HTTP endpoints (all on the HTTPS router, `cors` middleware = `Access-Control-Allow-Origin: *` only, no OPTIONS / preflight handler)

**`GET /.well-known/pay`** (`handle-wallet-requests.ts`): `200 application/json`
```json
{ "id": "https://{hostname}/.well-known/pay", "assetCode": "USD", "assetScale": 9,
  "authServer": "https://not-available", "resourceServer": "https://{hostname}" }
```
- The URL uses `hostname` without a port.
- **Deviation:** this is an Open-Payments-style wallet address document. **SPSP (RFC 9) is not implemented:** there is no `application/spsp4+json` content negotiation and no `destination_account` / `shared_secret` response. `spspResultSchema` in `AD/utils/resolve-payment-pointer.ts` is defined but unused.

**`GET /incoming-payments`**: `200 {"result": [IncomingPayment, ...]}` for **all** rows. No pagination, no auth, no `walletAddress` filter. **Deviation** from the Open Payments list format, which uses `pagination` and `result`.

**`POST /incoming-payments`**: body (zod, else 400 "Invalid HTTP request body"):
`{ walletAddress: string, incomingAmount: {value: string, assetCode: string, assetScale: int 0..255}, expiresAt?: string, metadata?: object }`.
- `incomingAmount` is **required** here (Open Payments makes it optional).
- Inserts `incoming_payment {id: nanoid(), ledger: ownerLedgerId, total_amount: BigInt(value), metadata: JSON.stringify(metadata ?? {})}`. `BigInt` of a non-integer string throws → 500.
- `walletAddress`, `expiresAt`, `assetCode` and `assetScale` from the request are **ignored** and not stored.
- Response: **200** (Open Payments specifies 201) with the formatted payment. No GNAP / authorization.

**IncomingPayment JSON** (`utils/format-incoming-payment.ts`), generated on every read:
```json
{ "id": "{url}/incoming-payments/{id}", "walletAddress": "{url}/.well-known/pay", "completed": false,
  "incomingAmount": {"value": "<total_amount>", "assetCode": "USD", "assetScale": 2},
  "receivedAmount": {"value": "0", "assetCode": "USD", "assetScale": 2},
  "methods": [{"type": "ilp", "ilpAddress": "<stream destinationAccount>", "sharedSecret": "<base64url>"}],
  "createdAt": "<now ISO>", "updatedAt": "<now ISO>" }
```
- `url` = `https://{hostname}` plus `:{httpsPort}` when the port is not 443. This can differ from the wallet `id`, which has no port.
- Credentials come from `streamServer.generateAddressAndSecret({connectionTag: payment.id})` and are **freshly generated on every format call**: list and create each produce new credentials.
- **Open question / known gap:** the following values are hard-coded and do not reflect state: `assetScale 2` and `assetCode "USD"` (inconsistent with the wallet's scale 9 and ILDCP's XRP/9), `receivedAmount` "0", `completed` false, `createdAt`/`updatedAt` = now.
- `GET /incoming-payments/{id}` is **not implemented**. Outgoing-payment and quote resources are not exposed over HTTP. There is no auth server (`authServer: "https://not-available"`).

### 9.4 Payment pointers (`AD/utils/resolve-payment-pointer.ts`)
- `paymentPointerToUrl(p)`: replace a leading `$` with `https://`, then parse as a URL. If the pathname is `/`, use `/.well-known/pay`. Input without `$` is used as a URL directly.
- `resolvePaymentPointer(p)`: `GET` the URL with no Accept header and parse the JSON with `walletSchema = {id, publicName?, assetCode, assetScale 0..255, authServer, resourceServer}`.
- RPC `payment.resolvePaymentPointer({paymentPointer})` (protected) returns the wallet.

### 9.5 Making a payment (`functions/make-payment.ts`; RPC `payment.createPayment({paymentId, paymentPointer, amount: string})`, protected; TODOs: validate paymentId length and uniqueness)
1. Resolve the pointer: `GET` with `Accept: application/json`. Non-2xx → throw. Parse with `walletSchema`.
2. `POST {resourceServer}/incoming-payments` with `Accept: application/json`, `Content-Type: application/json`, body `{walletAddress: wallet.id, incomingAmount: {value: amount, assetCode: wallet.assetCode, assetScale: wallet.assetScale}}`. **No authorization header (no GNAP).** Non-2xx → throw.
3. Parse the response with `incomingPaymentSchema`:
   - `{id, walletAddress, completed, incomingAmount?, receivedAmount, expiresAt?, metadata?, createdAt, updatedAt, methods}`.
   - `methods` is pre-filtered to drop well-formed non-`ilp` entries, then must be a non-empty array of `{type:"ilp", ilpAddress (max 1023), sharedSecret}`.
   - The first method is used.
4. Quote: **TODO**, assumes 1:1 (`sourceAmount = amount`, even though the comment says `amount` is in destination units).
5. Insert `outgoing_payment {id: paymentId, destination: incomingPayment.id, ledger: ownerLedgerId, total_amount: sourceAmount, metadata: "{}"}`. The row is never updated afterwards (no status, no sent amount).
6. Create a plugin on a new scope and `createConnection({plugin, destinationAccount: ilpAddress, sharedSecret: Buffer.from(secret, "base64")})`. Node's base64 decoder also accepts base64url. Then `createStream()` and `setSendMax(sourceAmount)`. Upstream defaults apply (probing, slippage).
7. On each `outgoing_money` event: `sentAmount += amount`. If `sentAmount + amount >= sourceAmount`, call `connection.end()` and then dispose the scope. **Bug:** the newest chunk is counted twice, so the connection can end before the full amount is sent. The RPC returns as soon as the connection is set up, without waiting for completion. There is no failure handling ("TODO: Make sure this is cleaned up even in the case of failure").

### 9.6 Persistence (`AD/open-payments/tables/*`, registered in `AD/database/schema.ts`)
| table | column | type | constraints |
|---|---|---|---|
| `incoming_payment` | `id` | TEXT | NOT NULL PRIMARY KEY |
| | `ledger` | TEXT | NOT NULL |
| | `total_amount` | INTEGER | NOT NULL |
| | `metadata` | TEXT (JSON) | NOT NULL |
| `outgoing_payment` | `id` | TEXT | NOT NULL PRIMARY KEY |
| | `destination` | TEXT (incoming payment URL) | NOT NULL |
| | `ledger` | TEXT (LedgerId) | NOT NULL |
| | `total_amount` | INTEGER | NOT NULL |
| | `metadata` | TEXT | NOT NULL |

Related DB scalar: `config.exchange_rate_url` (TEXT, `AD/config/schemas/database-scalars.ts`).

---

## 10. BTP schemas (`packages/lib-protocol-utils/src/btp.ts`)
- `BtpType`: Response = 1, Error = 2, Message = 6, Transfer = 7. Types 3 (Prepare), 4 (Fulfill) and 5 (Reject) from BTP v1 are not defined.
- `BtpContentType`: ApplicationOctetStream = 0, TextPlainUtf8 = 1, ApplicationJson = 2.
- `protocolDataSchema = SEQUENCE OF { protocolName IA5String var, contentType UInt8, data OCTET STRING var }`
- `btpMessageSchema = { protocolData }`
- `btpTransferSchema = { amount UInt64, protocolData }`
- `btpErrorSchema = { code IA5String(3), name IA5String, triggeredAt IA5String (GeneralizedTime text, not validated), data OCTET STRING 0..8192, protocolData }`
- `btpEnvelopeSchema = { messageType UInt8, requestId UInt32, message OCTET STRING var }`. Matches RFC 23 BTP v2.
- Helpers: `generateBtpMessage`, `generateBtpTransfer`, `generateBtpResponse`, `generateBtpError` (each returns serialized envelope bytes).

BTP auth handshake details (first message must be MESSAGE with protocolData[0] = `auth` (octet-stream, empty) and an `auth_token` entry (text/plain) that matches a stored BTP token, answered by RESPONSE with empty protocolData) are specified in [3 §14](03-peer-network.md) (`AD/btp-server/register-btp-http-upgrade.ts`).

---

## 11. STREAM (`packages/lib-protocol-stream/src`)

Own implementation, modelled on `ilp-protocol-stream`. Public exports (`index.ts`):
- `createServer`, `createClient`, `createTestEnvironment`
- `StreamPolicy`, `DEFAULT_POLICY`
- frame schemas, `ErrorCode`, `FrameType`, `streamPacketSchema`, `streamFrameSchema`
- `getPskEnvironment`, `PskEnvironment`, `multiplyByRatio`, `Ratio`

Everything is `bigint` (no uint64 truncation). Expected failures are returned as `Failure` values rather than thrown.

### 11.1 Context (`context/context.ts`, `context/policy.ts`)
`StreamProtocolContext = {crypto, logger, endpoint: IlpEndpoint, scope, clock, policy}`.

`DEFAULT_POLICY`:
| field | value | meaning |
|---|---|---|
| minimumConcurrency | 1 | AIMD floor |
| maximumConcurrency | 100 | AIMD ceiling |
| concurrencyIncreaseIncrement | 1 | additive increase per fulfill |
| concurrencyDecreaseFactor | 0.5 | multiplicative decrease per reject of a fulfillable packet |
| closeTimeout | 30000 ms | how long to keep retrying ConnectionClose |
| closeRetries | 3 | maximum close attempts |
| deMinimisAmount | 1000n | remaining amounts <= this are treated as zero |

### 11.2 Packet format (`packets/schema.ts`)
```
StreamPacket ::= SEQUENCE {
  version     UInt8 constant 1          -- parse fails on any other value
  packetType  UInt8                     -- 12 prepare, 13 fulfill, 14 reject (ILP type it accompanies)
  sequence    VarUInt (bigint, unbounded)
  amount      VarUInt (bigint, unbounded)  -- prepareAmount / amount received
  frames      SEQUENCE OF Frame
}
Frame ::= SEQUENCE { type UInt8, data VarOctetString (open type containing the frame body) }
```
| type | name | body |
|---|---|---|
| 0x01 | ConnectionClose | errorCode UInt8, errorMessage UTF8 var |
| 0x02 | ConnectionNewAddress | sourceAccount IA5 var 1..1023 |
| 0x03 | ConnectionMaxData | maxOffset VarUInt |
| 0x04 | ConnectionDataBlocked | maxOffset VarUInt |
| 0x05 | ConnectionMaxStreamId | maxStreamId VarUInt |
| 0x06 | ConnectionStreamIdBlocked | maxStreamId VarUInt |
| 0x07 | ConnectionAssetDetails | sourceAssetCode UTF8 var, sourceAssetScale UInt8 |
| 0x10 | StreamClose | streamId VarUInt, errorCode UInt8, errorMessage UTF8 var |
| 0x11 | StreamMoney | streamId VarUInt, shares VarUInt |
| 0x12 | StreamMaxMoney | streamId, receiveMax, totalReceived (VarUInt each) |
| 0x13 | StreamMoneyBlocked | streamId, sendMax, totalSent |
| 0x14 | StreamData | streamId, offset VarUInt, data VarOctetString |
| 0x15 | StreamMaxData | streamId, maxOffset |
| 0x16 | StreamDataBlocked | streamId, maxOffset |
| 0x17 | StreamReceipt | streamId, receipt VarOctetString |

`ErrorCode`: NoError = 1, InternalError = 2, EndpointBusy = 3, FlowControlError = 4, StreamIdError = 5, StreamStateError = 6, FrameFormatError = 7, ProtocolViolation = 8, ApplicationError = 9.

**Deviations (wire level):**
- An **unknown frame type makes the entire packet fail to parse**. RFC 29 and the reference implementation skip unknown frames.
- **Trailing bytes after the frames** (allowed by RFC 29 for future extensions) make parsing fail, because of the non-canonical check.
- Values larger than uint64 are decoded exactly as bigints. RFC and reference saturate some fields to uint64 max; see the fixture `frame:stream_max_money:receive_max:greater_than_max_uint64`, which decodes `receiveMax = 2^64`.
- `ConnectionNewAddress` with an empty address cannot be parsed (min length 1).
- Non-minimal VarUInt encodings are accepted.

Fixture example: an empty prepare packet `{v1, type 12, seq 0, amount 0, frames []}` = `01 0c 01 00 01 00 01 00` = `AQwBAAEAAQA=`. Full RFC-derived fixture set (sequence/amount 0, max_js, max_uint64; every frame type) is in `lib-protocol-stream/src/test/fixtures/packets.ts`, e.g. ConnectionClose(1,"fail") = `AQwBAAEAAQEBBgEEZmFpbA==` and ConnectionNewAddress("example.alice") = `AQwBAAEAAQECDg1leGFtcGxlLmFsaWNl`.

`packets/parser.ts`: `deserializePacket` and `serializePacket` throw on failure.
`packets/expiry.ts`: every STREAM prepare expires at `clock.now() + DEFAULT_PACKET_TIMEOUT (30000 ms)`.

### 11.3 Crypto (`crypto/functions.ts`), matching RFC 29
```
encryptionKey   = HMAC-SHA256(sharedSecret, "ilp_stream_encryption")
fulfillmentKey  = HMAC-SHA256(sharedSecret, "ilp_stream_fulfillment")
fulfillment     = HMAC-SHA256(fulfillmentKey, encryptedStreamData)   -- over the *ciphertext* (ILP data field)
condition       = SHA-256(fulfillment)
encrypt(pt)     = iv(12 random bytes) || tag(16) || AES-256-GCM(encryptionKey, iv, pt)   -- no AAD
decrypt(ct)     = split iv = ct[0:12], tag = ct[12:28], body = ct[28:]; failure → DecryptionFailure
```
- `generateRandomCondition`: 32 random bytes (`CONDITION_LENGTH`), used for unfulfillable packets.
- `generateTokenNonce`: 18 random bytes (`TOKEN_NONCE_LENGTH`). Unused here; a vestige of ilp-protocol-stream server tokens.
- Test vectors (secret = UTF-8 `"foo"`, `crypto.test.ts`):
  - encryption key `cdf386314fee63e10e50213ab4ddd6844efb6c5fcdea94b4c123a63201003040`
  - fulfillment key `961f27189089a2cbdf51e3da534bb1c572b78cdbd5d98fb98afb75187d4cba1b`
  - fulfillment of `"bar"` = `c1803ed8ee0868f07da893d9184fd9973570d5ea7185b64483d3f886483bb92c`
  - ciphertext `HweWFgwWTh/cwwbKHqRLNeDtuakR4E8ltFSDyQ7v31ImmnQ=` decrypts to `"foo bar"`
- **Open question:** the Node crypto adapter (`packages/lib-reactive-io/src/node/crypto.ts`) maps only errors named `OperationError` to `DECRYPTION_FAILURE`. Node's GCM auth failure throws a plain `Error`, so an unauthenticated ciphertext may throw instead of returning a failure. The browser WebCrypto adapter does throw `OperationError`.

### 11.4 Server (`server/*.ts`)
- `createServer({context})`:
  - Runs ILDCP via `context.endpoint` (failure → `ConfigurationDetectionFailure`).
  - Creates a child scope.
  - Registers `endpoint.handlePackets(handler)`.
  - On dispose, closes every active connection (§11.10).
  - Returns `Server {generateCredentials(), topics.connection}`.
- `generateCredentials()`:
  - `uniqueId` = base64url of 4 random bytes (6 chars, no padding), unique among `activeCredentials` and `activeConnections`.
  - `secret` = 32 random bytes (`SHARED_SECRET_LENGTH`).
  - Stores `activeCredentials[uniqueId] = secret` and returns `{destination: "{ildcpAddress}.{uniqueId}", secret}`.
  - **Deviation:** credentials are kept in memory per server instance (not derived statelessly from a server secret as in ilp-protocol-stream), never expire, are never removed, and are lost on restart. Connection tags are not supported.
- Packet handler:
  1. If `destination` does not `startsWith(ildcpAddress)` → Reject F02 "Incorrect destination" (`triggeredBy` = server address, empty data).
  2. `localId = destination.slice(address.length + 1)`. This is everything after the address plus separator; extra segments are **not** stripped, so `addr.id.extra` is looked up as `id.extra`.
  3. Existing connection → handle.
  4. Else if `activeCredentials` has `localId` → create a connection state (side `server`, `ourAddress = {address}.{localId}`), store it in `activeConnections`, emit `connection`, then handle.
  5. Else → Reject F02 "Unknown STREAM credentials".

### 11.5 Client (`connection/client.ts`)
- `createClient({context, credentials:{destination, secret}})`: ILDCP for its own address. Connection state: side `client`, `ourAddress` = the ILDCP address (no suffix), `remoteAddress` = destination.
- Registers `endpoint.handlePackets` so incoming packets go to this single connection. Cleanup closes the connection.
- Returns a `Connection`.

### 11.6 Connection state (`connection/initial-state.ts`, `state.ts`)
| field | initial value | notes |
|---|---|---|
| `nextSequence` | 0 | incremented for every prepare sent, including probes and closes |
| `nextStreamId` | client 1, server 2 | step 2: clients odd, servers even |
| `maxStreamId` (local limit on remote-opened streams) | **client 2, server 1** | **Open question / known gap:** never increased and never advertised (`setMaxStreamId` is never called), so a server accepts only stream 1 from a client and a client accepts only stream 2 |
| `remoteMaxStreamId` | — | stored from ConnectionMaxStreamId, **never enforced** when creating streams |
| `maxPacketAmount` | `UINT64_MAX` | `DEFAULT_MAXIMUM_PACKET_AMOUNT` |
| `concurrency` | 1 | `INITIAL_CONCURRENCY`; may be fractional |
| `exchangeRate` | undefined | must be set before sending money |
| `remoteKnowsAddress` | server true, client false | |
| `isClosed` | false | |
| `streams`, `closedStreams` | empty | |

Topics: `stream`, `closed`.

### 11.7 Receiving a packet (`connection/handle-packet.ts`)
1. Decrypt `prepare.data`. Failure → Reject **F06** "Failed to decrypt incoming STREAM packet data" (unencrypted, empty data).
2. Parse the STREAM packet. Failure → Reject **F06** "Failed to parse incoming STREAM packet data".
3. `fulfillment = HMAC(fulfillmentKey, prepare.data)`. Fulfillable iff `SHA-256(fulfillment) == executionCondition`.
4. Create a response builder carrying `sequence` = request sequence and `amount` = **prepare.amount (actually received)**.
5. `packetType != 12` → reject F06 "Wrong packet type". The reject carries an encrypted response.
6. For each frame, `handleControlFrame` (§11.8).
7. `streamPacket.amount > prepare.amount` → reject **F04** "Packet did not deliver enough money". **Deviation:** RFC 29 / ilp-protocol-stream use F99 for "received less than minimum".
8. For each StreamMoney frame with `shares > 0`:
   - Look up the stream, or open a new remote stream (§11.9).
   - Invalid stream id → queue a StreamClose(errorCode, reason) in the response and skip it.
   - Otherwise collect `{streamId, shares, receivedAmount, receiveMaximum}` and add to `totalShares`.
9. `allocateAmounts(totalShares, prepare.amount, list)` (§11.11). Failure → add StreamMaxMoney{receiveMax, totalReceived} for every listed stream and reject F99 "Amount exceeds maximum receive amount".
10. For each allocation: `stream.receivedAmount += amount` and emit `money(amount)`.
11. `tryFulfill()`: if not fulfillable → reject F99 "Condition did not match". This is the path that answers exchange-rate probes. Otherwise Fulfill `{fulfillment, data: encrypted response}`.

**Bugs / edge cases:**
- Step 10 runs **before** the fulfillability check. An *unfulfillable* packet carrying StreamMoney frames credits `receivedAmount` and emits `money` events although the packet is rejected. Probes carry no money frames, so this does not occur in normal use.
- A fulfillable packet with `amount > 0` but no StreamMoney frames is fulfilled and the money is attributed to no stream.
- Packets for a connection already marked closed are still processed (there is no `isClosed` check).

Response packet (`create-response.ts`): `{version 1, packetType 13|14, sequence = request sequence, amount = prepare.amount, frames}`. Frames in this order:
1. ConnectionAssetDetails (if requested, see ConnectionNewAddress)
2. ConnectionMaxStreamId (never set)
3. StreamMaxMoney entries (skipped for streams that also get a StreamClose)
4. StreamClose entries (first reason per stream wins)

It is encrypted and placed in Fulfill `data` or Reject `data`; Reject `triggeredBy` = configuration address. **Note:** no StreamMaxMoney is sent on successful receipt, only on allocation failure.

### 11.8 Control frames (`handle-control-frames.ts`), used both on incoming prepares and on decrypted responses
- **ConnectionNewAddress:** if the address differs from the current one, reset `concurrency` to `minimumConcurrency` (anti-amplification). Ask the response builder to include our ConnectionAssetDetails (ILDCP code/scale). Set `remoteAddress`.
- **ConnectionMaxStreamId:** store `remoteMaxStreamId`.
- **ConnectionAssetDetails:** store `remoteAssetDetails`.
- **ConnectionClose:** `markConnectionClosed` closes all streams, sets `isClosed`, and emits `closed`. The server does not remove the connection from `activeConnections`.
- **StreamMaxMoney:** for an unknown stream, warn and skip. Otherwise:
  - `remoteReceivedAmount = max(current, totalReceived)`
  - `remoteReceiveMaximum = receiveMax` if the current value is still `UINT64_MAX` or `receiveMax` is greater. The first advertisement can lower it; later ones can only raise it.
  - emit `remoteMoney{receivedAmount, receiveMaximum}`
- **Ignored:**
  - StreamClose from the remote: remote stream closes are not processed. **Gap.**
  - StreamMoneyBlocked
  - All data-related frames (StreamData, StreamMaxData, StreamDataBlocked, ConnectionMaxData, ConnectionDataBlocked): **data streams are not implemented.**
  - ConnectionStreamIdBlocked
  - StreamReceipt: receipts are not implemented.

### 11.9 Streams (`stream/*.ts`, `connection/handle-new-stream.ts`, `connection/create-stream.ts`)
StreamState fields:
- `sendMaximum = 0`, `sendHoldAmount = 0`, `sentAmount = 0`
- `receiveMaximum = 0`, `receivedAmount = 0`
- `remoteReceivedAmount = 0`, `remoteReceiveMaximum = UINT64_MAX`
- `isClosed`, `isRemoteClosed`
- topics `money`, `moneySent`, `remoteMoney`, `closed`

Opening and limits:
- Local `createStream`: id = `nextStreamId`, then `nextStreamId += 2`.
- Remote-opened stream validation, in order:
  1. a server rejects even ids ("client stream IDs must be odd")
  2. a client rejects odd ids
  3. id 0 is rejected
  4. id > `maxStreamId` is rejected

  Codes: ProtocolViolation (8) for the first three, StreamIdError (5) for the last. On success, emit `stream`.
- The receiver must call `stream.addReceiveAmount(n)` (`receiveMaximum += n`); otherwise all money to the stream is refused.

`Stream` API:
- `send({amount, timeout=30000})` → `sendAndAwait`
- `close()` → local close: moves the stream from `streams` to `closedStreams` and emits `closed`
- `addSendAmount(n)`: requires `remoteAddress` and `exchangeRate`, else `NoRemoteAddressFailure` / `NoExchangeRateFailure`; then `sendMaximum += n`
- `addReceiveAmount(n)`
- `topics`

**Bug / gap:** because `closeStream` removes the stream from `streams`, and `buildPacket` only iterates `streams`, the "send StreamClose to remote" branch is unreachable. **StreamClose frames are never sent.**

### 11.10 Sending (`connection/send-*.ts`, `stream/send-*.ts`)

**Desired send for a stream:** `min(sendMaximum - sendHoldAmount - sentAmount, remoteReceiveMaximum - remoteReceivedAmount)`.

**`hasWorkToDo(state)`:** false if the connection is closed. Otherwise true if any stream that is not (closed and not remote-closed) has a desired send amount **> deMinimisAmount (1000)**.
**Edge case:** a stream whose remaining amount is <= 1000 is never sent. `send({amount: 500})` immediately ends with `SEND_INCOMPLETE_FAILURE`.

**`sendUntilDone`:** a single loop per connection (`sendLoopPromise`). Each iteration:
- If there is no work and no packets are in flight → exit.
- If the number in flight >= `concurrency`, or there is no work → wait for any in-flight packet or for `sendLoopWaker`, then re-check.
- Otherwise start `sendOnce` without awaiting it and add it to the pool.

The loop also stops issuing new packets once the connection scope is disposed. **Note:** `sendLoopWaker` is created but nothing ever resolves it; the loop only wakes on in-flight completions.

**`sendOnce`** (requires an exchange rate, else throws). `buildPacket`:
1. If the connection is closed → only ConnectionClose(NoError, "").
2. If `!remoteKnowsAddress` → add ConnectionNewAddress(ourAddress) and ConnectionAssetDetails(ILDCP code/scale). Mark `remoteKnowsAddress = true` once a decryptable STREAM response arrives.
3. For each stream, in insertion order: `actualSend = min(packet capacity left, desired)`. If > 0, `prepareSend` (hold) and add StreamMoney(streamId, shares = actualSend). On fulfill → `fulfillSend` (`sentAmount += a`, `hold -= a`, emit `moneySent`). On reject or failure → `rejectSend` (`hold -= a`).

Packet: `sourceAmount = Σ shares`, `destinationAmount (STREAM prepareAmount) = floor(sourceAmount * rate[0] / rate[1])`. It is always fulfillable.

**`sendPacket`** (`connection/send-packet.ts`):
- Requires `remoteAddress`, else `NoRemoteAddressFailure`.
- Serializes `{v1, type 12, sequence = nextSequence++, amount = destinationAmount, frames}` and encrypts it.
- Sends the ILP Prepare `{destination: remoteAddress, amount: sourceAmount, expiresAt: now + 30 s, executionCondition: fulfillable ? SHA256(HMAC(fulfillmentKey, ciphertext)) : random32, data: ciphertext}`.
- F08 handling: parse the F08 data. If it is valid and `maximumAmount < receivedAmount`, set `maxPacketAmount = floor(sourceAmount * maximumAmount / receivedAmount)`. Otherwise ignore it (with a log line).
- **AIMD congestion control:**
  - on Fulfill: `concurrency = min(concurrency + 1, 100)`
  - on Reject of a *fulfillable* packet: `concurrency = max(concurrency * 0.5, 1)` (intentionally not rounded)
  - rejected probes do not change concurrency
- Decrypts the response `data` (Fulfill or Reject). On success it parses the response and applies its control frames. The STREAM response's `sequence` and `packetType` are **not validated** against the request (**Deviation**).
- Returns `{ilp, stream | DecryptionFailure | ParseFailure}`.

**No retry or abort policy:**
- Rejects of any class (F/T/R) simply release the hold, and the loop re-sends while work remains.
- There is no backoff and no "give up on final errors". **Deviation** from ilp-protocol-stream, which closes on F-errors.
- `maxPacketAmount` is only ever lowered (via F08).

**`sendAndAwait({amount, timeout=30000})`:**
1. `addSendAmount`; the target is the resulting `sendMaximum`.
2. Resolve success when `sentAmount >= target`.
3. **Also resolve success** when a `remoteMoney` event shows `receiveMaximum - receivedAmount < deMinimisAmount`, even if not everything was sent.
4. On timeout → `SendFailure("Sending timed out")`. If the send loop ends first → `SendFailure("Send loop exited without completing send")`. The first outcome wins.

**`Connection#pay({sourceAmountLimit})`** (`connection/make-payment.ts`):
1. Creates a new stream.
2. `sendAndAwait(amount = sourceAmountLimit)` with the default 30 s timeout.
3. Returns the failure if any.
4. Otherwise closes the stream locally (no StreamClose is sent, see §11.9) and returns `undefined`.
- Money already sent is not refunded on failure.
- An exchange rate must have been set first.

**Exchange rate API** (`connection/set-exchange-rate.ts`, `measure-exchange-rate.ts`):
- `setExchangeRate([num, den])`: negative → `InvalidExchangeRateFailure("Exchange rate must be non-negative")`; zero numerator or denominator → `"Exchange rate must be non-zero"`. The rate is destination units per source unit and acts as the **minimum** rate the receiver enforces via prepareAmount.
- `dangerouslyIgnoreExchangeRate()`: rate = [0, 1], so prepareAmount is always 0.
- `measureExchangeRate()`:
  - **Not done automatically** on connect. **Deviation:** ilp-protocol-stream always probes; Dassie does it only on request.
  - Sends five **unfulfillable** probes **concurrently** with source amounts `[1, 1e3, 1e6, 1e9, 1e12]` and destinationAmount 0.
  - Each successful probe yields rate `[response.stream.amount, sourceAmount]`, i.e. the amount the receiver reports in its encrypted reject.
  - Picks the successful probe with the largest `log10(amount)`.
  - If no probe succeeded, or only the 1-unit probe did (digits = 0) → `ProbingUnsuccessfulFailure`.
  - Reduces the rate by its GCD.
  - A probe rejected with F08 adjusts `maxPacketAmount`; its response is undecryptable, so the probe counts as failed.
- `dangerouslyMeasureExchangeRate()`: measure, then `setExchangeRate`. A measured rate of 0 gives `InvalidExchangeRateFailure`.

### 11.11 Amount allocation across streams (`math/allocate-amounts.ts`)
Input:
- `totalShares`
- `totalAmount` (= ILP amount received)
- a list of `{streamId, shares, receivedAmount, receiveMaximum}`

```
remaining = totalAmount; ratio = [remaining, totalShares]; alloc[i] = 0
loop:
  last = remaining; totalShares = 0
  for each stream i:
    byShare  = floor(shares_i * ratio[0] / ratio[1])
    canRecv  = receiveMaximum_i - receivedAmount_i - alloc[i]
    a        = min(byShare, canRecv, remaining)
    remaining -= a; alloc[i] += a
    if a < canRecv: totalShares += shares_i      -- stream still has capacity
  if remaining == 0: return alloc
  if totalShares == 0: return AmountExceedsMaximumReceiveAmountFailure
  ratio = (remaining == last) ? [remaining, 1] : [remaining, totalShares]
```
Empty list → `[]` (success, nothing allocated).
**Deviation (documented in README):** excess from capped streams is redistributed to streams with capacity, and rounding dust is assigned to some stream. ilp-protocol-stream rejects the whole packet or drops the dust instead.

`multiplyByRatio(v, [n, d]) = floor(v * n / d)`.

### 11.12 Closing (`connection/close.ts`, `mark-closed.ts`)
`closeConnection(state)`:
- No-op if already closed.
- Otherwise `markConnectionClosed` (close all streams locally, set `isClosed`, emit `closed`) and then `sendClose`.

`sendClose`:
- Up to `closeRetries` (3) attempts, stopping once `now - start > closeTimeout` (30 s).
- Each attempt is a *fulfillable* zero-amount packet with ConnectionClose(NoError = 1, "").
- Returns `true` on Fulfill, `false` after exhausting attempts, or `NoRemoteAddressFailure`.
- The calling client or server cleanup ignores failures.

`Connection#closeAfterDone()` = `sendUntilDone` then `closeConnection`.

### 11.13 Summary of README-declared differences from ilp-protocol-stream
1. No automatic exchange-rate probing (explicit API only).
2. Over-allocated money is redistributed instead of rejecting the packet; rounding dust is allocated.
3. Multiple packets in flight (AIMD) versus one at a time.
4. BigInt everywhere, no uint64 truncation.
5. Failures returned as values.

### 11.14 Test environment (`test/mocks/test-environment.ts`, exported)
A simulated mini-connector:
- Each context gets the address `test.{name}`.
- ILDCP replies `{test.{name}, 9, XRP}`.
- Optional simulations: latency (log-normal jitter), maxPacketsInFlight (→ T03), packet loss (→ T00 "Random packet loss"), maxPacketAmount (→ F08 with proper data), per-endpoint `unitsPerToken` for FX (amount scaled by `in * unitsIn / unitsOut`), F02 for unknown destinations.
- Balance signals per context.

`test/mocks/compatibility.ts` and `compatibility.test.ts` verify interop with upstream `ilp-protocol-stream`: a Dassie client measures the rate and sends money to an upstream server, and an upstream client sends to a Dassie server.

---

## 12. Constants (complete list for this area)

| Constant | Value | File |
|---|---|---|
| ILP_MESSAGE_WINDOW | 1000 ms | `AD/ilp-connector/constants/expiry-constraints.ts` |
| MAXIMUM_HOLD_TIME | 30000 ms | same |
| (internal pending timeout) | 5000 ms, hard-coded | `AD/ilp-connector/functions/schedule-timeout.ts` |
| MAX_PACKET_AMOUNT | 100_000_000n | `AD/ilp-connector/constants/max-packet-amount.ts` |
| outgoing request id range | `trunc(random * 0xFFFFFFFF)` | `process-prepare-packet.ts` |
| INTERNAL_PRECISION | 15 | `AD/exchange/constants/internal-precision.ts` |
| CURRENCIES | USD scale 9, XRP scale 9 | `AD/exchange/constants/currencies.ts` |
| default exchangeRateUrl | `https://api.coinbase.com/v2/exchange-rates` | `AD/config/database-config.ts` |
| ILP_OVER_HTTP_CONTENT_TYPE | `application/octet-stream` | `AD/ilp-http/constants/content-type.ts` |
| outbound ILP-HTTP URL | `http://localhost:3002`, auth `Bearer test-USD` | `send-ilp-http-packets.ts`, `send-async-prepare.ts` |
| PAYMENT_POINTER_PATH | `/.well-known/pay` | `AD/open-payments/constants/payment-pointer.ts` |
| ILDCP_ADDRESS | `peer.config` | `lib-protocol-ildcp/src/address.ts` |
| ILDCP_CONDITION | SHA-256(0^32) = `66687aad…5f2925` | `lib-protocol-ildcp/src/condition.ts` |
| ILDCP server response asset | XRP / 9 | `AD/ildcp-server/handle-ildcp-requests.ts` |
| INTERLEDGER_TIME_LENGTH | 17 | `lib-protocol-ilp/src/date.ts` |
| IlpType | Prepare 12, Fulfill 13, Reject 14 | `lib-protocol-ilp/src/schema.ts` |
| ILP limits | address 1..1023, data 0..32767, message <= 8191 chars | same |
| BtpType / BtpContentType | 1, 2, 6, 7 / 0, 1, 2 | `lib-protocol-utils/src/btp.ts` |
| STREAM DEFAULT_PACKET_TIMEOUT | 30000 ms | `lib-protocol-stream/src/packets/expiry.ts` |
| STREAM DEFAULT_POLICY | see §11.1 | `context/policy.ts` |
| INITIAL_CONCURRENCY | 1 | `connection/initial-state.ts` |
| DEFAULT_MAXIMUM_PACKET_AMOUNT | 2^64-1 | same |
| SHARED_SECRET_LENGTH | 32 bytes | `server/generate-credentials.ts` |
| credential ID_LENGTH | 4 random bytes → 6 base64url chars | `server/generate-unique-id.ts` |
| TOKEN_NONCE_LENGTH | 18 | `crypto/functions.ts` |
| CONDITION_LENGTH | 32 | same |
| HMAC / AEAD | hmac-sha256 / aes-256-gcm (iv 12, tag 16) | same, `lib-reactive-io/src/*/crypto.ts` |
| key-derivation strings | `ilp_stream_encryption`, `ilp_stream_fulfillment` | same |
| PROBE_PACKET_AMOUNTS | 1, 1e3, 1e6, 1e9, 1e12 | `connection/measure-exchange-rate.ts` |
| sendAndAwait DEFAULT_TIMEOUT | 30000 ms | `stream/send-and-await.ts` |
| lib-oer PARSE_ERROR_CONTEXT_BYTES | 20 | `lib-oer/src/utils/failures.ts` |
| lib-oer max length-of-length | parse 6 / serialize 4 | `lib-oer/src/utils/length-prefix.ts` |
| HTTP DEFAULT_MAX_BODY_SIZE | 2 MiB | `lib-http-server/src/constants.ts` |

`packages/app-dassie/src/constants/**` (shared app constants):
- `general.ts`: `APP_NAME = "dassie-node"`, `VALID_REALMS = ["test","live"]`, `EMPTY_UINT8ARRAY = new Uint8Array(0)`, `DEV_SECURITY_TOKEN_LENGTH = 64`.
- `object-identifiers.ts`:
  - `dassieObjectIdentifierPrefix = 1.3.6.1.4.1.54392.5.1399`
  - `dassiePeerProtocolId = …1399.1`, `dassiePeerSignedHelloId = …1399.1.1`, `dassiePeerNodeInfoId = …1399.1.2`
  - `dassieNodeFingerprintId = …1399.2`, `dassieNodeFingerprintV1Id = …1399.2.1` (used for the node ID, §3.1)
- `seed-paths.ts`: `SEED_PATH_NODE = "node"` (Ed25519 node key), `SEED_PATH_NODE_LOGIN = "node/login"`, `SEED_PATH_DEV_SESSION = "dev/session"`, `SEED_PATH_SETTLEMENT_MODULE = "node/settlement-module"`.
- `bootstrap-nodes.ts`:
  - `dNKbwOMCw0DTToPM4tyyVASWxM4NP3MKW` @ `https://test1.dassie.xyz`, publicKey `QBe5HLbdYFtIpjFTIu_Lz8lkoUJ3fmAvFHfCb9CVwIM`
  - `dyrjUANXiJ5S6JXxwo7jdKKJjdLtK129U` @ `https://test2.dassie.xyz`, publicKey `tmS1PB42DEjYi-Ua4RNguhweId9YjT9Kml3uq3iZSuQ`
- `cookie-name.ts`: `SESSION_COOKIE_NAME = "__Host-Dassie-Session"`.
- `ui-query-parameter-names.ts`: `QUERY_PARAMETER_DEVELOPMENT_SESSION = "_DASSIE_DEV_SESSION"`.

---

## 13. Persistence summary
- **Persisted (SQLite):** `incoming_payment` and `outgoing_payment` tables (§9.6), and the scalar `config.exchange_rate_url`. Ledger transfers are persisted only once *posted* ([5 §2.5](05-accounting-and-settlement.md)).
- **In-memory only:**
  - connector pending packets and timeouts
  - local endpoint registry and outstanding requests
  - ILP-HTTP request-id map
  - routing table
  - exchange rates (re-fetched each start)
  - STREAM servers, credentials and connections (both lib and upstream; the upstream server secret is random per process)
  - pending ledger transfers

---

## 14. Consolidated known gaps / open questions (reimplementation decisions)
- **ILP-1** Fixed 5 s internal timeout, independent of expiry (up to 30 s). Late fulfills are dropped after an upstream R00, which risks loss (§4.6).
- **ILP-2** Invalid fulfillment is dropped (no reject), which leads to R00 after 5 s (§4.4).
- **ILP-3** Malformed ILP packets throw instead of returning F01. Unknown own-prefix destinations and unpeered next hops throw instead of returning F02 (§3.4, §4.1).
- **ILP-4** F08 rejects carry no amount data (§4.3).
- **ILP-5** The first FX ledger transfer leaks when the second fails. Same-account transfers (owner↔owner, e.g. local↔BTP with amount > 0) assert/throw (§4.3).
- **ILP-6** Missing exchange rate throws in the pipeline. Rates are never refreshed. No spread or fees. Imprecise `10**k` for scales < 8 (§5).
- **ILP-7** ILDCP always answers XRP/9. BTP clients are told `{addr}.{connectionId}` but routed at `{addr}.{nanoid6}` (§3.3, §6.2).
- **ILP-8** Request IDs are random 32-bit values with no collision check (§4.2).
- **ILP-9** ILP-over-HTTP: async only, no auth, single shared `http:TODO` endpoint, outbound URL and token hard-coded, `callback-url` has a stray `}`, request map leaks, no outbound routes exist (§8).
- **ILP-10** Open Payments: no GNAP auth, no SPSP, no `GET /incoming-payments/{id}`, hard-coded USD/2 and received 0, new credentials on every read, no linking of received STREAM money to payments, port mismatch between wallet `id` and payment URLs, the outgoing-payment completion check double-counts, no quoting (1:1), no failure cleanup (§9).
- **ILP-11** STREAM (lib):
  - unknown frames and trailing data are rejected
  - StreamClose is never sent or processed
  - max stream ID is fixed (server accepts only stream 1) and not advertised
  - remote max stream ID is not enforced
  - no data streams or receipts
  - no retry/backoff and no final-error handling
  - amounts <= 1000 (deMinimis) are never sent
  - money is credited before the fulfillability check
  - receipt-amount check uses F04 instead of F99
  - response sequence/type not validated
  - credentials are in-memory, never expire, and have no connection tags
  - `sendLoopWaker` is never triggered (§11)
- **ILP-12** lib-oer bugs listed in §2:
  - signed-range width selection
  - fixed octet string bounds
  - preamble zero-bit check for multiples of 8
  - extension bitmap parsing
  - OID serialization at a non-zero offset
  - variable bitstring unused-bits value
  - VisibleString rejects `~`
  - NumericString rejects `9`
  - string serialize skips the character filter
  - length-prefix 5-6 byte overflow and off-by-one canonical check
  - enumerated values > 127 mis-encoded
  - non-minimal integers accepted

  A wire-compatible reimplementation should implement correct X.696 for the encodings Dassie actually emits, and decide whether to mirror the parse-side quirks: notably peer-protocol `url` VisibleString (rejects `~`), ILP `triggeredBy` min length 1, and STREAM unknown frames.
- **ILP-13** Node crypto adapter may throw instead of returning `DecryptionFailure` on GCM auth failure (§11.3).
- **ILP-14** If the exchange-rate fetch fails at startup, the rest of the node services never start (§0).
