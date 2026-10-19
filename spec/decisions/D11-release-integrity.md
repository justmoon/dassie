# D11. Release integrity

**Status:** active, confirmed 2026-10-11, extended 2026-10-16.
**Areas:** [Release](../areas/release.md)

## Facts

The original's bundles are unsigned, `meta/latest` is plain text, the updater does not check the HTTP status (RT-18, UI-11), and releases are uploaded by hand (UI-12).

## Decision

- **Signed sums.** Each release publishes `SHA256SUMS` and a minisign signature whose trusted comment names the version, so the signed sums of an old release cannot pass for a new one's. The public key is compiled into the binary and into the install script.
- **The key's secret half** lives in the repository's `Release` environment, which only `v*` tags may use. The workflow verifies what it signed with `resources/minisign.pub` before it uploads, so a release signed with another key never reaches the mirror.
- **`meta/<channel>` is not signed, so it is not trusted to say which way to move:** the updater and the installer move on their own only to a later version of the same channel.
- **Verify before extracting,** and unpack only plain files and directories.
- **Atomic switch and rollback.** The `current` link moves with one rename. The updater keeps the version it came from, and goes back to it if the new one does not come up. Since a version refuses a database newer than it knows, the daemon keeps a copy of the database just before it migrates, and the rollback puts it back; without it, a new version that migrated and then failed left the node down.
- **The installer verifies with minisign where there is minisign, and else with OpenSSL,** which reads minisign's format by hand: no extra tool to install first.
- **CI builds and publishes releases,** reproducibly through the Nix flake ([D14](D14-build-system.md)), and canaries get a channel of their own.

[Release](../areas/release.md) states all of it.

## Rejected

- *Sigstore:* its verification needs the network or a bundled trust root, and is heavier to embed.
