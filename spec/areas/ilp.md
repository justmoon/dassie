# ILP

The Interledger packets that carry every payment, and the two protocols that ride on them in Dassie: ILDCP, by which a client learns its address and asset, and BTP 2.0, by which ILP clients such as `ilp-plugin-btp` connect over a WebSocket. `dassie-ilp` encodes and decodes them; what the node does with them is the [connector](connector.md)'s.

**Paths:** `crates/dassie-ilp/`
**Read with:** [Encoding](encoding.md), [Connector](connector.md), [STREAM](stream.md)

The formats are the standards', byte for byte, so that existing Interledger software interoperates: [RFC 27](https://interledger.org/developers/rfcs/interledger-protocol/) (ILPv4), [RFC 15](https://interledger.org/developers/rfcs/ilp-addresses/) (addresses), [RFC 31](https://interledger.org/developers/rfcs/dynamic-configuration-protocol/) (ILDCP) and [RFC 23](https://interledger.org/developers/rfcs/bilateral-transfer-protocol/) (BTP 2.0). This doc states only where Dassie is stricter or bounds what the standards leave open. The golden vectors (`tests/data/vectors.json`, generated from the original's fixtures by `testdata/original/`) pin the bytes, natively and through the WASM build.

## Packets

Parsing is strict and total: every malformed input is a `ParseError`, never a panic, and the connector answers it with F01 ([connector-timing](../decisions/connector-timing.md)). Beyond the encoding:

- **Addresses must follow the RFC 15 grammar**, in the destination and in a non-empty `triggeredBy`. Much Interledger software accepts any string; refusing malformed addresses at the edge keeps them out of the routing table. An empty `triggeredBy`, which ILPv4 allows, reads as no address.
- **Expiries must be real dates** from 1970 on; a time past year 9999 is written as its end. A packet from before 1970 has expired anyway.
- **Buffers are sized once,** by `MAX_PACKET_LENGTH`, the largest packet of any type the format allows, so no packet path allocates per packet ([tigerstyle](../decisions/tigerstyle.md)). That is a Reject's, not a Prepare's: its message adds to the longest address and data, and a connector passes on any Reject that decodes, so a buffer that fits only the largest Prepare would lose one. The BTP and peer message limits hold it with its envelope.

## Addresses

**Prefixes compare by whole segments:** `test.alice` is a prefix of `test.alice.bob`, not of `test.alicebob`. Routing and the STREAM server both match this way (`Address::strip_prefix`), so a node cannot capture packets for another whose address merely starts with the same characters.

## F08 data

An F08 Amount Too Large carries the amount that arrived and the most the rejecting hop accepts, which senders scale their packets by ([STREAM](stream.md#responses)). Dassie's own F08s always carry it ([connector-timing](../decisions/connector-timing.md)). Other software may send F08 without data, so a reader gets nothing for missing or malformed data and must cope.

## ILDCP

RFC 31 leaves the asset code unbounded; Dassie bounds it to 255 characters so a hostile parent cannot hand out megabytes. Which address and asset the node answers each endpoint with is the [connector](connector.md)'s ([owner-ledger](../decisions/owner-ledger.md)).

## BTP 2.0

- Only BTP 2.0's packet types exist; v1's Prepare, Fulfill and Reject types are unknown types.
- A packet carries at most 16 protocol data entries. RFC 23 sets no limit, and real clients send one or two.
- Protocol data entries are read in place, without allocating.

How the server authenticates a client, what routes a connection gets and how each packet type is handled are the [connector](connector.md)'s.

## Decisions

- [owner-ledger](../decisions/owner-ledger.md): ILDCP answers each endpoint's own asset.
- [connector-timing](../decisions/connector-timing.md): F08 carries amount data, malformed packets get F01, nothing throws on the packet path, prefix matching is segment-aware.
- [tigerstyle](../decisions/tigerstyle.md): bounded decoding and no clock in the core.
