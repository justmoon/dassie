# Market maker VM

A VM on GCP that runs `dassie market-maker` ([Rate feed](rate-feed.md#market-maker)) against core nodes of the testnet, its targets, with the market maker's two nodes beside it, so that the testnet's core nodes are brought back toward balance and their feeds have a subscriber that acts on them.

**Paths:** `apps/market-maker/`
**Read with:** [Rate feed](rate-feed.md#market-maker), [Release](release.md), [Node](node.md), [Network](network.md#pinned-uplinks), [Settlement hubs](hubs.md#in-the-node), [DSUSD keeper](dsusd-keeper.md#its-vm)

**It runs once the testnet's default hubs are built in.** Its nodes join the testnet's default XRP hub and default DSUSD hub, one each, as members by their rules built into the release (`member::DEFAULT_TESTNET_HUB` and `member::DEFAULT_TESTNET_DSUSD_HUB`, [Settlement hubs](hubs.md#in-the-node)), and so must its targets. While a release builds in no rules of a hub, `dassie setup` cannot add that hub's scheme, and the market maker does not start. So the VM stays stopped until boot1–4 found both hubs and a release builds them in; starting it then is an operator's step, from fresh node databases (the nodes' state directories under `/var/lib/dassie-mm/`): `dassie setup` adds only the schemes that are missing and changes none it finds, so a node that kept a database from before would keep its XRP Ledger schemes peering rather than as owner books.

It is tooling ([engineering](../engineering.md#where-the-rules-bend)): shell scripts and systemd units in `vm/`, with no code of its own. The nodes and the market maker are the released `dassie` binary, so the VM runs what owners run.

## What runs

- **Two nodes,** instances of `dassie-mm-node@.service`, one per asset of the pool, each a member of the settlement hub of its asset, which it funds from its owner book of that asset ([Settlement](settlement.md#owner-books)). They are periphery nodes: standalone, with no hostname, and no relay, since nobody pays them at a name. Both pin every target as an uplink ([Network](network.md#pinned-uplinks)), by the node ID each target's [discovery document](http.md#the-nodes-discovery-document) names, which is where the market maker reads each target's ID and key too.
- **They listen on every address.** QUIC binds the HTTPS listener's address, and must reach the targets; nothing reaches the listeners from outside, since the VM's network has no firewall rule. Their rate feed's socket is on loopback, at a port the system chooses: a periphery node publishes no feed, and two nodes on one host cannot both take the default port.
- **The market maker,** `dassie-mm.service`, starts after both nodes, and after the clock is synced (`time-sync.target`, which the VM holds back until then with its clock daemon's wait service), since it and `dassie setup` refuse a phrase that says it was made more than a day after the clock, and nobody is there to set it ([phrase-format](../decisions/phrase-format.md)). Restarting a node restarts it too. A node that does not start keeps it waiting, not down: it tries again until the node answers. Before it runs, it sets each node up from its passphrase with `dassie setup`, which claims a fresh node and checks that one set up before is the phrase's, and joins the hub it names (`--hub`) unless the node is in it: the XRP node the XRP hub, with the owner book of XRP, and the DSUSD node the DSUSD hub, with the owner books of XRP, which pays its hub's gas, and of DSUSD, each book peering with nobody and each hub's scheme with no rules, so the default hub of its asset. With a hub named, setup joins no other, so the DSUSD node joins no XRP hub. Then it writes its config, which names each node's hub ledger and owner book, and runs.
- **All three run as one system user,** with the sandbox of `dassie.service` ([Release](release.md#systemd-units)), and each node's database and IPC socket in a directory of its own that only that user reaches.

Why two nodes, and not one, or two per target, is the market maker's design ([Rate feed](rate-feed.md#two-nodes-each-in-one-hub-pinned-to-every-target)): a payment from one to the other through a target is what makes that target convert, and each node's uplink to every target lets the two trade through any of them.

## Secrets

The nodes' two passphrases are secrets in Secret Manager, phrases of the test realm made once and kept: each is a node's identity and its accounts' keys ([Identity](identity.md)), and whoever reads it holds that node's funds.

- **The VM's service account can read those two secrets and nothing else** in a project that also holds the bootstrap nodes' phrases. Any process on the VM can ask the metadata server for its token; the token reads only what the nodes and the market maker already hold the keys of.
- **On tmpfs, only while the market maker runs.** Root reads the phrases before the market maker starts (`phrases.sh`) into its unit's runtime directory, readable only by its user, which systemd removes when it stops. No phrase or token is written to disk, logged or put on a command line.
- The market maker needs the phrases themselves, not only the nodes: it signs its swaps on the ledger with the nodes' account keys, derived from them ([Rate feed](rate-feed.md#two-nodes-each-in-one-hub-pinned-to-every-target)).

## Releases and updates

The VM runs signed releases only, checked as an owner's install checks them ([release-integrity](../decisions/release-integrity.md)).

- **The first install is `install.sh`'s,** with `--no-service`: the checkout's script, with the checkout's release key written in as a release writes it, so that it installs what any owner's install would, in the [install layout](release.md#install-layout), without `dassie.service` on ports 80 and 443, which this VM does not run.
- **Updates are the updater's,** run daily with `--unit` for each node (`update.sh`): it installs none of the units a release brings, since the VM runs units of its own, and restarts both nodes in place of `dassie.service`, and the market maker with them.
- **A node that does not come up sends the VM back** as the updater sends any install back ([Rolling back](release.md#rolling-back)): to the version `current` named before, with each node's database from before a migration put back, and the rollback recorded, so that the next update passes over that version until the mirror offers another. A market maker that the rollback stopped is started again.
- **A version from before `--unit` updates as it always has,** once (`update-before-unit.sh`): without its restart, the units it brings sent to a scratch directory, and a rollback of its own that puts back no database.

## Settings

The targets, the trade sizes, the bands, the margin and the slippage are instance metadata, each with a default in `startup.sh`, which writes them for the units at every boot. The defaults are starting points.

- **The targets** are `mm-targets`, URLs separated by commas, boot1–boot3 by default (boot4 runs no DSUSD scheme). The config names them as `targets`, the one form the market maker reads, however many there are.
- **Their node IDs,** which the nodes pin (`DASSIE_UPLINKS`), are what each target's discovery document names, read at every boot. A target that does not answer keeps the ID read for it before; one never read is left out, so that both nodes still start, and pinned at a later boot. Two URLs of one node pin it once.
- **The bands** are about what a node holds of its asset in its hub and its owner book together ([Rate feed](rate-feed.md#bands)), and each band's middle is above the most a node keeps in its hub, so what a rebalance spends is in the book. The XRP band's top is below what the testnet faucet grants a new account, so the market maker's first rebalance buys DSUSD into its DSUSD node from the XRP node's surplus, and the VM needs no DSUSD bought by hand. It is also above what the XRP node keeps once that rebalance has filled the DSUSD band: the two bands together hold the whole grant, so neither node starts above its band with the other full.

The units and the scripts come from `main` at every boot; `startup.sh` itself comes from the instance metadata, as for the [DSUSD keeper](dsusd-keeper.md#its-vm). So a change to a setting, a unit or a script takes effect when the VM is reset, and a change to `startup.sh` once it is put in the metadata too.

## Watching it

It needs no SSH, and takes no project-wide SSH keys. The nodes, the market maker and the updater log to the serial console as well as the journal, which `gcloud compute instances get-serial-port-output` reads. There the nodes' line that says they listen names their QUIC endpoint, `dassie setup` says what it did for each node, and the market maker logs one line per decision ([Rate feed](rate-feed.md#the-loop)). On each target, `dassie status` counts the feed's subscribers ([Rate feed](rate-feed.md#what-the-owner-sees)).

## Decisions

- [passive-rebalancing](../decisions/passive-rebalancing.md): market makers, not the nodes, move a node's holdings back toward balance.
- [release-integrity](../decisions/release-integrity.md): signed sums, the trusted comment, the switch with one rename and the rollback, which the VM takes from the installer and the updater.
