# RPC contract

The typed contract between the node and its owner's clients: the wallet in the browser over a WebSocket, the `dassie` CLI and the dev environment over the IPC socket, and paired owner apps over relayed streams. One contract serves all three.

**Paths:** `proto/dassie/owner/v1/` (the contract: `transport.proto` holds the envelopes, errors and method list, the other files one area each), `packages/rpc/` (`src/gen/` is generated), `crates/dassie-node/src/rpc/`, `crates/dassie-node/src/http/ws.rs`, `crates/dassie-node/src/ipc.rs`, `crates/dassie-node/src/owner_app.rs`, `crates/dassie-node/src/node/devices.rs`, `crates/dassie-node/build.rs`
**Read with:** [HTTP](http.md), [Node](node.md), [Wallet](wallet.md), [Payments](payments.md), [Dev](dev.md)

## The contract

- **`proto/dassie/owner/v1` is normative:** change it first, then both sides. Each `.proto` file documents its methods' arguments, results and errors. `dassie-node`'s build script compiles it for Rust; `packages/rpc` generates the TypeScript (`node --run gen`), and records the contract's hash beside it, so a test fails when the contract changed without regenerating. `buf lint` checks it in CI.
- **The package is `dassie.owner.v1`:** the owner's contract, whichever client speaks it (the wallet, the CLI, a paired app), and the WebSocket's subprotocol has the same name.
- **A point in time is an `int64` named `*_unix_ms`,** milliseconds since 1970, in every message; a duration is a `uint64` named `*_ms`. Signed, so that a client's language reads it as it reads its own clock.
- **One contract, three transports.** The browser's WebSocket at `/rpc`, the IPC socket and an owner app's relayed stream carry the same messages; only the framing and who the caller is differ.
- **Methods are numbered `oneof` members.** While every client ships in the same release as the node, a method or field that goes is deleted with its number, and nothing in the contract keeps a fallback for an older peer of the same binary; once owner apps ship apart from the node, a retired number is `reserved`, never reused. A node that does not know a method (the client is newer) answers `UNIMPLEMENTED`.
- **Every call ends with exactly one terminal message,** a response, a stream's end or an error, even when its handler panics. A failure is always a typed error, never a success that carries one; `INTERNAL` says only that the log has the details, so a bug never leaks internals to a client.
- The dev environment's control contract, `proto/dassie/dev/v1`, is separate ([dev.md](dev.md)); nothing in the node accepts it.

## Authentication

### IPC socket

Whoever can open the socket is the owner, and every call on it is the owner's: its file mode is the only access control. The daemon binds it so that it is never reachable with looser permissions ([node.md](node.md)).

### WebSocket

