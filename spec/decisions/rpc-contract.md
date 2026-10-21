# UI and CLI RPC contract

**Areas:** [RPC](../areas/rpc.md), [Wallet](../areas/wallet.md)

## Facts

The original's RPC (superjson over WebSocket) has no contract either side is generated from, some procedures answer success with a failure inside, and the UI polls for live data.

## Decision

One protobuf contract in `proto/dassie/ui/v1`, generated into Rust and TypeScript, serving both the browser (WebSocket) and the CLI (IPC socket): unary calls and server streams, typed errors, amounts as integers with an explicit scale. Live data comes from streams, not polling.

## Rejected

- *JSON with a JSON-Schema contract:* easier to debug, but code generation for Rust is weaker.
