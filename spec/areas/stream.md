# STREAM

STREAM sends money and data over Interledger: it splits a payment into ILP packets, encrypts what travels in them, and has the receiver report what arrived. `dassie-stream` implements it as two state machines, a connection and a server, compatible on the wire with `ilp-protocol-stream`. The node's STREAM receivers and payers, its quote probes and the website's STREAM toy all run on it.

**Paths:** `crates/dassie-stream/`, `crates/dassie-wasm/src/stream.rs`, `packages/wasm/web_stream.ts`, `tests/interop/stream_driver.ts`
**Read with:** [ILP](ilp.md), [Encoding](encoding.md), [Payments](payments.md), [Connector](connector.md), [Website](website.md)

The protocol is [RFC 29](https://interledger.org/developers/rfcs/stream-protocol/). Its receipts, [RFC 39](https://github.com/interledger/rfcs/tree/main/deprecated/0039-stream-receipts), are deprecated, and Dassie only decodes them ([Receipts](#receipts)). This doc states what Dassie chooses where they leave room, and where it differs from them and from `ilp-protocol-stream`, the reference implementation most peers run.

## Shape

- **Sans-IO and deterministic.** A connection never touches a socket, a clock or the OS RNG; the caller passes them in. The same code therefore runs in the node, in simulations and in the browser, and the same inputs give the same outputs ([tigerstyle](../decisions/tigerstyle.md)).
- **Bounded.** Every table, queue and buffer is sized from the connection's [policy](#policy) when it is created; nothing grows per packet. `check_invariants` checks a connection's and a server's state; tests and simulations call it after every step.
- **Exact amounts.** Amounts are `u64` and rates exact integer ratios, never floats. Wire values above 64 bits saturate, as RFC 29 asks.
- **Lenient on the wire, as RFC 29 asks:** padding after the frames, unknown frame types and bytes after a known frame's fields are skipped, so frames can gain fields without breaking older receivers. A packet has at most 512 frames, a bound RFC 29 does not set and no sender comes near.

## Credentials and tokens

A server derives each client's destination and shared secret from a 32-byte server secret and a random token, as `ilp-protocol-stream` does (`src/token.rs`). The token is encrypted under the server secret and carries the application's **connection tag** (for example the incoming payment it pays), and nothing else: not the receipt keys `ilp-protocol-stream` puts after it ([Receipts](#receipts)). Only the server that issued a token reads it, so its layout is the server's own. So the server keeps no table of issued credentials: they never pile up, and they survive restarts.

The node derives each receiver's server secret from its node key at a seed path that names the receiver's ledger by its ID, so the credentials it issued stay valid across restarts and no two receivers share a secret; that path never changes ([Identity](identity.md#seed-paths)).

## Server

A server routes every Prepare below its address to the connection its token names. The token is the first segment after the server's address, compared by whole segments ([ILP](ilp.md#addresses)); further segments are ignored, as `ilp-protocol-stream` does.

- **Admission.** Before any packet reaches a connection, new or existing, the application is asked with the connection's tag whether it may in. It can refuse with an ILP code, or accept and set the connection's [money limit](#money-limit). This is how a receiver ties a connection to what it pays into.
- **A full table** first forgets connections that are closed and settled, then the one heard from least recently, and only then rejects (T03). A forgotten sender's next packet reopens its connection, and admission restores what it may still receive, so no remote can keep the table full with idle connections. The [idle timeout](#closing) frees the rest.

## Connections

A connection is the **client** (given the destination and shared secret) or the **server** (which issued them). The client announces its address, and each side its asset (if it has one) and stream limit, in every request until a response shows the remote heard them, and again whenever one changes. A changed remote address resets congestion control, since the new path's capacity is unknown.

### Streams

- **A remote-opened stream** accepts `remote_stream_receive_max` until the application raises it: the application hears of the stream only after the packet that opened it, so a receiver that takes any payment must set this up front. The node's receivers set it to the maximum and bound the connection with its money limit instead.
- **Closed streams never reopen.** Money or data for one rejects the packet, and the response says the stream is closed. Closed remote IDs are remembered in fixed memory, as a window above a base that slides up as the remote opens higher IDs; IDs below the base count as closed, as QUIC treats them.
- **Our stream limit** rises as the remote's streams go, so it may open another in each one's place, as `ilp-protocol-stream` does.
- Frames for stream 0, for an ID of our parity we never opened, or above our stream limit end the connection.

### Money limit

An application that pays a connection into something with a total, such as an incoming payment, sets before each packet how much more the connection is to receive over all its streams, how much it already counts as received, and a tolerance (`set_money_limit`; a server does it through admission). Under a limit, every stream reports its received total from the application's count, so it only grows, even when the server forgot the sender's earlier connection after a restart or to make room in its table, for as long as the application keeps its count. The node's receivers drop a count only for a connection that gave its place in its incoming payment to a new payer, having paid less than dust ([Payments](payments.md#receiving)). Its receive maximum is that count plus the limit, so a sender aims at the total itself. **What a stream is told never falls,** as RFC 29 §4.4.4 requires: when something else takes from the total, such as another connection paying the same incoming payment, the stream keeps reporting the most it was told ([close-rather-than-lower](../decisions/close-rather-than-lower.md)). A stream opened since was never told more, and is told the room. Only a connection that was forgotten is told less: by the server, which no longer knows what it told, or by the application, whose count for it fell, after which what its streams were told no longer stands.

- **Within the tolerance.** A packet may overshoot what is left by at most the tolerance, and is taken: rounding on the way can put the last packet a little over, and no sender can size it to the unit. A limit within the tolerance counts as nothing left. The application gives the tolerance; the node's receivers give an incoming payment's ([paid-within-a-tolerance](../decisions/paid-within-a-tolerance.md)).
- **Over what it was told, refused and told.** A packet that overshoots by more, and overshoots what its streams were told too, is refused with F99, and its reply says what arrived, in its prepare amount, and what the receiver takes, in each stream's `StreamMaxMoney`: what the stream received plus the room left, or what it was told if more, and its total. F99 because the receiver will never take that packet; not F08, which is the path's largest packet and does not change, nor T04, which is liquidity that comes back.
- **Within what it was told but over what is left, closed.** A payment that fits what its streams were told but overshoots what is left by more than the tolerance met money something else took. A refusal could not tell its sender less, and a sender that keeps to RFC 29 would offer the packet until its deadline, unaware why, so the connection closes, with the code the application gave with the limit, as when nothing is left. Only a packet the receiver would fulfill closes it; a rate probe is refused as usual.
- **Nothing left, closed.** Once nothing is left, a packet that offers money closes the connection, a close every sender understands, with the code the application gave with the limit: `NoError` when the total was reached, `ApplicationError` when the application stopped short of it ([full-or-stopped-short](../decisions/full-or-stopped-short.md)). The response still reports the streams' totals, so the sender learns what it delivered.

Rate probes and packets without money are answered as usual. A connection never takes more than its limit and tolerance (asserted).

Dassie's sender takes a maximum that fell, from the receiver's latest reply ([Responses](#responses)), and sends what is left: a refused packet costs one round trip, and the next fits. Receivers that lower a maximum are other implementations, and a Dassie receiver that forgot the connection. Other senders only ever raise a stream's receive maximum, as RFC 29 §4.4.4 tells them to, since replies arrive out of order: `ilp-protocol-stream` 2.7.2-alpha.2 keeps the higher of the two in the `StreamMaxMoney` case of `handleControlFrames` (`dist/src/connection.js`), and `@interledger/pay` 0.4.0-alpha.9 takes `updateReceiveMax(reply).orGreater(...)` (`dist/src/senders/payment.js`). Told less by a receiver that lowered it, and left room that is less than their packet but more than nothing, they offer the packet again: `ilp-protocol-stream` at once, until its application gives up, `@interledger/pay` at its pace, until ten seconds pass without a Fulfill.

**A packet over what its streams were told is refused, and does not close the connection.** Every refusal is F99 with the reply above, however often it comes. A too-large packet can be innocent: one sized before the sender heard of a lower maximum, of many in flight, arrives after the refusal that told it, and closing would end a sender that would have sent what fits ([paid-within-a-tolerance](../decisions/paid-within-a-tolerance.md)).

## Receiving

`handle_prepare` answers a Prepare. Its rules, beyond RFC 29's:

- **Money is credited only to a fulfilled packet.** Frames that change limits, addresses and closes apply whether or not the packet is fulfilled; money and data apply only if every check passes.
- **A packet with money must say where it goes:** every `StreamMoney` names an open stream, and a packet with an amount above 0 has at least one. The amount is split by [allocation](#allocation); if the streams cannot take it all, the packet is rejected.
- **The event queue cannot overflow.** It has room for a fixed number of packets' worth of events; while it lacks room for what one packet can cause, packets are rejected with T05 and senders retry.
- **Every response carries every open stream's limits and totals,** on fulfill and on reject, unlike `ilp-protocol-stream`, so the sender's view of them stays current whatever happened to its packet.

## Sending

`poll_transmit` writes the next request, if one is due.

- **The receiver's room is converted at the expected rate,** the rate the path is known to deliver, not the minimum the sender accepts. Converted at the minimum, a packet that fits would arrive too large whenever the real rate is better, and be refused forever. What is in flight takes from the room already, so packets sent together never offer more than the receiver takes. The expected rate starts at the minimum and rises to the best rate a reply showed, whether its packet was fulfilled or refused.
- **A delivery cap bounds what a stream delivers** in total, as the receiver counts it, when the application sets one (`set_delivery_max`, as a payment that fixes what it delivers does: [receive-limit-caps-delivery](../decisions/receive-limit-caps-delivery.md)). What is left of it is converted at the expected rate, exactly, since hops only round down, less what is in flight. Until a reply showed a rate, a packet is offered at most half of what is left, so a path better than the minimum does not carry the first packet past it; if half fits no packet that delivers a unit but the whole does, the least such packet goes. The application can say what an earlier connection delivered (`set_delivered`), which counts until the receiver says more, so a new connection's cap starts from what is left. Under a cap, a packet must deliver a unit at the minimum rate, so none pays for nothing as the last units come near; a stream that can offer no such packet, with nothing in flight, has reached its cap (`delivery_cap_reached`). Packets in flight when the path's rate rises past the expected rate overshoot by that rise, and a rate learned from a small packet understates a larger one's by up to a unit per hop. Setting the minimum rate again starts the expected rate from it, and the halving with it.
- **A receiver whose maximum fell is asked again,** for receivers that lower it ([Money limit](#money-limit)). A stream with money left, none of it in flight, whose receiver lowered its maximum and now reports no room at all, offers the least that delivers a unit at its rate, once per back-off: something else takes from what the receiver pays into, and the receiver may close with why it takes no more, or report room it has again. So does a stream whose receiver reports less received than the application expects it to take (`set_expected_delivery`, as an incoming payment's amount): another payment may have taken from the total before the stream first heard its maximum, so that it never saw it fall. A receiver that never lowered its maximum, and took what it was expected to, is simply full, and is not asked. One that still reports room, less than a source unit delivers, is offered nothing it would have to refuse.
- **Dust waits only while other money is in flight,** since those packets may fail and free a larger amount. Alone, a small amount goes, so a payment below the threshold completes.
- **Raised receive limits are sent again** if the packet that carried them was lost or refused, so the remote never waits on an update it missed.

## Responses

A reply inside a Fulfill or Reject is trusted only if it decrypts and its sequence and type are the request's. Replies can arrive out of order, and the receiver can take packets in another order than they were sent, so which reply says what the receiver takes now, its latest word, is read from the total each reports a stream received. A reply that reports more than the latest word is the new latest word on that stream and sets its receive maximum, lower than before too. One that reports less was written before the latest word, unless its request was sent after the latest word was heard: the receiver's count can fall, when it dropped a connection's count for a new payer ([Money limit](#money-limit)), and then the lower total is the latest. A reply written before is stale and leaves the maximum as it is: taken, it would bring back room the receiver no longer has, such as after it came within its tolerance of the total, and the payer would read a payment its receiver completed as short ([paid-within-a-tolerance](../decisions/paid-within-a-tolerance.md)). Of replies that report the same, only the one to the newest request the connection heard back on sets the maximum; an older one may only raise it, as `ilp-protocol-stream` always does. A packet the remote sends of its own reports its limits the same way and counts as its latest word only if it reports more than the latest. The room is the latest word's maximum less its total; what the stream is known to have delivered is the most any of them reported. After a dropped count, a reply the receiver wrote before the drop and that arrives late reports more, and brings back the room from before the drop until the next reply corrects it; nothing tells the two apart, and the cost is at most one refusal.

**A Reject with a trusted reply is the receiver's word, whatever its code; one without came from a node on the path,** since only the receiver can write a reply, and nothing else in a Reject proves who sent it. The packets of one payment may take different paths ([weighted-next-hops](../decisions/weighted-next-hops.md)), so a failure on one path says little about the next packet ([path-failures-retried](../decisions/path-failures-retried.md)). A rejection releases the packet's money and rewinds its data, then:

- **F99 from the receiver showing less arrived than the minimum:** the path the packet took delivers too little. Its money goes again in a later packet, which may take a cheaper path. No back-off: it is not congestion.
- **F class or an unknown class from a node on the path:** that node will not carry the packet. Its money goes again too, with no back-off, so one bad node slows no other path. A close packet is bounded by its close attempts instead.
- **No progress:** `MAX_FAILURES_WITHOUT_PROGRESS` of these two in a row, with no Fulfill of a packet with money between, stop money until the application sets a rate again, and an event says so, with the latest too-low refusal among them, if any. So a sender that no path serves stops looping and its application decides: a lower rate, a new connection, or giving up. Once money stopped, a failure on the path backs off as a T class does, so data cannot loop either. Only a Fulfill of a packet with money ends a run: one without money shows nothing of the rate, and data flowing would keep money retrying. Other failures neither count nor end one.
- **Other F99 from the receiver:** its limits apply; back off, since sending again at once could repeat the refusal.
- **F08:** the largest packet amount falls by the ratio in the F08 data, or halves when the data is missing ([ILP](ilp.md#f08-data)). No back-off.
- **T or R class:** back off.
- **F class or an unknown class from the receiver:** the receiver will not take this connection's packets, so the connection closes, as `ilp-protocol-stream` does.

A rate measurement's probes are refused by design, and are not counted ([Exchange rates](#exchange-rates)).

Packets in flight still settle after the connection closed, since money they carried may have been delivered.

### Congestion control

AIMD on the number of packets in flight: an extension RFC 29 does not have (`ilp-protocol-stream` sends one packet at a time). Packets in flight together are usually refused together, by the same full credit limit, so a burst of rejections counts as one back-off: only a packet sent after the last back-off triggers another. However many packets a connection has in flight, the connector caps the money in flight with a peer ([packets-in-flight](../decisions/packets-in-flight.md)).

## Exchange rates

A connection sends money only at a rate the application sets, the least the receiver must get per source unit, enforced through each packet's `prepareAmount`. The rate is measured only when the application asks (`measure_exchange_rate`): volleys of unfulfillable probes of growing size, until a result has enough significant digits; the connection then sends at that rate less slippage. Each further volley tries the packet maxima F08s revealed and, smaller, every probe that was lost or refused for the moment, unless a reply already showed a larger amount, which it could not improve on. A lossy path therefore fails a measurement only by losing every useful probe of every volley, not by losing the wrong two. A measurement that finds none within its volleys fails as an event, not an error, and the application decides whether to measure again. The node's quotes measure routes this way ([Payments](payments.md)).

Conversions round so a sender never overpays.

## Allocation

The amount that arrives is split among a packet's streams by their shares, without giving any stream more than it can take. Unlike RFC 29 and `ilp-protocol-stream`, which reject the packet or drop the remainder when a stream is full, Dassie keeps the original's algorithm on purpose (`src/allocate.rs`): what a full stream cannot take goes to the others by their shares, and rounding remainders go to a stream with room. A packet then fails only when the streams together cannot take it. The split always adds up to exactly the amount (asserted).

## Data

Each stream carries an ordered byte stream each way, with RFC 29's flow control.

- **Only open streams count against the connection's limit,** on both sides, as in `ilp-protocol-stream`, so closing a stream frees its share. Counting closed streams too stalls a connection after one connection window.
- **Reassembly is bounded:** a stream keeps a fixed number of out-of-order fragments, and data that would need more is refused now, to be sent again.

## Receipts

`dassie-stream` issues no RFC 39 receipts: RFC 39 is deprecated in favour of Open Payments, which has none ([open-payments](../decisions/open-payments.md)). It still decodes the `StreamReceipt` frame, so a receiver that sends receipts, as `ilp-protocol-stream` does when its credentials carry receipt keys, can be paid, and the website's packet decoder names the frame. A connection ignores the receipts it is sent.

## Closing

Closing a connection sends close packets that the remote can fulfill, a bounded number of times, and gives up after a timeout; with nobody to tell, it closes at once. A connection also closes when the remote closes it, breaks a rule, rejects with a final error, or nothing is heard for the idle timeout, which bounds how long an abandoned connection holds a server's table. A connection that closed itself answers every later packet with the code it closed with, so a sender that hears a later reply first learns the same as from the first ([full-or-stopped-short](../decisions/full-or-stopped-short.md)); one the remote closed answers `NoError`.

## Policy

Every limit of a connection is a field of `Policy` (`src/policy.rs`), with the original's default where it had one and `ilp-protocol-stream`'s where it did not, so both kinds of peer see familiar behaviour. A policy that does not hold together fails an assertion when a connection or server is created. The one bound with an argument behind it: at most 100 streams in all, so every packet can describe every open stream's limits within 512 frames and ILP's 32 KiB of data, as [Receiving](#receiving) requires.

## Differences from RFC 29 and `ilp-protocol-stream`

Compatible on the wire with both ([Frozen surfaces](../overview.md#frozen-surfaces)). Kept from the original because they work better and cost no compatibility: [AIMD concurrency](#congestion-control), [allocation](#allocation) that redistributes, exact integer amounts, and rate measurement only on request. Dassie's own: [dust](#sending) waits only while other money is in flight, [responses](#receiving) always carry every open stream's limits, a [too-low rate or a node's final error](#responses) sends the money again, until failures without progress stop it, the sender takes a receive maximum that [fell](#money-limit), a receiver under a money limit takes a packet within its [tolerance](#money-limit) over what is left, and closes a connection whose payment fits what it was told but not what is [left](#money-limit).

## In the browser and in scripts

The WASM build (`crates/dassie-wasm`'s `stream` module, [Identity](identity.md#in-the-browser-and-in-scripts)) exposes a client connection, a server and a packet decoder. The JavaScript side owns the clock and the network: the website's STREAM toy steps its connections itself ([Website](website.md)), and the interop tests drive them over an ILP endpoint with `tests/interop/stream_driver.ts`.

## What pins compatibility

- **Golden vectors** from the original (`tests/data/vectors.json`, by `testdata/original/`): its STREAM packet fixtures and crypto vectors. Where Dassie differs on purpose, the test has its own vector.
- **Interop** (`tests/interop/`): `dassie-stream`, native and WASM, against `ilp-protocol-stream` in both directions.
- **Simulations** (feature `sim`): a client and a server across a simulated connector with loss, latency and a rate, deterministic per seed ([random-seed-simulations](../decisions/random-seed-simulations.md), [Testing](testing.md)).

## Decisions

- [open-payments](../decisions/open-payments.md): STREAM under Open Payments, and no receipts; why each payment has a connection of its own is in [Payments](payments.md).
- [tigerstyle](../decisions/tigerstyle.md): sans-IO, deterministic, bounded and asserted.
- [packets-in-flight](../decisions/packets-in-flight.md): why the connector caps money in flight with a peer, though a STREAM connection may have many packets in flight.
- [random-seed-simulations](../decisions/random-seed-simulations.md): the STREAM simulation runs from any seed.
- [owners-share](../decisions/owners-share.md): why a node's receiver may forget what a connection that paid only dust received.
- [full-or-stopped-short](../decisions/full-or-stopped-short.md): why the application gives the code that closes a connection with nothing left.
- [paid-within-a-tolerance](../decisions/paid-within-a-tolerance.md): why a money limit has a tolerance, and how large the node's receivers make it.
- [path-failures-retried](../decisions/path-failures-retried.md): why a too-low rate or a node's final error sends the money again, until failures without progress stop it.
- [weighted-next-hops](../decisions/weighted-next-hops.md): why a payment's packets may take different paths.
