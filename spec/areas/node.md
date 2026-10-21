# Node

The `dassie` binary: the daemon that composes every protocol crate into a running node, and the command-line tools that configure, inspect and update it.

**Paths:** `crates/dassie-node/src/` (`daemon.rs`, `supervisor.rs`, `engine/`, `store/`, `env.rs`, `systemd.rs`, `ipc.rs`, `log.rs`, `clock.rs`, `node/`), `crates/dassie-node-core/src/`, `crates/dassie-cli/src/`
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
- If the write fails, the core panics without handing anything over: nothing of the step goes out, so nothing the node sends, settles or acknowledges rests on state that never reached the disk. The supervisor restarts the task, which rebuilds the core from the database, at its last good commit: packets in flight do not survive, clients reconnect, and a peer request the engine was answering gets a 503. A failed commit is the only fault the simulations inject at the store ([Testing](testing.md#failed-commits), [D27](../decisions/D27-simulations-run-continuously-from-random-seeds.md)).
- The engine starts once the node has an identity, since its ILP address comes from the node ID.

### Checking state before each commit

In tests and simulations a commit never holds a broken invariant, so what stops the core leaves the state that last held for its successor. Release builds run only the cheap assertions on every commit, and the full sweep at most once a second, so there a broken invariant that only the sweep catches can reach the disk before the next sweep stops the core.

- **Cheap assertions** run at every change.
- **The full sweep** of every state machine's invariants runs before every commit when debug assertions are on (tests, simulations, the end-to-end binary). In release builds it is throttled (`SWEEP_EVERY_MS` in `engine/core/mod.rs`), since with full tables it costs about as much as a busy commit. The throttle counts engine time, not wall time, so a run stays the same for the same inputs.
- **The store commits with `synchronous = FULL`,** so a commit that returned is on disk, which the Fulfill rule above relies on.

## Startup

`daemon.rs`. The rules that the order serves:

- **Everything is checked before anything starts.** Every environment variable is read once, from a snapshot (`env.rs`), and a bad value stops the daemon with a message that names it. A daemon that cannot start exits 1 rather than running half-started, and leaves no IPC socket behind.
- **The database opens, and migrates, before anything is served,** so the copy taken before a migration holds everything a peer has seen ([Database](#database)).
- **systemd's sockets are taken before the async runtime exists,** while the process has one thread.
- **The listeners come up first,** before anything that waits for the network, so ACME's HTTP challenge works at the first start.
- **Nothing blocks on the network.** Exchange rates, the settlement schemes' ledger connections and the bootstrap polls are readiness conditions with retries, not steps startup waits for. Services that need an identity wait for one, which may be long after startup.
- **The IPC socket appears only once the daemon answers.** The standalone daemon binds it in a private directory and renames it into place, so it is never reachable with looser permissions, before the daemon answers, or after one that failed ([RPC](rpc.md#ipc-socket)).
- **The setup URL is never logged:** whoever opens it first claims the node, and the journal is readable by groups that do not administer it. The daemon logs that `sudo dassie setup-url` prints it.

### Readiness

The daemon reports readiness (`READY=1` under systemd) once the node works, not once it listens, since an update rolls back only a node that does not come up ([Release](release.md#rolling-back), [D30](../decisions/D30-a-node-is-up-once-its-engine-has-committed-a-step.md)).

- **Ready once the engine has committed its first step,** with the listeners bound and the services started. The engine takes that step as soon as it has rebuilt itself from the database, without waiting for a command, a timer or the network, so a node with nothing to do is ready at once.
- **A node without an identity is ready at once.** It has no engine to wait for: the engine starts once the owner sets the node up, and its failures from then on are restarted like any other service's.
- **An engine that fails a few times before its first step ends the daemon** with an error (`START_ATTEMPTS` in `supervisor.rs`), instead of being restarted for ever: the process exits 1 without having reported readiness, so systemd fails the start and `dassie update` rolls back. Once the node is up, the engine is restarted as any service is.
- Readiness proves the engine starts on this database, not that it keeps running: a failure after its first step, such as a panic on some packet, is restarted and not rolled back.

## Shutdown

On SIGTERM or SIGINT the daemon stops accepting, gives requests in progress a bounded time to finish, closes the database and removes the socket it made. The engine stops at once, between two steps: what it committed is on disk, and packets in flight are lost, as in a crash ([Persistence](#persistence)). Draining them is [not built](../roadmap.md#not-built-yet).

## Runtimes

`--runtime standalone|systemd` chooses how the daemon gets its sockets; without it, `systemd` when `LISTEN_PID` is the daemon's own PID.

- **standalone** binds the configured ports (or `--http-port`/`--https-port` for one run), a UDP socket for QUIC on the HTTPS listener's address and port, and an IPC socket path of its own.
- **systemd** takes its sockets by name from the socket units ([Release](release.md#systemd-units)), and reports readiness to `NOTIFY_SOCKET` itself. Sockets that are not this process's, or a missing name of the three it needs, stop the daemon rather than letting it bind its own.
- **QUIC is optional.** Without its UDP socket (units from before QUIC, or a port taken), the node runs as before and says so in its log: its link state does not say it takes QUIC, so peers ask it over HTTPS ([Peer protocol](peer-protocol.md#transport)).
- **Either way,** URLs the node prints or redirects to use the ports it actually listens on, which under systemd need not be the configured ones.

## A chosen relay

A node can keep a QUIC connection open to a relay it chooses, so that clients reach it through that relay ([Peer protocol](peer-protocol.md#relayed-streams)), even without a public address or any peering ([the design](../designs/periphery.md#5-reaching-a-periphery-node)). The relay is configured as a bootstrap node is, by node ID, URL and key (`env.rs`), and the connection is pinned to that key; the two need not peer.

- The node dials the relay once it has its key and its QUIC endpoint serves, keeps the connection alive well within the idle timeout, so that a NAT keeps its mapping, and dials again when it is lost. The wait before a dial doubles, up to a minute, after each dial that fails and each connection that did not last a minute, so a relay that takes the connection and closes it at once is not dialled every second.
- The relay keeps the connection as it keeps any it is given, and reaches the node over it. Either node may hold other connections to the other beside it, when it is a peer or a bootstrap node, say: whichever is used, the kept connection stands by and takes over once the others close ([Peer protocol](peer-protocol.md#quic)), so the relay never loses the node while it stays connected.
- A node never dials itself: one whose `DASSIE_RELAY` names its own node ID keeps no relay connection and says so in its log. The check waits for the node's key, which may come after startup.
- A node without a QUIC endpoint cannot keep one.
- A periphery node may keep one beside its uplinks ([Network](network.md#periphery-nodes)), and the relay is not one of them: its connection carries relayed streams, and only a peering makes an uplink. One whose relay is also an uplink holds two connections to it, both kept alive alike, one of them standing by.
- A [front door](http.md#front-doors) is a relay a node chooses so that Open Payments clients reach it at its name under the front door's domain. The front door logs the entry that names it, its key and a URL for its QUIC endpoint (`resources/front-door/`).

## Database

One SQLite file, `dassie.sqlite3`, in the data directory (`env.rs` has where). `store/`.

- **The application ID** claims a new file and refuses another application's. A file with the v0 node's ID is refused with a message to set the node up again from the passphrase, since the node ID and ledger accounts follow from it ([D3](../decisions/D03-database-continuity.md)).
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
- **Per-peer replay floors,** so a restart does not reopen a replay window ([Peer protocol](peer-protocol.md#replay-protection)), and **settlement channels,** kept after a peering ends ([Settlement](settlement.md#settlement-channels)).
- **Payment state with its STREAM credentials,** written with the journal rows that pay it, so a restart neither loses money received nor pays twice ([Payments](payments.md#persistence)).
- **The setup token,** so a restart keeps the setup URL valid, and **the self-signed certificate,** made once and kept ([HTTP](http.md#certificates)).

Deliberately not kept: ILP packets in flight (their pending transfers were never on disk, so the books stand as if they were rejected) and ACME challenge tokens (the daemon that serves them runs the order).

### Secrets at rest

([D10](../decisions/D10-secrets-at-rest.md))

- The data directory is 0700 and the database file 0600, created so before SQLite opens it; looser modes found at start are tightened. The copy before a migration is created by SQLite (`VACUUM INTO`) with the process's umask, which the systemd unit sets to 0077, and set to 0600 before it takes its name; until then the directory's 0700 keeps it from other users.
- Session, BTP and GNAP tokens are stored only as SHA-256 hashes.
- The node key, the TLS key, the ACME credentials, the setup token and STREAM shared secrets are stored as they are: the node must sign and settle unattended, and a key stored next to them would add nothing.

## Configuration

The owner's configuration is one row, read into a validated `NodeConfig` (`dassie-node-core/src/config.rs`), with one RPC setter per value. Nothing derived is stored: URLs are computed from the current values, so a new hostname takes effect at once. A hostname is refused unless `https://<hostname>:65535` is a URL peers take, at most 256 bytes: with a longer one, the node would run as a periphery node ([Network](network.md#periphery-nodes)). The realm `live` is refused until the conditions for [real money](../roadmap.md#real-money) hold ([D8](../decisions/D08-real-money.md)). Settings that belong to an area are in that area's tables.

## Dev switches warn on a public node

`DASSIE_PEER_ALLOW_PRIVATE` (peers on private and loopback addresses) and `DASSIE_DEV_TIMINGS` (the dev timers) are plain environment variables that make a node unsafe on the internet. If either is set while any listener is bound beyond loopback, the daemon warns at startup, naming the variable and what it does. The address checked is the one actually bound, and an unspecified address counts as beyond loopback. Dev and the tests listen on loopback, so they stay quiet.

## Logging

Events go through `tracing` (`log.rs`). Core crates do not log; the node maps their events ([engineering](../engineering.md)). Besides standard output, the node keeps a bounded ring of its last lines for the wallet's logs page, whatever the log level says, with fields named like secrets redacted.

## The `dassie` command

The CLI talks to the daemon over the IPC socket with the same RPC contract as the wallet ([D5](../decisions/D05-ui-and-cli-rpc-contract.md)); access to the socket is the owner's ([RPC](rpc.md#ipc-socket)).

- **`dassie init`** is the original's wizard. The daemon runs the ACME order and the wizard shows its progress, so there is one ACME client, not two. Every prompt has a flag, so scripts and tests can run it unattended.
- **`dassie status`** names no secret, so its output can be pasted where the owner asks for help; that is why the setup URL has a command of its own, `dassie setup-url`. It also reports the last update ([Release](release.md#dassie-update)).
  - It gives a hint only where something is wrong, so the normal state of a node never reads as a fault. A periphery node ([Network](network.md#periphery-nodes)) registers nowhere and needs no domain name, so status does not report its membership or send its owner to `dassie init`. It names the node's uplinks and whether each is connected, its [chosen relay](#a-chosen-relay) and whether the node holds its connection to it, and the names the node answers Open Payments to ([Payments](payments.md#the-nodes-names)) as its addresses. A periphery node without an uplink, or with a relay it cannot connect to, is unreachable, and those get a hint.
  - A node that is not a periphery node reports how many bootstrap nodes list it, and a relay if it chose one.
- **`dassie verify-install`** checks that this binary works on this host (SQLite, TLS, the embedded wallet, a version the updater can parse) and names the release key it trusts. The installer and the updater run it before switching to a binary.
- **`dassie update`** is [Release](release.md#dassie-update)'s.

## Decisions

- [D3](../decisions/D03-database-continuity.md): a new schema, v0 databases refused, and what a v0 owner loses by setting up again.
- [D5](../decisions/D05-ui-and-cli-rpc-contract.md): the CLI talks to the daemon over the same contract as the wallet.
- [D8](../decisions/D08-real-money.md): why the `live` realm is refused.
- [D10](../decisions/D10-secrets-at-rest.md): file modes, hashed tokens, and why the node key stays plaintext.
- [D15](../decisions/D15-bootstrap-network-for-v1.md): the default bootstrap nodes, and `DASSIE_BOOTSTRAP_NODES`.
- [D16](../decisions/D16-engineering-style-pragmatic-tigerstyle-and-deterministic-simulation.md): injected time and randomness, decisions in `dassie-node-core`, and checking state before each commit.
- [D23](../decisions/D23-each-node-books-its-own-fulfills.md): why an upgrade lifts the bans made for a grace limit and makes owed Fulfills final.
- [D27](../decisions/D27-simulations-run-continuously-from-random-seeds.md): why a failed commit is the only fault injected at the store.
- [D30](../decisions/D30-a-node-is-up-once-its-engine-has-committed-a-step.md): readiness once the engine has committed a step, and a daemon that exits when its engine cannot start.
