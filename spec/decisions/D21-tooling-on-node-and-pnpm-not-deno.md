# D21. Tooling on Node and pnpm, not Deno

**Status:** active, decided 2026-10-16.
**Areas:** [Engineering](../engineering.md), [Release](../areas/release.md), [Dev](../areas/dev.md)

## Facts

Cloudflare acquired Deno and announced that development of the Deno runtime ends in a year. Deno ran only the TypeScript side ([D1](D01-what-runs-in-rust-and-what-in-typescript.md)): the dev environment, the Vite builds, the tests and the scripts. Nothing shipped contains it.

## Decision

The TypeScript side moves to Node and pnpm, at the versions the flake pins, keeping its conventions: Deno's formatting style through dprint, Deno's recommended lint rules through oxlint, `@std/assert` for tests, sources run by stripping their types. [Engineering](../engineering.md) states the tooling. The node stays Rust, and no release contains Node.

## Rejected

- *Staying on Deno until it stops:* a year spent on a dead end, and every fix after that would be ours to make.
- *Bun:* also a runtime owned by one company (Anthropic, since December 2025), and Node is what Vite, Solid's tooling and Puppeteer are built and tested on.
- *A compatibility layer that keeps Deno working beside Node:* two runtimes to test against, for one that is ending.
