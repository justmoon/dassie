# D14. Build system

**Areas:** [Engineering](../engineering.md), [Release](../areas/release.md)

## Decision

Follow Vorstand: a Nix flake for CI and release builds, plain `cargo` and `node --run` locally. The binaries are cross-compiled from one Linux builder with musl, and the wallet is built in the Nix sandbox from the lockfile's hashes.

## Rejected

- *GitHub Actions with `cargo-zigbuild` or `cross`:* no reproducible builds by default.
