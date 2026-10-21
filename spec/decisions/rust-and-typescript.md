# What runs in Rust and what in TypeScript

**Areas:** [Overview](../overview.md), [Engineering](../engineering.md)

## Decision

The node (daemon, CLI, all protocols) is Rust. The wallet and the dev dashboard are Solid. Tooling around the node (dev environment, website, build and release scripts, interop tests) is TypeScript on Node ([node-and-pnpm](node-and-pnpm.md)). The core crates also build to WASM, so the browser and the website run the node's own code.

## Why

- The packet path, the ledger and the long-running network services benefit from Rust's performance and type system.
- A static binary is what installing and updating a node wants. A TypeScript node would need a bundled runtime, which made the original's releases large and fragile.

## Rejected

- *A TypeScript node calling Rust cores through WASM or FFI:* for the reasons above.
