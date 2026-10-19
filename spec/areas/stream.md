# STREAM

STREAM (RFC 29) sends money and data over Interledger: it splits a payment into ILP packets, encrypts what travels in them, and has the receiver report what arrived. `dassie-stream` implements it as sans-IO state machines, a connection and a server, that interoperate with `ilp-protocol-stream`. The node's STREAM receivers and payers, the quote probes and the website's STREAM toy all run on it.

**Paths:** `crates/dassie-stream/`, `packages/identity-wasm/stream.ts`, `packages/identity-wasm/web_stream.ts`
**Read with:** [ILP](ilp.md), [Encoding](encoding.md), [Payments](payments.md), [Connector](connector.md), [Website](website.md)

## Overview

- **Sans-IO.** A connection never touches a socket, a clock or the OS RNG. The caller passes in the time, an RNG and the packets that arrive, and takes out the packets to send, the next deadline and events:
  - `handle_prepare` answers an incoming ILP Prepare;
  - `poll_transmit` writes the next outgoing Prepare, if one is due;
  - `handle_response` takes the Fulfill or Reject for a request;
  - `handle_timeout` expires what took too long, at `poll_timeout`;
  - `poll_event` reports what changed.

  The same inputs give the same outputs ([D16](../decisions/D16-engineering-style-pragmatic-tigerstyle-and-deterministic-simulation.md)), so the same code runs in the node, in simulations and in WASM.
- **Bounded.** Every table, queue and buffer is sized from the connection's [policy](#policy) when it is created; nothing grows per packet. Each connection works in one caller-provided buffer of `BUFFER_LENGTH`, room for one ILP packet to write and one to decrypt (twice ILP's largest packet, 67,714 octets).
- **Asserted.** `check_invariants` checks a connection's and a server's state after any step; tests and simulations call it after every one.
- **Failures are values.** Nothing a remote sends makes the crate panic; malformed input is rejected or ignored as described below.
- **Amounts** are unsigned 64-bit integers. Rates are exact integer ratios; there are no floats anywhere.

## Packets

A STREAM packet is the plaintext inside an ILP packet's data ([Crypto](#crypto) encrypts it). In canonical OER ([Encoding](encoding.md)):

```
StreamPacket ::= SEQUENCE {
  version       UINT8,                 -- 1
  packetType    UINT8,                 -- 12, 13 or 14: the ILP packet type it travels on
  sequence      VarUInt,               -- identifies a request and its response
  prepareAmount VarUInt,               -- request: the least the receiver must get
                                       -- response: what the receiver got
  frames        SEQUENCE (SIZE(0..512)) OF Frame }

Frame ::= SEQUENCE {
  type  UINT8,
  data  OPEN TYPE }                    -- LEN || the frame's fields
```

- **Parsing** refuses a version other than 1, a packet type other than 12, 13 or 14, and more than 512 frames (RFC 29 sets no limit; this one is far above what any sender needs).
- **Bytes after the frames are ignored**, as RFC 29 asks so that senders can pad packets. The original refused them (ILP-11).
- **Unknown frame types are skipped**, as RFC 29 asks; the frame must still be well formed. The original refused the whole packet (ILP-11).
- **Bytes after a known frame's fields, inside its open type, are ignored**, as `ilp-protocol-stream` does, so frames can gain fields without breaking older receivers.
- **Integers above 64 bits saturate.** Every `VarUInt` (sequence, amounts, limits, stream IDs, offsets) is read as `2^64 − 1` when larger, as RFC 29 §5.2 asks. The original decoded them as exact big integers.

### Frames

| Type | Frame | Fields |
|---|---|---|
| `0x01` | ConnectionClose | `code UINT8`, `message UTF8String` |
| `0x02` | ConnectionNewAddress | `sourceAccount Address or empty` (IA5String 0..1023) |
| `0x03` | ConnectionMaxData | `maxOffset VarUInt` |
| `0x04` | ConnectionDataBlocked | `maxOffset VarUInt` |
| `0x05` | ConnectionMaxStreamId | `maxStreamId VarUInt` |
| `0x06` | ConnectionStreamIdBlocked | `maxStreamId VarUInt` |
| `0x07` | ConnectionAssetDetails | `assetCode UTF8String`, `assetScale UINT8` |
| `0x10` | StreamClose | `streamId VarUInt`, `code UINT8`, `message UTF8String` |
| `0x11` | StreamMoney | `streamId VarUInt`, `shares VarUInt` |
| `0x12` | StreamMaxMoney | `streamId VarUInt`, `receiveMax VarUInt`, `totalReceived VarUInt` |
| `0x13` | StreamMoneyBlocked | `streamId VarUInt`, `sendMax VarUInt`, `totalSent VarUInt` |
| `0x14` | StreamData | `streamId VarUInt`, `offset VarUInt`, `data OCTET STRING` |
| `0x15` | StreamMaxData | `streamId VarUInt`, `maxOffset VarUInt` |
| `0x16` | StreamDataBlocked | `streamId VarUInt`, `maxOffset VarUInt` |
| `0x17` | StreamReceipt | `streamId VarUInt`, `receipt OCTET STRING` (RFC 39) |

