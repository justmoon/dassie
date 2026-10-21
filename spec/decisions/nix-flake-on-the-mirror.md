# NixOS takes a release from a flake on the mirror

**Areas:** [Release](../areas/release.md).

## Facts

- `install.sh` and `dassie update` write to `/opt/dassie`, `/usr/local/bin` and `/etc/systemd/system`. On NixOS the system is built from its configuration, and what is written there by hand is not part of it. The updater also runs only from `/opt/dassie/<version>/bin/dassie`.
- A release's binaries are static, built against musl. They run on NixOS as they are.
- A release's units hold the node's sandbox, and NixOS installs a package's own units (`systemd.packages`), with what the configuration sets in drop-ins.
- Building Dassie from source takes the Rust workspace and the wallet's npm packages, which is many minutes even on a large machine. Only the project's Nix cloud caches it, and that cache is not public.
- A flake input can be a tarball's URL. Nix locks it by the URL and the hash of what it unpacked, and with the lock, fetches the same URL again on another machine.
- The mirror's files under `<version>/` never change once published ([Release](../areas/release.md#publishing)).

## Decision

- **Each release publishes a flake, `<version>/flake.tar.gz`,** beside its bundles. It holds a NixOS module, a package, and `release.json`, which names the version and, for each platform, the `.tar.xz` bundle and its sum. Its one input is nixpkgs, locked as this repository locks it, for the user to `follows`.
- **The package is the release's own bundle,** fetched from the mirror by its sum, which is the sum `SHA256SUMS` lists. Nothing is compiled, and the binary is the one every other install runs.
- **The module installs the bundle's own units,** with `ExecStart` pointed at the package, and enables them as `install.sh` does. Options set environment variables and open the firewall, in drop-ins. The sandbox stays in one place, `resources/systemd`.
- **There is no updater on NixOS.** The updater's units are left out of the package, and the flake's version is the node's. Updating is changing the flake input.
- **`SHA256SUMS` lists the flake,** so the release key signs it with the bundles.

## Accepted

- **Nix does not check the signature.** It trusts the mirror's TLS certificate when it first locks the flake, and the lock's hash after that. What the lock holds is what the release key signed only if the owner checks once: `minisign -Vm SHA256SUMS -P <key>` with the key from `resources/minisign.pub`, then `sha256sum -c --ignore-missing SHA256SUMS` over the downloaded `flake.tar.gz`. The flake's sums of the bundles then hold the binaries to the signed ones.
- **A rollback across a migration stops the node.** A version refuses a database newer than it knows ([Node](../areas/node.md#database)). Going back to an older NixOS generation runs the older version on the newer database, and only the updater puts the copy from before the migration back. The owner does it by hand: stop the node, and put `dassie.sqlite3.pre-migration` in place of `dassie.sqlite3` and its `-wal` and `-shm`. The copy is one step deep.
- **`dassie status` says the binary is not updated,** which is true: Nix updates it.
- **A flake per version, and none for a release published before it.**

## Rejected

- *A NixOS module in this repository's flake, building from source:* every user builds the workspace and the wallet, for a binary other than the signed one.
- *A package in nixpkgs:* it follows releases only as fast as its reviews, and is built again by Hydra, so its binary is not the signed one either.
- *Running `install.sh` on NixOS:* what it writes is outside the system's configuration, and is gone or stale at the next rebuild.
- *A flake that fetches the newest release itself:* a flake is evaluated without the network, and a URL whose contents change cannot be locked.
