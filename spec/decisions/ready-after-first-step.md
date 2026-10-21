# A node is up once its engine has committed a step

**Areas:** [Node](../areas/node.md#readiness), [Release](../areas/release.md#rolling-back).

## Facts

- `dassie.service` is `Type=notify`: systemd counts the node as started when it reports readiness, and fails the start if the process exits first or the start timeout passes. `dassie update` rolls back exactly when that start fails.
- The engine runs under the supervisor, which restarts it after any failure, so a daemon whose engine fails at every start keeps running: it serves the wallet and the IPC socket, and moves no money.
- The engine rebuilds itself from the database and takes a first step before it reads any command, so its first commit comes without waiting for a peer, a ledger or a timer. Much of what a new release can break shows there: state the code no longer reads, a scheme it no longer knows, an invariant it checks and the database breaks.
- A node that is not set up has no identity, and its engine waits for one, which may take days.
- Startup must not wait for the network ([Node](../areas/node.md#startup)).

## Decision

The daemon reports readiness once the engine has committed its first step, or at once on a node without an identity. Until then, an engine that fails a few times in a row ends the daemon with an error, so the start fails and an update rolls back. From then on, the supervisor restarts the engine as before.

## Accepted

- A failure after the first step is not rolled back: a release that panics on some packet, or after an hour, counts as up.
- Outside an update, a node whose engine cannot start on its database no longer runs on without it: it exits, and systemd starts it again and again. Its wallet and IPC socket answer only between attempts, so the owner reads why from the journal.
- A node not set up yet reports readiness without an engine, so an engine that fails once the owner sets the node up is restarted, not rolled back. A first install has no older version to go back to.

## Rejected

- *Report readiness once the listeners are bound:* a release whose engine fails at every start counts as up, so it is never rolled back, and the node moves no money until the next release.
- *Wait for the start timeout instead of exiting:* the engine would be restarted until the timeout, minutes in which the node serves peers what the rollback then discards, and a broken update would take that long to roll back.
- *Let the updater check the node's health after readiness:* the updater would need its own idea of a working node and its own timeout, and a broken engine would still count as up to systemd and to everything that starts the node but the updater.
- *Report readiness once the node is connected, to peers or ledgers:* startup would wait on the network, and a node alone or offline would fail its start.