- `ConnectionNewAddress` may carry an empty address, as RFC 29 allows; the original could not parse it. A non-empty one must be a valid ILP address ([ILP](ilp.md#addresses)).
- Every frame type is decoded. Which ones a connection acts on is in [Receiving](#receiving) and [Responses](#responses); it sends none of `ConnectionDataBlocked`, `StreamMoneyBlocked` and `StreamDataBlocked`, and ignores them when received.

### Error codes

| Code | Name | Code | Name |
|---|---|---|---|
| `0x01` | NoError | `0x06` | StreamStateError |
| `0x02` | InternalError | `0x07` | FrameFormatError |
| `0x03` | EndpointBusy | `0x08` | ProtocolViolation |
| `0x04` | FlowControlError | `0x09` | ApplicationError |
| `0x05` | StreamIdError | | |

Unknown codes pass through unchanged.

## Crypto

As RFC 29 §4, from the connection's shared secret (32 bytes in practice):

```
encryptionKey  = HMAC-SHA256(sharedSecret, "ilp_stream_encryption")
fulfillmentKey = HMAC-SHA256(sharedSecret, "ilp_stream_fulfillment")
ciphertext     = iv (12 random bytes) || tag (16) || AES-256-GCM(encryptionKey, iv, plaintext)   -- no AAD
fulfillment    = HMAC-SHA256(fulfillmentKey, ciphertext)      -- over the ILP Prepare's whole data field
condition      = SHA-256(fulfillment)
```

- A request's ciphertext is the Prepare's data; a response's is the Fulfill's or Reject's data.
- Decryption that fails (tampered, truncated, another key) is a value, not an exception (ILP-13).
- The IV comes from the caller's RNG, a fresh one per packet.
- The largest plaintext that fits ILP's 32,767 octets of data is 32,739 octets.

## Credentials and tokens

A server gives each client a destination address and a shared secret. Both derive from a 32-byte **server secret**, as in `ilp-protocol-stream`, so the server keeps no table of issued credentials: they survive restarts and never pile up. The original kept credentials in memory, never expiring, lost at a restart and without tags (ILP-11).

```
token        = AES-256-GCM under the server secret, laid out as above, of:
                 nonce            18 random bytes
                 connectionTag    OCTET STRING (SIZE(0..128))
                 receiptNonce     OCTET STRING, empty or 16 bytes
                 receiptSecret    OCTET STRING, empty or 32 bytes
destination  = <server address> "." base64url(token)            -- unpadded
sharedSecret = HMAC-SHA256(HMAC-SHA256(serverSecret, "ilp_stream_shared_secret"), token)
```

- A token is at most 226 bytes. Opening one refuses a token that does not decrypt, has bytes left over, a tag over 128 bytes, or receipt fields other than both empty or 16 and 32 bytes.
- **The connection tag** is the application's label for the connection, for example the incoming payment it pays ([Payments](payments.md)). The server hands it to the application before letting a packet in.
- **Receipt fields** make the connection issue receipts ([Receipts](#receipts)).
- **Base64url** here is unpadded and canonical: no padding, no whitespace, and the last character's unused bits must be zero.
- An application that keeps a token gives out the same credentials again with `credentials_for`.
- The node derives each receiver's server secret from its node key, at `node/stream-server/<ledger slug>` ([Identity](identity.md#seed-paths)), so credentials it issued stay valid across restarts. That path is part of the derivation chain and never changes ([Identity](identity.md#derivation-chain)).

## Server

A server answers every Prepare addressed below its address and routes it to the connection its token names.

1. **The token** is the first segment after the server's address (by whole segments). Further segments are ignored, as `ilp-protocol-stream` does; the original took everything after the address. A destination not below the address, or a segment that is too long or not base64url, is rejected with **F02**.
2. **A known token** goes to its connection.
3. **An unknown token** must open: one that does not open is rejected with **F02**.
4. **Admission.** Before a packet reaches a connection, new or existing, the application is asked with the connection's tag whether it may in:
   - `Refuse(code)` rejects it with that ILP code and opens no connection;
   - `Accept { money_limit, received }` lets it in and sets the connection's money limit ([Money limit](#money-limit)). Without an admission function, everything is accepted without a limit.
5. **A full table.** A new connection that finds the table full (its `max_connections`):
   - first forgets the connections that are closed, have nothing in flight, and whose events were all polled;
   - then forgets the connection heard from least recently. Its sender's next packet opens it again, and admission says what it may still receive, so no remote can keep the table full with idle connections;
   - otherwise is rejected with **T03**.
6. **A new connection** gets the next connection ID (assigned in order, never reused), is reported by `poll_opened` (a queue as long as the table, dropping the oldest notice if the application does not poll), and is created with role server, the server's address and asset, the token's receipt fields and the server's policy.

The server's rejections have an empty message, no data, and the server's address as `triggeredBy`. The node's receivers hold at most 256 connections each; the WASM `StreamServer` holds 64.

## Connections

A connection is one end: the **client** (given the destination and the shared secret) or the **server** (which issued them).

- **Addresses.** A client is created with the destination as the remote address, and may have an address of its own; without one it can send but not receive. A server's connection learns the client's address from `ConnectionNewAddress`. A changed remote address resets congestion control ([Congestion control](#congestion-control)), since a new path's capacity is unknown.
- **Announcement.** A side tells the other its address (clients only), its asset (`ConnectionAssetDetails`, if it has one) and its stream limit (`ConnectionMaxStreamId`):
  - in every request until a response shows the remote heard it. A server's connection starts out known to the client, since its address is how the client found it;
  - again, in requests and responses, whenever something changed: the remote announced a new address (it wants ours in return), or a closed remote stream raised the stream limit.
- **Remote asset** is taken from `ConnectionAssetDetails`; a frame with the same code as the one known is ignored.
- **Connected.** The `Connected` event is emitted when the first request decrypts (server) or the first response does (client).
- **Sequence numbers** start at 0 and increase with every request: money, data, probes and closes alike.

### Streams

- **IDs.** Clients open odd IDs from 1, servers even IDs from 2, each in steps of 2 (RFC 29 §2.2). ID 0 is never valid.
- **Opening locally** (`open_stream`) fails if the connection is closing or closed, if this side has `max_local_streams` open (default 10), or if the ID is above the remote's stream limit. In the last case the next request carries `ConnectionStreamIdBlocked` with the ID wanted.
- **The remote's stream limit** is assumed to be 20 (ten streams, as in `ilp-protocol-stream`) until it says; `ConnectionMaxStreamId` only ever raises it.
- **Our stream limit** starts at `2 · max_remote_streams` (20 at the default) and rises by 2 for each stream the remote opened that is gone, so the remote may open another in its place (as `ilp-protocol-stream` does).
- **A remote opens a stream** by sending a frame for a new ID of its parity, at most our limit, while the connection is open. The stream is created and `StreamOpened` is emitted. A remote-opened stream accepts `remote_stream_receive_max` (default 0) until the application changes it: the application only hears of the stream after the packet that opened it, so a receiver that takes any payment sets this instead. The node's receivers set it to the maximum and limit the connection instead ([Money limit](#money-limit)).
- **Rule violations end the connection** (the response carries `ConnectionClose` with the code):

  | Frame for | Code |
  |---|---|
  | stream 0 | ProtocolViolation |
  | an ID of our parity we never opened | ProtocolViolation |
  | an ID above our stream limit | StreamIdError |

- **Closed IDs.** A frame for a stream that is gone (one of ours we opened before, or one of the remote's we remember as closed) never reopens it. Money or data for it rejects the packet, and the response carries `StreamClose` with StreamStateError; limits for it are ignored. Closed remote IDs are remembered in fixed memory: a window of 256 IDs above a base that slides up as the remote opens higher IDs; IDs below the base count as closed, as QUIC treats streams below the highest opened. The original kept no record, so closed streams could be reopened.
- **The application** sets a stream's send maximum (`set_send_max`, never below what it sent or has in flight) and receive maximum (`set_receive_max`, never below what it received), writes data into its send buffer and reads data out of its receive buffer.

### Money limit

An application that pays a connection into something with a total, such as an incoming payment, sets before each packet how much more the connection may receive over all its streams, and how much it already counts as received (`set_money_limit(remaining, received)`; the server does it through admission). `u64::MAX` means no limit.

- A packet whose amount exceeds the remaining limit is rejected.
- Under a limit, every stream reports `totalReceived` as the application's count and `receiveMax` as `min(the stream's maximum, count + remaining)`. Both only grow, as senders expect, even when the sender's earlier connection was forgotten, after a restart or an eviction.
- A connection never takes more than its limit (asserted).

## Receiving

`handle_prepare` answers a Prepare, in this order:

1. **Decrypt and parse.** A Prepare whose data does not decrypt, does not parse, or holds a STREAM packet whose type is not 12 is rejected with **F06** and no data (RFC 29 §4.3). Rejections in this list up to step 3 are unencrypted, with no message and our address as `triggeredBy`.
2. **Expired** (expiry at or before now): **R00**.
3. **No room for events.** While the event queue lacks room for what one packet can cause, packets are rejected with **T05**, so a remote cannot overflow it. Senders retry.
4. **Closed connection:** rejected, with a response carrying `ConnectionClose` (NoError).
5. **Frames that apply whether or not the packet is fulfilled:**
   - `ConnectionNewAddress` (non-empty): the remote's address;
   - `ConnectionAssetDetails`: the remote's asset;
   - `ConnectionMaxStreamId`, `ConnectionMaxData`: raise the remote's limits (never lowered);
   - `StreamClose`: the stream is closed by the remote (first code wins);
   - `StreamMaxMoney`: the remote's received total for the stream is raised to `totalReceived`; its receive maximum is set by the first value and only raised by later ones;
   - `StreamMaxData`: raises the remote's data limit for the stream;
   - `StreamMoney`, `StreamData`: open the stream if new ([Streams](#streams));
   - `ConnectionClose`: the connection closes once this packet is answered, closed by the remote.

   A violation ends the connection at once. Other frames are ignored.
6. **Fulfillable?** The packet is fulfilled only if all of these hold, and rejected otherwise:
   - no frame carried money or data for a closed stream;
   - the condition matches: `SHA-256(HMAC-SHA256(fulfillmentKey, data)) = executionCondition`. Rate probes fail here on purpose ([Exchange rates](#exchange-rates));
   - the amount that arrived is at least the packet's `prepareAmount`;
   - the amount is within the connection's money limit;
   - **money:** every `StreamMoney` names an open stream, and a packet with an amount above 0 has at least one (the original fulfilled such a packet and kept the money unassigned). Several frames for one stream add their shares. The amount is split among the streams by [allocation](#allocation); if their capacity (receive maximum less received; 0 for a stream we closed) cannot take it all, the packet is rejected;
   - **data:** every `StreamData` names an open stream, ends within the stream's window, and fits the stream's fragment table ([Data](#data)). Data beyond a stream's window, or open streams' highest offsets adding up to more than our connection limit, end the connection with FlowControlError. A packet too fragmented to track now is rejected, to be sent again.
7. **Apply.** Each stream's received total grows by its allocation (`MoneyReceived`); data is stored (`DataReadable`); the money limit shrinks by the amount.

Money is credited only to a packet that is fulfilled. The original credited it before checking the condition.

**The response** is a STREAM packet with the request's sequence, `prepareAmount` = the amount that arrived, and the type of the ILP packet it travels in: in the Fulfill's data, or in an **F99** Reject's data (no message, our address as `triggeredBy`). Its frames, in order:

1. our announcement, if one is due;
2. `ConnectionMaxData`;
3. for each open stream: `StreamClose` if we closed it, otherwise `StreamMaxMoney` and `StreamMaxData`;
4. `StreamClose` for each stream the packet was refused for (StreamStateError);
5. `StreamReceipt` for each stream credited, if the connection issues receipts;
6. `ConnectionClose`, if this packet ends the connection with an error, or the connection is closing or closed.

So every response tells the sender every open stream's limits and totals, on success as on failure. The original sent them only when allocation failed.

## Sending

`poll_transmit` writes the next request into the buffer, if one is due, and returns its request ID (its sequence), length and expiry. Nothing is sent once the connection is closed, before the remote's address is known, or before a back-off ends. Each packet in flight has a slot: `max_concurrency` for ordinary packets, 8 for probes and one for a close, allocated with the connection.

What a request carries, in order of priority:

1. **Closing:** only a close packet ([Closing](#closing)).
2. **A queued probe** ([Exchange rates](#exchange-rates)): our announcement if due, nothing else. Probes are not limited by congestion control.
3. **An ordinary packet**, while fewer than the allowed number are in flight ([Congestion control](#congestion-control)).

**Money in an ordinary packet.** Only with an exchange rate set, and not while a too-low rate stopped money ([Responses](#responses)).

- For each open stream in the order opened, it takes `min(what the stream has left to send, what the remote still accepts on it)`, until the packet's budget, the largest packet amount, is used. What the remote accepts (its receive maximum less what it received, in its units) is converted to source units at the *expected* rate, the rate the path is known to deliver, rounding down: converted at the minimum rate, a packet that fits would arrive too large whenever the real rate is better, and be refused forever.
- Each stream's part is a `StreamMoney` frame whose shares are its source amount. The Prepare's amount is their sum, and the packet's `prepareAmount` is `floor(amount × minimum rate)`.
- **De minimis.** Less than `de_minimis_amount` (1000) waits while other money is in flight, since those packets may fail and free a larger amount. Alone, a small amount goes, so a payment below the threshold completes; the original never sent it (ILP-11).

**Other contents of an ordinary packet:** our announcement if due, `ConnectionStreamIdBlocked` if a stream is blocked, `ConnectionMaxData`, for each stream `StreamClose` (if we closed it and the remote has not heard) or `StreamMaxMoney` and `StreamMaxData`, the `StreamMoney` frames, then `StreamData` frames ([Data](#data)).

An ordinary packet is sent only if it has something to do: money, data, a stream close, a client's first announcement, a blocked stream ID, or raised receive limits the remote has not heard of. Raised limits (more read, a higher receive maximum) that a lost or refused packet carried are sent again; a reply acknowledges an update only if nothing was raised since.

**The Prepare:** destination the remote's address, the amount above, expiry `now + packet_timeout` (30 s), data the encrypted packet, and condition `SHA-256(HMAC-SHA256(fulfillmentKey, data))`, so the receiver can fulfill it. A probe gets 32 random bytes as its condition instead. Money a packet carries is held from its streams' send amounts until it settles.

## Responses

`handle_response` settles a request by its request ID; a response to an unknown or settled request is ignored. A response that does not parse, a Fulfill that does not satisfy the condition, and a Fulfill of a probe count as a **lost** packet (RFC 27). Otherwise:

- **The reply** in a Fulfill, or in an F99 Reject, is decrypted and trusted only if its sequence is the request's and its type the ILP packet's it came in (the original checked neither). A trusted reply:
  - shows the remote knows us, and acknowledges our announcement if the request carried it;
  - applies `StreamMaxMoney` and `StreamMaxData` to known streams (a reply never opens one), and `StreamClose`, `ConnectionMaxStreamId`, `ConnectionMaxData` and `ConnectionAssetDetails` as in [Receiving](#receiving);
  - on a Fulfill, keeps the stream's latest `StreamReceipt`;
  - with `ConnectionClose`, closes the connection, closed by the remote;
  - says what arrived: its `prepareAmount`.
- **Fulfilled:** the money is sent (`MoneySent`), stream closes it carried are acknowledged, concurrency rises, and the retry delay resets.
- **Rejected:** the packet's money is released and its data rewound to be sent again. A reply means the receiver processed the frames, so its stream closes count as acknowledged. Then, by kind:

| Rejection | What follows |
|---|---|
| F99 with a trusted reply (the receiver refused it) | If it carried money and less arrived than its `prepareAmount`, the rate is too low: money stops (`ExchangeRateTooLow`) until the application sets a rate. Otherwise a better rate than expected is learned. Then back off. |
| F08 | The largest packet amount falls to `amount × maximumAmount / receivedAmount` from the F08 data, or half the amount without valid data (ILP-4), and in any case below the amount. No back-off. A limit of 0 closes the connection (InternalError). An F08 for a packet without money is ignored. |
| T or R class | Back off. |
| F class, or an unknown class | The path will not carry this connection's packets: close (InternalError), as `ilp-protocol-stream` does. The original kept retrying. |

- **Lost** (a bad response, or expiry in `handle_timeout`): released and rewound as a rejection, then back off. Packets in flight still settle after the connection closed, since money they carried may have been delivered.

### Congestion control

AIMD, as in the original (an extension RFC 29 does not have; `ilp-protocol-stream` sends one packet at a time):

- Concurrency, counted in thousandths, starts at `min_concurrency` (1). The ordinary packets allowed in flight are its whole part, at least 1.
- Each fulfilled packet raises it by 1, up to `max_concurrency` (100).
- Each back-off halves it, down to `min_concurrency`.
- **Back-off** also delays the next send by the retry delay, which starts at 100 ms and doubles up to 30 s, and resets to 100 ms on a fulfill. Packets in flight together are usually refused together, by the same full credit limit, so a burst counts once: only a packet sent after the last back-off triggers another.

## Exchange rates

A connection sends money only at a rate the application sets: `set_exchange_rate(rate)`, the least the receiver must get per source unit, enforced through each packet's `prepareAmount`. It also resumes money a too-low rate stopped. `dangerously_ignore_exchange_rate` sets a rate of 0: anything that arrives is accepted, and the receiver's limits are converted at whatever rate replies show.

**Measuring.** The rate is not measured automatically; the application asks with `measure_exchange_rate` (a second call while one runs does nothing). The original also measured only on request.

- **Probes** are unfulfillable packets of growing size: first `1, 10^3, 10^6, 10^9, 10^12`, each capped at the largest packet amount, duplicates dropped, at most 8 queued. They carry no money frames and a `prepareAmount` of 0, so the receiver rejects each with a reply saying what arrived.
- **A result** is a reply showing more than 0 arrived: the rate `arrived / sent`. The best is the one whose arrived amount has the most decimal digits, and of equal digits the larger probe.
- **When a volley settles** (nothing queued or in flight):
  - a best result with at least 3 significant digits (`min_rate_precision_digits`) is adopted, less 1% slippage (`slippage_permille` 10): the connection sends at `rate × 990/1000`, and `ExchangeRateMeasured` reports the measured rate;
  - after 5 volleys (`max_probe_volleys`) without one, `ExchangeRateMeasurementFailed`;
  - otherwise a new volley probes the packet maxima F08 rejects revealed, and, if a probe hit a temporary error or was lost, two thirds of the smallest such amount, after a back-off. A volley with nothing to probe fails the measurement.
- The node's quotes measure routes this way ([Payments](payments.md)).

**Rates** are exact ratios of two `u64` in lowest terms. Conversion is `floor(source × numerator / denominator)`, saturating; the most source that converts to at most a given destination amount rounds down, so a sender never overpays. A ratio whose terms exceed 64 bits after reduction is shifted down, losing precision, never magnitude.

## Allocation

The amount that arrives is split among the streams of a packet's `StreamMoney` frames in proportion to their shares, without giving any stream more than its capacity. This is the original's algorithm, kept on purpose ([Compatibility](../overview.md#compatibility-with-the-original)): what a full stream cannot take goes to the others by their shares, and rounding remainders go to a stream with room. RFC 29 and `ilp-protocol-stream` split proportionally and reject a packet a full stream cannot take, or drop the remainder.

```
remaining = total; ratio = (total, Σ shares); alloc[i] = 0
repeat:
  before = remaining; open = 0
  for each stream i, in order:
    room = capacity_i − alloc[i]
    a    = min(floor(shares_i × ratio.0 / ratio.1), room, remaining)
    remaining −= a; alloc[i] += a
    if a < room: open += shares_i                   -- the stream still has room
  if remaining = 0: done
  if open = 0: the streams cannot take it all → reject
  ratio = (remaining, 1) if remaining = before, else (remaining, open)
```

The loop ends within `2 × streams + 4` rounds; afterwards the allocations add up to exactly the total (asserted). An amount of 0 allocates nothing.

## Data

Each stream carries an ordered byte stream in each direction, with flow control as in RFC 29 and `ilp-protocol-stream`. The original did not implement data at all.

- **Sending.** `write` queues as much as fits the stream's send buffer (`stream_send_buffer_bytes`, 16 KiB) and says how much. Ordinary packets carry `StreamData` frames from each stream's next unsent offset, within the packet's room (the largest plaintext less the packet's other frames, less 25 octets of frame overhead per stream), the stream's limit, and the connection's limit. Bytes are freed once every packet carrying them has settled; a lost or rejected packet's data is sent again from its offset.
- **Limits we grant.** A stream accepts up to what the application read plus `stream_receive_window_bytes` (16 KiB), advertised as `StreamMaxData`. The connection accepts, over its open streams' highest offsets added up, what the application read on them plus `connection_receive_window_bytes` (65,534), advertised as `ConnectionMaxData`.
- **Limits we observe.** A stream's limit is assumed to be 16 KiB until the remote says, and the connection's limit our own window; both only rise. New bytes beyond a stream's highest offset count against the connection's limit; retransmissions do not. As in `ilp-protocol-stream`, both sides count only open streams, so closing a stream frees its share; counting closed streams too stalls a connection after 64 KiB.
- **Reassembly.** A stream's receive buffer keeps the received ranges past the read offset, at most 8 out-of-order fragments. Data that would need a ninth is refused for now, to be sent again. Data entirely before the read offset, and empty data, are accepted and change nothing.
- `read` copies in-order data out, frees it, and lets the remote send more.

## Receipts

A connection whose credentials carry receipt fields issues STREAM receipts (RFC 39), which a third party holding the receipt secret can verify:

```
receipt (58 bytes) = version 1 (1) || receiptNonce (16) || streamId (1) || totalReceived UINT64 (8)
                     || HMAC-SHA256(receiptSecret, the first 26 bytes) (32)
```

- Each fulfilled packet's response carries, for every stream it credited, a receipt of the stream's total received. RFC 39 has one byte for the stream ID, so streams above 255 get none.
- Verifying checks the length, the HMAC in constant time and the version.
- A sender keeps the latest receipt per stream from its fulfilled packets.
- The node issues no receipts: they are not part of Open Payments ([D9](../decisions/D09-open-payments-fully-and-no-spsp.md)).

## Closing

**A stream** is closed with `close_stream(code)`. Money and data not yet sent are dropped (its send maximum becomes what it sent and has in flight); what is in flight completes. Its `StreamClose` goes in every ordinary packet, and every response, until a packet carrying it is fulfilled, or rejected with a reply. A stream is forgotten once one side closed it (and, if we did, the remote heard), nothing of it is in flight, and its data was read or we closed it ourselves. Then `StreamClosed` reports its code (ours, else the remote's, else NoError) and its total sent and received.

**The connection** is closed with `close(code)`:

- every stream is closed, unsent money and data are dropped, and a measurement stops;
- if the remote's address is unknown or the connection never connected, there is nobody to tell, and it closes at once;
- otherwise it is *closing*: only close packets are sent, fulfillable, without money, carrying `ConnectionClose` with the code. At most `close_retries` (3) are sent, one at a time. It closes once nothing is in flight and a close was fulfilled or every attempt is spent, or `close_timeout` (30 s) after `close` at the latest. A closing connection still answers incoming packets, with `ConnectionClose` in the response.

**Other ways a connection closes:**

- the remote's `ConnectionClose`, in a request or a trusted reply: closed by the remote;
- a protocol or flow-control violation by the remote ([Receiving](#receiving));
- a final ILP error, or a largest packet amount of 0 ([Responses](#responses)): InternalError;
- **idle:** nothing from the remote (no request accepted, no response) for `idle_timeout` (60 s) since the last sign of life or since the first chance to send, as in `ilp-protocol-stream`: NoError. This bounds how long abandoned connections hold a server's table.

`ConnectionClosed` reports the code and whether the remote closed it. Packets in flight still settle afterwards.

## Events

| Event | When |
|---|---|
| `Connected` | the remote answered for the first time |
| `StreamOpened { stream_id }` | the remote opened a stream |
| `MoneyReceived { stream_id }` | a stream received money |
| `MoneySent { stream_id }` | money sent on a stream was delivered |
| `DataReadable { stream_id }` | a stream has data to read |
| `StreamClosed { stream_id, code, total_sent, total_received }` | a stream is closed and gone |
| `ExchangeRateMeasured { rate }` | a measurement finished; the connection sends at the rate less slippage |
| `ExchangeRateMeasurementFailed` | a measurement got no result precise enough |
| `ExchangeRateTooLow { expected, received }` | less arrived than the minimum; money stopped |
| `ConnectionClosed { code, by_remote }` | the connection is closed |

- `MoneyReceived`, `MoneySent` and `DataReadable` are coalesced: one waiting in the queue stands for any number of changes to that stream, and the stream's accessors give the current totals.
- The queue holds `6 × streams + 16` events (136 at the default). A packet is refused (T05) unless `3 × streams + 4` places are free, what one packet can cause at most.
- `stats` counts packets sent, fulfilled, rejected and timed out, packets received and fulfilled, amounts sent, delivered and received, and data bytes sent and received.

## Policy

Every limit of a connection, from the defaults of the original where it had one and of `ilp-protocol-stream` where it did not:

| Field | Default | Meaning |
|---|---|---|
| `packet_timeout_ms` | 30,000 | a request's expiry after it is sent |
| `min_concurrency` | 1 | the fewest ordinary packets in flight backed off to |
| `max_concurrency` | 100 | the most ordinary packets in flight |
| `de_minimis_amount` | 1000 | the least money worth a packet while more is in flight |
| `max_remote_streams` | 10 | streams the remote may have open |
| `max_local_streams` | 10 | streams this side may have open |
| `stream_receive_window_bytes` | 16,384 | each stream's receive window |
| `stream_send_buffer_bytes` | 16,384 | each stream's send buffer |
| `connection_receive_window_bytes` | 65,534 | the connection's receive window |
| `close_timeout_ms` | 30,000 | how long closing waits |
| `close_retries` | 3 | close packets sent at most |
| `slippage_permille` | 10 | how far below a measured rate packets may deliver |
| `min_rate_precision_digits` | 3 | significant digits a measurement needs |
| `max_probe_volleys` | 5 | volleys a measurement may send |
| `retry_delay_start_ms` | 100 | the first back-off delay |
| `retry_delay_max_ms` | 30,000 | the longest back-off delay |
| `max_packet_amount` | `2^64 − 1` | the largest packet amount before any F08 |
| `remote_stream_receive_max` | 0 | what a remote-opened stream accepts until the application says |
| `idle_timeout_ms` | 60,000 | silence after which a connection closes |

A policy must hold together, or creating a connection or server fails an assertion: `1 ≤ min_concurrency ≤ max_concurrency ≤ 1000`; at least one stream each way, and at most 100 streams in all, so every packet can describe every open stream within 512 frames and ILP's 32 KiB; stream windows and buffers at most 16 MiB; slippage at most 100%; at least one close attempt and one probe volley; a largest packet amount of at least 1; the retry delay's start at most its maximum; an idle timeout above 0.

## Differences from RFC 29 and `ilp-protocol-stream`

Compatible on the wire with both. Kept from the original on purpose ([Compatibility](../overview.md#compatibility-with-the-original)):

- **AIMD concurrency**: many packets in flight instead of one ([Congestion control](#congestion-control)).
- **Allocation** redistributes what full streams cannot take, and the remainder ([Allocation](#allocation)).
- **Exact amounts**: integer amounts and rates throughout, never floats. (The original used arbitrary-precision integers; here every amount is a `u64`, and larger wire values saturate as RFC 29 asks.)
- **Explicit rate measurement** and failures as values.

Also differing from `ilp-protocol-stream`: dust waits only while other money is in flight; responses always carry every open stream's limits; a too-low rate stops money until the application sets a new one.

The original's gaps (ILP-11, ILP-13) are closed: unknown frames and trailing data are skipped, StreamClose is sent and processed, stream limits are advertised and enforced, data streams and receipts work, temporary errors back off and final ones close, small final amounts are sent, money is credited only when fulfilled, the receipt-amount check rejects with F99, replies are checked against their requests, credentials are stateless with tags, and decryption failures are values.

## In the browser and in scripts

`dassie-stream` is built to WASM with `dassie-identity` ([Identity](identity.md#in-the-browser-and-in-scripts)). The bindings are byte-oriented: the JavaScript side owns the clock (milliseconds) and the network, and randomness comes from the platform at this edge.

- **`StreamConnection`** wraps a client connection with the default policy: open and close streams, set send and receive maxima, write and read, totals and receipts, set, measure or ignore the rate, `pollTransmit`, `handleResponse`, `handlePrepare`, `handleTimeout`, `pollTimeout`, `pollEvent`.
- **`StreamServer(secret, address, assetCode, assetScale, streamReceiveMax)`**: a server of 64 connections whose remote-opened streams accept `streamReceiveMax`. It issues credentials (with a tag, and receipts if both a 16-byte nonce and a 32-byte secret are given), answers Prepares (one that does not decode gets F01), and drives its connections by ID.
- **`decodeStreamPacket(sharedSecret, data)`** decrypts and decodes a STREAM packet, for logs such as the toy's packet log.
- **The drivers** (`@dassie/identity-wasm/stream`): `ClientDriver` and `ServerDriver` send what a connection produces through an `IlpEndpoint` (`sendPacket(prepare) → response`), feed responses back, run the timers and deliver events. `ClientDriver.pay(amount)` measures the rate if none is known, opens a stream and resolves once it sent the amount; a failed send counts as lost and is settled by its expiry. The website's STREAM toy and the interop tests run on them ([Website](website.md)).

## Tests

- **Golden vectors** (`tests/data/vectors.json`, from the original by `testdata/original/`): all 53 STREAM packet fixtures of the original, every frame type among them, and the crypto vectors. With the secret `"foo"`: encryption key `cdf386314fee63e10e50213ab4ddd6844efb6c5fcdea94b4c123a63201003040`, fulfillment key `961f27189089a2cbdf51e3da534bb1c572b78cdbd5d98fb98afb75187d4cba1b`, fulfillment of `"bar"` `c1803ed8ee0868f07da893d9184fd9973570d5ea7185b64483d3f886483bb92c`, and the ciphertext `HweWFgwWTh/cwwbKHqRLNeDtuakR4E8ltFSDyQ7v31ImmnQ=`, which decrypts to `"foo bar"`. An empty Prepare packet is `01 0c 01 00 01 00 01 00`. Where the rewrite differs from a fixture on purpose (saturation, unknown frames), the test has its own vector named after the gap.
- **Property tests:** no packet, frame, token, receipt or base64url input panics; allocation hands out exactly the total within capacity; encryption round-trips; rates never overpay.
- **Hostile input:** arbitrary Prepares, responses and destinations never panic a connection or a server.
- **Simulations** (`sim` module, feature `sim`): a client and a server joined by a simulated connector with a mock clock, a seeded RNG, latency, loss, an exchange rate and a largest packet amount. They measure rates, pay, move data both ways, issue receipts, close, respect limits across a server restart, and are deterministic per seed ([Testing](testing.md)).
- **Interop** (`tests/interop/`): `dassie-stream`, native and WASM, against `ilp-protocol-stream` in both directions, receipts and data included.

## Decisions

- [D9](../decisions/D09-open-payments-fully-and-no-spsp.md): STREAM under Open Payments, a connection per payment, no receipts from the node.
- [D16](../decisions/D16-engineering-style-pragmatic-tigerstyle-and-deterministic-simulation.md): sans-IO, deterministic, bounded and asserted.
- [D24](../decisions/D24-money-in-flight-with-a-peer-stays-within-a-few-packets.md): why the connector caps money in flight with a peer, though a STREAM connection may have 100 packets in flight.
- [D27](../decisions/D27-simulations-run-continuously-from-random-seeds.md): the STREAM simulation runs from any seed.

## Original

- [4 §11](../original/04-interledger.md): the original's STREAM library, its policy, packet format, crypto, server, sending and allocation, with its gaps (ILP-11, ILP-13).
- [4 §9.2](../original/04-interledger.md): the upstream STREAM server the original's Open Payments used.
