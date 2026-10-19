# Release

How Dassie is built, signed, published, installed and updated. A release is one static binary per platform with the wallet built in, packed in bundles whose sums are signed with the release key; the installer and the updater trust nothing they cannot verify with that key.

**Paths:** `crates/dassie-release/`, `crates/dassie-cli/src/update.rs`, `crates/dassie-cli/build.rs`, `resources/` (`minisign.pub`, `systemd/`, `installer/install.sh`), `nix/release.nix`, `nix/pnpm.nix`, `scripts/release.ts`, `.github/workflows/release.yml`
**Read with:** [Node](node.md), [Testing](testing.md)

## Versions and channels

`dassie-release` defines what a version is, sans IO.

- **A release** is `major.minor.patch`, dassie-cli's version (`crates/dassie-cli/Cargo.toml`), published on the channel `latest`. Each part is a decimal number without sign or leading zeros.
- **A canary** is `canary-YYYY-MM-DD-HHMMSS-<commit>`: when the commit it is built from was made, in UTC, and its hash, 7 to 40 lowercase hex digits. It is published on the channel `canary`. Month 1–12, day 1–31, hour 0–23, minute and second 0–59. `dassie-release` also accepts the hash followed by `-dirty`, which a local build of a tree with uncommitted changes gets; the installer and the release script do not.
- A version names a directory on the mirror and in `/opt/dassie`, so only these two shapes parse: digits, lowercase letters, dots and dashes, at most 80 bytes. A tree without a commit builds a canary named `…-unknown`, which is no version and cannot be signed.
- **The updater moves on its own only to a later version of the same channel** (`Version::supersedes`): a higher release, or a canary of a later commit time. Nothing else supersedes, since `meta/<channel>` is not signed: a mirror cannot take a node back, or to the other channel. Only the owner can.

## The release key

- **The key** is the minisign public key in `resources/minisign.pub` (key ID `5D960652FE941432`). Releases must verify with it.
- **The binary trusts it.** `crates/dassie-cli/build.rs` compiles in the key from the file `DASSIE_RELEASE_KEY` names, or else `resources/minisign.pub`. With the cargo feature `test-key` it compiles in `testdata/release/test.pub` instead, for tests only; no release has it. A key that does not parse fails the build. A binary without a key refuses to update, and `verify-install` says so.
- **The installer trusts it.** The release build writes the key's base64 line into `install.sh` in place of `@RELEASE_KEY@`. An installer without one stops with `E_NO_RELEASE_KEY`.
- **The secret half** lives in the repository's `Release` environment ([Publishing](#publishing)). It is never on a developer's machine or in CI.

## The build

`nix/release.nix`, on x86_64-linux ([D14](../decisions/D14-build-system.md)).

