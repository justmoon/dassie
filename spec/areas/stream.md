# STREAM

STREAM sends money and data over Interledger: it splits a payment into ILP packets, encrypts what travels in them, and has the receiver report what arrived. `dassie-stream` implements it as two state machines, a connection and a server, compatible on the wire with `ilp-protocol-stream`. The node's STREAM receivers and payers, its quote probes and the website's STREAM toy all run on it.

**Paths:** `crates/dassie-stream/`, `packages/identity-wasm/stream.ts`, `packages/identity-wasm/web_stream.ts`
**Read with:** [ILP](ilp.md), [Encoding](encoding.md), [Payments](payments.md), [Connector](connector.md), [Website](website.md)

The protocol is [RFC 29](https://interledger.org/developers/rfcs/stream-protocol/); receipts are [RFC 39](https://github.com/interledger/rfcs/tree/main/deprecated/0039-stream-receipts). This doc states what Dassie chooses where they leave room, and where it differs from them and from `ilp-protocol-stream`, the reference implementation most peers run.

## Shape

- **Sans-IO and deterministic.** A connection never touches a socket, a clock or the OS RNG; the caller passes them in. The same code therefore runs in the node, in simulations and in the browser, and the same inputs give the same outputs ([D16](../decisions/D16-engineering-style-pragmatic-tigerstyle-and-deterministic-simulation.md)).
- **Bounded.** Every table, queue and buffer is sized from the connection's [policy](#policy) when it is created; nothing grows per packet. `check_invariants` checks a connection's and a server's state; tests and simulations call it after every step.
- **Exact amounts.** Amounts are `u64` and rates exact integer ratios, never floats. Wire values above 64 bits saturate, as RFC 29 asks.
- **Lenient on the wire, as RFC 29 asks:** padding after the frames, unknown frame types and bytes after a known frame's fields are skipped, so frames can gain fields without breaking older receivers. A packet has at most 512 frames, a bound RFC 29 does not set and no sender comes near.

## Credentials and tokens

A server derives each client's destination and shared secret from a 32-byte server secret and a random token, as `ilp-protocol-stream` does (`src/token.rs`). The token is encrypted under the server secret and carries the application's **connection tag** (for example the incoming payment it pays) and optional receipt keys. So the server keeps no table of issued credentials: they never pile up, and they survive restarts.

The node derives each receiver's server secret from its node key at a fixed seed path, so the credentials it issued stay valid across restarts; that path never changes ([Identity](identity.md#derivation-chain)).

## Server

A server routes every Prepare below its address to the connection its token names. The token is the first segment after the server's address, compared by whole segments ([ILP](ilp.md#addresses)); further segments are ignored, as `ilp-protocol-stream` does.

- **Admission.** Before any packet reaches a connection, new or existing, the application is asked with the connection's tag whether it may in. It can refuse with an ILP code, or accept and set the connection's [money limit](#money-limit). This is how a receiver ties a connection to what it pays into.
- **A full table** first forgets connections that are closed and settled, then the one heard from least recently, and only then rejects (T03). A forgotten sender's next packet reopens its connection, and admission restores what it may still receive, so no remote can keep the table full with idle connections. The [idle timeout](#closing) frees the rest.

## Connections

A connection is the **client** (given the destination and shared secret) or the **server** (which issued them). Each side announces its address, asset and stream limit in every request until a response shows the remote heard it, and again whenever one changes. A changed remote address resets congestion control, since the new path's capacity is unknown.

### Streams

- **A remote-opened stream** accepts `remote_stream_receive_max` until the application raises it: the application hears of the stream only after the packet that opened it, so a receiver that takes any payment must set this up front. The node's receivers set it to the maximum and bound the connection with its money limit instead.
- **Closed streams never reopen.** Money or data for one rejects the packet, and the response says the stream is closed. Closed remote IDs are remembered in fixed memory, as a window above a base that slides up as the remote opens higher IDs; IDs below the base count as closed, as QUIC treats them.
- **Our stream limit** rises as the remote's streams go, so it may open another in each one's place, as `ilp-protocol-stream` does.
- Frames for stream 0, for an ID of our parity we never opened, or above our stream limit end the connection.

### Money limit

An application that pays a connection into something with a total, such as an incoming payment, sets before each packet how much more the connection may receive over all its streams and how much it already counts as received (`set_money_limit`; a server does it through admission). Under a limit, every stream reports its received total and receive maximum from the application's count, so both only grow as senders expect, even when the sender's earlier connection was forgotten after a restart or an eviction. A connection never takes more than its limit (asserted).

## Receiving

`handle_prepare` answers a Prepare. Its rules, beyond RFC 29's:

- **Money is credited only to a fulfilled packet.** Frames that change limits, addresses and closes apply whether or not the packet is fulfilled; money and data apply only if every check passes.
- **A packet with money must say where it goes:** every `StreamMoney` names an open stream, and a packet with an amount above 0 has at least one. The amount is split by [allocation](#allocation); if the streams cannot take it all, the packet is rejected.
- **The event queue cannot overflow.** It has room for a fixed number of packets' worth of events; while it lacks room for what one packet can cause, packets are rejected with T05 and senders retry.
- **Every response carries every open stream's limits and totals,** on fulfill and on reject, unlike `ilp-protocol-stream`, so the sender's view of them stays current whatever happened to its packet.

## Sending

`poll_transmit` writes the next request, if one is due.

- **The receiver's room is converted at the expected rate,** the rate the path is known to deliver, not the minimum the sender accepts. Converted at the minimum, a packet that fits would arrive too large whenever the real rate is better, and be refused forever.
- **Dust waits only while other money is in flight,** since those packets may fail and free a larger amount. Alone, a small amount goes, so a payment below the threshold completes.
- **Raised receive limits are sent again** if the packet that carried them was lost or refused, so the remote never waits on an update it missed.

## Responses

A reply inside a Fulfill or Reject is trusted only if it decrypts and its sequence and type are the request's. A rejection releases the packet's money and rewinds its data, then:

- **F99 with a reply showing less arrived than the minimum:** the rate is too low, and money stops until the application sets a rate, rather than retrying packets the path cannot deliver at that rate.
- **F08:** the largest packet amount falls by the ratio in the F08 data, or halves when the data is missing ([ILP](ilp.md#f08-data)). No back-off.
- **T or R class:** back off.
- **F class or an unknown class:** the path will not carry this connection's packets, so the connection closes, as `ilp-protocol-stream` does.

Packets in flight still settle after the connection closed, since money they carried may have been delivered.

### Congestion control

AIMD on the number of packets in flight: an extension RFC 29 does not have (`ilp-protocol-stream` sends one packet at a time). Packets in flight together are usually refused together, by the same full credit limit, so a burst of rejections counts as one back-off: only a packet sent after the last back-off triggers another. However many packets a connection has in flight, the connector caps the money in flight with a peer ([D24](../decisions/D24-money-in-flight-with-a-peer-stays-within-a-few-packets.md)).

## Exchange rates

A connection sends money only at a rate the application sets, the least the receiver must get per source unit, enforced through each packet's `prepareAmount`. The rate is measured only when the application asks (`measure_exchange_rate`): volleys of unfulfillable probes of growing size, retried at the packet maxima F08s reveal, until a result has enough significant digits; the connection then sends at that rate less slippage. A measurement that finds none fails as an event, not an error. The node's quotes measure routes this way ([Payments](payments.md)).

Conversions round so a sender never overpays.

## Allocation

The amount that arrives is split among a packet's streams by their shares, without giving any stream more than it can take. Unlike RFC 29 and `ilp-protocol-stream`, which reject the packet or drop the remainder when a stream is full, Dassie keeps the original's algorithm on purpose (`src/allocate.rs`): what a full stream cannot take goes to the others by their shares, and rounding remainders go to a stream with room. A packet then fails only when the streams together cannot take it. The split always adds up to exactly the amount (asserted).

## Data

Each stream carries an ordered byte stream each way, with RFC 29's flow control.

- **Only open streams count against the connection's limit,** on both sides, as in `ilp-protocol-stream`, so closing a stream frees its share. Counting closed streams too stalls a connection after one connection window.
- **Reassembly is bounded:** a stream keeps a fixed number of out-of-order fragments, and data that would need more is refused now, to be sent again.

## Receipts

A connection whose credentials carry receipt keys issues RFC 39 receipts. Its format has one byte for the stream ID, so streams above 255 get none. The node issues none: they are not part of Open Payments ([D9](../decisions/D09-open-payments-fully-and-no-spsp.md)).

## Closing

Closing a connection sends close packets that the remote can fulfill, a bounded number of times, and gives up after a timeout; with nobody to tell, it closes at once. A connection also closes when the remote closes it, breaks a rule, the path rejects with a final error, or nothing is heard for the idle timeout, which bounds how long an abandoned connection holds a server's table.

## Policy

Every limit of a connection is a field of `Policy` (`src/policy.rs`), with the original's default where it had one and `ilp-protocol-stream`'s where it did not, so both kinds of peer see familiar behaviour. A policy that does not hold together fails an assertion when a connection or server is created. The one bound with an argument behind it: at most 100 streams in all, so every packet can describe every open stream's limits within 512 frames and ILP's 32 KiB of data, as [Receiving](#receiving) requires.

## Differences from RFC 29 and `ilp-protocol-stream`

Compatible on the wire with both ([Frozen surfaces](../overview.md#frozen-surfaces)). Kept from the original because they work better and cost no compatibility: [AIMD concurrency](#congestion-control), [allocation](#allocation) that redistributes, exact integer amounts, and rate measurement only on request. Dassie's own: [dust](#sending) waits only while other money is in flight, [responses](#receiving) always carry every open stream's limits, and a [too-low rate](#responses) stops money until the application sets a new one.

## In the browser and in scripts

The WASM build (`src/wasm.rs`, with `dassie-identity`, [Identity](identity.md#in-the-browser-and-in-scripts)) exposes a client connection, a server and a packet decoder. The JavaScript side owns the clock and the network; `packages/identity-wasm/stream.ts` drives connections over an ILP endpoint for the website's STREAM toy and the interop tests ([Website](website.md)).

## What pins compatibility

- **Golden vectors** from the original (`tests/data/vectors.json`, by `testdata/original/`): its STREAM packet fixtures and crypto vectors. Where Dassie differs on purpose, the test has its own vector.
- **Interop** (`tests/interop/`): `dassie-stream`, native and WASM, against `ilp-protocol-stream` in both directions.
- **Simulations** (feature `sim`): a client and a server across a simulated connector with loss, latency and a rate, deterministic per seed ([D27](../decisions/D27-simulations-run-continuously-from-random-seeds.md), [Testing](testing.md)).

## Decisions

- [D9](../decisions/D09-open-payments-fully-and-no-spsp.md): STREAM under Open Payments, a connection per payment, no receipts from the node.
- [D16](../decisions/D16-engineering-style-pragmatic-tigerstyle-and-deterministic-simulation.md): sans-IO, deterministic, bounded and asserted.
- [D24](../decisions/D24-money-in-flight-with-a-peer-stays-within-a-few-packets.md): why the connector caps money in flight with a peer, though a STREAM connection may have many packets in flight.
- [D27](../decisions/D27-simulations-run-continuously-from-random-seeds.md): the STREAM simulation runs from any seed.
