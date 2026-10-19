# ILP

The Interledger Protocol v4 packets that carry every payment, and the two protocols that ride on them in Dassie: ILDCP, by which a client learns its address and asset, and BTP 2.0, by which ILP clients connect over a WebSocket. `dassie-ilp` encodes and decodes them; what the node does with them is the connector's.

**Paths:** `crates/dassie-ilp/`
**Read with:** [Encoding](encoding.md), [Connector](connector.md), [STREAM](stream.md)

All of it is byte for byte the standards (RFC 27 ILPv4, RFC 15 addresses, RFC 31 ILDCP, RFC 23 BTP 2.0) and the original Dassie, so that existing Interledger software interoperates. Decoding borrows from the input and never allocates; the crate is `no_std` and reads no clock: callers pass the time in ([D16](../decisions/D16-engineering-style-pragmatic-tigerstyle-and-deterministic-simulation.md)).

## Packets

An ILP packet is a type octet followed by its body as an open type (a length determinant, then the body), in canonical OER ([Encoding](encoding.md)).

```
IlpPacket ::= SEQUENCE {
  type  UINT8,            -- 12 Prepare, 13 Fulfill, 14 Reject
  data  OPEN TYPE }       -- LEN || body

Prepare ::= SEQUENCE {
  amount             UINT64,                         -- 8 octets
  expiresAt          IA5String (SIZE(17)),           -- Interledger timestamp, no length
  executionCondition OCTET STRING (SIZE(32)),        -- no length
  destination        Address,                        -- IA5String (SIZE(1..1023))
  data               OCTET STRING (SIZE(0..32767)) }

Fulfill ::= SEQUENCE {
  fulfillment OCTET STRING (SIZE(32)),
  data        OCTET STRING (SIZE(0..32767)) }

Reject ::= SEQUENCE {
  code        IA5String (SIZE(3)),                   -- no length
  triggeredBy Address or empty,                      -- IA5String (SIZE(0..1023))
  message     UTF8String (SIZE(0..8191)),            -- counted in octets
  data        OCTET STRING (SIZE(0..32767)) }
```

- **The condition** is `SHA-256(fulfillment)`. A Fulfill satisfies a Prepare when the SHA-256 of its fulfillment equals the Prepare's execution condition.
- **`triggeredBy` may be empty**, as ILPv4 allows; the original refused such packets (ILP-3). An empty field reads as no address; a non-empty one must be a valid address.
- **`message`** is at most 8191 octets of UTF-8, as RFC 27 says. The original counted UTF-16 code units.
- **The largest packet** is a Prepare with the longest destination and data: 33,857 octets (`MAX_PACKET_LENGTH`), which callers size buffers by once.
- **Decoding refuses**, besides any OER error:
  - an unknown type octet (`UnknownType`), or a type the context does not allow: a Prepare where only a Prepare fits, a Fulfill or Reject where only a response fits;
  - bytes left inside the body after its last field, and bytes after the packet;
  - a destination or `triggeredBy` that is not a valid address (`InvalidAddress`), or an empty destination;
  - an expiry that is not a valid timestamp (`InvalidTimestamp`);
  - a code with an octet above `0x7F`.
- **Encoding a header alone.** A packet's encoding up to its data can be written separately, so a caller can place the data right after it, as STREAM does to encrypt in place.

## Parsing never throws

Parsing returns a `ParseError { offset, kind }` for any malformed input, never a panic (ILP-3). Its kinds are an OER error, an unknown type, an invalid address and an invalid timestamp. The connector answers any of them with F01 ([Connector](connector.md), [D13](../decisions/D13-connector-timing-and-packet-limits.md)). Every decoder has a "never panics on arbitrary input" property test.

## Addresses

An ILP address follows the RFC 15 grammar:

```
address = scheme 1*( "." segment )
scheme  = "g" / "private" / "example" / "peer" / "self" / "test" / "test1" / "test2" / "test3" / "local"
segment = 1*( ALPHA / DIGIT / "_" / "~" / "-" )
```

- An address is 1 to 1023 octets, has at least one segment after the scheme, and no empty segment.
- The original accepted any 7-bit string. Following the standard keeps malformed destinations out of the routing table.
- **Prefixes compare by whole segments** (NET-33): `test.alice` is a prefix of `test.alice` and `test.alice.bob`, not of `test.alicebob`. `strip_prefix` gives the part after a proper ancestor and its dot. Routing and the STREAM server match addresses this way.
- Addresses written as literals in the code are checked when the crate builds.

