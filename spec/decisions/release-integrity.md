# Release integrity

**Areas:** [Release](../areas/release.md)

## Facts

The original's bundles are unsigned, the updater trusts whatever the mirror serves, and releases are uploaded by hand.

## Decision

- **Releases are signed with minisign,** over their sums, with the version in the signed comment so the sums of one release cannot pass for another's. The public key is built into the binary and the install script.
- **The secret key is usable only by the release workflow on a version tag,** which verifies what it signed with the committed public key before it uploads.
- **The mirror's channel files are not signed, so they never decide direction:** the updater and the installer move on their own only to a later version of the same channel.
- **Nothing is unpacked before it is verified.**
- **Updates roll back.** The switch is one rename, and a version that does not come up is replaced by the one before. Since a version refuses a database newer than it knows, the daemon copies the database just before it migrates, and a rollback restores the copy; without it, a version that migrated and then failed would leave the node down.
- **The installer needs no extra tool:** it verifies with minisign where there is one, and else with OpenSSL.
- **CI builds a release once, with Nix, and signs what it built** ([build-system](build-system.md)). The archives are deterministic, so the same binaries give the same sums.

## Accepted

- **The signature vouches for one build.** A builder or cache that was tampered with would have its output signed, and nothing shows whether the binaries reproduce on another builder. A second, independent build compared before signing would catch both. It waits ([roadmap](../roadmap.md#rebuilding-releases)): GitHub's runners are very slow, and the project is early and wants to move fast.

## Rejected

- *Sigstore:* its verification needs the network or a bundled trust root, and is heavier to embed.
