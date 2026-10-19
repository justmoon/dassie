# Node

The `dassie` binary: the daemon that composes every protocol crate into a running node, and the command-line tools that configure, inspect and update it. This document covers the process, its services and their start and stop, the two runtimes, the database and what it keeps, configuration, environment variables, logging and the CLI verbs.

**Paths:** `crates/dassie-node/src/` (`daemon.rs`, `supervisor.rs`, `systemd.rs`, `env.rs`, `log.rs`, `ipc.rs`, `clock.rs`, `store/`, `engine/mod.rs`, `node/`), `crates/dassie-node-core/src/config.rs`, `crates/dassie-cli/src/`
**Read with:** [Release](release.md), [HTTP](http.md), [RPC](rpc.md), [Ledger](ledger.md), [Connector](connector.md), [Network](network.md), [Identity](identity.md)

## Process

- **One static binary,** `dassie`, built from `dassie-cli`. It holds the daemon, the CLI and every protocol, with the wallet's built assets embedded. There is no launcher script, no bundled interpreter and no native add-on.
- **Build inputs** (read by build scripts, not at run time):
  - `DASSIE_WALLET_DIST`: the built wallet to embed. Only this variable is read, never a default path. Without it the binary embeds a page saying it has no wallet, so `cargo build` and `cargo test` need no Node.js, and `verify-install` fails.
  - `DASSIE_VERSION`: the version the binary names (a release or a canary, [Release](release.md#versions-and-channels)). Without it, the crate's version.
  - `DASSIE_RELEASE_KEY`: the release key the updater trusts ([Release](release.md#the-release-key)).
- **What replaces the original's libraries:** a tokio task tree for `lib-reactive` ([Runtime model](#runtime-model)); `rusqlite` (SQLite bundled) with STRICT tables and `user_version` migrations for `lib-sqlite`, keeping its application-ID and schema-check ideas ([Database](#database)); `tracing` with the same `das:node:*` targets and `DEBUG` patterns for `lib-logger` ([Logging](#logging)); `rcgen` for `lib-x509`'s self-signed certificate ([HTTP](http.md)); `cliclack` prompts for `lib-terminal-graphics`, with the same prompts and notes in `dassie init`; `lexopt` for the command line.
- **The install layout, the systemd units and socket names, the CLI verbs and the environment variable names are the original's** where they still mean something, for operators' habits and docs. The original's variables that mean nothing to this node are not read: `NODE_ENV`, `DEBUG_HIDE_DATE`, `DASSIE_ROOT`, the cache and temporary directories (`DASSIE_CACHE_DIRECTORY`, `CACHE_DIRECTORY`, `DASSIE_TEMPORARY_DIRECTORY`) and `DASSIE_DEV_SECURITY_TOKEN`. Nothing in the node accepts a dev token.

## Runtime model

The original was a tree of reactive actors that re-ran when signals they read changed. The node keeps that model's observable behaviour without its framework.

- **Services are supervised tokio tasks.** Each owns a `CancellationToken` and its cleanup. A service that returns or panics before shutdown is restarted (RT-24) after a backoff that doubles from 1 s to 60 s, and the failure is logged at error level. A service that ran 5 minutes before failing starts the backoff over.
  - The supervisor keeps each service's state for the wallet's service inspector: running, restarting or stopped; when it last started; how often it was restarted; the last failure (at most 256 bytes) and when; and, while restarting, when it starts again.
  - The services are `http`, `https`, `ipc`, `acme` (certificate renewal, [HTTP](http.md)), `sessions` (deletes expired sessions every hour), `engine` and `rates` ([Exchange rates](connector.md#exchange-rates)).
- **The engine owns the node's state:** the node table, peers, routes, registrations, the ledgers, the journal and the Open Payments resources. It counts how often each kind changed on disk (`Versions`: ledger, network, payments), in a `tokio::sync::watch` cell; live views fetch again when the count they follow moves ([RPC](rpc.md)). The config is a `watch` cell too: the engine waits on `changed()` and reconfigures itself, where the original restarted actors on a signal change.
- **Streams of events** go out on broadcast channels: the packets the engine traces, and session revocations. The log ring keeps the number of its newest line in a `watch` cell, which the logs stream follows.
- **Time and randomness are injected.** `clock.rs` is the only place the node reads the wall clock or the OS RNG; everything else gets a `Clock` and an RNG from it, so tests and simulations substitute their own. A clock before 1970 or past the representable range reads as the edge rather than panicking. A failing OS RNG panics.
- **The node's decisions live in `dassie-node-core`:** setup and login checks, session lifetimes, rate limits, the same-origin rule, configuration values and when to renew a certificate are pure functions of their inputs, `now` and an injected RNG ([engineering](../engineering.md)). `dassie-node` only carries them out.
- **Limits and timings** come from one `Policies` value: production defaults, dev settings ([Dev switches](#dev-switches-warn-on-a-public-node)), and shorter ones in tests.

### The engine

One task owns the money path: the ledger, the connector, the STREAM receivers and payers, and the settlement schemes. None of them needs a lock, and their steps happen in one order, as in a simulation.

- Everything else talks to it through a queue of at most 1024 commands. It takes up to 256 at a time, steps the core with each, persists what the batch changed in one transaction, and only then sends the batch's packets and replies. So a Fulfill goes upstream only once its transfer is on disk ([Connector](connector.md), [Ledger](ledger.md)).
- If the write fails, the core panics without handing anything over. The supervisor restarts the task, which rebuilds the core from the database: packets in flight do not survive, and their clients reconnect.
- The engine starts once the node has an identity: its ILP address comes from the node ID. Live streams it feeds end with `UNAVAILABLE` when it restarts ([RPC](rpc.md)).
- Each start counts a boot epoch (`boot.epoch`), which with a sequence number names each packet's transfers in the journal.

### Checking state before each commit

The node checks its state before each commit, so a broken invariant never reaches the disk: what stops the core leaves the state that last held for its successor.

- **Cheap assertions** run at every change.
- **The full sweep** of every state machine's invariants (ledger, connector, peer protocol, schemes, receivers, payers) runs before every commit where debug assertions are on: in tests, simulations and the end-to-end binary. In release builds it runs at most once a second of engine time, since with full tables it costs about as much as a busy commit. Engine time, not wall time, so a run stays the same for the same inputs.
- **The store commits with `synchronous = FULL`,** so a commit that returned is on disk; a torn write is SQLite's to handle. A commit that fails stops the core, which restarts from its last good commit.

## Startup

`dassie daemon` starts in this order. Steps 1 to 5 run before the async runtime exists, since taking systemd's sockets must happen while the process has one thread.

1. **Check the environment.** Every variable is read and checked before anything starts ([Environment variables](#environment-variables)). A bad value stops the daemon with a message that names the variable.
2. **Set up logging** ([Logging](#logging)).
3. **Choose the runtime** ([Runtimes](#runtimes)).
4. **Open the database:** secure the data directory and file, keep a copy if a migration is due, migrate, and check the schema ([Database](#database)).
5. **Take or bind the sockets.**
6. **Install the signal handlers** for SIGTERM and SIGINT.
7. **Make the node:** load the certificate, or make the self-signed one ([HTTP](http.md)), and build the TLS configuration. Warn if `DASSIE_DEV_WALLET_DIR` is set, for each [dev switch](#dev-switches-warn-on-a-public-node) set on a public listener, and if there are no bootstrap nodes ("the node runs alone").
8. **Start the services** under the supervisor.
9. **Publish the IPC socket** (standalone runtime): only now do clients find it, when the daemon answers them and handles signals.
10. **Report readiness.** Log `listening` with the version, runtime and the ports actually bound. If the node is not set up yet, warn that `sudo dassie setup-url` prints the setup URL. The URL itself is never logged: whoever opens it first claims the node, and the journal is readable by groups that do not administer it. Under systemd, send `READY=1`.

Rules:

- **The listeners come up first,** before anything that waits for the network: ACME's HTTP challenge works at the first start (RT-8).
- **Identity-dependent services start when an identity exists,** which may be long after startup: the engine waits for it.
- **Nothing blocks on the network.** The exchange-rate fetch, the ledger connections of settlement schemes and the bootstrap polls are readiness conditions with retries, not steps startup waits for (ILP-14, ACC-26, NET-14).
- **A daemon that cannot start exits with code 1** instead of running half-started: an unusable environment, a database it cannot open or check, sockets it cannot take or bind, a node it cannot make, TLS it cannot set up, or signals it cannot handle. It leaves no IPC socket behind.

## Shutdown

On SIGTERM or SIGINT the daemon stops accepting, lets requests in progress finish, closes the database and removes the socket it made (RT-7). It does not drain packets in flight.

1. Log `shutting down`, and under systemd send `STOPPING=1`.
2. Cancel every service. Listeners stop accepting; HTTP requests in progress get at most 10 s to finish, and WebSockets are closed with code 1001 ([HTTP](http.md#listeners)). IPC connections get the same 10 s.
3. The engine stops at once, between two steps: what it committed is on disk, and packets in flight are lost, as in a crash ([Persistence](#persistence)).
4. Wait at most 15 s for the services in all; past that, warn and go on.
5. Drop the node, which closes the database.
6. Remove the IPC socket the daemon bound (standalone runtime), and log `stopped`. Exit 0.

## Runtimes

`dassie daemon --runtime standalone|systemd` chooses how the daemon gets its sockets; the names replace the original's `nodejs` and `nodejs-systemd`, since the binary does not run on Node.js. Without the flag it uses `systemd` when `LISTEN_PID` is its own PID and `LISTEN_FDS` is set, and `standalone` otherwise.

| | `standalone` | `systemd` |
|---|---|---|
| HTTP and HTTPS | Binds the configured ports, or `--http-port` and `--https-port` for this run | The sockets named `dassie-http.socket` and `dassie-https.socket` |
| Address | `--listen <ip>`, or every address: `::` where the system has IPv6, else `0.0.0.0` | Whatever the socket units say |
| IPC | Binds `DASSIE_IPC_SOCKET_PATH`, else `$XDG_RUNTIME_DIR/dassie.sock`, else `/run/dassie.sock` | The socket named `dassie-ipc.socket` |
| Readiness | none | `READY=1` and `STOPPING=1` to `NOTIFY_SOCKET` |

- **Either way,** URLs the node prints or redirects to use the ports it actually listens on. Under systemd the configured ports are not used.
- **`--http-port` and `--https-port`** go together, each 1 to 65535, and must differ. They are for running unprivileged and for the dev environment's many nodes.
- **`--listen`** keeps the dev environment's nodes, whose keys are public, on loopback.

### Standalone IPC socket

- If a daemon already answers on the path, this one refuses to start. A stale file is replaced.
- The socket is bound at a temporary name inside a new directory `<path>.<pid>.d` of mode 0700, set to mode 0660, and renamed into place once the daemon is ready (startup step 9). So it is never reachable with looser permissions, nor before the daemon answers, nor left behind by a daemon that failed.

### Socket activation

- `LISTEN_PID` must be this process's PID; otherwise the sockets are not ours and the daemon refuses to start (RT-20).
- `LISTEN_FDS` must be a number, and `LISTEN_FDNAMES` must name exactly that many sockets, separated by `:`. File descriptors start at 3, the *i*-th named by the *i*-th name.
- All three names must be present. A socket of another name is ignored with a warning.
- Readiness goes straight to `NOTIFY_SOCKET` as a datagram, without `systemd-notify`. The variable must be a path starting with `/` or an abstract name starting with `@`; anything else stops the daemon at start.

## Database

One SQLite file, `dassie.sqlite3`, in the data directory.

- **The data directory** is `DASSIE_STATE_DIRECTORY`, else the first entry of systemd's `STATE_DIRECTORY` (a `:`-separated list), else `$XDG_DATA_HOME/dassie`, else `$HOME/.local/share/dassie`. With none of them set the daemon stops at start. Under systemd it is `/var/lib/dassie`.
- **Modes** ([D10](../decisions/D10-secrets-at-rest.md)): the directory is created 0700 and the file 0600 before SQLite opens it, so the file is never readable by others. Modes found looser at start are tightened, with a warning.
- **Connection:** WAL journal, `synchronous = FULL`, a busy timeout of 5 s, foreign keys on. One connection, used synchronously behind a mutex: a statement takes microseconds, and the engine commits a whole batch at once.
- **Application ID** `0x7DA5E2C9` (like the original's, a random positive int32 with nibbles 2–4 set to `DA5`).
  - A new, empty file (application ID 0 and no schema yet) is claimed with it. A file with application ID 0 that already holds tables is another application's.
  - A file with the v0 ID `0x1DA53B81` is refused with a message telling the owner to move it away and set the node up again with their passphrase: the node ID and ledger accounts follow from it ([D3](../decisions/D03-database-continuity.md)). There is no import of v0 configuration or tokens.
  - A file with any other ID is refused as another application's.
- **Migrations** are numbered by `user_version`, each run in its own transaction that also sets `user_version`. A migration that has shipped is never edited; a change adds another. A file whose `user_version` is above what this version knows is refused ("from a newer version of Dassie").
- **The copy before a migration.** When an existing database (schema version above 0 and below this version's) is about to be migrated, the daemon first writes a consistent copy with `VACUUM INTO` to `dassie.sqlite3.pre-migration` beside it: mode 0600, synced, replacing any earlier copy. This happens before it serves anything, so nothing the copy lacks has reached a peer. A rollback of an update puts it back ([Release](release.md#rolling-back)).
- **Upgrades that change what is kept.** Some migrations carry an older version's state over to rules that changed:
  - Bans that older versions made on their own, for a grace limit, are lifted, with every ban's record; the owner's bans stay ([Network](network.md#bans)). The owner's grace share goes.
  - Fulfills that an older version still owed a verdict on are final ([Ledger](ledger.md#fulfills-booked-as-final)): what the node's receiver took for them counts as received by their incoming payments and STREAM connections, completing a payment they fill. The tables of verdicts owed and taken go.
  - The credit a peer extended to us, which older versions kept per peering, goes: a node keeps only the credit it extends ([Settlement](settlement.md#credit)).
  - The `refusal` rows that nodes up to 0.1.3 booked stay in the journal ([Ledger](ledger.md#fulfills-booked-as-final)).
- **The schema check** runs at every open, after migrating: every table exists, is `STRICT`, has the expected `WITHOUT ROWID` setting, and has exactly the expected columns in order (name, type, `NOT NULL`, primary key). An extra column is an error too. A mismatch stops the daemon.
- **Configuration is typed columns with `CHECK` constraints in single-row tables** (`id = 1`), not a key/value table, so that SQLite itself refuses an invalid port or realm.
- **Secret columns** are never read by the database inspector of the wallet's debug pages, which shows them as redacted: `node_key.private_key`, `tls_certificate.key_pem`, `acme_account.credentials`, `sessions.id`, `btp_tokens.token_hash`, `incoming_payments.shared_secret`, `gnap_grants.continue_token_hash`, `gnap_grants.interaction`, `gnap_tokens.token_hash`, `quotes.shared_secret`, `outgoing_payments.shared_secret`, `setup_token.token`.

### Persistence

What must survive a restart. Most of the original's integrity gaps came from state it kept only in memory, or rebuilt wrongly at start (ACC-5, ACC-6, ACC-16, ACC-18, NET-1, NET-2). Each row below is written in the same transaction as the change it records; the linked document states the rule.

| Data | Tables | What is kept | Rule in |
|---|---|---|---|
| Configuration | `node_config` | Realm, hostname, HTTP and HTTPS ports, alias, ACME directory; every value has an RPC setter (RT-4) | [Configuration](#configuration) |
| Node key | `node_key` | The raw 32-byte Ed25519 seed and when it was set; written once | [Identity](identity.md), [Secrets at rest](#secrets-at-rest) |
| Setup token | `setup_token` | The 32-byte token until setup completes, so a restart keeps the setup URL valid | [HTTP](http.md#setup) |
| TLS certificate | `tls_certificate` | Source (self-signed, ACME or manual), chain and key in PEM, when installed. The self-signed one is made once and kept (RT-14) | [HTTP](http.md#certificates) |
| ACME account | `acme_account` | Credentials and contact per directory URL | [HTTP](http.md#acme) |
| ACME challenge tokens | none | Memory only: the daemon that serves them also runs the order | [HTTP](http.md#acme) |
| Sessions | `sessions` | SHA-256 of the token, created, last used, user agent. At most 64; expiry and eviction as HTTP says (RT-11) | [HTTP](http.md#sessions) |
| BTP tokens | `btp_tokens` | SHA-256 of the token (shown once at creation), label, ledger, created and revoked times; the spending cap and its usage, written with the journal rows that use it | [Connector](connector.md#btp-tokens), [Spending caps](connector.md#spending-caps) |
| Node table | `peer_nodes` | Each node's verified link state (signed bytes), why it is in the table, when first seen, its registration with us; written on every change (NET-1, NET-2) | [Network](network.md#persistence) |
| Peerings | `peers`, `peer_terminations` | Scheme, version and the scheme's state for the peer, the credit we extend, since when, inbound or not, last heard from, how long its room has been short (`short_for_ms`); peerings being ended | [Network](network.md#persistence), [Settlement](settlement.md#credit) |
| Replay floors | `peers.replay_floor_ms` | Per peer, the newest accepted message timestamp, written with the batch it took effect in | [Peer protocol](peer-protocol.md#replay-protection) |
| Own link-state sequence | `own_link_state` | The high-water mark, which only goes up (NET-7) | [Network](network.md#the-own-link-state) |
| Settlement channels | `settlement_channels` | Per node ever peered with: highest settlement sequence applied and sent, the message not yet acknowledged, a final settlement due; kept after the peering ends | [Settlement](settlement.md#settlement-channels) |
| Bans | `peer_bans` | Node IDs the owner banned | [Network](network.md#bans) |
| Peer settings | `peer_settings` | The owner's connector fee and FX spread | [Connector](connector.md#fees-and-spread) |
| Ledger | `ledger_accounts`, `ledger_transfers`, `boot` | Accounts with posted totals cached in the same transaction; the append-only journal of transfers that reach posted balances, plus pending settlements; the boot epoch | [Ledger](ledger.md#the-journal) |
| Pending ILP transfers | none | Memory only: packets in flight do not survive a restart | [Ledger](ledger.md#persistence), [Connector](connector.md#responses-and-expiry) |
| Pending outgoing settlements | `ledger_transfers` | Pending journal rows with the scheme's reference, reconciled at start (ACC-16) | [Settlement](settlement.md#outgoing-settlements) |
| Ledger fees, trust lines, deposits, withdrawals, reconciliations | `ledger_transfers` | Journal rows of their own kinds; trust is posted once per peer account, changes are explicit transfers (ACC-5) | [Ledger](ledger.md), [Settlement](settlement.md) |
| Settlement schemes | `settlement_schemes`, `settlement_checkpoints` | Each active scheme's config; its last reconciled on-ledger balance and its state (for XRPL, cursor, sequence, transactions in flight, peers' addresses), written in the batch they explain (ACC-2, ACC-6) | [Settlement](settlement.md#checkpoints-and-activation), [XRP Ledger](xrpl.md#state-and-restarts) |
| Incoming payments | `incoming_payments`, `incoming_connections` | Full Open Payments state with the STREAM credentials (ILP-10), and what each STREAM connection received, written with the journal rows that pay them | [Payments](payments.md#persistence) |
| Quotes and outgoing payments | `quotes`, `outgoing_payments` | Full state, with the amount sent as posted, the wallet address paid, and whether the amount to send or to deliver is fixed | [Payments](payments.md#persistence) |
| GNAP grants and tokens | `gnap_grants`, `gnap_tokens`, `grant_spend` | Grants with access, client key and interaction; tokens as SHA-256 hashes; each grant's spend per interval window, written with the payment that spends it | [Payments](payments.md#persistence) |
| Payment settings | `payment_settings` | The ledger `$host` names; the currency totals are shown in | [Payments](payments.md#persistence), [Exchange rates](connector.md#exchange-rates) |
| Exchange rates | `rate_sources`, `exchange_rates` | The owner's sources (at most 8); each currency's last rate with when it was fetched, restored at start | [Exchange rates](connector.md#exchange-rates) |

### Secrets at rest

([D10](../decisions/D10-secrets-at-rest.md))

- The data directory is 0700 and the database file 0600, as is the copy before a migration.
- Session tokens, BTP tokens and GNAP access and continuation tokens are stored only as SHA-256 hashes.
- The node key (and so the settlement entropy derived from it), the TLS key, the ACME credentials, the setup token and STREAM shared secrets are stored as they are. The node must sign and settle unattended, and encrypting them with a key stored next to them would add nothing.
- Under systemd the daemon runs as a dynamic user with no privileges and `UMask=0077` ([Release](release.md#systemd-units)).

## Configuration

The owner's configuration is one row of `node_config`, read into a validated `NodeConfig` (`dassie-node-core`). Nothing derived is stored: URLs are computed from the current values every time, so changing the hostname takes effect at once (RT-3).

| Field | Default | Valid values | Set by |
|---|---|---|---|
| `realm` | `test` | `test` or `live` (a `CHECK` in the table). Setting `live` is refused with `FAILED_PRECONDITION`, "the live realm is not supported yet", until the conditions for [real money](../roadmap.md#real-money) hold | `SetRealm`; `dassie init` sets `test` |
| `hostname` | none | A DNS name of at most 253 bytes, stored lowercase: labels of 1 to 63 letters, digits and hyphens, no leading or trailing hyphen; the last label not all digits; not an IP address. Single-label names such as `localhost` are allowed | `SetHostname`, `dassie init` |
| `http_port`, `https_port` | 80, 443 | 1 to 65535, different from each other. Used by the standalone runtime only; the listeners keep their ports until the node restarts | `SetPorts` (answers `restart_required`) |
| `alias` | empty | At most 64 bytes of UTF-8, no control characters; shown to other nodes next to the node ID | `SetAlias` |
| `acme_directory_url` | Let's Encrypt production, `https://acme-v02.api.letsencrypt.org/directory` | `https://` and a host, at most 2048 bytes of printable ASCII | `ObtainCertificate`'s directory ([HTTP](http.md)) |

- The ILP allocation scheme follows the realm: `test` for `test`, `g` for `live`.
- Let's Encrypt's staging directory is `https://acme-staging-v02.api.letsencrypt.org/directory` (`dassie init --acme-staging`).
- Settings that belong to an area are in that area's tables: peer settings ([Connector](connector.md)), rate sources ([Exchange rates](connector.md#exchange-rates)), payment settings ([Payments](payments.md)).

## Environment variables

Every variable the daemon reads is checked at start. Only `dassie-node`'s `env.rs` and `systemd.rs` read the daemon's environment, as a snapshot passed in, so tests pass their own.

**The daemon**

| Variable | Meaning |
|---|---|
| `DASSIE_STATE_DIRECTORY` | The data directory, first in [precedence](#database). Also where `dassie update` looks for the database (default `/var/lib/dassie`) |
| `STATE_DIRECTORY` | Set by systemd's `StateDirectory=`: its first entry is the data directory |
| `XDG_DATA_HOME`, `HOME` | The data directory's fallbacks: `$XDG_DATA_HOME/dassie`, then `$HOME/.local/share/dassie` |
| `DASSIE_IPC_SOCKET_PATH` | The IPC socket, for the standalone daemon and for every CLI command |
| `XDG_RUNTIME_DIR` | The standalone daemon's IPC socket is `$XDG_RUNTIME_DIR/dassie.sock` when the variable above is not set; the CLI tries it after `/run/dassie.sock` |
| `DASSIE_LOG_LEVEL` | `debug`, `info`, `warn`, `error` or `none`; default `info`. Anything else stops the daemon |
| `DEBUG` | Patterns of targets whose debug events are shown ([Logging](#logging)) |
| `DASSIE_BOOTSTRAP_NODES` | The bootstrap node list, as JSON (below). Default: the testnet's three bootstrap nodes ([Network](network.md)). `[]` makes the node run alone, with a warning |
| `DASSIE_PEER_ALLOW_PRIVATE` | `1` lets peers, bootstrap nodes and outbound requests reach private, loopback and link-local addresses, and resolves `*.localhost` to loopback. `0` or unset: refused ([Peer protocol](peer-protocol.md)). Dev and tests only |
| `DASSIE_DEV_TIMINGS` | `1` runs the peer protocol's timers and the settlement check on their dev values, polling and settling every second ([Network](network.md), [Settlement](settlement.md)). `0` or unset: production values. Dev and tests only |
| `DASSIE_PEER_CA_FILE` | A PEM file of extra roots the node's outbound client trusts beside the platform's: a dev or test CA. That client reaches peers, bootstrap nodes, exchange-rate sources and other wallets |
| `DASSIE_ACME_CA_FILE` | A PEM file of the only roots the ACME client trusts, instead of the platform's: for test CAs such as Pebble |
| `DASSIE_DEV_WALLET_DIR` | An absolute path to an existing directory holding a development build of the wallet, served instead of the embedded one; the daemon warns that it does |
| `LISTEN_PID`, `LISTEN_FDS`, `LISTEN_FDNAMES` | Socket activation ([Socket activation](#socket-activation)) |
| `NOTIFY_SOCKET` | Where readiness goes under systemd |

- `DASSIE_PEER_ALLOW_PRIVATE` and `DASSIE_DEV_TIMINGS` take exactly `1` or `0`; any other value stops the daemon.
- A variable read as text that is not UTF-8 stops the daemon. Paths are taken as they are, and a path variable that is set but empty counts as unset.
- A CA file set in `DASSIE_PEER_CA_FILE` or `DASSIE_ACME_CA_FILE` that cannot be read stops the daemon at start, rather than failing the first request.
- **`DASSIE_BOOTSTRAP_NODES`** is a JSON array of at most 64 objects `{"id", "url", "publicKey"}`, with no other fields. Each `url` must be a valid peer URL ([Peer protocol](peer-protocol.md)), each `publicKey` 32 bytes in base64url, and each `id` the node ID that key gives (NET-19). No node may be listed twice, by ID or by URL, since it would vote twice for the majority. Any violation stops the daemon with a message naming the entry.

**`dassie update`** ([Release](release.md#dassie-update))

| Variable | Meaning |
|---|---|
| `DASSIE_MIRROR_URL` | The mirror, unless `--mirror` names one; default `https://get.dassie.land`. Must be `https://` |
| `DASSIE_MIRROR_CA_FILE` | Extra roots to trust the mirror by |
| `DASSIE_SYSTEMD_UNIT_DIR` | Where units are installed, for tests; default `/etc/systemd/system` |
| `DASSIE_STATE_DIRECTORY` | Where the database is, for a rollback; default `/var/lib/dassie` |

**`install.sh`** reads `DASSIE_MIRROR_URL`, `DASSIE_CHANNEL`, `DASSIE_VERSION` (the same as its options), `NO_COLOR` and `TERM` (colour), and `SUDO_USER` ([Release](release.md#installsh)).

## Dev switches warn on a public node

`DASSIE_PEER_ALLOW_PRIVATE` and `DASSIE_DEV_TIMINGS` are plain environment variables. If either is set while a listener is bound beyond loopback, the daemon warns at startup, naming the variable and what it does, and saying to unset it on a public node.

- An unspecified address (`0.0.0.0`, `::`) counts as beyond loopback. One listener beyond loopback is enough. An IPv4-mapped loopback address counts as loopback.
- The address is the one actually bound, whether `--listen`, the standalone default or systemd's socket chose it.
- Dev and the tests listen on loopback, so they stay quiet.

## Logging

Events go through `tracing`, under the original's `das:node:*` targets. Core crates have no logging dependency: they report events and counters, and the node maps them to `tracing` ([engineering](../engineering.md)).

- **Targets:** `das:node:daemon` (start, stop, setup), `das:node:database`, `das:node:http`, `das:node:ipc`, `das:node:acme`, `das:node:auth` (setup, login, sessions), `das:node:rpc`, `das:node:systemd`, `das:node:accounting` (the engine), `das:node:btp`, `das:node:peer-protocol`, `das:node:routing`, `das:node:payment`, `das:node:settlement`.
- **Standard output** shows events at or above `DASSIE_LOG_LEVEL` (debug < info < warn < error; `none` shows nothing). Debug events are shown only for targets that match a `DEBUG` pattern.
- **`DEBUG` patterns** are separated by commas or whitespace. `*` matches any run of characters, every `*` of a pattern (the original replaced only the first, RT-28). A leading `-` excludes. Patterns match the whole target, and the first pattern that matches decides. Example: `DEBUG=das:node:*,-das:node:http`.
- **Format:** the target, the level and the message with its fields, and no timestamp: journald adds one. Colour only when standard output is a terminal.
- **The log ring** keeps the node's last lines for the wallet's logs page, whatever `DASSIE_LOG_LEVEL` says: info and above, and debug for the targets `DEBUG` names.
  - At most 1000 lines and 1 MiB of text. Each line is numbered from 1 since the process started, and has its time, level, target, message and fields.
  - A message is cut at 1024 bytes, a field's value at 256 bytes (each cut marked `…`), and at most 16 fields are kept.
  - Fields named like secrets are kept as `[redacted]`: `secret`, `token`, `password`, `cookie`, `private_key`, `credentials`, `setup_url`, and any name ending in `_secret` or `_token`.
  - Clients subscribe to it over RPC as a snapshot followed by new lines ([RPC](rpc.md)).
- There is no RPC to change the level at run time.

## The `dassie` command

```
dassie <command> [options]
dassie -h | --help        the usage, exit 0
dassie -V | --version     "dassie <version>", exit 0
```

- Run without a command, `dassie` prints the usage and exits 2. An unknown command, an unexpected argument or a malformed value prints the problem and the usage, and exits 2.
- Commands that talk to the daemon (`init`, `status`, `setup-url`) use the IPC socket: `DASSIE_IPC_SOCKET_PATH` alone if set, else `/run/dassie.sock`, then `$XDG_RUNTIME_DIR/dassie.sock`. Each failure gets a sentence that says what to do (RT-9): nothing listening ("start it with `systemctl start dassie` or `dassie daemon`"), permission denied ("run this as a member of the dassie-users group, or as root"), or the error itself.

| Command | Options | What it does |
|---|---|---|
| `daemon` | `--runtime standalone\|systemd`, `--http-port <port>` with `--https-port <port>`, `--listen <ip>` | Runs the node in the foreground, for a process manager ([Startup](#startup), [Runtimes](#runtimes)). Exit 1 if it could not start |
| `init` | `--hostname <name>`, `--email <address>`, `--agree-tos`, `--acme-directory <url>`, `--acme-staging`, `--skip-acme` | Configures the node: realm, hostname and certificate ([below](#dassie-init)) |
| `status` | none | Shows how the node is doing ([below](#dassie-status)) |
| `setup-url` | none | Prints the link that completes setup in a browser |
| `update` | `-f`/`--force`, `-t`/`--target-version <v>`, `--channel latest\|canary`, `--mirror <url>`, `--no-restart` | Installs a newer release from the mirror ([Release](release.md#dassie-update)) |
| `verify-install` | none | Checks that this binary works here ([below](#dassie-verify-install)) |

### `dassie init`

The original's wizard, with the same steps and prompts. The difference is that the daemon runs the ACME order and the wizard shows its progress, so there is one ACME client, not two (RT-13). Every prompt has a flag, so scripts and tests can run it unattended; a prompt that would be needed without a terminal is an error instead ("`--hostname` is required when not running in a terminal").

1. Header "Dassie Configuration". Read the config (`GetConfig`).
2. Note that the realm is set to `test`, the only one this preview supports, and set it (`SetRealm`).
3. Note "Public HTTPS Setup": the node needs a domain name that points at the host, with port 80 reachable from the internet. Ask "What is the publicly accessible domain name of this host?", pre-filled with the current hostname and validated as a hostname, or take `--hostname`. Set it if it changed (`SetHostname`).
4. Decide whether to get a certificate. Not with `--skip-acme`. Without a CA-issued certificate, yes. With one, interactively, ask "An existing TLS certificate was found, would you like to skip ACME/LetsEncrypt setup?" (default yes); unattended, keep it.
5. Getting one:
   - Read the CA's terms of service from its directory (`GetAcmeTerms`), the configured directory or `--acme-directory` (`--acme-staging` names Let's Encrypt's staging directory).
   - Agree with `--agree-tos`, or show the terms' URL and ask "Do you agree?" (default no). Refusing ends with "A certificate from an ACME CA such as Let's Encrypt is required in this preview."
   - Take `--email`, or ask "What is your email address?" (optional; empty unattended).
   - Call `ObtainCertificate` and show its progress stream: "Creating account...", "Ordering certificate...", "Preparing challenge...", "Waiting for confirmation from ACME provider...", "Finalizing order...", "Fetching certificate...", "Installing certificate...", then "Certificate installed".
6. Finish. If the node is not set up, print the setup URL on standard output and end with "Configuration completed! You may now continue setup in your browser by visiting <url>". Otherwise end with "Configuration completed! You may now visit your Dassie node at <url>/", where the URL carries the HTTPS port the daemon actually listens on.

Cancelling a prompt (Ctrl+C) ends with "Canceled. Exiting...". A failed call ends with its error and "Configuration did not complete." Both exit 1.

### `dassie status`

How the node is doing, in a few lines, for the owner. It names no secret, so its output can be pasted where the owner asks for help; that is why the setup URL has a command of its own.

- "Dassie <version> is running, for <duration> (since <UTC time>)."
- **Address:** `https://<hostname>/`, with the port if not 443; or that `sudo dassie init` sets the domain name.
- **Certificate:** self-signed ("browsers warn about it: `sudo dassie init` gets a real one"); from ACME, valid until a date and renewed by the node; or installed by hand, valid until a date and not renewed. It warns when the certificate ends within 14 days or has expired, and shows the last renewal error.
- **Setup:** done, with the node ID; or not yet, pointing to `sudo dassie setup-url`.
- **Network** (once set up): no bootstrap nodes set, so the node runs alone; or whether it is a member, listed by how many of how many bootstrap nodes, and how many peers are peered.
- **Ledgers** (once set up): each ledger's owner balance with its asset, and its connection: in sync, connecting, catching up, waiting for funds at its deposit address, or failed with the problem.
- **Updates:** from `<base>/last-update` ([Release](release.md#dassie-update)): none yet, up to date, updated to a version, rolled back with the reason, or failed with the reason; or that this binary was not installed by the installer.
- If the daemon cannot be reached, it prints "Dassie is not running.", the updates line and the reason, and exits 1.

### `dassie setup-url`

Prints the setup URL (`GetSetupUrl`), which the IPC socket hands only to `dassie-users` and root. Once the node is set up, it says so ("log in to the wallet with your passphrase") and exits 1. The token and URL are [HTTP](http.md)'s.

### `dassie verify-install`

Checks what the original checked, and more. Each check prints its title, and the first failure prints "Error: …" and exits 1.

1. "Verifying that SQLite is working...": open an in-memory database with the full schema, migrations and schema check, and compute `SELECT 1 + 1`.
2. "Verifying that TLS is working...": make a self-signed certificate and load it.
3. "Verifying that the wallet is built in...": fails for a binary built without `DASSIE_WALLET_DIST`.
4. "Verifying the version...": the binary's version must parse as a release or canary version, as the updater needs.

Then it prints the release key's ID ("Updates are verified with release key <ID>."), or a note that this build trusts no key and cannot update itself, and "Success! Dassie appears to be installed correctly."

## IPC socket

- Every connection is the owner's: access is the socket file's permissions, nothing else. Under systemd the socket unit makes it `/run/dassie.sock`, mode 0660, owner root, group `dassie-users`; the standalone daemon makes its socket 0660 itself (RT-9).
- At most 64 connections at once; more are closed with a warning. At shutdown, open connections get the same drain as HTTP.
- It carries the RPC contract in frames with a 4-byte big-endian length prefix ([RPC](rpc.md)).

## Decisions

- [D3](../decisions/D03-database-continuity.md): a new schema, v0 databases refused, and what a v0 owner loses by setting up again.
- [D5](../decisions/D05-ui-and-cli-rpc-contract.md): the CLI talks to the daemon over the same contract as the wallet.
- [D8](../decisions/D08-real-money.md): why the `live` realm is refused.
- [D10](../decisions/D10-secrets-at-rest.md): file modes, hashed tokens, and why the node key stays plaintext.
- [D15](../decisions/D15-bootstrap-network-for-v1.md): the default bootstrap nodes, and `DASSIE_BOOTSTRAP_NODES`.
- [D16](../decisions/D16-engineering-style-pragmatic-tigerstyle-and-deterministic-simulation.md): injected time and randomness, decisions in `dassie-node-core`, and checking state before each commit.
- [D23](../decisions/D23-each-node-books-its-own-fulfills.md): why an upgrade lifts the bans made for a grace limit and makes owed Fulfills final.
- [D27](../decisions/D27-simulations-run-continuously-from-random-seeds.md): why a failed commit is the only fault the store injects.

## Original

- [2 §1–§2](../original/02-node-runtime.md): the original's launcher, entry point, runtimes, actor start order and shutdown.
- [2 §3–§5](../original/02-node-runtime.md): its filesystem layout, environment variables, configuration scalars and database (`lib-sqlite`, `application_id 0x1DA53B81`).
- [2 §11–§13](../original/02-node-runtime.md): its IPC server and client, CLI reference and logger.
- [2 §17–§18](../original/02-node-runtime.md): its constants, and `lib-reactive`, `lib-sqlite`, `lib-logger`, `lib-terminal-graphics`.
