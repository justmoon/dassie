# D1. What runs in Rust and what in TypeScript

**Status:** active, confirmed 2026-10-11. The TypeScript side moved from Deno to Node in [D21](D21-tooling-on-node-and-pnpm-not-deno.md); the split did not change.
**Areas:** [Overview](../overview.md), [Engineering](../engineering.md)

## Decision

The node (daemon, CLI, all protocols) is Rust. The wallet and the dev dashboard are Solid 2, built with Vite. The dev environment, the website, the build and release scripts and the interop tests are TypeScript on Node. The core crates also ship as WASM, so the browser and the website run the node's own code.

## Why

- The packet path, the ledger and the long-running network services benefit from Rust's performance and type system.
- A static binary is what the install and update story wants.
- A TypeScript node would bring back a bundled runtime, the problem the original had with Node.

## Rejected

- *The node as a TypeScript app* (as Vorstand's servers are), calling Rust cores through WASM or FFI: for the reasons above.
