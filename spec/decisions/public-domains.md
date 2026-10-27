# Payment names live under dassiepay.net, the testnet's under test.dassiepay.net, memo types and the mirror under dassie.land

**Areas:** [Payments](../areas/payments.md#the-nodes-names), [HTTP](../areas/http.md#front-doors), [XRPL](../areas/xrpl.md), [Release](../areas/release.md#the-mirror), [Network](../areas/network.md), [Overview](../overview.md#frozen-surfaces).

## Facts

- **Some names leave the binary and are kept by others.** Once someone outside the release stores a name, a release can no longer move it:
  - A node without a hostname is paid at `<node ID>.dassiepay.net`, and a node that holds a name at `<name>.dassiepay.net` ([Payments](../areas/payments.md#the-nodes-names)). Other wallets keep those URLs as wallet addresses, in grants and in payment requests, and a Dassie-aware client checks the node's signature against the node ID in the host ([Payments](../areas/payments.md#signed-responses)).
  - Each XRPL scheme marks its settlements with a memo type of its own, `https://dassie.land/settlement/<scheme>/v<n>`, whose scheme ID names its network (`xrpl-testnet-xrp`, `xrpl-xrp`, …: [scheme-names](scheme-names.md)), and the deposit that opens a settlement hub's account carries the hub's, `https://dassie.land/hub/open/v1` ([Settlement hubs](../areas/hubs.md#the-escrow)). They stay on the ledger for good, and a node tells its peers' settlements by them ([XRPL](../areas/xrpl.md)).
  - Installed nodes' updaters and the installer fetch from `get.dassie.land` (`DEFAULT_MIRROR` in `crates/dassie-release/src/layout.rs`), `sh.dassie.land` serves `install.sh`, and a NixOS owner's `flake.lock` records a release's flake at `get.dassie.land` ([Release](../areas/release.md#the-mirror)).
- **Other names are only the release's.** The bootstrap nodes (`boot*.dassie.xyz`), the default relay (`relay.dassiepay.net`) and the proof servers (`*.xrpl.dassieland.net`) are named in the binary, the first two with their keys, and nothing outside a release stores them. Each release can move them; nodes follow when they update.
- **A node's ID, its name in DNS, is the same in both realms today.** It derives from the passphrase alone ([Identity](../areas/identity.md#node-id)). A front door reads one network's registrars ([HTTP](../areas/http.md#front-doors)), and the live realm will have bootstrap nodes of its own ([roadmap](../roadmap.md#real-money)).
- **dassie.land is the project's site,** served from the same load balancer as the mirror ([website-hosting](website-hosting.md)). Under `dassiepay.net`, every name but `relay` and `test` belongs to someone's node, and its answers are that node's; both are reserved, so no owner registers them (`RESERVED` in `crates/dassie-peer/src/names/name.rs`).

## Decision

- **Payment names** are under `dassiepay.net` in the live realm, `<node ID>.dassiepay.net` for a node without a hostname and `<name>.dassiepay.net` for a name, and under `test.dassiepay.net` on the testnet, `<node ID>.test.dassiepay.net` and `<name>.test.dassiepay.net`. Each realm's names are served by a front door of that realm's.
- **Memo types** are URIs under `https://dassie.land/`: `settlement/<scheme>/v<n>` for a scheme's settlements, and a path of its reader's for an owner's payment that carries one, such as `hub/open/v<n>`. A memo type is a protocol version: its path names what reads it and the version, so a new meaning is a new path.
- **The mirror** is `get.dassie.land`, and the installer is at `sh.dassie.land`.
- These are frozen ([Overview](../overview.md#frozen-surfaces)): both payment domains, the memo types' domain, the mirror and the installer's host, which README one-liners and owners' provisioning scripts name. A change is a new name served beside the old one, for as long as anyone may still use the old.
- **The realms share the mirror and the memo types' domain.** One binary serves both realms, so it updates from one mirror. Each network's scheme has a memo type of its own (`xrpl-testnet-xrp` and `xrpl-xrp`, `xrpl-testnet-rlusd` and `xrpl-rlusd`), so a settlement on one network never passes for another's, and the domain need not tell them apart.
- **One passphrase is a different node in each realm,** so no name can mean one node on the testnet and another, or the same, in the live realm. The phrase itself is to say its realm, so a live phrase is refused on a test node and the other way round ([roadmap](../roadmap.md#smaller-open-points)).
- **The rest stay the release's:** the bootstrap nodes, the default relay and the proof servers move by a release, with no step for anyone outside it.

## Accepted

- **Two domains to keep for good,** and a subdomain of one of them for the mirror. Losing either breaks what others stored: wallet addresses and payment requests under `dassiepay.net`, and the meaning of every memo on the ledgers under `dassie.land`.
- **A memo type names a web domain that need not serve anything at that path.** It is an identifier, not a link; nothing fetches it.
- **The testnet's names may be reset with the testnet,** as its names are ([Network](../areas/network.md#names)), and move once, from `dassiepay.net` to `test.dassiepay.net`. Once the live realm opens, its names are kept for good.

## Rejected

- *Payment names under dassie.land:* a node's answers would be same-site with the project's own pages, so a cookie scoped to the site, or a page that trusts same-site requests, would reach every node's name. A domain apart keeps the nodes' names away from the project's.
- *Memo types under dassiepay.net:* they mark a scheme's meaning on the ledger, which is the protocol's, not a payment name's, and every name under `dassiepay.net` but `relay` is a node's.
- *A mirror per realm:* the binary is the same, so two mirrors would hold the same releases, and a node would need to know its realm before it could update.
- *Freezing the bootstrap nodes', the relay's and the proof servers' names too:* nothing outside a release stores them, so freezing them would only stop the project from moving its own machines.
- *The testnet keeping `dassiepay.net`, and the live realm taking a domain of its own:* the live realm's names are the ones kept for good, printed and stored by others, so they get the shorter name; the testnet's are reset anyway, so moving them costs nothing.
- *Memo types under a domain that never serves the testnet:* each scheme ID already names its network, and a memo type is a protocol version, not a site, so a second domain would tell apart nothing the path does not.
