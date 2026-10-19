# RPC contract

The typed contract between the node and its owner's clients: the wallet in the browser over a WebSocket, and the `dassie` CLI and the dev environment over the IPC socket. One contract serves both.

**Paths:** `proto/dassie/ui/v1/` (the contract: `transport.proto` holds the envelopes, errors and method list, the other files one area each), `packages/rpc/` (`src/gen/` is generated), `crates/dassie-node/src/rpc/`, `crates/dassie-node/src/http/ws.rs`, `crates/dassie-node/src/ipc.rs`, `crates/dassie-node/build.rs`
**Read with:** [HTTP](http.md), [Node](node.md), [Wallet](wallet.md), [Payments](payments.md), [Dev](dev.md)

## The contract

- **`proto/dassie/ui/v1` is normative:** change it first, then both sides. Each `.proto` file documents its methods' arguments, results and errors. `dassie-node`'s build script compiles it for Rust; `packages/rpc` generates the TypeScript (`node --run gen`), and records the contract's hash beside it, so a test fails when the contract changed without regenerating.
- **One contract, two transports.** The browser's WebSocket at `/rpc` and the IPC socket carry the same messages; only the framing and who the caller is differ.
- **Methods are numbered `oneof` members,** and a retired number is `reserved`, never reused. A node that does not know a method (the client is newer) answers `UNIMPLEMENTED`.
- **Every call ends with exactly one terminal message,** a response, a stream's end or an error, even when its handler panics. A failure is always a typed error, never a success that carries one; `INTERNAL` says only that the log has the details, so a bug never leaks internals to a client.
- The dev environment's control contract, `proto/dassie/dev/v1`, is separate ([dev.md](dev.md)); nothing in the node accepts it.

## Authentication

### IPC socket

Whoever can open the socket is the owner, and every call on it is the owner's: its file mode is the only access control. The daemon binds it so that it is never reachable with looser permissions ([node.md](node.md)).

### WebSocket

- A socket carries the session its cookie names, if any. An anonymous socket may call only `GetBasicState`, which returns only what anyone who can reach the node may know. The upgrade is held to the [same-origin rule](http.md#same-origin-rule).
- **The session is checked on every call,** not only at the upgrade, so logout and revocation take effect at once.
- **Anonymous sockets have places of their own:** fewer than all, a few per client, and a lifetime. The Origin check binds only browsers, so anyone can open one; this way sockets with a session always find room (`http/ws.rs`).

## A session's end closes its sockets

Every way a session ends (logout, revocation, expiry, eviction for a newer one) closes its sockets with the close code `SESSION_ENDED` (4001, `http/ws.rs`). The node stops reading, lets the calls in flight answer for a moment, then closes. The wallet's client reports it, and the page returns to the login screen.

## Live streams

The `Subscribe*` methods stream live state, which feeds the wallet's signals and stores. Each is a **value stream** (the whole value each time it changes), a **snapshot-plus-delta stream** (a snapshot, then upserts and removals by key; an item with `reset` replaces what the client holds, and a large snapshot is split over items), or, for `SubscribePackets`, an **event stream**, whose `lagged` events count what a subscriber that fell behind missed.

- **A stream does not replay the engine's events.** It follows the engine's change counts (ledger, network, payments), and when the count it follows moves it fetches the state afresh. So a slow stream never lags; it sees fewer, later states.
- **Only what is on disk is published:** a change appears once the step that made it is written, so a client never shows state a crash would undo.
- Items are coalesced to a few a second, the state is refetched now and then whatever the counts say, and a quiet stream sends a keepalive so the client can tell it from a dead one (`rpc/live.rs`).
- When the engine restarts, its streams end with `UNAVAILABLE` and the client subscribes again.

## Backpressure

Every queue, stream and socket is bounded ([D16](../decisions/D16-engineering-style-pragmatic-tigerstyle-and-deterministic-simulation.md)); the values are in `rpc/connection.rs`, `http/ws.rs` and `node/mod.rs`.

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

- [D5](../decisions/D05-ui-and-cli-rpc-contract.md): a protobuf contract in `proto/dassie/ui/v1` over one WebSocket per client and the IPC socket, typed errors, and `Subscribe*` streams instead of polling; why not JSON.
- [D16](../decisions/D16-engineering-style-pragmatic-tigerstyle-and-deterministic-simulation.md): every queue, stream and socket bounded; idle sockets close.
