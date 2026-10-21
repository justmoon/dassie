# The desktop app runs the node in its process and shows the wallet from the node's origin

**Areas:** [Desktop](../areas/desktop.md), [Node](../areas/node.md), [HTTP](../areas/http.md), [Overview](../overview.md#how-the-code-is-split), [designs/periphery](../designs/periphery.md#6-owner-apps)

## Facts

- A node without a hostname is a periphery node: it dials its uplinks and keeps a connection to its relay, which reaches it at its default name ([Network](../areas/network.md#periphery-nodes), [default-relay](default-relay.md)). It needs no public IP, so it can run on a computer its owner uses every day.
- A periphery node is reachable only while it runs.
- The wallet works only from the node's own `https://` origin: the node's [same-origin rule](../areas/http.md#same-origin-rule), its `__Host-` session cookie and the wallet's `wss://` RPC URL all assume it. The node's certificate on a machine without a hostname is self-signed for `localhost` ([Certificates](../areas/http.md#certificates)).
- The owner enters the passphrase into the page that serves the wallet ([Wallet](../areas/wallet.md#key-derivation)), so whatever serves that page can keep it.
- A window on Linux means GTK and WebKitGTK, which are system libraries linked against glibc. The `dassie` binary is static, against musl ([Release](../areas/release.md)).
- The daemon is a library function that neither owns its runtime nor handles signals (`daemon::run`); the CLI adds both.

## Decision

- **The app runs the node in its own process,** on a runtime and a thread of its own, and stops it when it quits. One binary, one version: nothing to keep in step, and the app reads what it shows (the setup URL, the certificate, how the relay stands) from the node directly.
- **The window loads the wallet from the node's origin,** `https://localhost:<port>` on loopback, and trusts exactly the certificate the node in its process serves there. The node's security rules stay as they are for every client.
- **Tauri, on WebKitGTK, linked against glibc,** as the [periphery design](../designs/periphery.md#10-how-it-fits-the-node) named for owner apps. It is the one binary that is not static: a window cannot be.
- **The app keeps running in a tray, and starts at login** unless its owner turns that off, since its node is reachable only while it runs.
- **Linux first, as a Nix package** in the repository's flake (`nix run .#desktop`). Packages for other distributions, the app in the mirror's flake, its updates, and other systems are [open](../roadmap.md#periphery).

## Accepted

- **A node's panic ends the app,** as it ends `dassie daemon`; starting the app again recovers the node, as a restart does.
- **The app is a second build of the node's code,** against glibc and with the window's libraries, beside the static release binary. Both are the same crates; the app's build is not the one the release signs.
- **Every build of the whole workspace needs GTK and WebKitGTK,** in CI and in the dev shell, since the app is a workspace member like any other crate and is linted and tested with them.
- **A local process can reach the wallet's port,** as it can on any machine that runs a node; logging in still needs the passphrase, and the session cookie the window keeps is readable by the owner's user alone ([Private files](../areas/desktop.md#private-files)).
- **Unmaintained crates in Tauri's tree:** its Linux backend is built on the GTK 3 bindings, and its plugins on zbus 5, which bring crates RustSec lists as unmaintained. `deny.toml` ignores those advisories by ID, each with its reason, so that any other advisory against them still fails.
- **WebKitGTK is the system's,** with its own updates and graphics quirks.

## Rejected

- *Running the released `dassie` binary as a child process:* the node would be byte for byte the server's, and a crash of the window would not stop it. But the app would ship two binaries that must agree, supervise a process, and get the setup URL and certificate over IPC; the node's code is the same either way.
- *Serving the wallet from the app itself* (a Tauri custom protocol): the wallet's origin would be the app's, so the node would have to accept another origin for its session cookie and RPC, which weakens its same-origin rule for every node.
- *Plain HTTP on loopback:* the node serves HTTPS everywhere, in development too ([Overview](../overview.md#principles)), and `__Host-` cookies need a secure origin.
- *Trusting any certificate on `localhost`:* any process that took the port first would get the passphrase. The window trusts only the certificate its own node serves.
- *Opening the owner's browser at the node's URL:* the browser warns about the self-signed certificate, and an owner trained to click through that warning is an owner who can be phished.
- *The app only while its window is open:* the node would be offline most of the time, and its name would answer nothing.
- *AppImage, .deb or Flatpak first:* each needs its own packaging, signing and update story; a Nix package is the quickest way for owners to try the app, and the others follow.
