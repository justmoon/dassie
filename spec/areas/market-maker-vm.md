# Market maker VM

A VM on GCP that runs `dassie market-maker` ([Rate feed](rate-feed.md#market-maker)) against core nodes of the testnet, its targets, with the market maker's node beside it, so that the testnet's core nodes are brought back toward balance and their snapshots have a reader that acts on them. The node reads each target's snapshot over the peer protocol, and the market maker swaps through the target with the node ([peer-swaps](../decisions/peer-swaps.md)).

**Paths:** `apps/market-maker/`
**Read with:** [Rate feed](rate-feed.md#market-maker), [Release](release.md), [Node](node.md), [Network](network.md#pinned-uplinks), [Settlement hubs](hubs.md#in-the-node), [DSUSD keeper](dsusd-keeper.md#its-vm)

**It runs in the testnet's default hubs.** Its node joins the testnet's default XRP hub and default DSUSD hub, as a member by their rules built into the release (`member::DEFAULT_TESTNET_HUB` and `member::DEFAULT_TESTNET_DSUSD_HUB`, [Settlement hubs](hubs.md#in-the-node)), and so must its targets. While a release builds in no rules of a hub, `dassie setup` cannot add that hub's scheme, and the market maker does not start. A node refuses any database made before the testnet's reset, and starts only once it is moved away ([Node](node.md#database)), so the VM starts on a release with the reset's hubs only from a fresh node database (the node's state directory under `/var/lib/dassie-mm/`), an operator's step ([roadmap](../roadmap.md#token-hubs)).

It is tooling ([engineering](../engineering.md#where-the-rules-bend)): shell scripts and systemd units in `vm/`, with no code of its own. The node and the market maker are the released `dassie` binary, so the VM runs what owners run.

## What runs

- **One node,** `dassie-mm-node.service`, a member of the settlement hubs of both assets of the pool, which it funds from its owner books of each ([Settlement](settlement.md#owner-books)), both on its one XRP Ledger account. It is a periphery node: standalone, with no hostname, and no relay, since nobody pays it at a name. It pins every target as an uplink ([Network](network.md#pinned-uplinks)), by the node ID each target's [discovery document](http.md#the-nodes-discovery-document) names, and peers with each over both hubs.
- **It listens on every address.** QUIC binds the HTTPS listener's address, and must reach the targets; nothing reaches the listeners from outside, since the VM's network has no firewall rule. Its snapshot socket is on loopback, at a port the system chooses: a periphery node answers no request for a snapshot.
- **The market maker,** `dassie-mm.service`, starts after the node, and after the clock is synced (`time-sync.target`, which the VM holds back until then with its clock daemon's wait service), since it and `dassie setup` refuse a phrase that says it was made more than a day after the clock, and nobody is there to set it ([phrase-format](../decisions/phrase-format.md)). Restarting the node restarts it too. A node that does not start keeps it waiting, not down: it tries again until the node answers. Before it runs, it sets the node up from its passphrase with `dassie setup`, which claims a fresh node and checks that one set up before is the phrase's, and joins the hubs it names (`--hub`) unless the node is in them: the XRP hub and the DSUSD hub, with the owner books of XRP, which also pays the DSUSD hub's gas, and of DSUSD, each book peering with nobody and each hub's scheme with no rules, so the default hub of its asset; each hub's funding sweeps what is beyond the ceiling into the owner book, which the rebalances spend ([Rate feed](rate-feed.md#holdings), [Payments](payments.md#where-received-money-is-kept)). Then it sets each hub's target, what the node keeps there, to a hundred largest packets, 10 of the hub's asset, with `dassie hub target`, rather than the default thousand ([Settlement hubs](hubs.md#in-the-node)). The script comes from `main` and the binary is the installed release, so a release that knows no `hub target` only gets a note, and the market maker runs on that release's own limits; any other failure stops it. Setup and the targets run at every start, so a node set up before takes both too. Then it writes its config, which names the node's socket, its phrase's file, each asset's hub ledger, owner book and cap, and the strategy's settings, and runs. A release from before the market maker valued its inventory by a curve refuses that config, which `dassie market-maker --check` tells, and gets a config of bands instead, from the metadata's settings of bands where a VM set up before has them, so it runs until the update timer installs a newer one. The node swaps only when the market maker asks: it offers no swaps and has no targets of its owner's, and reads its uplinks' snapshots, as a new node does.
- **Both run as one system user,** with the sandbox of `dassie.service` ([Release](release.md#systemd-units)), and the node's database and IPC socket in a directory of its own that only that user reaches.

Why one node, and not one per asset, is the market maker's design ([Rate feed](rate-feed.md#one-node-in-both-hubs-pinned-to-every-target)): a swap with a target over two methods of their peering makes that target convert, and the node's uplink to every target lets it trade through any of them.

## Secrets

The node's passphrase is a secret in Secret Manager, `mm-phrase`, a phrase of the test realm made once and kept: it is the node's identity and its accounts' keys ([Identity](identity.md)), and whoever reads it holds the node's funds.

- **The VM's service account can read that secret and nothing else** in a project that also holds the bootstrap nodes' phrases. Any process on the VM can ask the metadata server for its token; the token reads only what the node and the market maker already hold the keys of.
- **On tmpfs, only while the market maker runs.** Root reads the phrase before the market maker starts (`phrase.sh`) into its unit's runtime directory, readable only by its user, which systemd removes when it stops. No phrase or token is written to disk, logged or put on a command line.
- The market maker needs the phrase itself, not only the node: it signs its rebalances on the ledger with the node's account key, derived from it ([Rate feed](rate-feed.md#one-node-in-both-hubs-pinned-to-every-target)).

## Releases and updates

The VM runs signed releases only, checked as an owner's install checks them ([release-integrity](../decisions/release-integrity.md)).

- **The first install is `install.sh`'s,** with `--no-service`: the checkout's script, with the checkout's release key written in as a release writes it, so that it installs what any owner's install would, in the [install layout](release.md#install-layout), without `dassie.service` on ports 80 and 443, which this VM does not run.
- **Updates are the updater's,** run daily with `--unit` for the node (`update.sh`): it installs none of the units a release brings, since the VM runs units of its own, and restarts the node in place of `dassie.service`, and the market maker with it.
- **A node that does not come up sends the VM back** as the updater sends any install back ([Rolling back](release.md#rolling-back)): to the version `current` named before, with the node's database from before a migration put back, and the rollback recorded, so that the next update passes over that version until the mirror offers another. A market maker that the rollback stopped is started again.
- **A version from before `--unit` updates as it always has,** once (`update-before-unit.sh`): without its restart, the units it brings sent to a scratch directory, and a rollback of its own that puts back no database.

## Settings

The targets, the caps, the curve, the margin, the threshold and the slippage are instance metadata, each with a default in `startup.sh`, which writes them for the units at every boot. The defaults are starting points.

- **The targets** are `mm-targets`, URLs separated by commas, boot1–boot3 by default (boot4 runs no DSUSD scheme). The market maker's config does not name them: they are the node's uplinks.
- **Their node IDs,** which the node pins (`DASSIE_UPLINKS`), are what each target's discovery document names, read at every boot. A target that does not answer keeps the ID read for it before; one never read is left out, so that the node still starts, and pinned at a later boot. Two URLs of one node pin it once.
- **The strategy's settings** are `mm-curve-ppm` (1%), `mm-margin-ppm` (0.2%), `mm-threshold-ppm` (5 points) and `mm-slippage-ppm` (1%), in parts per million ([Rate feed](rate-feed.md#trades), [market-maker-inventory](../decisions/market-maker-inventory.md)).
- **The caps,** `mm-xrp-cap` and `mm-dsusd-cap`, are the most one trade pays, 10 of each asset by default: what the node keeps in each hub at the target of 10 that `run.sh` sets ([generous-hub-target](../decisions/generous-hub-target.md)), since a trade pays from the hub, and the rebalances spend from the book. The target is in `run.sh`, not the metadata, since the caps' defaults rest on it, and the script comes from `main` at every boot. A node that starts on the testnet faucet's XRP alone is far from even, so the market maker's first rebalance buys DSUSD with half of it, and the VM needs no DSUSD bought by hand.

The units and the scripts come from `main` at every boot; `startup.sh` itself comes from the instance metadata, as for the [DSUSD keeper](dsusd-keeper.md#its-vm). So a change to a setting, a unit or a script takes effect when the VM is reset, and a change to `startup.sh` once it is put in the metadata too.

## Watching it

It needs no SSH, and takes no project-wide SSH keys. The node, the market maker and the updater log to the serial console as well as the journal, which `gcloud compute instances get-serial-port-output` reads. There the node's line that says it listens names its QUIC endpoint, `dassie setup` says what it did, and the market maker logs a line for each trade, rebalance or hold ([Rate feed](rate-feed.md#the-loop)), a swap's with what it paid and brought back. `GetSwapStatus` on the node counts its swaps ([Connector](connector.md#the-swap-service)).

## Decisions

- [passive-rebalancing](../decisions/passive-rebalancing.md): market makers move a node's holdings back toward balance.
- [peer-swaps](../decisions/peer-swaps.md): a market maker of one node, which swaps through its targets.
- [release-integrity](../decisions/release-integrity.md): signed sums, the trusted comment, the switch with one rename and the rollback, which the VM takes from the installer and the updater.