- A socket carries the session its cookie names, if any. An anonymous socket may call only `GetBasicState`, which returns only what anyone who can reach the node may know. The upgrade is held to the [same-origin rule](http.md#same-origin-rule).
- **The session is checked on every call,** not only at the upgrade, so logout and revocation take effect at once.
- **Anonymous sockets have places of their own:** fewer than all, a few per client, and a lifetime. The Origin check binds only browsers, so anyone can open one; this way sockets with a session always find room (`http/ws.rs`).

### Paired devices

An owner app is paired with the node through a device key, an Ed25519 key it makes itself, and then calls the node as the owner ([the design](../designs/periphery.md#6-owner-apps); [device-pairing](../decisions/device-pairing.md)).

- **The app reaches the node on a relayed stream with purpose `owner`** ([Peer protocol](peer-protocol.md#relayed-streams)), through a relay the node keeps a connection to or by dialling the node's QUIC endpoint directly. The stream's TLS is pinned to the node's key and proves the device key, so a relay reads and forges nothing. Messages are framed as on the IPC socket. The wallet, `/api/*` and `/rpc` are still never relayed: an app proves a key rather than carrying a browser's cookie.
- **Only the owner on the node's own machine starts a pairing:** over the IPC socket (`dassie pair`), or in a browser that reached the node from a loopback address. A session's cookie used from anywhere else cannot turn itself into a device's lasting access, which would survive revoking every session. A paired device cannot start one either.
- **A pairing is one secret, in memory only,** good for a few minutes and claimed once. Starting another replaces it, and a restart forgets it. The pairing code is a `dassie-pair:` URI with a version (`v`), the node ID, the secret, and where the node is reached: its own URL, where its QUIC endpoint answers there, else the peers it is connected to, which relay to it, a few at most. The wallet shows it as a QR code. An app reads a code of a version it knows and refuses another, and ignores parameters it does not know, so a version adds parameters without a new number; the code is read by apps outside the node's release, which is why it has a version at all.
- **A device that is not paired may call only `GetBasicState` and `ClaimPairing`,** which pairs the key its stream proved under the name it gives. Its stream lasts only as long as an anonymous socket does, whatever it sends, so that streams nobody can use do not hold the places of the owner's apps at a relay; once paired, the app connects again. The node keeps a bounded list of devices; a full list refuses a new one rather than dropping one the owner paired.
- **The device is checked on every call,** as a session is. Revoking it closes its streams, as a session's end closes its sockets.

## A session's end closes its sockets

Every way a session ends (logout, revocation, expiry, eviction for a newer one) closes its sockets with the close code `SESSION_ENDED` (4001, `http/ws.rs`). The node stops reading, lets the calls in flight answer for a moment, then closes. The wallet's client reports it, and the page returns to the login screen.

## Live streams

The `Subscribe*` methods stream live state, which feeds the wallet's signals and stores. Each is a **value stream** (the whole value each time it changes), a **snapshot-plus-delta stream** (a snapshot, then upserts and removals by key; an item with `reset` replaces what the client holds, and a large snapshot is split over items), or, for `SubscribePackets`, an **event stream**, whose `lagged` events count what a subscriber that fell behind missed.

- **A stream does not replay the engine's events.** It follows the engine's change counts (ledger, network, payments), and when the count it follows moves it fetches the state afresh. So a slow stream never lags; it sees fewer, later states.
- **Only what is on disk is published:** a change appears once the step that made it is written, so a client never shows state a crash would undo.
- Items are coalesced to a few a second, the state is refetched now and then whatever the counts say, and a quiet stream sends a keepalive so the client can tell it from a dead one (`rpc/live.rs`).
- When the engine restarts, its streams end with `UNAVAILABLE` and the client subscribes again.

## Backpressure

Every queue, stream and socket is bounded ([tigerstyle](../decisions/tigerstyle.md)); the values are in `rpc/connection.rs`, `http/ws.rs` and `node/mod.rs`.

- **A client that does not read is disconnected, not buffered for.** Each connection has a bounded queue toward its client, drained only by the connection's own loop. A client that does not accept a message in time is disconnected, and so is one whose refusals no longer fit in the queue: it sends faster than it reads.
- **Live items wait for room; progress items are dropped.** A lost live item would corrupt the client's state, while a lost progress step (`ObtainCertificate`) is harmless.
- A call past the limits on calls or streams in flight gets `RESOURCE_EXHAUSTED`, and the connection stays open.

## The inspector

`SubscribeLogs`, `ListDatabaseTables`, `ListDatabaseRows` and `ListServices` show the node's own workings for the debug pages. They do not need the engine, so they work before setup, over IPC.

- **The database is read without secrets.** The SQL is built only from the schema's own table definitions, never from a name the caller passed. Secret columns (`SECRET_COLUMNS` in `store/schema.rs`) are selected as `NULL`, so their values are never read, and are marked redacted.
- **The log ring** keeps the latest lines whatever the log level says, with fields named like secrets redacted ([node.md](node.md#logging)).

## The TypeScript client

`packages/rpc` (`@dassie/rpc`) is the client of the wallet, the dev dashboard and the Node tooling; `@dassie/rpc/ipc` runs it over the IPC socket.

- **A lost connection fails the calls in flight** with `UNAVAILABLE`; they are not sent again, since a call may have changed the node. The client reconnects with a jittered backoff, and calls made meanwhile wait for the connection.
- **Cancelling** a call (an `AbortSignal`, or its timeout) sends `Cancel`, so the node stops working on it too.
- **`SESSION_ENDED`** calls `onSessionEnded`.

## Decisions

- [rpc-contract](../decisions/rpc-contract.md): a protobuf contract in `proto/dassie/owner/v1` over one WebSocket per client and the IPC socket, typed errors, and `Subscribe*` streams instead of polling; why not JSON.
- [tigerstyle](../decisions/tigerstyle.md): every queue, stream and socket bounded; idle sockets close.
- [device-pairing](../decisions/device-pairing.md): only the owner on the node's machine starts a pairing, its secret kept in memory and claimed once over the `owner` stream; a full device list refuses a new device.
