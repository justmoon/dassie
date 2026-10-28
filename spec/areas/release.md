# Release

How Dassie is built, signed, published, installed and updated. A release is one static binary per platform with the wallet built in, packed in bundles whose sums are signed with the release key; the installer and the updater trust nothing they cannot verify with that key.

**Paths:** `crates/dassie-release/`, `crates/dassie-cli/src/update.rs`, `crates/dassie-cli/build.rs`, `resources/` (`minisign.pub`, `systemd/`, `installer/install.sh`), `nix/release.nix`, `nix/pnpm.nix`, `nix/flake/`, `scripts/release.ts`, `scripts/flake-latest.ts`, `scripts/flake-latest/`, `.github/workflows/release.yml`
**Read with:** [Node](node.md), [Testing](testing.md)

## Versions

`crates/dassie-release/src/version.rs`, sans IO.

- **A release** is the workspace's `major.minor.patch` (`[workspace.package]` in `Cargo.toml`, which every crate takes). There is one channel: `meta/latest` names the newest release.
- **Only that shape parses,** strictly and bounded, because a version names a directory on the mirror and in `/opt/dassie`.
- **The updater and the installer move on their own only to a later release** (`Version::supersedes` in the updater, the same rule with `sort -V` in the installer). `meta/latest` is not signed, so it never decides the direction: a mirror cannot take a node back. Only the owner can, by naming a version.
- **No pre-releases.** Every published version is signed by the release workflow on a tag, so a build of every change to `main` could not be signed; pre-releases, if wanted, are [not built](../roadmap.md#smaller-open-points).

## Trust

**What is signed:** a version's `SHA256SUMS`, with minisign. The signature's trusted comment is `dassie <version>`, so the signed sums of one release cannot pass for another's. A bundle is trusted only through its sum. Legacy minisign signatures (of the file itself) are refused, since nothing here makes them.

**What is trusted:** the release key in `resources/minisign.pub`, and nothing else.

- `crates/dassie-cli/build.rs` compiles it into the binary. A key that is missing or does not parse fails the build, so no binary is built that could never update. The cargo feature `test-key` compiles in `testdata/release`'s key instead, for tests; no release has it.
- The release build writes it into `install.sh`, which every release publishes; the script as the repository holds it, without a key, stops.
- Its secret half lives only in the repository's `Release` environment, which only `v*` tags may use. It is never on a developer's machine or in CI.

**What is not trusted:** the mirror, its HTTPS certificate and `meta/latest`. A mirror can withhold a release or offer an old one; it cannot make a node install anything the key did not sign for that version, nor move it backwards.

**Nothing is parsed or unpacked before it is verified.**

- `SHA256SUMS` is parsed only once its signature holds. Every parser in `dassie-release` is bounded, since all it reads comes from the network.
- A bundle is unpacked only after it matches its sum, and only into a staging directory. `dassie_release::bundle` admits only directories and regular files under `dassie/` with plain names: no links, no path that leaves the bundle.
- The new binary must name the version it was installed as and pass `dassie verify-install` before anything switches to it.

## The build

`nix/release.nix`, on x86_64-linux ([build-system](../decisions/build-system.md)): `nix build .#release` makes the unsigned mirror tree.

- **The wallet** is built in the Nix sandbox, offline, from npm packages fetched one by one with the integrity `pnpm-lock.yaml` records (`nix/pnpm.nix`, [node-and-pnpm](../decisions/node-and-pnpm.md)), so a change to the lockfile needs no hash updated by hand.
- **The binaries** are statically linked against musl, for x86_64 and, cross-compiled, aarch64.
- **The archives are deterministic:** sorted, with fixed times and owners, so the same binaries give the same bytes.
- **A release is built once,** on the Vorstand Nix cloud, and what that build made is what is signed. Nothing builds it a second time to compare, so whether the binaries come out byte for byte the same on another builder is untested, and the signature vouches for what the one builder and its cache produced. Checking that by building again is [not built](../roadmap.md#rebuilding-releases).
- **`static-binaries`** checks that both binaries are static and pass `verify-install`, the arm64 one under qemu.

## The mirror

`get.dassie.land` holds `meta/latest`, and under `<version>/` one bundle per platform, `dassie-<version>-linux-<arch>.tar.gz`, the flake `flake.tar.gz` ([NixOS](#nixos)), `SHA256SUMS`, which lists the bundles and the flake, and its `.minisig` (`dassie_release::layout`). `get.dassie.land/flake/latest.tar.gz` is the newest release's flake ([NixOS](#nixos)), and `sh.dassie.land` serves `install.sh`. The mirror's load balancer also serves dassie.land, from a bucket of its own ([Website](website.md#deployment), [website-hosting](../decisions/website-hosting.md)).

- **The names stay as installed updaters know them.** Every installed `dassie update` fetches `meta/latest` and its own platform's bundle by these names, so a release that renamed them, or the platforms in them (`x64`, `arm64`), would be one that every node before it fails to update to. A rename needs a release that carries both names and every node to pass through it, so the names stay.
- **One compression, gzip,** which the updater, the installer and Nix all read with nothing to install.

## Publishing

`.github/workflows/release.yml`, for every tag `v<version>`, whose version must be the tag's. Nothing else is published.

- **Nothing is signed until the install tests pass** on every distribution ([Tests](#tests)); signing is the last job, the only one in the `Release` environment.
- **What is signed is verified before it is uploaded,** with `dassie-release verify`, the code the updater trusts, against `resources/minisign.pub`. So a release signed with another key, or in a way the updater refuses, never reaches the mirror.
- **A version's files are immutable once published.** Each upload is read back and must be the signed tree's bytes, so a rerun completes an interrupted publish and refuses to overwrite a file published with other bytes. The signature goes last.
- **`meta/latest` moves last and never backwards:** it names the new version only once the version's files are all up, and only if `dassie-release supersedes` says it supersedes the published one.
- **`/flake/latest.tar.gz` follows `meta/latest`** ([NixOS](#nixos)): once `meta/latest` names the release, `scripts/flake-latest.ts` points the route at its flake. The rewrite and the `Link` header change together, in one update of the URL map, validated for the load balancer's scheme and refused if the map changed since it was read.
- The workflow does not consult the simulation campaign; that check is [not built](../roadmap.md#continuous-simulation).

## Install layout

The original's layout. Each version is unpacked into `/opt/dassie/<version>`, and `/opt/dassie/current`, a symlink switched with one rename, names the one that runs. The previous version stays beside it, for a rollback. The node runs as a dynamic systemd user, with its data in `/var/lib/dassie`; members of the group `dassie-users` may use the IPC socket, and so administer the node.

**It is frozen** ([Overview](../overview.md#frozen-surfaces)): every installed node's `dassie update`, and `install.sh`, unpack into `/opt/dassie/<version>`, switch `current` and lock `.update.lock` by these names, and the units run what `current` names. So a new layout would break the updaters already installed: a rename needs a release that understands both and every node to update through it, as the mirror's names do. The install tests pin it ([Tests](#tests)).

## systemd units

`resources/systemd`, carried in each bundle, so a version installs its own units. The unit files explain their options.

- `dassie.service` is socket-activated ([Node](node.md#runtimes)) and sandboxed. It requires the HTTP, HTTPS, IPC and QUIC (UDP 443) sockets, the node not running without QUIC ([quic-only](../decisions/quic-only.md)), and only wants the one for snapshot requests (UDP 7768, [Rate feed](rate-feed.md#the-socket)), which it can go without: an update that brings such a unit enables it, and restarting the node starts it. The node reports readiness once its engine works ([Node](node.md#readiness)), and exits if it cannot start the engine. Its readiness timeout bounds how long an update waits for the new node before rolling back.
- `dassie-update.timer` runs `dassie update` daily. An owner who went back to an older version and wants to stay there stops it, since it would move the node forward again.
- The owner's changes belong in drop-ins, which updates leave alone.

## `install.sh`

`resources/installer/install.sh`, POSIX sh, run as `curl … https://sh.dassie.land | sh`.

- **The whole script is one function called on its last line,** so a download cut short runs nothing.
- **The options it shares with `dassie update` have the same names:** `--target-version` installs any version, `--mirror` or `DASSIE_MIRROR_URL` names another mirror. It reads no version from the environment: a pin is a flag (`sh -s -- --target-version <v>`), so nothing exported for one tool reaches another. The build stamps its version from `DASSIE_BUILD_VERSION` for the same reason.
- **The mirror does not choose the direction,** as for the updater ([Versions](#versions)). The installed version is what the installed binary names, which it named before it was installed, so the mirror cannot fake it.
- **It verifies with minisign where there is minisign, and else with OpenSSL,** reading minisign's format by hand, so there is no tool to install first ([release-integrity](../decisions/release-integrity.md)).
- **It sets `umask 022`,** so that on a hardened host what it installs stays readable by the node's own user.
- **`--no-service` installs the files alone,** for a host that runs the node under units of its own, such as the [market maker's VM](market-maker-vm.md#releases-and-updates), or none, such as a container. It then needs no systemd.
- **It never runs beside `dassie update`:** it takes the updater's lock with `flock` before it reads an existing install, or before it makes the first one, and holds it to the end, uninstalling included. An update prunes every version it does not keep and what an install left half done, so beside an install it could delete the version `current` is about to name. Where an update holds the lock, it stops and changes nothing.
- **Uninstalling keeps the node's data, the group `dassie-users` and the owner's drop-ins.** It removes the units, every version and the link in `/usr/local/bin`, but the data holds the node key and the books, and the drop-ins are the owner's changes, which it leaves alone as updates do ([systemd units](#systemd-units)); installing again picks all of it up. An owner who wants a clean slate removes `/etc/systemd/system/dassie*.d` too.

## `dassie update`

`crates/dassie-cli/src/update.rs`, run by the timer or the owner.

1. **One at a time,** under a lock in `/opt/dassie` (`flock(2)` on `.update.lock`), which `install.sh` takes too.
2. **Choose the version** from `meta/latest` by the rule in [Versions](#versions), passing over the version the last rollback was for ([Rolling back](#rolling-back)). The owner may name a version (`--target-version`, older too), or another mirror, which may be on a private network since the owner names it.
   - **Units of the owner's own:** `--unit <name>.service=<data directory>`, once per node, names the units that run the install's nodes in place of `dassie.service`. The update then installs none of the release's units, nor any version's on a rollback, restarts the named ones, and on a rollback stops them, puts back each node's database and starts them again. So a host with units of its own gets the updater's rollback and record rather than a copy of them.
3. **Verify, unpack into staging and check the new binary** ([Trust](#trust)). Modes are set explicitly, whatever the umask, since the node runs as a user of its own, and every file and directory is synced, so `current` never names a version a power cut left half written.
4. **Switch:** install the version's units where they changed, move `current`, restart `dassie.service` and wait for its readiness. If the units or `current` cannot be put in place, `current` goes back to the version it named, which need not be the one running the update, with that version's units, and the units the new version added go, before systemd has read any of them.
5. **Record** how it went in `/opt/dassie/last-update`, which `dassie status` reads, and **prune** every version but the new one, the one `current` named before it, for the owner to go back to, and the one running the update, which the owner may have kept.

### Rolling back

If the node does not come up after the switch, the update disables and removes the units the new version added, points `current` back at the version it named, which need not be the one running the update, puts back that version's units, stops the node, puts back the database if it must, and starts the old version again. No unit of the new version stays behind for systemd to start, at the next boot or otherwise. Coming up means reporting readiness, which a node whose engine cannot start on the database never does ([Node](node.md#readiness)).

- **Why the database:** a version refuses a database whose schema is newer than it knows ([Node](node.md#database)), so after a migration the old version could not start. The daemon keeps a copy from just before it migrated, taken before it served anything, so nothing the copy lacks has reached a peer.
- **Only when it must, and only a copy it can trust:** the updater puts the copy back only if the old version could not open the database as it is, only if the copy was made by this update (by its time), and only if the old version can open the copy (`store::restore_pre_migration`).
- The database it replaces is kept beside it (`dassie.sqlite3.rolled-back`) for the owner to look at.
- **A version that did not start is not tried again until the mirror offers another.** Tried again, it would take the node down at every run of the timer until a fix is out. `last-update` names it and why it did not start, and every record keeps both, failed runs included, until an update installs a version; the updater and `dassie status` say why it passes over it. The owner can still install it, with `--force` or `--target-version`. Without a record that reads, every version is tried, as before the first update.
- **If the database cannot be put back, the old version is not started:** it would refuse a database too new for it, or, finding none, start afresh with another identity and no books. The node stays stopped, and the update says where the database, the one the new version left and the copy are, for the owner to choose. Nor does a node start afresh where a rollback stopped half way, whatever starts it ([Node](node.md#database)).

## NixOS

`nix/flake/`, published as `<version>/flake.tar.gz` ([nix-flake-on-the-mirror](../decisions/nix-flake-on-the-mirror.md)). An owner points a flake input at it, or at `https://get.dassie.land/flake/latest.tar.gz` to follow releases, and enables `services.dassie`.

- **`/flake/latest.tar.gz` locks as the version it names.** The mirror's load balancer serves it as the newest release's flake, with `Link: <https://get.dassie.land/<version>/flake.tar.gz>; rel="immutable"`, which Nix locks in its place, so a lock stays good when the release after it is out. The header needs the load balancer's `EXTERNAL_MANAGED` scheme, and the backend must stay without Cloud CDN, which would keep the old flake under the same URL. The release workflow moves it ([Publishing](#publishing)).

- **The package is the release's own bundle,** fetched by the sum `SHA256SUMS` lists, which `nix build .#release` writes into the flake's `release.json`. Nothing is compiled or patched.
- **The units are the bundle's,** with `ExecStart` naming the package's binary, enabled as `install.sh` enables them. The module sets environment variables and opens the firewall in drop-ins, and makes the group `dassie-users`. The socket units keep their names, which the node takes its sockets by ([Node](node.md#runtimes)).
- **No updater:** the updater's units are left out, and the flake's version is the node's. Updating is changing the flake input, and going back is going back a generation, which across a migration needs the database copy put back by hand.
- **Nix checks the flake by its lock, not the release key.** An owner who checks `flake.tar.gz` once against the signed `SHA256SUMS` holds the binaries to the signed ones from then on.

## Tests

- `crates/dassie-cli/tests/update.rs` runs the updater against a TLS mirror signed with the test key, including tampered and mis-signed releases.
- The store's tests check the copy before a migration, and when a rollback puts it back.
- `tests/e2e/install_test.ts` installs, updates, rolls back, passes over the version it rolled back, uninstalls, and installs without units (`--no-service`) on several distributions against `nix build .#install-test-mirror`, whose newest release's node fails to start on purpose, so the rollback runs for real ([Testing](testing.md#installer-and-updater)). It runs in the release workflow before signing, not in CI.
- `scripts/release_test.ts` tests the signing script, and `scripts/flake-latest_test.ts` the change of the URL map that routes the flake; the installer's ShellCheck runs with `nix flake check`.
- `nixos-units`, with `nix flake check`, builds the units a NixOS system with the module would have, around a stand-in binary. `nixos-test` boots the module in a VM with the install tests' 1.0.0, and checks that the node answers, that only root and `dassie-users` reach its IPC socket, and that there is no updater. It needs KVM, so it runs in the release workflow before signing.

## Decisions

- [release-integrity](../decisions/release-integrity.md): signed sums with minisign rather than Sigstore, the trusted comment, an unsigned `meta/latest`, no canaries, verify before extract, atomic switch and rollback; a release built once, until a second build can check it.
- [build-system](../decisions/build-system.md): the Nix flake for release builds, cross-compiled with musl.
- [nix-flake-on-the-mirror](../decisions/nix-flake-on-the-mirror.md): NixOS takes the signed binary from a flake each release publishes, with its own units and no updater.
- [node-and-pnpm](../decisions/node-and-pnpm.md): the wallet's packages fetched from the lockfile's hashes.
- [random-seed-simulations](../decisions/random-seed-simulations.md): releases do not wait for the simulation campaign yet; they will once a live network runs on them ([roadmap](../roadmap.md#continuous-simulation)).
- [ready-after-first-step](../decisions/ready-after-first-step.md): a release whose engine cannot start fails its start, and is rolled back.
