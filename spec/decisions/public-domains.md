# Payment names live under dassiepay.net, memo types and the mirror under dassie.land

**Areas:** [Payments](../areas/payments.md#the-nodes-names), [HTTP](../areas/http.md#front-doors), [XRPL](../areas/xrpl.md), [Release](../areas/release.md#the-mirror), [Network](../areas/network.md), [Overview](../overview.md#frozen-surfaces).

## Facts

- **Some names leave the binary and are kept by others.** Once someone outside the release stores a name, a release can no longer move it:
  - A node without a hostname is paid at `<node ID>.dassiepay.net`, and a node that holds a name at `<name>.dassiepay.net` ([Payments](../areas/payments.md#the-nodes-names)). Other wallets keep those URLs as wallet addresses, in grants and in payment requests, and a Dassie-aware client checks the node's signature against the node ID in the host ([Payments](../areas/payments.md#signed-responses)).
  - Each XRPL scheme marks its settlements with a memo type of its own, a URI under `https://dassie.land/settlement/`, and the owner's payments with `https://dassie.land/owner-payment/v1` (`crates/dassie-xrpl/src/scheme.rs`, `rlusd.rs`, `dsusd.rs`). They stay on the ledger for good, and a node tells its peers' settlements by them ([XRPL](../areas/xrpl.md)).
  - Installed nodes' updaters and the installer fetch from `get.dassie.land` (`DEFAULT_MIRROR` in `crates/dassie-release/src/layout.rs`), `sh.dassie.land` serves `install.sh`, and a NixOS owner's `flake.lock` records a release's flake at `get.dassie.land` ([Release](../areas/release.md#the-mirror)).
- **Other names are only the release's.** The bootstrap nodes (`boot*.dassie.xyz`), the default relay (`relay.dassiepay.net`) and the proof servers (`*.xrpl.dassieland.net`) are named in the binary, the first two with their keys, and nothing outside a release stores them. Each release can move them; nodes follow when they update.
- **A node's ID, its name in DNS, is the same in both realms.** It derives from the passphrase alone ([Identity](../areas/identity.md#node-id)). A front door reads one network's registrars ([HTTP](../areas/http.md#front-doors)), and the live realm will have bootstrap nodes of its own ([roadmap](../roadmap.md#real-money)).
- **dassie.land is the project's site,** served from the same load balancer as the mirror ([website-hosting](website-hosting.md)). Under `dassiepay.net`, every name but `relay` belongs to someone's node, and its answers are that node's.

## Decision

- **Payment names** are under `dassiepay.net`: `<node ID>.dassiepay.net` for a node without a hostname, and `<name>.dassiepay.net` for a name, served by the front door that `relay.dassiepay.net` is.
- **Memo types** are URIs under `https://dassie.land/`: `settlement/<scheme>/v<n>` for a scheme's settlements, and `owner-payment/v<n>` for the owner's payments.
- **The mirror** is `get.dassie.land`, and the installer is at `sh.dassie.land`.
- These three are frozen ([Overview](../overview.md#frozen-surfaces)). A change is a new name served beside the old one, for as long as anyone may still use the old.
- **The realms share the mirror and the memo types' domain.** One binary serves both realms, so it updates from one mirror. Each network's scheme has a memo type of its own (`xrp-testnet` and `xrp`, `rlusd-testnet` and `rlusd`), so a settlement on one network never passes for another's, and the domain need not tell them apart.
- **The live realm's default payment names are not decided.** Both realms use `dassiepay.net` today, which is enough while the live realm is refused. A live front door must read the live registrars, and a passphrase used in both realms gives one node ID in both. So the two realms cannot share one front door, and the live realm needs a domain of its own or a part of this one ([roadmap](../roadmap.md#smaller-open-points)).
- **The rest stay the release's:** the bootstrap nodes, the default relay and the proof servers move by a release, with no step for anyone outside it.

## Accepted

- **Two domains to keep for good,** and a subdomain of one of them for the mirror. Losing either breaks what others stored: wallet addresses and payment requests under `dassiepay.net`, and the meaning of every memo on the ledgers under `dassie.land`.
- **A memo type names a web domain that need not serve anything at that path.** It is an identifier, not a link; nothing fetches it.
- **The testnet's names may be reset with the testnet,** as its names are ([Network](../areas/network.md#names)). Once the live realm opens, its names are kept for good.

## Rejected

- *Payment names under dassie.land:* a node's answers would be same-site with the project's own pages, so a cookie scoped to the site, or a page that trusts same-site requests, would reach every node's name. A domain apart keeps the nodes' names away from the project's.
- *Memo types under dassiepay.net:* they mark a scheme's meaning on the ledger, which is the protocol's, not a payment name's, and every name under `dassiepay.net` but `relay` is a node's.
- *A mirror per realm:* the binary is the same, so two mirrors would hold the same releases, and a node would need to know its realm before it could update.
- *Freezing the bootstrap nodes', the relay's and the proof servers' names too:* nothing outside a release stores them, so freezing them would only stop the project from moving its own machines.