## Interledger timestamps

`expiresAt` is the time in UTC as exactly 17 ASCII digits, `YYYYMMDDHHmmssSSS`, with no separators and no zone: `20171223012140549` is 2017-12-23T01:21:40.549Z.

- **Parsing** requires every character to be a digit and the date to exist: a year from 1970 to 9999, a real month and day (leap years counted), hours below 24, minutes and seconds below 60. Anything else is `InvalidTimestamp`. (The original turned non-digits into NaN.) A packet from before 1970 has expired anyway.
- **Formatting** zero-pads each field. A time beyond 9999-12-31T23:59:59.999Z is written as that.
- **Time in Dassie's protocol code** is a `Timestamp`, milliseconds since the Unix epoch. Adding to one saturates at that maximum.

## Error codes

A reject code is three characters, a class letter and two digits by convention. The class says what retrying does: **F** final (it will not help), **T** temporary (later may succeed), **R** relative (the packet's parameters were wrong for the path). Any other first letter is class Unknown, which STREAM treats as final ([STREAM](stream.md#responses)).

| Code | Name | Code | Name | Code | Name |
|---|---|---|---|---|---|
| F00 | Bad Request | T00 | Internal Error | R00 | Transfer Timed Out |
| F01 | Invalid Packet | T01 | Peer Unreachable | R01 | Insufficient Source Amount |
| F02 | Unreachable | T02 | Peer Busy | R02 | Insufficient Timeout |
| F03 | Invalid Amount | T03 | Connector Busy | R99 | Application Error |
| F04 | Insufficient Destination Amount | T04 | Insufficient Liquidity | | |
| F05 | Wrong Condition | T05 | Rate Limited | | |
| F06 | Unexpected Payment | T99 | Application Error | | |
| F07 | Cannot Receive | | | | |
| F08 | Amount Too Large | | | | |
| F99 | Application Error | | | | |

Any other code passes through unchanged and is named "Unknown Error". F02 is "Unreachable", as RFC 27 has it (the original said "Unavailable"). What the node rejects with, and when, is the connector's ([Connector](connector.md)).

## F08 data

An F08 Amount Too Large reject carries, in its data, what arrived and the most the rejecting hop accepts, both in that hop's units:

```
AmountTooLargeData ::= SEQUENCE {
  receivedAmount UINT64,       -- 8 octets
  maximumAmount  UINT64 }      -- 8 octets; 16 octets in all
```

Senders scale their packets down by `maximumAmount / receivedAmount` ([STREAM](stream.md#responses)). Dassie's own F08s always carry it ([D13](../decisions/D13-connector-timing-and-packet-limits.md)). Reading it from a reject gives nothing for any other code, or for data that is not exactly these 16 octets: the original's connector sent F08 without data (ILP-4), and other software may too, so readers cope with its absence.

## ILDCP

The Interledger Dynamic Configuration Protocol (RFC 31): a child asks its parent for its ILP address and asset.

- **The request** is a Prepare to `peer.config` with amount 0, the condition below, no data, and an expiry 30 s after it is sent.
- **The fulfillment** is 32 zero octets, so the condition is its SHA-256:

  ```
  66687aadf862bd776c8fc18b8e9f8e20089714856ee233b3902a591d0d5f2925
  ```

  A Prepare is an ILDCP request when its destination is `peer.config` and its condition is this one.
- **The response** travels in the Fulfill's data:

  ```
  IldcpResponse ::= SEQUENCE {
    address    Address,                    -- IA5String (SIZE(1..1023)), a valid address
    assetScale UINT8,                      -- an amount of 1 is 10^-assetScale units
    assetCode  IA5String (SIZE(0..255)) }  -- for example XRP or USD
  ```

  RFC 31 leaves the asset code unbounded; the bound keeps a hostile parent from handing out megabytes ([D16](../decisions/D16-engineering-style-pragmatic-tigerstyle-and-deterministic-simulation.md)).

Which address and asset the node answers each endpoint with, the endpoint's own ledger's asset ([D6](../decisions/D06-owner-ledger-and-multi-currency-accounts.md)), is the connector's ([Connector](connector.md)).

## BTP 2.0

The Bilateral Transfer Protocol (RFC 23) carries ILP packets over a WebSocket between the node and its clients, such as `ilp-plugin-btp`. Each WebSocket binary message is one BTP packet:

```
BtpPacket ::= SEQUENCE {
  type      UINT8,                 -- 1 Response, 2 Error, 6 Message, 7 Transfer
  requestId UINT32,
  data      OPEN TYPE }            -- LEN || body by type

Response ::= SEQUENCE { protocolData ProtocolData }
Message  ::= SEQUENCE { protocolData ProtocolData }
Transfer ::= SEQUENCE { amount UINT64, protocolData ProtocolData }
Error    ::= SEQUENCE {
  code         IA5String (SIZE(3)),          -- as an ILP code
  name         IA5String (SIZE(0..8192)),
  triggeredAt  IA5String (SIZE(0..8192)),    -- GeneralizedTime as text, not interpreted
  data         OCTET STRING (SIZE(0..8192)),
  protocolData ProtocolData }

ProtocolData ::= SEQUENCE (SIZE(0..16)) OF SEQUENCE {
  protocolName IA5String (SIZE(0..255)),     -- for example ilp, auth, auth_token
  contentType  UINT8,                        -- 0 octet-stream, 1 text/plain UTF-8, 2 JSON
  data         OCTET STRING }
```

- BTP v1's types 3 (Prepare), 4 (Fulfill) and 5 (Reject) are not defined; any type octet but 1, 2, 6 and 7 is `UnknownType`.
- A packet carries at most 16 protocol data entries; RFC 23 sets no limit, and real clients send one or two ([D16](../decisions/D16-engineering-style-pragmatic-tigerstyle-and-deterministic-simulation.md)). Unknown content types pass through.
- Decoded entries are read in place from the packet's bytes, without allocating. Lookups give the first entry, or the first with a given name.
- **The authentication request** (RFC 23 §"Authentication") is a Message whose first entry is named `auth` with empty data, and which has an entry named `auth_token` of content type 1 holding UTF-8: the token. Any other packet carries no token.

How the server authenticates a client with it, the routes it gives a connection, and how Messages, Transfers, Responses and Errors are handled are the connector's ([Connector](connector.md)).

## In the browser and in scripts

`dassie-stream`'s WASM build includes this crate's bindings (feature `wasm`), exported from `@dassie/identity-wasm` and `@dassie/identity-wasm/web/stream` ([Identity](identity.md#in-the-browser-and-in-scripts)):

- `decodeIlpPacket(bytes)` gives the packet's type and its fields (amount, expiry, condition, address, code, message, data);
- `encodePrepare`, `encodeFulfill` and `encodeReject` encode one;
- `encodeAmountTooLarge` and `decodeAmountTooLarge` handle F08 data.

## Test vectors

From the original by `testdata/original/` (`tests/data/vectors.json`), checked natively and through the WASM build:

- the ILP fixtures of the original's `lib-protocol-ilp`, for example the Prepare `{amount 107, expiresAt 20171223012140549, destination example.alice, …}`, which encodes as `0c 68 …` (type 12, a body of 104 octets);
- the ILDCP response `{address test.alice, assetScale 9, assetCode XRP}`, which encodes as `0a 74 65 73 74 2e 61 6c 69 63 65 09 03 58 52 50`, and the ILDCP address and condition;
- F08 data, Interledger timestamps (0 is `19700101000000000`), and BTP packets of every type.

Each fixed gap has a test of its own (`tests/fixes.rs`): an empty `triggeredBy` parses (ILP-3), malformed packets are errors and not panics (ILP-3), and timestamps must be digits and real dates.

## Decisions

- [D6](../decisions/D06-owner-ledger-and-multi-currency-accounts.md): ILDCP answers each endpoint's own asset.
- [D13](../decisions/D13-connector-timing-and-packet-limits.md): F08 carries amount data, malformed packets get F01, nothing throws on the packet path, prefix matching is segment-aware.
- [D16](../decisions/D16-engineering-style-pragmatic-tigerstyle-and-deterministic-simulation.md): bounded decoding and no clock in the core.

## Original

- [4 §1](../original/04-interledger.md): the original's ILP packets, timestamps, error codes and F08 data, with their deviations.
- [4 §6](../original/04-interledger.md): ILDCP in the original.
- [4 §10](../original/04-interledger.md), [3 §14](../original/03-peer-network.md): the BTP schemas and the original's BTP server.
- [3 §3](../original/03-peer-network.md): how the original addressed nodes and endpoints.
