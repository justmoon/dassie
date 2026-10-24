# Node

The `dassie` binary: the daemon that composes every protocol crate into a running node, and the command-line tools that configure, inspect and update it.

**Paths:** `crates/dassie-node/src/` (`daemon.rs`, `supervisor.rs`, `engine/`, `store/`, `env.rs`, `systemd.rs`, `ipc.rs`, `log.rs`, `clock.rs`, `node/`, `feed.rs`, `hub/`), `crates/dassie-node-core/src/`, `crates/dassie-cli/src/`
**Read with:** [Release](release.md), [HTTP](http.md), [RPC](rpc.md), [Ledger](ledger.md), [Connector](connector.md), [Network](network.md), [Identity](identity.md)

## Runtime model

- **One static binary** holds the daemon, the CLI, every protocol and the wallet, embedded at build time from `DASSIE_WALLET_DIST`. Without it the binary embeds a placeholder, so `cargo build` and `cargo test` need no Node.js; `verify-install` then fails, so no such binary is released.
- **Services are supervised tokio tasks** (`supervisor.rs`). A service that returns or panics before shutdown is restarted after a growing backoff; its state is what the wallet's service inspector shows. The one exception is an engine that keeps failing before the node is up, which ends the daemon instead ([Readiness](#readiness)).
- **The engine owns the money path's state and is its only writer** ([below](#the-engine)): the ledger, peers and settlement schemes, BTP tokens, and payments as they are created, paid and sent, with what grants have spent. Everything else changes that state only through the engine's commands, and live views follow change counters it publishes in `watch` cells ([RPC](rpc.md)). State that moves no money, such as the configuration, sessions, certificates, exchange rates, and Open Payments grants and quotes, is written to the store directly by the service it belongs to, which also forgets payments that are over. The config is a `watch` cell too, and the engine reconfigures itself when it changes.
- **The node's decisions are pure, in `dassie-node-core`:** setup and login checks, session lifetimes, rate limits, the same-origin rule, configuration values and certificate renewal are functions of their inputs, `now` and an injected RNG ([engineering](../engineering.md)). `dassie-node` only carries them out.
- **Time and randomness are injected.** Everything the node decides takes its time and randomness from `clock.rs`, so tests and simulations substitute their own. Two things decide nothing and are exempt: pacing I/O on the monotonic clock (timeouts, keepalives, the supervisor's backoff), and the TLS and ACME keys, which `rcgen` generates from the OS RNG.
- **Limits and timings** come from one `Policies` value (`node/mod.rs`): production, dev ([below](#dev-switches-warn-on-a-public-node)) and test settings.

### The engine

One task owns the money path: the ledger, the connector, the STREAM receivers and payers, and the settlement schemes. None of them needs a lock, and their steps happen in one order, as in a simulation.

- Everything else sends it commands through a bounded queue. It takes a batch, steps the core with each command, and persists what the batch changed in **one or more transactions, each before what it covers goes out**: settlements run only once their pending transfers are committed, and are committed in turn, and closing a BTP link that failed while the batch's messages went out is committed in another. The replies go out last. So a Fulfill goes upstream only once its transfer is on disk ([Connector](connector.md), [Ledger](ledger.md)).
- If the write fails, the core panics without handing anything over: nothing of the step goes out, so nothing the node sends, settles or acknowledges rests on state that never reached the disk. The supervisor restarts the task, which rebuilds the core from the database, at its last good commit: packets in flight do not survive, clients reconnect, and a peer request the engine was answering gets a 503. A failed commit is the only fault the simulations inject at the store ([Testing](testing.md#failed-commits), [random-seed-simulations](../decisions/random-seed-simulations.md)).
- The engine starts once the node has an identity, since its ILP address comes from the node ID.

### Checking state before each commit

In tests and simulations a commit never holds a broken invariant, so what stops the core leaves the state that last held for its successor. Release builds run only the cheap assertions on every commit, and the full sweep at most once a second, so there a broken invariant that only the sweep catches can reach the disk before the next sweep stops the core.

- **Cheap assertions** run at every change.
- **The full sweep** of every state machine's invariants runs before every commit when debug assertions are on (tests, simulations, the end-to-end binary). In release builds it is throttled (`SWEEP_EVERY_MS` in `engine/core/mod.rs`), since with full tables it costs about as much as a busy commit. The throttle counts engine time, not wall time, so a run stays the same for the same inputs.
- **The store commits with `synchronous = FULL`,** so a commit that returned is on disk, which the Fulfill rule above relies on.

## Startup

`daemon.rs`. The rules that the order serves:

- **Everything is checked before anything starts.** Every environment variable is read once, from a snapshot (`env.rs`), and a bad value stops the daemon with a message that names it. A daemon that cannot start exits 1 rather than running half-started, and leaves no IPC socket behind.
- **Pinned uplinks** (`DASSIE_UPLINKS`, [Network](network.md#pinned-uplinks)) are node IDs, each once, at most as many as a periphery node may keep (`MAX_UPLINKS`); a longer list stops the daemon, as any bad value does.
- **A setting the node's kind ignores is said at start.** A core node given [pinned uplinks](network.md#pinned-uplinks) warns that it ignores them: they are not wrong, since the node may turn periphery, but an owner who set them meant something the node does not do now.
- **The database opens, and migrates, before anything is served,** so the copy taken before a migration holds everything a peer has seen ([Database](#database)).
- **systemd's sockets are taken before the async runtime exists,** while the process has one thread.
- **The listeners come up first,** before anything that waits for the network, so ACME's HTTP challenge works at the first start.
- **Nothing blocks on the network.** Exchange rates, the settlement schemes' ledger connections and the bootstrap polls are readiness conditions with retries, not steps startup waits for. Services that need an identity wait for one, which may be long after startup.
- **The IPC socket appears only once the daemon answers.** The standalone daemon binds it in a private directory and renames it into place, so it is never reachable with looser permissions, before the daemon answers, or after one that failed ([RPC](rpc.md#ipc-socket)).
- **The setup URL is never logged:** whoever opens it first claims the node, and the journal is readable by groups that do not administer it. The daemon logs that `sudo dassie setup-url` prints it.

### Readiness

The daemon reports readiness (`READY=1` under systemd) once the node works, not once it listens, since an update rolls back only a node that does not come up ([Release](release.md#rolling-back), [ready-after-first-step](../decisions/ready-after-first-step.md)).

- **Ready once the engine has committed its first step,** with the listeners bound and the services started. The engine takes that step as soon as it has rebuilt itself from the database, without waiting for a command, a timer or the network, so a node with nothing to do is ready at once.
- **A node without an identity is ready at once.** It has no engine to wait for: the engine starts once the owner sets the node up, and its failures from then on are restarted like any other service's.
- **An engine that fails a few times before its first step ends the daemon** with an error (`START_ATTEMPTS` in `supervisor.rs`), instead of being restarted for ever: the process exits 1 without having reported readiness, so systemd fails the start and `dassie update` rolls back. Once the node is up, the engine is restarted as any service is.
- Readiness proves the engine starts on this database, not that it keeps running: a failure after its first step, such as a panic on some packet, is restarted and not rolled back.

## Shutdown

On SIGTERM or SIGINT the daemon stops accepting, gives requests in progress a bounded time to finish, closes the database and removes the socket it made. The engine stops at once, between two steps: what it committed is on disk, and packets in flight are lost, as in a crash ([Persistence](#persistence)). Draining them is [not built](../roadmap.md#not-built-yet).

## Runtimes

`--runtime standalone|systemd` chooses how the daemon gets its sockets; without it, `systemd` when `LISTEN_PID` is the daemon's own PID.

- **standalone** binds the configured ports (or `--http-port`/`--https-port` for one run), a UDP socket for QUIC on the HTTPS listener's address and port, a UDP socket for the [rate feed](rate-feed.md#the-socket) on the HTTPS listener's address at the feed's port (or `--feed-listen`), and an IPC socket path of its own. A host that embeds the daemon may bind QUIC on an address of its own (`DaemonOptions::quic`), so that a node whose listeners stay on loopback, where only its owner reaches the wallet, still dials its relay and uplinks from every address, as the [desktop app](desktop.md#sockets) does.
- **systemd** takes its sockets by name from the socket units ([Release](release.md#systemd-units)), and reports readiness to `NOTIFY_SOCKET` itself. Sockets that are not this process's, or a missing name of the three it needs, stop the daemon rather than letting it bind its own.
- **QUIC is optional.** Without its UDP socket (units from before QUIC, or a port taken), the node runs as before and says so in its log: its link state does not say it takes QUIC, so peers ask it over HTTPS ([Peer protocol](peer-protocol.md#transport)).
- **So is the rate feed.** Without its UDP socket (`dassie-feed.socket`, units from before the feed, or a port taken), the node runs, says so in its log, and publishes no feed: neither `GET /stats` nor its link state names a feed port ([Rate feed](rate-feed.md#the-socket)).
- **Either way,** URLs the node prints or redirects to use the ports it actually listens on, which under systemd need not be the configured ones.

## A chosen relay

A node can keep a QUIC connection open to a relay it chooses, so that clients reach it through that relay ([Peer protocol](peer-protocol.md#relayed-streams)), even without a public address or any peering ([the design](../designs/periphery.md#5-reaching-a-periphery-node)). The relay is configured as a bootstrap node is, by node ID, URL and key (`env.rs`), and the connection is pinned to that key; the two need not peer.

- **By default, the front door of the node's default name,** so that a node without a hostname is reached at `<label>.dassiepay.net` with no configuration ([default-relay](../decisions/default-relay.md)). With `DASSIE_RELAY` unset, the relay is that front door (`DEFAULT_RELAY` in `env.rs`, built in as the default bootstrap nodes are, and checked as any entry), and the node keeps it only while it has no hostname: a node with a hostname answers under it alone ([Payments](payments.md#the-nodes-names)), so it does not dial the front door, and closes the connection when the owner sets a hostname. It dials again if the hostname is cleared. An entry in `DASSIE_RELAY` is kept whether or not the node has a hostname. An empty `DASSIE_RELAY` is the opt-out: the node keeps no relay. Tests and the dev environment set it empty, as they set their own bootstrap nodes, so that none of their nodes dials the public front door.
- The node reports the relay it keeps now, and how it stands with it (connected, connecting or unreachable, below), in `GetStatus`: the default relay of a node with a hostname is none.
- The node dials the relay once it has its key and its QUIC endpoint serves, keeps the connection alive well within the idle timeout, so that a NAT keeps its mapping, and dials again when it is lost. The wait before a dial doubles, up to a minute, after each dial that fails and each connection that did not last a minute, so a relay that takes the connection and closes it at once is not dialled every second.
- **Connecting or unreachable.** While the node holds no connection, it counts as connecting to its relay until the wait before its next dial has reached its cap, and as unreachable from then on (`RelayLink` in `quic/relay.rs`). A relay that restarts, or a path that drops for a moment, is dialled again within seconds of coming back, so in those first dials a missing connection is no fault. Once the wait has reached its cap, every dial has failed for longer than the cap (the waits before it add up to that), longer than a restart or a brief drop lasts, and the node dials only once per cap; a relay that takes each connection and drops it soon after gets there too. The cap is the bound, so no constant of its own decides when a relay counts as a fault. A relay that is the node itself is unreachable from the start.
- The relay keeps the connection as it keeps any it is given, and reaches the node over it. Either node may hold other connections to the other beside it, when it is a peer or a bootstrap node, say: whichever is used, the kept connection stands by and takes over once the others close ([Peer protocol](peer-protocol.md#quic)), so the relay never loses the node while it stays connected.
- A node never dials itself: one whose `DASSIE_RELAY` names its own node ID keeps no relay connection and says so in its log. The check waits for the node's key, which may come after startup.
- A node without a QUIC endpoint cannot keep one.
- A periphery node may keep one beside its uplinks ([Network](network.md#periphery-nodes)), whether it finds them itself or its owner [pins them](network.md#pinned-uplinks), and the relay is not one of them: its connection carries relayed streams, and only a peering makes an uplink. One whose relay is also an uplink holds two connections to it, both kept alive alike, one of them standing by.
- A [front door](http.md#front-doors) is a relay a node chooses so that Open Payments clients reach it at its name under the front door's domain. The front door logs the entry that names it, its key and a URL for its QUIC endpoint (`resources/front-door/`); another front door than the default is named with that entry.

## Database

One SQLite file, `dassie.sqlite3`, in the data directory (`env.rs` has where). `store/`.

- **One node per data directory.** The store holds an exclusive lock on `dassie.lock` in the data directory for as long as it is open, and a second daemon on the same directory stops at startup, saying so. Two nodes on one database would each sign, settle and book as if the other did not exist; SQLite's own locks keep only their transactions apart, and two daemons on different ports never meet at a socket. The lock goes with the process, so a crash leaves none behind.

- **The application ID** claims a new file and refuses another application's. A file with the v0 node's ID is refused with a message to set the node up again from the passphrase, since the node ID and ledger accounts follow from it ([database-continuity](../decisions/database-continuity.md)).
- **Migrations** are numbered by `user_version`, each in its own transaction. A shipped migration is never edited; a change adds another. A database from a newer version is refused, which is why a rollback needs the copy below.
- **The copy before a migration.** Before migrating an existing database, the daemon writes a consistent copy (`VACUUM INTO`) to `dassie.sqlite3.pre-migration`. It does so before serving anything, so nothing the copy lacks has reached a peer, and a rollback of an update may put it back ([Release](release.md#rolling-back)).
- **No new database while a rollback is half way.** A rollback moves the database aside as `dassie.sqlite3.rolling-back`, with its side files, puts the copy in, and only then renames what it moved aside to `dassie.sqlite3.rolled-back`. So `dassie.sqlite3.rolling-back` without `dassie.sqlite3` means exactly that a rollback stopped between those steps, and the node refuses to start, rather than start afresh with another identity, until the owner puts the copy in place or removes what was moved aside. A finished rollback leaves nothing that stops a deliberate fresh start.
- **The schema check** runs at every open, after migrating: every table and column is exactly what the code expects (`store/schema.rs`), and a mismatch stops the daemon rather than letting it run on a schema it does not understand.
- **Configuration is typed columns with `CHECK` constraints in single-row tables,** not a key/value table, so SQLite itself refuses an invalid value.
- **Secret columns** (`SECRET_COLUMNS` in `store/schema.rs`) are never read by the wallet's database inspector.

### Persistence

What must survive a restart. Most of the original's integrity gaps came from state it kept only in memory, or rebuilt wrongly at start. Every durable change is written in the same transaction as the change it records ([The engine](#the-engine)), so a restart finds the books and the protocol state agreeing. Each area's document states its own rule; the ones whose loss would be unsafe:

- **The ledger's journal,** including pending outgoing settlements, which are reconciled with the scheme at start ([Ledger](ledger.md), [Settlement](settlement.md#outgoing-settlements)), and each scheme's checkpoint, written in the batch it explains ([Settlement](settlement.md#checkpoints-and-activation)).
- **The node table and the own link-state sequence,** which only goes up: a restarted node must not reuse a sequence number its peers have seen ([Network](network.md#persistence)).
- **Replay floors** of every sender whose replay state is kept for good (peers, former peers we hold a channel with, registered and bootstrap nodes), so a restart does not reopen a replay window ([Peer protocol](peer-protocol.md#replay-protection)), and **settlement channels,** kept after a peering ends ([Settlement](settlement.md#settlement-channels)).
- **Payment state with its STREAM credentials,** written with the journal rows that pay it, so a restart neither loses money received nor pays twice ([Payments](payments.md#persistence)).
- **The setup token,** so a restart keeps the setup URL valid, and **the self-signed certificate,** made once and kept ([HTTP](http.md#certificates)).
- **Paired devices,** by their device key's node ID, in a bounded list ([RPC](rpc.md#paired-devices)).
- **The settlement hubs the node validates:** each hub's consensus state and the state after its last final block, written in one transaction before the step's messages go out, so a validator never signs a vote that conflicts with one it sent; the states after a few recent final blocks and the last blocks of recent epochs, bounded, which validators far behind catch up from; how far it followed the hub's escrow, and the newest ledger it saw there, to tell a reset from a server behind; and the payout bundles it signed, the newest few ([Settlement hubs](hubs.md#the-validator-in-the-node)); what it keeps of each founding it took part in, by nonce, a sponsor's foundings with the payment that funds the escrow, so a restart never pays it twice, and the payout bundles a member kept of its hub ([Settlement hubs](hubs.md#founding-a-hub)).

Deliberately not kept: ILP packets in flight (their pending transfers were never on disk, so the books stand as if they were rejected), ACME challenge tokens (the daemon that serves them runs the order) and a pairing's secret (it works for minutes; the owner starts another).

### Secrets at rest

([secrets-at-rest](../decisions/secrets-at-rest.md))

- The data directory is 0700 and the database file 0600, created so before SQLite opens it; looser modes found at start are tightened. The copy before a migration is created by SQLite (`VACUUM INTO`) with the process's umask, which the systemd unit sets to 0077, and set to 0600 before it takes its name; until then the directory's 0700 keeps it from other users.
- Session, BTP and GNAP tokens are stored only as SHA-256 hashes.
- The node key, the TLS key, the ACME credentials, the setup token and STREAM shared secrets are stored as they are: the node must sign and settle unattended, and a key stored next to them would add nothing.

## Configuration

The owner's configuration is one row, read into a validated `NodeConfig` (`dassie-node-core/src/config.rs`), with one RPC setter per value. Nothing derived is stored: URLs are computed from the current values, so a new hostname takes effect at once. A hostname is refused unless `https://<hostname>:65535` is a URL peers take, at most 256 bytes: with a longer one, the node would run as a periphery node ([Network](network.md#periphery-nodes)). The realm `live` is refused until the conditions for [real money](../roadmap.md#real-money) hold ([real-money](../decisions/real-money.md)). Settings that belong to an area are in that area's tables.

The validator role of [settlement hubs](hubs.md#the-validator-in-the-node) is configured apart, by a file `DASSIE_VALIDATOR_CONFIG` names (`hub/config.rs`), read and checked at start like the environment: validating is something an owner turns on, for the sponsors it lists, and a node without the file validates nothing. The file names the sponsors, the XRPL server escrows are followed on, and hubs written in by hand, which may be none: a listed sponsor founds hubs with the node over the hub's founding stream ([Settlement hubs](hubs.md#founding-a-hub)).

## Dev switches warn on a public node

`DASSIE_PEER_ALLOW_PRIVATE` (peers on private and loopback addresses) and `DASSIE_DEV_TIMINGS` (the dev timers) are plain environment variables that make a node unsafe on the internet. If either is set while any listener, the QUIC and rate-feed sockets among them, is bound beyond loopback, the daemon warns at startup, naming the variable and what it does. The address checked is the one actually bound, and an unspecified address counts as beyond loopback. Dev and the tests listen on loopback, so they stay quiet.

## Logging

Events go through `tracing` (`log.rs`). Core crates do not log; the node maps their events ([engineering](../engineering.md)). Besides standard output, the node keeps a bounded ring of its last lines for the wallet's logs page, whatever the log level says, with fields named like secrets redacted.

Which transport the peer protocol uses can be read from the journal at the default level, under either runtime, without watching the network: the line that says the node listens names its QUIC endpoint and its rate feed's socket, or says it has none, and each peer's transport ([Peer protocol](peer-protocol.md#transport)) is logged when its first answer in a peering, or since the engine started, arrives and whenever it changes. Never once per request, so the volume stays that of the peerings.

## The `dassie` command

The CLI talks to the daemon over the IPC socket with the same RPC contract as the wallet ([rpc-contract](../decisions/rpc-contract.md)); access to the socket is the owner's ([RPC](rpc.md#ipc-socket)).

- **`dassie init`** is the original's wizard. The daemon runs the ACME order and the wizard shows its progress, so there is one ACME client, not two. Every prompt has a flag, so scripts and tests can run it unattended.
- **`dassie status`** names no secret, so its output can be pasted where the owner asks for help; that is why the setup URL has a command of its own, `dassie setup-url`. It also reports the last update ([Release](release.md#dassie-update)).
  - It gives a hint only where something is wrong, so the normal state of a node never reads as a fault. A periphery node ([Network](network.md#periphery-nodes)) registers nowhere and needs no domain name, so status does not report its membership or send its owner to `dassie init`. It names the node's uplinks and whether each is connected, its [chosen relay](#a-chosen-relay) and whether the node holds its connection to it, is connecting or finds it unreachable, and the names the node answers Open Payments to ([Payments](payments.md#the-nodes-names)) as its addresses. A periphery node without an uplink, or whose relay is unreachable, is out of reach, and those get a hint; a relay the node is still connecting to, right after a start or a brief drop, gets none.
  - A node that is not a periphery node reports how many bootstrap nodes list it, and a relay if it chose one, and its rate feed: where it is served and to how many, with its counters, or that it is not served yet or has no socket ([Rate feed](rate-feed.md#what-the-owner-sees)).
  - A node that validates settlement hubs reports each: which validator it is, the last final block and view, with a hint only where something is wrong: no block final for a while, a stall it declared, an escrow no quorum checked, or an XRPL server it cannot reach ([Settlement hubs](hubs.md#the-validator-in-the-node)).
- **`dassie setup`** sets the node up from its owner's passphrase, in a file on the node's host, for a node nobody sets up in a browser, such as a market maker's ([headless-setup](../decisions/headless-setup.md)). It is the wallet's setup without the browser: it derives the node key and the login proof, and posts them with the setup token from the IPC socket to the node's own setup route on loopback ([HTTP](http.md#setup)), so the node checks it as it checks the wallet, and only the key reaches the node. Then it waits, bounded, for the engine, and adds each settlement scheme it was given that is not active.
  - It is idempotent, so a unit can run it at every start: a node set up with the same phrase is left as it is, and one set up with another phrase is refused with an exit code of its own, with nothing changed, its schemes included.
  - The phrase is read once from a bounded file and wiped once the key is derived; no line it prints names it.
- **`dassie hub`** shows the node's account in its settlement hub, and moves the owner's funds between it and the node's XRPL testnet account, lets the node move them by itself, or gives up a late move ([Settlement hubs](hubs.md#in-the-node)). `dassie status` gives the hub a line while a hub scheme runs.
- **`dassie hub found <node ID>@<host:port>...`** founds a hub with these validators, on the starting terms or those a file of JSON sets (`--terms`), funding its escrow from the node's XRPL testnet account (`--escrow-funding`); `dassie hub foundings` shows how far each founding is and, once fixed, the hub's rules; `dassie hub drop` drops one; `dassie status` gives each a line ([Settlement hubs](hubs.md#founding-a-hub)). `dassie hub payout` shows the payout bundles kept of the node's hub, and the transactions to submit once a quorum's sign one set ([Settlement hubs](hubs.md#payout-bundles)).
- **`dassie pair`** prints a code that pairs an owner app, as the wallet does on `localhost` ([RPC](rpc.md#paired-devices)): the IPC socket is the owner on this machine, so a node whose wallet is reached only remotely still pairs apps.
- **`dassie verify-install`** checks that this binary works on this host (SQLite, TLS, the embedded wallet, a version the updater can parse) and names the release key it trusts. The installer and the updater run it before switching to a binary.
- **`dassie update`** is [Release](release.md#dassie-update)'s.
- **`dassie market-maker`** is no node, nor a part of one: it is a client of two nodes of its owner's, over their IPC sockets, as the CLI is of one, and of a core node's public feed ([Rate feed](rate-feed.md#market-maker)). It ships in the binary because it speaks the node's RPC and reads its feed with the node's own code, so the two never disagree on a version. Its config names the nodes' passphrase files, which only it and `dassie setup` read: the keys of their XRPL accounts, which it signs swaps with, stay in its memory.

## Decisions

- [database-continuity](../decisions/database-continuity.md): a new schema, v0 databases refused, and what a v0 owner loses by setting up again.
- [rpc-contract](../decisions/rpc-contract.md): the CLI talks to the daemon over the same contract as the wallet.
- [real-money](../decisions/real-money.md): why the `live` realm is refused.
- [secrets-at-rest](../decisions/secrets-at-rest.md): file modes, hashed tokens, and why the node key stays plaintext.
- [bootstrap-network](../decisions/bootstrap-network.md): the default bootstrap nodes, and `DASSIE_BOOTSTRAP_NODES`.
- [tigerstyle](../decisions/tigerstyle.md): injected time and randomness, decisions in `dassie-node-core`, and checking state before each commit.
- [own-fulfills](../decisions/own-fulfills.md): why an upgrade lifts the bans made for a grace limit and makes owed Fulfills final.
- [random-seed-simulations](../decisions/random-seed-simulations.md): why a failed commit is the only fault injected at the store.
- [ready-after-first-step](../decisions/ready-after-first-step.md): readiness once the engine has committed a step, and a daemon that exits when its engine cannot start.
- [default-relay](../decisions/default-relay.md): the default relay, kept only without a hostname, and `DASSIE_RELAY=` to keep none.
- [headless-setup](../decisions/headless-setup.md): `dassie setup` from a passphrase file on the node's host, through the node's own setup route.
