# D14. Build system

**Status:** active, confirmed 2026-10-11.
**Areas:** [Engineering](../engineering.md), [Release](../areas/release.md)

## Decision

Follow Vorstand: a Nix flake for CI builds and release artifacts, plain `cargo` and `node --run` locally.

- The binaries are cross-compiled from x86_64-linux with nixpkgs' musl toolchains, so one builder makes both platforms.
- The wallet is built in the Nix sandbox too, from its npm packages fetched one by one with the hashes `pnpm-lock.yaml` records.

## Rejected

- *GitHub Actions with `cargo-zigbuild` or `cross`* for musl x64 and arm64: no reproducible builds by default.
