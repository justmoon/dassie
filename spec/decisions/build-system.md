# Build system

**Areas:** [Engineering](../engineering.md), [Release](../areas/release.md)

## Decision

Follow Vorstand: a Nix flake for CI and release builds, plain `cargo` and `node --run` locally. The binaries are cross-compiled from one Linux builder with musl, and the wallet is built in the Nix sandbox from the lockfile's hashes.

The TypeScript checks build in the sandbox too, offline: the packages from the lockfile's hashes, dprint's plugins from the checksums `dprint.json` pins, and the binaries the tests run built once, by the flake. Only checking the lockfile against the registries, which needs the network, runs outside it.

## Rejected

- *GitHub Actions with `cargo-zigbuild` or `cross`:* no reproducible builds by default.
- *The TypeScript checks on a CI runner, beside the flake:* the Rust they test is built a second time, nothing is kept by input, so every change runs them all, and on a machine the end-to-end tests' fixed ports are shared with whatever else runs.
