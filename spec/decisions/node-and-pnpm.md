# Tooling on Node and pnpm, not Deno

**Areas:** [Engineering](../engineering.md), [Release](../areas/release.md), [Dev](../areas/dev.md)

## Facts

Deno's development is ending: Cloudflare acquired it and announced the runtime's end. Only the tooling ran on it ([rust-and-typescript](rust-and-typescript.md)); nothing shipped contains it.

## Decision

The TypeScript side runs on Node and pnpm, keeping Deno's conventions (its formatting style through dprint, its recommended lint rules through oxlint, `@std/assert` in tests, sources run by stripping their types). No release contains Node.

## Rejected

- *Staying on Deno until it stops:* time spent on a dead end, and every fix after that would be ours to make.
- *Bun:* also a runtime owned by one company, and Node is what Vite, Solid's tooling and Puppeteer are built and tested on.
- *Keeping Deno working beside Node:* two runtimes to test against, for one that is ending.
