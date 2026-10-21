# Desktop app

`dassie-desktop` is a node for a computer its owner uses every day: it runs the owner's node in its own process, as a [periphery node](network.md#periphery-nodes), and shows the wallet in a window. No public IP, hostname or certificate authority is involved: the node dials out to its uplinks and keeps its [relay](node.md#a-chosen-relay), which reaches it at its default name. It is built for Linux, as a Nix package ([desktop-app](../decisions/desktop-app.md)).

**Paths:** `crates/dassie-desktop/`, `nix/desktop.nix`, `nix/desktop-libs.nix`
**Read with:** [Node](node.md), [HTTP](http.md), [Wallet](wallet.md), [designs/periphery](../designs/periphery.md)

It is not the paired owner app of the [periphery design](../designs/periphery.md#6-owner-apps), which reaches a node on another machine; pairing is [not built](../roadmap.md#periphery).

## The node

The app runs the daemon as `dassie daemon` does (`daemon::run`), on an async runtime and a thread of its own, so the node never shares the window's event loop or its runtime. It reads the same environment, with the same defaults: the data directory, the IPC socket, the bootstrap nodes and the default relay are a server node's, so `dassie status` reports on the desktop node too. The app sets no dev switch.

The node is the same code as the server's, and stops the same way: quitting the app cancels the daemon and waits for it to [shut down](node.md#shutdown). A node that stops on its own, or does not start, is shown in the window with why, and the app keeps nothing else running.

## Sockets

- **HTTP and HTTPS listen on loopback only,** so only this machine reaches the wallet, `/api` and `/rpc`. Their ports are chosen once, kept in the app's data directory and taken again at the next start, so that the wallet's origin, and what the window stores for it, stay the same. A port another program took is replaced by a new one.
- **QUIC listens on every address, on a port the system picks** ([Runtimes](node.md#runtimes)): a periphery node dials its relay and its uplinks, and is never dialled.
- **Being core is not supported.** A hostname set in the wallet makes the node core, which needs its listeners reachable from the internet and port 80 for ACME. The desktop app keeps them on loopback.

## The window

The wallet is served by the node, at `https://localhost:<port>`, and the window loads it from there, so the node's [same-origin rule](http.md#same-origin-rule), its `__Host-` session cookie and its CSP hold as in a browser, and the passphrase is entered only into a page the node itself serves ([Wallet](wallet.md#key-derivation)).

- **The certificate the window trusts there is exactly the node's.** The node's certificate is self-signed ([Certificates](http.md#certificates)); the window trusts the one the node in this process serves, for `localhost` alone, before its first load. Should the node have made a new one since, the window trusts what it is shown only if it is byte for byte the certificate the node serves now, on the node's own origin. No other certificate error is ever passed over.
- **The page gets no native API.** No Tauri capability names the node's origin or the app's own page, so nothing the wallet runs can call into the app.
- **The window loads only the node's origin and the app's own page,** which says that the node starts, or why it could not. A link to a public `https` page opens in the owner's browser; anything else, other local origins and private addresses among them, loads nowhere, so a page in the window cannot reach another service on the machine or its network.

## Lifecycle

A periphery node is reachable only while it runs, so the app keeps running when its window closes, with a tray icon that shows how the node stands (set up, its relay connected, connecting or unreachable), opens the wallet again and quits.

- **Starting at login** is on unless the owner turns it off in the tray: an XDG autostart entry starts the app with `--hidden`, in the tray without a window. The owner's choice is kept in the app's data directory, so an entry the owner turned off stays off. While it is on, the entry is written again at every start, naming the binary by name where `PATH` finds it, so that an upgrade that moves the binary, as a Nix profile does, does not leave the entry behind.
- **Closing the window** destroys it, which frees its web process; the node and the tray stay.

## One node

A second launch shows the first one's window and starts nothing. Beneath that, [one daemon per data directory](node.md#database) holds whatever starts a node, so a `dassie daemon` on the same data directory, or a second app, refuses to start rather than book beside the first.

## Decisions

- [desktop-app](../decisions/desktop-app.md): the node in the app's process, the wallet from the node's own origin, and a Nix package first.
