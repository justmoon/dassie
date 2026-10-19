# D5. UI and CLI RPC contract

**Status:** active, confirmed 2026-10-11.
**Areas:** [RPC](../areas/rpc.md), [Wallet](../areas/wallet.md)

## Facts

The original's RPC (`dassie-rpc-01`, superjson over WebSocket) has no contract either side is generated from, and some procedures answer success with a failure inside (RT-5). The UI polls for live data.

## Decision

A normative contract in `proto/dassie/ui/v1`, following Vorstand's `proto/` convention, generated into Rust and TypeScript (`packages/rpc`):

- unary calls and server streams, the streams either whole values or a snapshot followed by deltas;
- protobuf binary frames;
- amounts as fixed-width integers with an explicit scale;
- typed errors, never a success that carries a failure.

The same contract serves the browser over WebSocket and the CLI over the IPC socket. Live data comes from `Subscribe*` streams instead of polling.

## Rejected

- *JSON over WebSocket with a JSON-Schema contract:* easier to debug, but code generation for Rust is weaker.
