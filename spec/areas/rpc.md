# RPC contract

The typed contract between the node and its owner's clients: the wallet in the browser over a WebSocket, and the `dassie` CLI and the dev environment over the IPC socket. One contract serves both.

**Paths:** `proto/dassie/ui/v1/`, `packages/rpc/` (`mod.ts`, `ipc.ts`, `proto.ts`, `scripts/`; `src/gen/` is generated), `crates/dassie-node/src/rpc/`, `crates/dassie-node/src/http/ws.rs`, `crates/dassie-node/src/ipc.rs`, `crates/dassie-node/build.rs`
**Read with:** [HTTP](http.md), [Node](node.md), [Wallet](wallet.md), [Payments](payments.md), [Dev](dev.md)

## The contract

- `proto/dassie/ui/v1` is normative: change it first, then both sides. `transport.proto` holds the envelopes, errors and the method list; the other files hold one area's messages each.
- **Rust:** `dassie-node`'s build script compiles the `.proto` files with protox (no `protoc` binary) into `dassie.ui.v1.rs`.
- **TypeScript:** `packages/rpc` generates `src/gen/` with buf and protoc-gen-es (`node --run gen` in `packages/rpc`). Each contract's generated directory records the contract's SHA-256 (`contract.sha256`: each `.proto` file in name order, as its name, a newline, its length, a newline and its bytes), and a test fails when the contract changed without regenerating.
- **Encoding:** protobuf binary.
- **Amounts** are unsigned integers in the asset's smallest unit, with the asset's scale given beside them: an amount of 1 is 10^-scale units of the asset. Open Payments amounts carry their own asset code and scale (`OpenPaymentsAmount`), since what a receiver gets may be in another asset.
- **Shape:** unary calls and server streams. Streams are value streams, snapshot-plus-delta streams, or event streams ([Live streams](#live-streams)).
- The dev environment has its own control contract, `proto/dassie/dev/v1`, between the dev server and its dashboard ([dev.md](dev.md)); `packages/rpc/dev.ts` exports it. Nothing in the node accepts it.

## Envelopes

```
ClientMessage { call_id: uint32, kind: oneof { Request request, Cancel cancel } }
ServerMessage { call_id: uint32, kind: oneof { Response response, StreamItem item, StreamEnd end, Error error } }
Request    { oneof method { <one typed request per method> } }
Response   { oneof method { <one typed response per unary method> } }
StreamItem { oneof method { <one typed item per streaming method> } }
Error      { ErrorCode code, string message }
```

- Methods are `oneof` members, not string paths. A method's number is the same in `Request`, `Response` and `StreamItem`: its response or items always carry its request's number. Retired numbers are `reserved`, such as 70, the retired `CreateStreamCredentials`: incoming payments hand out STREAM credentials instead.
- The client picks the call ID, unique among its calls in flight. The ID may be reused once the call has ended. A request that reuses the ID of a call in flight is a protocol error and closes the connection.
- The server answers each call with exactly one of:
  - a `Response`, for a unary method;
  - any number of `StreamItem`s, then a `StreamEnd`, for a streaming method;
  - an `Error`, which ends either kind of call.
- Every call ends with exactly one terminal message, even when its handler panics (`INTERNAL`).
- A `Cancel` ends a call early: the server stops its work and sends nothing more for it.
- Calls run concurrently; answers may arrive in any order.
- A message that does not decode as a `ClientMessage`, or one with neither a request nor a cancel, is a protocol error and closes the connection. A request whose method the node does not know (the client is newer) gets `UNIMPLEMENTED`.

## Errors

Every failure is an `Error` with a code the client branches on and a message for people. No success response ever carries a failure.

| Code | Meaning |
|---|---|
| `UNAUTHENTICATED` | the call needs an authenticated session (`log in first`) |
| `INVALID_ARGUMENT` | the request is malformed or a value is out of range |
| `FAILED_PRECONDITION` | the node is not in a state that allows the call: setup is done, no hostname, the live realm, funds that do not cover a payment |
| `NOT_FOUND` | no such thing |
| `RESOURCE_EXHAUSTED` | too many calls in flight, too many streams, or another limit was reached |
| `UNAVAILABLE` | something the node depends on failed or is not running (the engine, the ACME server, another wallet); retrying later may help. A lost connection also fails calls with it on the client side |
| `INTERNAL` | a bug in the node. The message is only `internal error; see the node's log`; the details go to the log |
| `UNIMPLEMENTED` | the node does not know the method |

## Transports

| | WebSocket | IPC socket |
|---|---|---|
| Where | `wss://<host>/rpc`, subprotocol `dassie.ui.v1` | the daemon's Unix socket ([node.md](node.md) has its path) |
| Framing | one message per binary WebSocket message | one message per frame with a 4-byte big-endian length prefix |
| Message limit | 1 MiB, either way | 1 MiB, either way |
| Who | anonymous, or the session the cookie names | always the owner |
| Calls in flight per connection | 16 | 16 |
| Live streams per connection | 12 | 12 |
| Connections | 256 sockets; 128 of them without a session, 4 of those per client | 64 |
| Idle timeout | 10 minutes without a message either way, while no call is in flight | none |
| Lifetime | 60 s for a socket without a session | none |

A call past the limit on calls or streams gets `RESOURCE_EXHAUSTED` (`too many calls in flight`, `too many streams open`) and the connection stays open. The node serves at most 64 live streams over all connections (`RESOURCE_EXHAUSTED` `too many live streams`).

**Flow control.** Each connection queues at most 64 messages for its client, and only the connection's own loop drains the queue. The client must accept each message within 30 s, or it is disconnected. A refusal that does not even fit in the queue disconnects the client: it sends faster than it reads. Progress items (`ObtainCertificate`) are dropped when the queue is full; live items wait for room, since a lost item would corrupt the client's state.

### WebSocket

The node answers the handshake itself, so it can refuse it properly:

| Check | Refusal |
|---|---|
| A WebSocket handshake: `Upgrade: websocket`, `Sec-WebSocket-Version: 13`, a key | 400 `Expected a WebSocket handshake` |
| `Sec-WebSocket-Protocol` lists `dassie.ui.v1` | 400 `Expected the dassie.ui.v1 subprotocol` |
| The [same-origin rule](http.md#same-origin-rule) | 403 |
| A place among the 256 sockets, and, without a valid session, among the 128 anonymous ones and the client's 4 | 503 `Too many open sockets` |

A client is an IPv4 address or an IPv6 /56; loopback is not counted per client where private peers are allowed. Anyone can open an anonymous socket, since the Origin check binds only browsers; so anonymous sockets have places of their own, fewer than all, a few per client and a lifetime, and sockets with a session always find room. A socket holds its places until it closes.

Only binary messages carry the contract. A text message is a protocol error.

How the node closes a socket:

| Why | Code | Reason |
|---|---|---|
| Protocol error | 1002 | `expected dassie.ui.v1 messages` |
| The session ended: logout, revocation, expiry, eviction for a newer one | 4001 | `session ended` |
| Idle: no call in flight and no message for 10 minutes | 1000 | `idle` |
| A socket without a session reached its 60 s lifetime, whatever it sent | 1000 | `lifetime over` |
| The node shuts down | 1001 | `node shutting down` |

On a session's end and at the end of the lifetime, the node stops reading, lets the calls in flight send their answers (for at most 1 s), then closes. The client reconnects on its next call.

### IPC socket

Whoever can open the socket is the owner: its file mode (0660, the `dassie-users` group under systemd) is the only access control, and every call is authenticated. The daemon binds it so that it is never reachable with looser permissions, before the daemon answers, or after a daemon that failed ([node.md](node.md)). IPC connections have no idle timeout and no lifetime.

## Authentication

- `GetBasicState` is the only public method: an anonymous socket may call it and nothing else. Every other call on a socket without a session gets `UNAUTHENTICATED`.
- A browser socket's session is checked against the database on every call, not only at the upgrade, so logout and revocation take effect at once; the socket also closes with 4001 when the session ends.
- IPC calls are always the owner's.
- `GetBasicState` says `UNINITIALIZED` before setup, and after it `AUTHENTICATED` or `ANONYMOUS` for the caller; with the node ID, hostname, ILP allocation scheme (`test` or `g`), version and active settlement schemes. It returns only what anyone who can reach the node may know.

## Availability

- Methods numbered 50 and up (ledgers, BTP tokens, Open Payments, the books, the network, grants, Send, peer settings, rates) are `UNAVAILABLE` while the node's engine is not running: before setup, or for a moment after a failure.
- The inspector methods (`SubscribeLogs`, `ListDatabaseTables`, `ListDatabaseRows`, `ListServices`) do not need the engine and work before setup too, over IPC.
- Open Payments methods are `FAILED_PRECONDITION` until the node has a hostname, which every Open Payments URL names.

## Live streams

The `Subscribe*` methods stream live state, which feeds the wallet's signals and stores. A stream does not replay the engine's events: it follows the engine's change counts (ledger, network, payments), and when the count it follows moves it fetches the state afresh. So a slow stream never lags; it sees fewer, later states.

- **Value streams** send the whole value each time it changes: `SubscribeLedgers`, `SubscribeRoutes`, `SubscribeRegistration`, `SubscribeGrants`, `SubscribeOutgoingPayment`.
- **Snapshot-plus-delta streams** send a snapshot, then upserts and removals by key: `SubscribeNodes`, `SubscribeIncomingPayments`, `SubscribeOutgoingPayments` (the newest N, up to 1000, 100 by default; `removed` lists those no longer among them), `SubscribeTransfers` (the journal's last rows, then rows written or changed; a settlement's row comes again when it is resolved, and clients replace rows by ID), `SubscribeLogs`.
  - The first item, and the first of every snapshot split over several items, has `reset` set; it replaces what the client holds.
  - An item is at most 256 KiB; a larger snapshot is split.
- **`SubscribePackets`** is an event stream: each packet as it arrives and as it is resolved. Its `lagged` events count what a subscriber that fell behind missed; the first item, and one at least every minute, has 0, so the client knows the stream is live.

Timing:

| Rule | Value |
|---|---|
| What is published | only what is on disk: a change appears once the step that made it is written |
| Most items | 4 a second per stream (one per 250 ms), coalescing changes in between |
| Refetch | every 5 s, whatever the counts say |
| Keepalive | an item at least every 60 s, so a client can tell a quiet stream from a dead one |
| Engine restart | the stream ends with `UNAVAILABLE`; the client subscribes again |

`ObtainCertificate` also streams: progress steps, ending after `Installed` ([http.md](http.md#acme)).

## Procedure groups

These are the methods of `transport.proto`, by area, with their numbers. Each file of `proto/dassie/ui/v1` documents its methods' arguments, results and errors.

| Numbers | Area (file) | Methods |
|---|---|---|
| 1–2 | Node (`node.proto`) | `GetBasicState` (public), `GetStatus` (version, runtime, start time, certificate, ACME account and next renewal check, ports) |
| 10–15 | Configuration (`config.proto`) | `GetConfig`, `GetSetupUrl` (until setup is done), `SetHostname`, `SetRealm` (`live` is `FAILED_PRECONDITION` until real money is enabled), `SetPorts` (says whether a restart is needed), `SetAlias`. The settings themselves are in [node.md](node.md) |
| 20 | TLS (`tls.proto`) | `SetCertificate` ([http.md](http.md#certificates)) |
| 30–31 | ACME (`acme.proto`) | `ObtainCertificate` (streaming), `GetAcmeTerms` ([http.md](http.md#acme)) |
| 40–42 | Sessions (`sessions.proto`) | `ListSessions`, `RevokeSession`, `RevokeAllSessions` ([http.md](http.md#sessions)) |
| 50–55 | Ledgers (`ledger.proto`) | `ListLedgers`, `AddSettlementScheme`, `RemoveSettlementScheme`, `StubDeposit`, `ListSettlementSchemes`, `SubscribeLedgers` ([ledger.md](ledger.md), [settlement.md](settlement.md), [xrpl.md](xrpl.md)) |
| 60–63 | BTP tokens (`btp.proto`) | `CreateBtpToken` (the only answer with the token), `ListBtpTokens`, `RevokeBtpToken`, `SetBtpTokenCap` ([connector.md](connector.md)) |
| 71–78 | Open Payments, the owner's side (`payments.proto`) | `ListWalletAddresses`, `SetDefaultLedger`, `CreateIncomingPayment`, `CompleteIncomingPayment`, `ListIncomingPayments`, `SubscribeIncomingPayments`, `ListOutgoingPayments`, `SubscribeOutgoingPayments` ([payments.md](payments.md#the-owners-side)) |
| 80–84 | The books (`debug.proto`) | `ListAccounts`, `ListTransfers`, `ListPackets`, `SubscribePackets`, `SubscribeTransfers` ([ledger.md](ledger.md), [connector.md](connector.md)) |
| 90–102 | Network (`network.proto`) | `ListNodes`, `ListPeers`, `ListRoutes`, `GetRegistrationStatus`, `TerminatePeering`, `BanNode`, `UnbanNode`, `ListBans`, `ListSettlementChannels`, `RegisterWithBootstrap`, `SubscribeNodes`, `SubscribeRoutes`, `SubscribeRegistration` ([network.md](network.md)) |
| 110–113 | Inspector (`inspect.proto`) | `SubscribeLogs`, `ListDatabaseTables`, `ListDatabaseRows`, `ListServices` (the supervisor's services and cells) |
| 114–118 | Grants (`payments.proto`) | `GetGrantRequest`, `DecideGrant` (the consent page), `ListGrants`, `SubscribeGrants`, `RevokeGrant` ([payments.md](payments.md#the-owners-consent)) |
| 119–122 | Send (`payments.proto`) | `ResolveRecipient`, `QuoteSend`, `SendPayment`, `SubscribeOutgoingPayment` ([payments.md](payments.md#the-owners-send)) |
| 123–124 | Peer settings (`network.proto`) | `GetPeerSettings`, `SetPeerSettings`: the owner's fee and spread ([connector.md](connector.md)) |
| 125–127 | Rates (`ledger.proto`) | `GetRateSources`, `SetRateSources`, `SetDisplayCurrency` ([Connector](connector.md#exchange-rates)) |

### The inspector

- **`SubscribeLogs`:** the node keeps a ring of its last 1000 log lines, at most 1 MiB, whatever `DASSIE_LOG_LEVEL` says: info and above, and debug for the targets `DEBUG` names. Fields named like secrets are redacted ([node.md](node.md)). The stream sends what the ring holds (oldest first, `reset` set), then each new line, at most 200 lines and 256 KiB an item, and counts lines dropped from the ring before they could be sent (`skipped`).
- **`ListDatabaseTables`, `ListDatabaseRows`:** every table and its rows, a page of at most 200 rows at a time, text cut at 1024 characters and blobs at 256 bytes. The SQL is built only from the schema's own table definitions, never from a name the caller passed. Secret columns are selected as `NULL`, so their values are never read, and are marked `redacted`: `node_key.private_key`, `tls_certificate.key_pem`, `acme_account.credentials`, `sessions.id`, `btp_tokens.token_hash`, `incoming_payments.shared_secret`, `gnap_grants.continue_token_hash`, `gnap_grants.interaction`, `gnap_tokens.token_hash`, `quotes.shared_secret`, `outgoing_payments.shared_secret`, `setup_token.token`. An unknown table is `INVALID_ARGUMENT`.
- **`ListServices`:** the supervised services with their state, and the shared cells they follow ([node.md](node.md)).

## The TypeScript client

`packages/rpc` (`@dassie/rpc`) is the client of the wallet, the dev dashboard and the Node tooling:

- `RpcClient` opens one WebSocket to `/rpc` with the subprotocol `dassie.ui.v1` when the first call needs it, and reconnects as needed. `call(method, request)` is a unary call; streams deliver their items to a callback.
- **Timeouts:** every unary call has one, 15 s by default; a stream fails after 5 minutes without a message by default. A call that times out fails and is cancelled on the node.
- **Cancelling:** an `AbortSignal` cancels a call or a stream, and the client sends `Cancel`, so the node stops too.
- **A lost connection** fails the calls in flight with `UNAVAILABLE`; they are not sent again blindly, since a call may change the node. The next connection waits 1 s, doubling after each loss up to 30 s, each wait multiplied by a random factor from 0.5 to 1.5; a successful open resets it. Calls made while waiting queue and go out once it opens.
- **Close code 4001** calls `onSessionEnded`, so the page can return to the login screen.
- The connection's state (`idle`, `connecting`, `open`, `waiting`, `closed`) can be followed, for a status light.
- `@dassie/rpc/ipc` (`ipcConnector`) runs the same client over the IPC socket with length-prefixed frames, for the Node tooling and tests; it holds at most 8 MiB waiting to be written before it takes the node to be stuck.

## Decisions

- [D5](../decisions/D05-ui-and-cli-rpc-contract.md): a protobuf contract in `proto/dassie/ui/v1` over one WebSocket per client and the IPC socket, typed errors, and `Subscribe*` streams instead of polling; why not JSON.
- [D16](../decisions/D16-engineering-style-pragmatic-tigerstyle-and-deterministic-simulation.md): every queue, stream and socket bounded; idle sockets close.

## Original

- [2 §10](../original/02-node-runtime.md): `dassie-rpc-01` over superjson, its error mapping that lost failures (RT-5), its client that hung across reconnects (RT-27), and its procedure table, which the groups above replace.
- [2 §11](../original/02-node-runtime.md): the original's IPC server and client, newline-framed.
- [2 §6.6](../original/02-node-runtime.md): its `/rpc` upgrade, authenticated once at the upgrade and without an Origin check (RT-6).