- **The wallet** is built by Vite in the Nix sandbox, offline. Its npm packages and JSR modules are fetched one by one, each checked against the integrity `pnpm-lock.yaml` records for it (`nix/pnpm.nix`). A registry mirror on loopback serves them to `pnpm install --frozen-lockfile`. A change to the lockfile needs no hash updated by hand.
- **The binary** is `dassie-cli` (`cargo --locked -p dassie-cli`) in the `release` profile (fat LTO, one codegen unit, overflow checks on), statically linked against musl, for `x86_64-unknown-linux-musl` and, cross-compiled with nixpkgs' musl toolchain, `aarch64-unknown-linux-musl`. The build sets `DASSIE_WALLET_DIST` (the wallet), `DASSIE_VERSION` (the version) and `DASSIE_RELEASE_KEY` (the key).
- **A bundle** holds `dassie/bin/dassie` (mode 0755) and the units of `resources/systemd` in `dassie/share/systemd` (0644); `dassie/` becomes `/opt/dassie/<version>`. It is archived as `dassie-<version>-linux-<arch>.tar.gz` (`gzip -9 --no-name`) and `.tar.xz` (`xz -6`, one thread), with `<arch>` `x64` or `arm64`. The archives are the same bytes for the same input: GNU tar, entries sorted by name, modification time 0, owner and group 0.
- **The mirror tree** is `nix build .#release` (the release version, `meta/latest`) or `.#canary` (the canary version, `meta/canary`): `<version>/` with the four bundles and `SHA256SUMS` (written by `sha256sum`), `meta/<channel>` holding the version, and `install.sh` with the release key. It is signed afterwards ([Publishing](#publishing)).
- **The check `static-binaries`** (`nix build .#static-binaries`) confirms that both release binaries are static (no `INTERP` program header, no `NEEDED` entry), print `dassie <version>`, and pass `verify-install`, the arm64 one under qemu.
- **`release-tool`** (`nix build .#release-tool`) is the `dassie-release` binary.
- **The installer's ShellCheck** (`--shell=sh`) runs with `nix flake check`.

## The mirror

`get.dassie.land` keeps the original's paths, plus the signature:

```
meta/latest                                  the newest release, no newline
meta/canary                                  the newest canary
<version>/dassie-<version>-linux-x64.tar.gz  and .tar.xz, and the same for arm64
<version>/SHA256SUMS                         as sha256sum writes it
<version>/SHA256SUMS.minisig                 minisign, trusted comment "dassie <version>"
install.sh                                   the installer, with the release key in it
```

`sh.dassie.land` serves `install.sh`.

## Signatures and sums

**The signature** is minisign's default: Ed25519 over the BLAKE2b-512 of `SHA256SUMS`. Its trusted comment names the version, so the signed sums of one release cannot pass for another's.

```
untrusted comment: <anything>
base64( "ED" ‖ key ID (8) ‖ Ed25519 signature of BLAKE2b-512(SHA256SUMS) (64) )
trusted comment: dassie <version>
base64( Ed25519 signature of (the 64-byte signature above ‖ the trusted comment) )
```

- A public key is `base64("Ed" ‖ key ID (8) ‖ Ed25519 key (32))`, as `minisign -G` writes it, with or without its untrusted comment line. Weak keys are refused. Its ID prints as 16 uppercase hex digits, last byte first, as minisign prints it.
- **`dassie-release` verifies** with `ed25519-dalek`'s strict verification. The key ID must be the key's, both signatures must hold, and the trusted comment must be exactly `dassie <version>` for the version asked for. Legacy signatures (`Ed`, of the file itself) are refused, since nothing here makes them.
- **`SHA256SUMS` is parsed only once the signature holds.** One line per file, `<64 lowercase hex digits>  <name>` (or ` *<name>`, as `sha256sum --binary` writes it), each ending in a newline. A name is letters, digits, `.`, `-` and `_`, at most 128 bytes, not starting with a dot: never a path. No name twice, and at least one line.
- **Every parser is bounded,** since everything it reads comes from the network: a version at most 80 bytes, a signature at most 1 KiB, `SHA256SUMS` at most 4 KiB and 16 lines.

**`dassie-release`** (the binary) gives the release workflow the code the updater trusts:

| Command | Exit status |
|---|---|
| `dassie-release verify <tree> <minisign.pub>` | 0 if the tree has exactly one `meta/<channel>` naming a version of that channel, its `SHA256SUMS` is signed by the key for that version, it lists exactly the four bundles, and each matches its sum; prints `verified dassie <version> on <channel> with key <ID>`. 1 otherwise |
| `dassie-release supersedes <new> <old>` | 0 if the updater would move from `old` to `new` on its own, 1 if not, 2 if either is not a version |

Anything else prints the usage and exits 2.

## Publishing

`.github/workflows/release.yml`.

- **Every tag `v<version>` builds a release,** whose version must be the tag's. A canary is built and signed by hand; no workflow publishes one.
- The workflow does not check the simulation campaign: the release check that would refuse a commit the campaign has not cleared is [not built](../roadmap.md#continuous-simulation).
- **Three jobs,** one publish at a time, never cancelled:
  1. **build**, on the Vorstand Nix cloud: the install tests' mirror, the release tree, `static-binaries` and `release-tool`.
  2. **install**, one job per distribution (Debian, Ubuntu, Fedora, CentOS), not failing fast: the install tests ([Tests](#tests)). These checks run here, not in CI.
  3. **release**, once every install job passed, in the `Release` environment. Only now is anything signed.
- **Signing** (`scripts/release.ts`): it reads the tree's one `meta/<channel>` and checks the version's shape, copies the tree, checks that `SHA256SUMS` lists exactly the four bundles and that each matches, and signs it with `minisign -S -t "dassie <version>"` (the key's password, if any, from `MINISIGN_PASSWORD`). Then `dassie-release verify` checks the signed tree with `resources/minisign.pub`, so a release signed with another key, or in a way the updater refuses, never reaches the mirror. The tree must hold the tag's version.
  - Without the secret key the workflow builds and stops, with a notice; without `resources/minisign.pub` it fails. The signed tree is kept as an artifact of the run for 14 days; without the bucket's workload identity settings, that is all it publishes.
- **The `Release` environment** holds the release key (`MINISIGN_SECRET_KEY`, `MINISIGN_PASSWORD`) and the bucket's settings, and admits only `v*` tags, so no branch's workflow can read them. The bucket is reached through workload identity federation. The job's actions are pinned to commits, and the Nix cache's signing key to a fixed key.
- **The upload** goes to the bucket behind `get.dassie.land`:
  - A version's files are uploaded once each, the signature last, cached for good (`public, max-age=31536000, immutable`). Each file is read back and must be the same bytes as the signed tree's, so a run again completes what an interrupted one left, and refuses a file published with other bytes.
  - Only then does `meta/latest` name the version, uncached (`no-cache, max-age=0`). It never moves to a version older than the one it names: if `dassie-release supersedes` says the new version does not supersede the published one, `meta/latest` is left as it is, with a warning, and the run ends there.
  - With `meta/latest`, a release publishes `install.sh`, uncached, as `text/plain; charset=utf-8`.

## Install layout

The original's layout ([2 §3.1](../original/02-node-runtime.md)), kept so that operators' habits and docs hold.

| Path | What |
|---|---|
| `/opt/dassie/<version>/` | An unpacked bundle: `bin/dassie`, `share/systemd/` |
| `/opt/dassie/current` | Symlink to `<version>`, the one that runs |
| `/usr/local/bin/dassie` | Symlink to `/opt/dassie/current/bin/dassie` |
| `/etc/systemd/system/` | The six units, from the running version's `share/systemd`. The owner's changes belong in drop-ins |
| group `dassie-users` | May use the IPC socket, and so administers the node |
| `/run/dassie.sock` | The IPC socket |
| `/var/lib/dassie/` | The node's data (`dassie.sqlite3`), 0700; systemd keeps it in `/var/lib/private/dassie` for the dynamic user |
| `/opt/dassie/last-update` | How the last update went, for `dassie status` |
| `/opt/dassie/.update.lock` | Held by the update that runs |

## systemd units

`resources/systemd`, carried by each bundle in `share/systemd`.

- **`dassie.service`** runs `/opt/dassie/current/bin/dassie daemon --runtime=systemd`, which takes its sockets by name and reports readiness itself (`Type=notify`, `NotifyAccess=main`).
  - It requires and comes after the three sockets, and after `network-online.target` and `time-set.target`.
  - It runs as a dynamic user (`DynamicUser=yes`), with `/var/lib/dassie` its state directory (`StateDirectoryMode=0700`) and working directory, `UMask=0077`, and `DASSIE_LOG_LEVEL=info`. `Restart=always`, `RestartSec=1`.
  - It is sandboxed: no capabilities, no new privileges, a read-only system (`ProtectSystem=strict`), no home directories, private `/tmp` and devices, kernel tunables, modules, logs, control groups, clock and hostname protected, only Unix and IP sockets, no namespaces, no realtime, no SUID, no writable executable memory, native system calls only, only the system calls of `@system-service` (others fail with `EPERM` rather than killing the node), no sight of other processes, and a user namespace of its own.
  - It has 5 minutes to report readiness (`TimeoutStartSec=5min`), so a node that never comes up after an update fails, and the update is rolled back.
- **The socket units** pass the node's sockets, by their unit names, with `NoDelay` on TCP:
  - `dassie-http.socket`: port 80, IPv4 and IPv6 (ACME challenges and redirects);
  - `dassie-https.socket`: port 443, IPv4 and IPv6;
  - `dassie-ipc.socket`: `/run/dassie.sock`, mode 0660, owner root, group `dassie-users`.
- **`dassie-update.timer`** starts `dassie-update.service` daily at a random time in the hour after 03:20 (`OnCalendar=*-*-* 03:20:00`, `RandomizedDelaySec=3600`, `Persistent=true`). An owner who went back to an older version and wants to stay there stops this timer, which would otherwise move the node forward again.
- **`dassie-update.service`** (oneshot, 30 minutes at most) runs `dassie update`. A drop-in setting `DASSIE_MIRROR_URL` points it at another mirror.
  - It runs as root, since it installs root's units and has systemd restart the node, but sandboxed like the node. It may write only `/opt/dassie`, `/etc/systemd/system` and the node's state directory. Of root's capabilities it keeps two, for the rollback: `CAP_DAC_OVERRIDE`, to reach the database the node's user owns, and `CAP_CHOWN`, with which SQLite gives that user what it creates there.

## `install.sh`

`resources/installer/install.sh`: POSIX sh, run as `curl --proto =https --tlsv1.2 -sSf https://sh.dassie.land | sh` (with `sh -s -- <options>` for options). The whole script is one function called on its last line, so a download cut short runs nothing.

```
install.sh [-c|--channel latest|canary] [-t|--target <version>] [-m|--mirror <url>] [-h|--help] [install|uninstall]
```

- The options can also be given as `DASSIE_CHANNEL`, `DASSIE_VERSION` and `DASSIE_MIRROR_URL`. The mirror must be `https://` (default `https://get.dassie.land`).
- **It needs** Linux; x86_64 (`x64`) or aarch64 (`arm64`); systemd, running or still starting (`running`, `degraded`, `starting` or `initializing`, as when cloud-init runs it); `tar`, `sha256sum`, `base64`, `awk` and `mktemp`; curl or wget; and minisign or OpenSSL 1.1.1 or later (`pkeyutl -rawin`). It takes the xz bundle where there is `xz`, and the gzip one otherwise.
- **It sets `umask 022`:** on a hardened host, what it installs must still be readable by the node's own user.
- **Downloads** are HTTPS only, failing on any error status: curl with `--proto =https --tlsv1.2 --fail --location --retry 3 --retry-delay 5`, or wget with `--https-only --tries=3`.

**`meta/<channel>` is not signed, so the mirror chooses neither the channel nor the direction.** The installed version is what `/opt/dassie/current/bin/dassie --version` names; an install whose binary names no version counts as none. Each binary named its version before it was installed, so the mirror cannot bring that about.

- Run again without `--channel`, it keeps to the installed version's channel, as `dassie update` does. A fresh install takes `latest`.
- Without `--target`, it reads `meta/<channel>` (whitespace removed). What it names must be a version of that channel: a release on `latest`, a canary on `canary` (`E_VERSION_INVALID`).
- It moves to the other channel only when `--channel` names it (`E_VERSION_OTHER_CHANNEL`), and never to a version of the same channel older than the one installed (`E_VERSION_OLDER`; canaries compare by commit time). `--target` installs any version.

**Nothing is unpacked before it is verified.**

1. It downloads `SHA256SUMS`, `SHA256SUMS.minisig` and the bundle into a temporary directory, removed on exit.
2. It checks the signature of `SHA256SUMS` with the release key built into it. With minisign that is `minisign -V`. With OpenSSL it is minisign's format by hand: four lines; the second decodes to 74 bytes starting `ED` with the key's ID; Ed25519 over the BLAKE2b-512 of the file; the fourth decodes to 64 bytes, the global signature over the signature and the trusted comment.
3. The trusted comment must be `dassie <version>` (`E_SIGNATURE_INVALID`), and the bundle must match its sum (`E_BUNDLE_VERIFY_FAILED`).
4. The unpacked bundle may hold only files and directories, its binary must be executable, name the version (`dassie <version>`) and pass `verify-install` (`E_SELF_TEST_FAILED`), all before anything is installed.

**It installs**, as root or through sudo for each step, showing each command:

1. The version: copied to `/opt/dassie/.install-<version>`; a version installed again is renamed aside first; moved into place with `mv -T`; then `sync`. Then `current` with one rename (`mv -T` of a new symlink), and `sync` again.
2. The command's link `/usr/local/bin/dassie`, and the group `dassie-users` (a system group, made if missing), adding the user who ran it, directly or through sudo (`SUDO_USER`), unless that is root. That user may use `dassie` once logged in again.
3. The units: copied to `/etc/systemd/system`, `daemon-reload`, the three sockets, `dassie.service` and `dassie-update.timer` enabled, the sockets started, and `dassie.service` and the timer restarted.

It ends with "Next, set up the node: sudo dassie init".

**`uninstall`** stops and disables all six units, removes them, reloads systemd, removes `/opt/dassie` and the command's link (if it is a symlink). It keeps the node's data and the group.

**Output and errors.** Steps print as `▶ <step>`, commands run as root as `# <command>` or `# sudo <command>`. Colour only on a terminal whose `TERM` is not `dumb`, and never with `NO_COLOR`. An error prints `! Error (<code>) <message>` and exits 1. The codes keep the original's `E_*` names, with new ones for verification:

`E_SYNTAX`, `E_UNSUPPORTED_OS`, `E_UNSUPPORTED_ARCH`, `E_SYSTEMD_REQUIRED`, `E_SYSTEMD_NOT_RUNNING`, `E_TOOL_REQUIRED`, `E_CURL_OR_WGET_REQUIRED`, `E_VERIFIER_REQUIRED`, `E_NO_RELEASE_KEY`, `E_NO_SUDO`, `E_TEMP_DIRECTORY_FAILED`, `E_VERSION_FETCH_FAILED`, `E_VERSION_INVALID`, `E_VERSION_OTHER_CHANNEL`, `E_VERSION_OLDER`, `E_BUNDLE_DOWNLOAD_FAILED`, `E_SIGNATURE_INVALID`, `E_BUNDLE_VERIFY_FAILED`, `E_BUNDLE_EXTRACTION_FAILED`, `E_SELF_TEST_FAILED`, `E_BUNDLE_INSTALL_FAILED`, `E_VERSION_SELECT_FAILED`, `E_DASSIE_COMMAND_SYMLINK_FAILED`, `E_GROUP_CREATION_FAILED`, `E_SYSTEMD_SERVICE_INSTALL_FAILED`, `E_SYSTEMD_DAEMON_RELOAD_FAILED`, `E_SYSTEMD_SERVICE_ENABLE_FAILED`, `E_SYSTEMD_SERVICE_START_FAILED`, `E_SYSTEMD_SERVICE_REMOVE_FAILED`, `E_BUNDLE_REMOVE_FAILED`.

## `dassie update`

`crates/dassie-cli/src/update.rs`. Options: `-f`/`--force`, `-t`/`--target-version <version>`, `--channel latest|canary`, `--mirror <url>`, `--no-restart`.

1. **Find the install.** The binary's own path, resolved, must be `<base>/<its version>/bin/dassie`; otherwise it says it was not installed by the installer. It needs the release key built in. It holds a lock on `<base>/.update.lock` while it works ("another update is running"). A permission error says to run it as root or with sudo.
2. **Choose the version.**
   - With `--target-version`, that version (any channel, older too); the one installed only with `--force`.
   - Otherwise it reads `meta/<channel>` of its own channel, or of the one `--channel` names: at most 128 bytes, trimmed, which must name a version of that channel, or the update fails. It moves only to a version that supersedes its own; to the same version only with `--force`, and to an older one never unless forced. A switch with `--channel` takes the version the new channel's file names: the mirror chooses neither a version of the channel left nor an older one of the same channel.
   - Nothing to do: "Already up to date (<version>)", or that the mirror offers nothing newer. Both succeed.
3. **The mirror** is `--mirror`, else `DASSIE_MIRROR_URL`, else `https://get.dassie.land`. It must be `https://`, and may be on a private network, since the owner names it; `DASSIE_MIRROR_CA_FILE` adds roots to trust it by. Any status but 200 fails.
4. **Verify.** `SHA256SUMS` and its signature (30 s each, the signature at most 1 KiB) are verified with the built-in key for the target version. Then the gzip bundle for its platform must be listed and match its sum. The bundle is at most 64 MiB and gets 10 minutes.
5. **Unpack** only into a staging directory beside the versions, `<base>/.staging-<version>-<pid>`. `dassie_release::bundle` admits only directories and regular files under `dassie/` with plain names (letters, digits, `.`, `-`, `_`, not starting with a dot), paths at most 256 bytes, at most 64 entries and 256 MiB unpacked.
   - Modes are set explicitly, 0755 for directories and executables and 0644 for the rest, whatever the umask: the node runs as a user of its own.
   - Every file and directory is synced, each directory after its contents, so `current` never names a version a power cut left half written.
6. **Check the new binary:** it must print `dassie <version>` for `--version` and pass `verify-install`, each within 60 s.
7. **Switch.**
   - The version moves into place (one already there is renamed aside and removed).
   - The units are installed into `/etc/systemd/system` (`DASSIE_SYSTEMD_UNIT_DIR` for tests), each only if it changed, written aside, mode 0644, synced and renamed into place. The owner's drop-ins are left alone.
   - `current` moves with one rename of a new symlink, synced.
   - Unless `--no-restart`, or systemd is not running (`/run/systemd/system` missing): systemd reloads if a unit changed, units new in the release are enabled, and `dassie.service` restarts. Its readiness bounds the wait.
8. **Record** how it went in `<base>/last-update` ([below](#recording)).
9. **Prune:** keep the version it came from and the new one, and remove every other version, and whatever an interrupted update or install left (`.staging-`, `.replaced-`, `.install-`, `.current-`).

### Rolling back

If the node does not come up after the switch, the update goes back:

1. Reinstall the old version's units, point `current` back at it, and reload systemd.
2. Stop `dassie.service`: systemd may be restarting the failed node, and the database must be left alone while it is put back.
3. Put back the database from before the migration, if the new version migrated it. The old version refuses a database whose schema is newer than it knows ([Node](node.md#database)), so the daemon keeps a copy, `dassie.sqlite3.pre-migration` beside the database, written with `VACUUM INTO` just before it migrates and before it serves anything; nothing the copy lacks has reached a peer.
   - The updater puts it back only if the old version could not open the database as it is (its application ID is Dassie's and its schema is newer), and only if the copy is from this update, by its time (at most 1 s before the update switched), and is a database the old version can open.
   - The database it replaces, with its WAL files, is kept as `dassie.sqlite3.rolled-back`, for the owner to look at.
   - The data directory is `/var/lib/dassie`, or where `DASSIE_STATE_DIRECTORY` says.
4. Start `dassie.service`. The update fails: "dassie <new> did not start, and <old> runs again".

The owner can go back too, with `dassie update --target-version <version>`.

### Recording

Each update writes `<base>/last-update` (mode 0644, written aside and renamed), lines of `key=value`:

```
at=<unix ms>
outcome=up-to-date|updated|rolled-back|failed
target=<version>          (updated, rolled-back)
problem=<one line>        (rolled-back, failed)
```

`dassie status` reads it ([Node](node.md#dassie-status)).

## Tests

- **`crates/dassie-cli/tests/update.rs`** runs the binary against a mirror served over TLS, from an install in a temporary directory. The binary is built with the `test-key` feature, which trusts `testdata/release`'s key; the Nix check enables it. The tests check:
  - installing, being up to date, and an older version on offer;
  - tampered bundles, sums signed for another version, a bad or missing signature, links and paths outside `dassie/`, and a binary that names another version;
  - one update at a time, and modes that let the node run under a strict umask;
  - the record each update leaves.
- **The store's tests** check the copy before a migration, and that a rollback puts it back only when it must and only a copy it can trust.
- **`tests/e2e/install_test.ts`** installs, updates, rolls back an update whose node does not start (with the database from before it), and uninstalls, on Debian 12, Ubuntu 24.04, Fedora 42 and CentOS Stream 9, against a mirror of test releases (`nix build .#install-test-mirror`: 1.0.0, 1.0.1, 1.0.2 and the canary `canary-2026-10-16-120000-abcdef0`, built in the `install-test` profile and signed with the test key; 1.0.2's unit passes an unknown option, so its node does not start). [Testing](testing.md#installer-and-updater) describes the containers, the mirror and each step. The release workflow runs one job per distribution, before it signs; CI does not run them.
- **`scripts/release_test.ts`** tests the signing script.
- **The installer's ShellCheck** runs with `nix flake check`.

## Decisions

- [D11](../decisions/D11-release-integrity.md): signed sums with minisign rather than Sigstore, the trusted comment, unsigned channel files, verify before extract, atomic switch and rollback.
- [D14](../decisions/D14-build-system.md): the Nix flake for release builds, cross-compiled with musl.
- [D21](../decisions/D21-tooling-on-node-and-pnpm-not-deno.md): the wallet's packages fetched from the lockfile's hashes.
- [D27](../decisions/D27-simulations-run-continuously-from-random-seeds.md): releases are to wait for the simulation campaign; that check is [not built](../roadmap.md#continuous-simulation).

## Original

- [6 §5](../original/06-user-interfaces-and-tooling.md): the original's Docker build, bundle layout with a bundled Node.js, unsigned mirror, units, installer and updater.
- [2 §3.1, §12.4–§12.5](../original/02-node-runtime.md): its install layout, `dassie update` and `install.sh`.
