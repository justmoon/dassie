# 2. Node runtime, operations surface and supporting libraries

This chapter covers the daemon as a process: startup and shutdown, configuration, persistence, HTTP/HTTPS/IPC listeners, TLS and ACME, authentication, the UI/CLI RPC, the CLI, logging, and the libraries everything else is built on (most importantly the reactive actor model, §18.1).

Source citations refer to the original implementation at commit `e962ab78` on `main` (see [the index](README.md#sources)).

Scope: `packages/app-dassie/src/` top-level files (`daemon.ts`, `index.ts`, `environment.d.ts`) and the feature folders `base/`, `config/`, `constants/`, `database/`, `http-server/`, `acme-certificate-manager/`, `authentication/`, `api-keys/`, `rpc-server/`, `local-ipc-server/`, `local-ipc-client/`, `command-line/`, `logger/`, `public-api-server/`, `utils/`. It also covers the supporting libraries (`lib-reactive`, `lib-reactive-rpc`, `lib-reactive-io`, `lib-rpc`, `lib-http-server`, `lib-sqlite`, `lib-logger`, `lib-x509`, `lib-terminal-graphics`, `lib-type-utils`, `lib-format-utils`, `lib-itergen-utils`), the packaging resources in `packages/app-build/resources/` that define the operational environment, and the docs under `docs/implementation-notes/` and `docs/contributing/`.

All paths are relative to the repository root. `APP` means `packages/app-dassie/src`.

Conventions:
- "Open question / known gap" (OQ) marks a TODO, bug, inconsistency or missing feature in the current code. A reimplementation should decide on purpose whether to reproduce the behaviour or fix it.
- "Observed behaviour" describes what the code actually does, even when it is probably not what was intended.

---

## 0. Executive summary

- The node is one long-running process, the **daemon**. It is built on `lib-reactive`, an actor/signal framework with dependency-tracked automatic restarts and lexical scopes for cleanup. All state is either in-memory signals and stores, or SQLite through `lib-sqlite`.
- There are three listeners:
  - an **HTTP** listener (default port 80). It serves the ACME http-01 challenge and redirects to HTTPS.
  - an **HTTPS** listener (default port 443). It serves the web UI, a JSON auth API, the WebSocket RPC at `/rpc`, and the public, peer, ILP, BTP and Open Payments endpoints.
  - a **Unix domain socket IPC** listener (default `/run/dassie.sock`). It serves the same RPC router, and every connection is treated as fully authenticated.
- There are two runtimes. `nodejs` binds the sockets itself. `nodejs-systemd` uses systemd socket activation and `sd_notify` readiness. Production uses `nodejs-systemd`.
- Configuration is split two ways:
  - **Environment** (immutable per process): paths, bootstrap nodes, log level, dev token.
  - **Database scalars** (mutable through RPC): realm, hostname, ports, TLS cert/key, node private key, ACME account.
- Node identity is an Ed25519 private key, `config.dassie_key`, stored as PEM in the DB.
  - It is derived client-side from a BIP39 mnemonic: `seed = BIP39(mnemonic)`, then `nodeKey = HMAC-SHA256(seed, "node")`.
  - The browser submits it once via `POST /api/setup`, gated by a random in-memory setup token.
  - Login proves knowledge of `HMAC-SHA256(nodeKey, "login")`.
- Sessions are random 32-byte hex tokens. They are stored in the DB table `sessions` and carried in the cookie `__Host-Dassie-Session` (HttpOnly, Secure, SameSite=Strict, Max-Age 10 years).
- Setup happens in two stages:
  1. The CLI wizard `dassie init` runs over IPC. It sets the realm and hostname, gets a Let's Encrypt certificate via ACME http-01 (the CLI drives ACME and the daemon serves the challenge tokens), and prints the setup URL.
  2. The browser completes setup by creating the key and submitting it.
- Certificates are renewed by an hourly task inside the daemon when they have 30 days or less left.

---

## 1. Process model and entry points

### 1.1 Shipped binary / launcher

Bundle layout (`packages/app-build/src/steps/copy-files-into-bundle.ts`). Bundle name: `dassie-<version>-linux-<arch>` (`packages/app-build/src/utils/bundle-name.ts`).

```
<DASSIE_ROOT>/                 e.g. /opt/dassie/0.0.1  (symlink /opt/dassie/current -> it)
  bin/node                     bundled Node.js binary (exact version pinned, engines "node": "=24.11.0")
  bin/dassie                   POSIX sh launcher (mode 0755)
  backend.mjs                  esbuild bundle of APP/command-line/entry.ts (ESM, node target)
  lib/better_sqlite3.node      native SQLite binding (prebuilt, ABI-matched)
  share/public/                built frontend (index.html, assets/*, favicon etc.)
  share/systemd/               dassie.service, dassie-http.socket, dassie-https.socket,
                               dassie-ipc.socket, dassie-update.service, dassie-update.timer
```

Launcher (`packages/app-build/resources/launcher/dassie.sh`):
```sh
BIN_FOLDER="$(dirname "$(readlink -f "$0")")"
DASSIE_ROOT="$(dirname "$BIN_FOLDER")"
export DASSIE_ROOT
exec "$BIN_FOLDER/node" "$DASSIE_ROOT/backend.mjs" "$@"
```
- Because of `readlink -f`, `DASSIE_ROOT` resolves to the **real** versioned directory (e.g. `/opt/dassie/0.0.1`), not `/opt/dassie/current`. This matters for `dassie update` (§12.4).

Build-time defines (`packages/app-build/src/steps/build-backend.ts`):
- `import.meta.env.MODE = "production"`, `import.meta.env.PROD = true`, `import.meta.env.DEV = false`.
- `__DASSIE_VERSION__`: the detailed version string. A release gets e.g. `"0.0.1"` (`LATEST_DASSIE_VERSION`). A canary gets `canary-YYYY-MM-DD-<shortsha>`, but its bundle directory is named `canary` (`packages/app-build/src/index.ts`). The global is declared in `APP/environment.d.ts`.
- These affect runtime behaviour:
  - `DEV` enables lib-reactive debug tools, actor-call diagnostics, dev-token RPC auth, the Let's Encrypt **staging** directory, and logger caller capture.
  - `PROD` makes the DB load the native binding from `<root>/lib/better_sqlite3.node`.

### 1.2 Entry: `APP/command-line/entry.ts`

```ts
const reactor = createReactor()
await main(reactor)
await reactor.dispose()
```
`main` (`APP/command-line/index.ts`) builds a `cmd-ts` `subcommands` binary named `dassie` with the subcommands `init`, `verify-install`, `daemon` and `update`, and runs it against `process.argv`.
- The CLI reactor is disposed once the subcommand handler resolves.
- The `daemon` handler is synchronous and creates a **separate** reactor: `createReactor(RUNTIMES[runtime])`. That reactor keeps running because open servers keep the event loop alive. So the outer `reactor.dispose()` does not stop the daemon.

### 1.3 Library exports: `APP/index.ts`

It exports `AttachLogger` and `DaemonActor`. These are used by the dev runner (`packages/app-dev/src/runner/launchers/node.ts`), which runs `DaemonActor` plus its own HTTPS server and dev-only routes.

### 1.4 Base context: `APP/base/types/dassie-base.ts`

```ts
interface DassieBase { clock: Clock; crypto: Crypto }
type DassieReactor = Reactor<DassieBase>
type DassieActorContext = ActorContext<DassieBase>
```
The base is supplied by `createRuntime()` from `lib-reactive-io/node` (§18.3) through `sig.withBase(...)`. All randomness (setup token, session tokens, BTP tokens, self-signed cert keys and serials) goes through `reactor.base.crypto`. That makes it mockable and deterministic in simulation.

---

## 2. Startup sequence, actor dependency order, shutdown

### 2.1 Runtime selection: `APP/command-line/commands/daemon/index.ts`

- `dassie daemon [--runtime <nodejs|nodejs-systemd>]`. The default is `nodejs`.
- The handler calls `createReactor(RUNTIMES[runtime])`, with `nodejs → NodejsDaemonActor` and `nodejs-systemd → NodejsSystemdDaemonActor`.

### 2.2 `nodejs` runtime: `APP/command-line/commands/daemon/nodejs/index.ts`

```
NodejsDaemonActor
 1. sig.run(LogToConsoleActor)            -> AttachLogger(): captureConsole + route log events to LogsStore + stdout
 2. dassieContext = sig.withBase(createRuntime())   (clock + crypto)
 3. await dassieContext.run(DaemonActor)  -> full feature tree (2.4); awaited to completion, including
                                             the first ACME expiry check and all node-identity services
 4. dassieContext.run(ServeHttpActor)     -> node:http createServer().listen(config.httpPort)
 5. dassieContext.run(ServeIpcSocketActor)-> unlink(ipcSocketPath) if it exists (ignore ENOENT), net.createServer().listen(path)
 6. await dassieContext.run(ServeHttpsActor) -> TLS cert from DB or freshly generated self-signed; node:https listen(config.httpsPort)
```
Notes:
- The listeners only start **after** `DaemonActor` resolves.
- Consequence: if the startup ACME renewal (2.4, `UpdateAcmeCertificateActor`) needs an http-01 challenge, nothing is listening on port 80 yet, so the challenge fails. It is retried an hour later. (OQ: startup ordering bug in `nodejs` runtime.)
- `ServeHttpActor` and `ServeHttpsActor` track `httpPort` / `httpsPort` / `url` / `tlsWebCert` / `tlsWebKey` with `readKeysAndTrack`. When any of them changes, the actor restarts: it removes its listeners, calls `server.close()`, then creates and listens on a new server.
  - OQ: there is a potential EADDRINUSE race because `close()` is asynchronous.
- The IPC socket file is created with the process umask. No `chmod` and no group ownership is applied (OQ, see §16).
- Listening errors such as EACCES on port 80 or EADDRINUSE are only logged (`"http server error"`, `"https server error"`, `"local ipc server error"`). The process keeps running.

### 2.3 `nodejs-systemd` runtime: `APP/command-line/commands/daemon/nodejs-systemd/*`

```
NodejsSystemdDaemonActor
 1. sig.run(LogToConsoleActor)
 2. state = getSocketActivationState()  (LISTEN_PID, LISTEN_FDS, LISTEN_FDNAMES must all be set)
      if missing: log error "could not find systemd socket activation environment; ... exiting",
                  process.exitCode = 1, dispose reactor, return
 3. systemdContext = sig.withBase({ ...createRuntime(), socketActivationState })
 4. systemdContext.run(ServeHttpActor)       fds named "dassie-http.socket"
 5. systemdContext.run(ServeIpcSocketActor)  fds named "dassie-ipc.socket"
 6. await systemdContext.run(ServeHttpsActor) fds named "dassie-https.socket"
 7. await systemdContext.run(DaemonActor)
 8. notifySystemdReady()   -> if NOTIFY_SOCKET set: assert starts with "@" or "/" and length>=2;
                               execSync("systemd-notify --ready")  (TODO: native sd_notify)
```
Socket activation (`socket-activation.ts`):
- FDs start at `SD_LISTEN_FDS_START = 3`. `LISTEN_FDNAMES` is split on `:`, and FD `3+i` is assigned to name `i`. Several FDs with the same name are grouped.
- The code asserts that `LISTEN_FDS == number of names`.
- `LISTEN_PID` is parsed but **not checked** against `process.pid` (OQ).
- `server.listen({fd})` is called once per FD, in the actor **factory** rather than the behaviour. Socket activation can only happen once per process, so a restart of the actor re-attaches the request handlers but does not re-listen.
- HTTPS updates certs in place with `server.setSecureContext({cert,key})`.
- **Ordering difference:** here the listeners are attached **before** `DaemonActor`. The routers start empty, so early requests get `404 Not Found` until the features register their routes. ACME renewal at startup therefore works. `READY=1` is only sent after `DaemonActor` resolves, which includes the first ACME check and possibly a full renewal. The unit has `TimeoutStartSec=infinity`.
- In systemd mode the DB `httpPort` and `httpsPort` are **ignored**, because the ports come from the socket units (80 and 443, IPv4 `0.0.0.0` only).

### 2.4 Feature actor tree: `APP/daemon.ts`

```
DaemonActor (async)
 ├─ sig.run(LocalRpcServerActor)          -> ServeLocalIpcActor: creates lib-rpc server(appRouter, superjson); exposes
 │                                           api.handleConnection(InboundConnection) with context {sig, isAuthenticated:true}
 ├─ sig.run(HttpServerActor)
 │    ├─ RedirectToHttpsActor             -> HttpRouter GET "*" (tracks httpsPort, HasTlsSignal)
 │    └─ ServeFrontendActor               -> HttpsRouter GET "*" (skipped if NODE_ENV==="development")
 ├─ await sig.run(AcmeCertificateManagerActor)
 │    ├─ ServeTokensActor                 -> HttpRouter GET /.well-known/acme-challenge/:token
 │    └─ await UpdateAcmeCertificateActor -> scheduled task, interval 1h; executes once immediately (awaited) then schedule()
 ├─ sig.run(TrpcServerActor)
 │    └─ RegisterTrpcHttpUpgradeActor     -> HttpsWebSocketRouter GET /rpc
 ├─ sig.run(AuthenticationFeatureActor)   -> tracks HasNodeIdentitySignal
 │    ├─ identity present: RegisterLoginRouteActor (POST /api/login), RegisterLogoutRouteActor (POST /api/logout)
 │    └─ identity absent:  RegisterSetupRouteActor (POST /api/setup)
 └─ await sig.run(StartNodeIdentityDependentServicesActor)   -> tracks HasNodeIdentitySignal
      if !hasNodeIdentity:
         setupUrl = readAndTrack(SetupUrlSignal); logger.warn(`Node identity is not configured, visit ${setupUrl}`); return
      else (order matters; "await" = wait for actor's behaviour promise):
         sig.run(AccountingActor)
         sig.run(BtpServerActor)
         sig.run(IldcpServerActor)
         sig.run(IlpHttpActor)
         sig.run(LocalIlpActor)
         await sig.run(ExchangeRatesActor)
         await sig.run(LedgersActor)
         await sig.run(OpenPaymentsServerActor)
         sig.run(PublicApiServerActor)        -> GET /stats, GET /nodelist/meta
         await sig.run(PeerProtocolActor)
         sig.run(RoutingActor)
```
- The features under `StartNodeIdentityDependentServicesActor` (accounting, BTP, ILDCP, ILP-HTTP, local ILP, exchange, ledgers, open payments, peer protocol, routing) are specified in chapters [3](03-peer-network.md), [4](04-interledger.md) and [5](05-accounting-and-settlement.md). This chapter only fixes their **start order** and the fact that they run only when the node identity exists.
- **Lazy resources.** `Database` is opened the first time something `use`s it. In practice that is `RedirectToHttpsActor` reading `DatabaseConfigStore` (nodejs runtime) or `ServeHttpsActor` (systemd runtime). Opening the database runs migrations and checks the schema (§5). `EnvironmentConfig` is evaluated on first `use`. Invalid env (e.g. a bad `DASSIE_DEV_SECURITY_TOKEN` length or invalid `DASSIE_BOOTSTRAP_NODES` JSON) throws at that point. The actor that hit it dies, the error is logged as "error in actor", and the process keeps running in a degraded state (OQ).

### 2.5 Reactive re-run triggers (what restarts when)

| Trigger | Restarted actors / effects |
|---|---|
| `setNodeIdentity` (via `POST /api/setup`) | `AuthenticationFeatureActor` (setup route removed; login/logout added); `StartNodeIdentityDependentServicesActor` (all identity services start); anything tracking the full `DatabaseConfigStore` |
| `setTlsCertificates` (ACME renewal, `tls.setNodeTlsConfiguration`) | `ServeHttpsActor` (new server in nodejs mode / `setSecureContext` in systemd mode); `RedirectToHttpsActor` (HasTls may flip); `SetupUrlSignal` |
| `setHostname` | `SetupUrlSignal`, and actors that `readAndTrack(DatabaseConfigStore)` or `readKeysAndTrack(...,["hostname"])` (e.g. Open Payments wallet handler). **Not** `ServeHttpsActor` (tracks `url`, which is not recomputed, see OQ in §4.2) |
| `setRealm` | `IlpAllocationSchemeSignal` (`test` → `"test"`, `live` → `"g"`) and dependents |
| `httpPort`/`httpsPort` change | Would restart the listeners in nodejs mode, but **no API exists to change ports** (OQ) |

### 2.6 Shutdown

- In production there are **no** SIGTERM/SIGINT handlers. Only the dev runner has `HandleShutdownSignalsActor`, in `packages/app-dev/src/actors/handle-shutdown-signals.ts`.
- When systemd stops the service (SIGTERM), Node exits with default behaviour. Scope cleanups do not run. That means `database.raw.close()` is not called, servers are not closed, and the IPC socket file is not unlinked.
- SQLite in better-sqlite3 uses rollback journal by default, and every write is a single synchronous statement or transaction, so this is crash-safe.
- `reactor.dispose()` is only called for CLI subcommands, and when socket activation is missing.
- Cleanup order when a scope is disposed: handlers run in **reverse registration order** and are awaited one after another (`lib-reactive/src/scope.ts`). `Database` registers `database.raw.close()` on the reactor scope.
- `Restart=always`, `RestartSec=1` in `dassie.service` handles crashes.
- OQ: a reimplementation should handle SIGTERM gracefully (close listeners, close the DB, remove the socket).

---

## 3. Filesystem layout, environment variables, permissions

### 3.1 Install layout (from `packages/app-build/resources/shell-installer/install.sh`)

| Path | Purpose |
|---|---|
| `/opt/dassie/<version>/` | Extracted bundle (see 1.1). Replaced entirely on reinstall of the same version |
| `/opt/dassie/current` | Symlink → `/opt/dassie/<version>` |
| `/usr/local/bin/dassie` | Symlink → `/opt/dassie/current/bin/dassie` |
| `/etc/systemd/system/{dassie.service,dassie-http.socket,dassie-https.socket,dassie-ipc.socket,dassie-update.service,dassie-update.timer}` | Copied from `share/systemd/` |
| group `dassie-users` | Created if missing. The invoking non-root user is added (the user must re-login) |
| `/run/dassie.sock` | IPC socket (systemd: `SocketMode=0660`, `SocketUser=root`, `SocketGroup=dassie-users`) |
| `/var/lib/dassie/` | `StateDirectory=dassie` → `STATE_DIRECTORY` → **dataPath**; contains `dassie.sqlite3` |
| `/var/cache/dassie/` | `CacheDirectory=dassie` → `CACHE_DIRECTORY` → cachePath (currently unused) |

The installer then runs:
1. `systemctl enable dassie.service dassie-update.timer`
2. `systemctl daemon-reload`
3. `systemctl restart dassie.service dassie-update.timer`
4. `dassie verify-install` (as root)

It then tells the user to run `dassie init`. `install.sh uninstall` stops and disables the units, removes the unit files and `rm -rf /opt/dassie`. It does **not** remove `/var/lib/dassie`, the group, or `/usr/local/bin/dassie` (OQ: leftover symlink; `dassie-update.service` is not removed either).

Installer details:
- Options: `-t|--target <version>` (default `latest`, resolved through `${MIRROR}/meta/latest`), `-m|--mirror <url>` (default `https://get.dassie.land`), commands `install` (default) and `uninstall`.
- It requires Linux, systemd running (state `running` or `degraded`), curl or wget, and tar.
- Architectures: x86_64→`x64`, aarch64/arm64→`arm64`, armv7l/armv8l→`arm7l`. (OQ: the build only ships `x64`, since better-sqlite3 arm mappings are commented out in `packages/app-build/src/steps/download-better-sqlite3.ts`.)
- It downloads `dassie-${VERSION}-linux-${ARCH}.tar.xz`, using curl with `--tlsv1.2 --proto =https --retry 3 --fail`, or wget with `--https-only --max-redirect=0`.
- Color handling follows `FORCE_COLOR`, then `NO_COLOR`, then a TTY check, then `TERM=dumb`.

Systemd unit `dassie.service`:
```
Type=notify, ExecStart=/opt/dassie/current/bin/dassie daemon --runtime=nodejs-systemd, NotifyAccess=all,
TimeoutStartSec=infinity, WorkingDirectory=/opt/dassie/current, DynamicUser=yes, StateDirectory=dassie,
CacheDirectory=dassie, Restart=always, RestartSec=1, Environment=HOME=/var/lib/dassie,
PATH=/usr/bin:/usr/local/bin, NODE_ENV=production, DASSIE_LOG_LEVEL=info, NoNewPrivileges=true
Requires/After: dassie-ipc.socket dassie-https.socket dassie-http.socket; After network-online.target time-set.target
```
- Socket units: `ListenStream=0.0.0.0:80` and `0.0.0.0:443` (with `NoDelay=true`), and `/run/dassie.sock`. FD names default to the unit names `dassie-http.socket`, `dassie-https.socket` and `dassie-ipc.socket`, which is what the code expects.
- `dassie-update.timer`: `OnCalendar=*-*-* 03:20:00`, `RandomizedDelaySec=3600`, `Persistent=true`. It triggers `dassie-update.service` (oneshot, `User=root`, `ExecStart=/opt/dassie/current/bin/dassie update`).

### 3.2 Runtime paths: `APP/config/environment-config.ts`

`envPaths("dassie-node")` (the `env-paths` v3 package) appends the default suffix `-nodejs`. On Linux this gives:
- data: `$XDG_DATA_HOME/dassie-node-nodejs`, or `~/.local/share/dassie-node-nodejs`
- cache: `$XDG_CACHE_HOME/dassie-node-nodejs`, or `~/.cache/dassie-node-nodejs`
- temp: `${os.tmpdir()}/<basename(homedir)>/dassie-node-nodejs`

| Field | Resolution order | Used by |
|---|---|---|
| `rootPath` | `DASSIE_ROOT` → `process.cwd()` | native binding path (`<root>/lib/better_sqlite3.node`, PROD only), frontend (`<root>/share/public`), `update` |
| `dataPath` | `DASSIE_STATE_DIRECTORY` → `STATE_DIRECTORY` → envPaths.data | DB file `<dataPath>/dassie.sqlite3`; created with `mkdirSync(recursive)` |
| `cachePath` | `DASSIE_CACHE_DIRECTORY` → `CACHE_DIRECTORY` → envPaths.cache | **unused** |
| `temporaryPath` | `DASSIE_TEMPORARY_DIRECTORY` → envPaths.temp | `update` download dir `<tmp>/dassie-update-<Date.now()>-<pid>` |
| `ipcSocketPath` | `DASSIE_IPC_SOCKET_PATH` → `/run/dassie.sock` | IPC server (nodejs runtime) and client |
| `bootstrapNodes` | JSON in `DASSIE_BOOTSTRAP_NODES` → `DEFAULT_BOOTSTRAP_NODES` | peer protocol / registration ([3 §11](03-peer-network.md)) |
| `devSecurityToken` | `DASSIE_DEV_SECURITY_TOKEN` → `false` | DEV-only RPC auth |
| `logLevel` | `DASSIE_LOG_LEVEL` (non-JSON) ∈ `debug,info,warn,error,none` → `"info"` | stdout log filter |

`parseConfigOptionWithSchema(name, schema, default, {json=true})` works as follows:
- If the variable is unset, return the default.
- If it is unset and there is no default, throw `Missing required environment variable X`.
- Otherwise `schema.parse(json ? JSON.parse(v) : v)`.

### 3.3 Every environment variable

| Variable | Read by | Meaning / default |
|---|---|---|
| `DASSIE_ROOT` | env config; launcher sets it | Install root; default `cwd` |
| `DASSIE_STATE_DIRECTORY` | env config | Data dir override (highest priority) |
| `STATE_DIRECTORY` | env config | Set by systemd `StateDirectory=` |
| `DASSIE_CACHE_DIRECTORY` | env config | Cache dir override |
| `CACHE_DIRECTORY` | env config | Set by systemd `CacheDirectory=` |
| `DASSIE_TEMPORARY_DIRECTORY` | env config | Temp dir override |
| `DASSIE_IPC_SOCKET_PATH` | env config; **also read directly** by `APP/local-ipc-client/index.ts` | Default `/run/dassie.sock` |
| `DASSIE_BOOTSTRAP_NODES` | env config | JSON array `[{id: string(2..45), url: string, publicKey: string}]` |
| `DASSIE_DEV_SECURITY_TOKEN` | env config; RPC server (DEV only) | Must be exactly 64 chars if set, otherwise EnvironmentConfig throws |
| `DASSIE_LOG_LEVEL` | env config → `StdoutLogLevel` signal | `debug|info|warn|error|none`; default `info` |
| `DASSIE_MIRROR_URL` | `update` command (`process.env` directly), installer | Default `https://get.dassie.land` |
| `NODE_ENV` | `ServeFrontendActor` | If `"development"`, frontend is not served. systemd sets `production` |
| `DEBUG` | lib-logger node context | Comma/space-separated namespace patterns enabling `logger.debug` (e.g. `das:node:*`). `console` enables captured `console.debug` |
| `FORCE_COLOR` | chalk (CLI formatter, terminal graphics), installer | Color forcing |
| `NO_COLOR` | installer (and chalk) | Disable colors |
| `DEBUG_HIDE_DATE` | Declared in `EnvironmentVariables`; set by dev runner; **not read** by app code | — |
| `NOTIFY_SOCKET` | `notify.ts` | If set, `systemd-notify --ready` is executed |
| `LISTEN_PID`, `LISTEN_FDS`, `LISTEN_FDNAMES` | `socket-activation.ts` | All three required for `nodejs-systemd` runtime |
| `HOME`, `PATH` | Set by unit file | Not read directly |
| Dev-only (set by `packages/app-dev/src/functions/start-node.ts`, consumed by dev runner, not by app-dassie): `DASSIE_DEV_ROOT`, `DASSIE_DEV_BASE`, `DASSIE_DEV_ENTRY`, `DASSIE_DEV_RPC_URL`, `DASSIE_DEV_NODE_ID` | — | — |

The typed interface is in `APP/config/types/environment-variables.ts`. It lists `FORCE_COLOR, DEBUG, DEBUG_HIDE_DATE, DASSIE_ROOT, DASSIE_STATE_DIRECTORY, STATE_DIRECTORY, DASSIE_CACHE_DIRECTORY, CACHE_DIRECTORY, DASSIE_TEMPORARY_DIRECTORY, DASSIE_IPC_SOCKET_PATH, DASSIE_BOOTSTRAP_NODES, DASSIE_DEV_SECURITY_TOKEN, DASSIE_LOG_LEVEL`.

### 3.4 Permissions

- In systemd mode the daemon runs as a dynamic, unprivileged user with `NoNewPrivileges`. Privileged ports come from socket activation.
- The IPC socket is `0660 root:dassie-users`. Any member of `dassie-users` gets **full admin RPC access** (§11).
- The DB file is created with the default umask (no explicit mode).
- The TLS private key, node private key and ACME account key are stored **in plaintext** in SQLite (OQ: encryption at rest).
- `update` requires write access to `DASSIE_ROOT`, its parent, `current` and the old version dir. Otherwise it prints "Insufficient permissions / Please run this command as root or with sudo".

---

## 4. Configuration

### 4.1 Environment config

See §3.2. `EnvironmentConfig` is a reactor factory. It is evaluated once and is immutable for the process lifetime.

### 4.2 Database config: `APP/config/database-config.ts`

`Config` (in-memory store `DatabaseConfigStore`, loaded once from DB scalars at first use):

| Field | Source scalar | Default when absent | Notes |
|---|---|---|---|
| `realm` | `config.realm` | `"test"` | `"test" \| "live"` (`VALID_REALMS`) |
| `hostname` | `config.hostname` | `undefined` | |
| `httpPort` | `config.http_port` | `80` | |
| `httpsPort` | `config.https_port` | `443` | |
| `alias` | `config.alias` | `""` | Used in own node-table entry (peer protocol) |
| `url` | **derived at load**: `https://${hostname}${httpsPort===443?"":":"+httpsPort}` | — | If hostname is unset it becomes the literal `"https://undefined"` |
| `exchangeRateUrl` | `config.exchange_rate_url` | `"https://api.coinbase.com/v2/exchange-rates"` | |
| `tlsWebCert` | `config.tls_web_cert` | `undefined` | PEM (ACME returns full chain) |
| `tlsWebKey` | `config.tls_web_key` | `undefined` | PEM |
| `dassieKey` | `config.dassie_key` | `undefined` | PEM PKCS#8 Ed25519 private key |

Store actions. Each one writes the DB scalar **and** updates the in-memory state through immer:
- `setRealm(realm)`
- `setHostname(hostname)`. **Does not recompute `url`** (OQ: `url` stays stale, typically `"https://undefined"`, until restart. It affects the log line "listening on …", `/stats` `node.url`, Open Payments ids based on `url`, and `ServeHttpsActor` re-runs.)
- `setTlsCertificates(cert, key)`
- `setNodeIdentity(dassieKey)`

Predicates:
- `hasTls(c) = tlsWebCert !== undefined && tlsWebKey !== undefined`
- `hasNodeIdentity(c) = dassieKey !== undefined`

OQ: there is no action or RPC for `httpPort`, `httpsPort`, `alias` or `exchangeRateUrl`. They can only be changed by editing the DB. The `config.enable_http_server` scalar is defined but never read.

### 4.3 Computed signals: `APP/config/computed/`

- `HasNodeIdentitySignal`: `hasNodeIdentity(DatabaseConfigStore)`.
- `HasTlsSignal`: `hasTls(DatabaseConfigStore)`.
- `IlpAllocationSchemeSignal`: `"test"` if realm is `test`, else `"g"`.
- `SetupUrlSignal` (`APP/authentication/computed/setup-url.ts`):
  - With TLS: `https://${hostname}/setup/${token}`. OQ: ignores a non-443 `httpsPort`.
  - Without TLS: `http://127.0.0.1/setup/${token}`. OQ: this URL cannot work, because the HTTP listener only redirects or returns 404, and the UI and `/api/setup` exist only on HTTPS.

Schemas (`APP/config/schemas/`):
- `nodeIdSchema` = string 2..45, with a TODO to enforce valid characters.
- `settlementSchemeIdSchema` = string 2..128, with the same TODO.
- `portSchema` = `z.coerce.number().int().min(1).max(65535)`.

### 4.4 Who mutates config

| Mutation | Entry points |
|---|---|
| realm | RPC `config.setRealm` (CLI `init` always sets `test`) |
| hostname | RPC `config.setHostname` (CLI `init`) |
| TLS cert/key | RPC `tls.setNodeTlsConfiguration` (CLI `init`), `UpdateAcmeCertificateActor` renewal |
| node identity | `POST /api/setup` only (once) |
| ACME account | RPC `acme.setAcmeCredentials` (CLI `init`) writes scalars directly (not via store) |

### 4.5 Configuration / setup wizard flow (end to end)

1. **Install** (§3.1). The service starts with an empty DB: realm `test`, no hostname, no TLS, no identity.
   - The HTTPS listener serves a self-signed cert (CN=localhost).
   - `StartNodeIdentityDependentServicesActor` logs `Node identity is not configured, visit <SetupUrl>`.
   - The setup token is 32 random bytes, hex encoded (64 chars). It is generated **in memory** per process start and is never persisted, so a restart invalidates old setup URLs.
2. **`dassie init`** (`APP/command-line/commands/init/index.ts`). It runs as a user in `dassie-users` and talks to the daemon over IPC:
   1. Header "Dassie Configuration". `config.getConfig.query()`.
   2. Note "Setting realm to 'test'" (preview supports test only). `config.setRealm({realm:"test"})`.
   3. Note "Public HTTPS Setup". Text prompt "What is the publicly accessible domain name of this host?", pre-filled with the current hostname. If canceled (Ctrl+C) or empty: note "Canceled. Exiting..." and stop. If the value differs from the current one: `config.setHostname({hostname})`. No validation of the domain.
   4. If `hasTls`: confirm "An existing TLS certificate was found, would you like to skip ACME/LetsEncrypt setup?" (default Yes). Choosing No forces ACME.
   5. **ACME flow** runs if there is no TLS or ACME is forced:
      - Show the Let's Encrypt ToS URL `https://letsencrypt.org/documents/LE-SA-v1.3-September-21-2022.pdf`. Confirm "Do you agree?" (default No). On rejection: an error note explaining that LE is required in the preview, then stop.
      - Text prompt "What is your email address?" (explanation: LE contact). It is not validated, and an empty string is accepted.
      - `acmeCrypto.createPrivateKey()` (acme-client default, RSA 2048 PEM). `client.createAccount({termsOfServiceAgreed:true, contact:["mailto:<email>"]})`, then `getAccountUrl()`.
      - `acme.setAcmeCredentials({accountUrl, accountKey: pem})`.
      - `createOrder({identifiers:[{type:"dns", value:domain}]})`, then `getAuthorizations`. For each authorization:
        1. Find the `http-01` challenge (throw if there is none).
        2. `getChallengeKeyAuthorization`.
        3. `acme.registerToken({token, keyAuthorization, expires: authorization.expires? Date : undefined})`.
        4. `verifyChallenge` (acme-client's local pre-check that fetches `http://<domain>/.well-known/acme-challenge/<token>`), then `completeChallenge`, then `waitForValidStatus`.
        5. `finally` `acme.deregisterToken({token})`.
      - `acmeCrypto.createCsr({commonName: domain})`. acme-client generates a new RSA 2048 key. Then `finalizeOrder`, then `getCertificate` (PEM chain).
      - `tls.setNodeTlsConfiguration({certificate, privateKey})`. The daemon then hot-swaps its HTTPS cert.
      - Progress notes: "Ordering certificate...", "Preparing challenge...", "Verifying challenge...", "Completing challenge...", "Waiting for confirmation from ACME provider...", "Generating certificate signing request...", "Finalizing order...", "Fetching certificate...", "Installing certificate...".
      - ACME directory: Let's Encrypt **production** in PROD builds, **staging** in DEV (`APP/acme-certificate-manager/constants/acme-service.ts`).
   6. If identity already exists: success note "Configuration completed! You may now visit your Dassie node at https://<domain>/". Otherwise: `config.getSetupUrl.query()` and note "Configuration completed! You may now continue setup in your browser by visiting <setupUrl>".
   7. When the command returns, the CLI reactor is disposed, the IPC socket is `end()`ed and the process exits.
3. **Browser setup** (frontend `packages/gui-dassie/src/app.tsx`, `pages/setup/*`, `utils/authentication.ts`). This is the contract the backend must support:
   - The UI loads at `https://<host>/setup/<token>`. The catch-all GET returns `index.html`.
   - The UI calls RPC `general.getBasicState`. If the result is `uninitialized`, it shows the setup flow.
   - The user generates a BIP39 English mnemonic (`generateMnemonic(wordlist)`, 128-bit, 12 words) and verifies one random word, or recovers from an existing mnemonic.
   - `seed = bip39.mnemonicToSeed(mnemonic)`: 64 bytes, empty passphrase.
   - `nodeKey = HMAC-SHA256(key=seed, msg="node")`: 32 bytes. This is the Ed25519 private seed.
   - `loginSecret = HMAC-SHA256(key=nodeKey, msg="login")`. Seed path `node/login` means one HMAC per path segment.
   - `POST /api/setup` with JSON `{setupAuthorizationToken: <token>, rawDassieKeyHex: hex(nodeKey), loginAuthorizationSignature: hex(loginSecret)}`.
   - The server verifies, stores the key, creates a session and sets the cookie. The UI is now authenticated.
4. **Subsequent logins**: the user enters the mnemonic. The UI derives `loginSecret` and sends `POST /api/login {loginAuthorizationSignature}`. On success the page reloads.
5. If `getBasicState.activeSettlementSchemes` is empty, the UI forces "create ledger account" (`ledgers.addSettlementScheme`).

Docs (`docs/implementation-notes/configuration-wizard.md`):
- Only nodes with public IPs are supported for now.
- Future work: deployments behind Cloudflare-like proxies (origin certificates; the real IP is revealed only to peers) and other ACME providers such as Google Trust Services.

---

## 5. Database

### 5.1 Mechanism: `lib-sqlite` (`packages/lib-sqlite/src/create-database.ts`, `utils/migrate.ts`, `internal/check-schema.ts`)

Opened by `APP/database/open-database.ts`:
- `mkdirSync(dataPath, {recursive:true})`
- Path `${dataPath}/dassie.sqlite3`
- `nativeBinding` = `<root>/lib/better_sqlite3.node` in PROD, otherwise the package default
- `checkSchema: true`, logger `das:node:database`
- `reactor.onCleanup(() => database.raw.close())`

Open steps:
1. `new SQLite(path, {nativeBinding})` (better-sqlite3, synchronous). `defaultSafeIntegers(true)`: **all INTEGERs are read as BigInt**.
2. If `PRAGMA application_id == 0` and `PRAGMA schema_version == 0` (a brand-new empty file): log "initializing a new sqlite database" and set `PRAGMA application_id = 0x1DA53B81` (decimal 497_368_961).
3. If `application_id != 0x1DA53B81`, throw "Database file has the wrong application ID - please make sure that the database file is not being shared between multiple applications".
4. **Migrate**:
   - `current = PRAGMA user_version`.
   - Validate that the migrations array is exactly versions 1..N in order (otherwise throw).
   - If `current > N`, throw "Database is newer than the current version of the app, cannot continue". **No downgrade support.**
   - For v = current+1..N, each in its own transaction: `migration.up(db)`, then `PRAGMA user_version = v`.
   - The option `dangerouslyRedoLastMigration` (down+up of the last migration) exists but is not used by the app.
5. **Schema check** for the `scalar` table and every table in the code schema:
   - The table exists (`table_info` is non-empty).
   - It is `STRICT`.
   - Its `WITHOUT ROWID` flag matches.
   - For each declared column: it exists, its type is exactly equal, NOT NULL matches (PRIMARY KEY implies NOT NULL), PK matches, and a declared default matches `CAST(? AS TEXT)` of the default value.
   - Any mismatch throws.
   - Columns present in the DB but not declared in code are **not** flagged. A default present in the DB but not declared in code is not flagged either (e.g. `peers.settlement_scheme_state`).
6. Kysely is wired over the same connection; queries are logged at debug.

Exposed API:
- `tables.<id>` (ConnectedTable): `select(where?)`, `selectAll()` (includes `rowid` unless the table is WITHOUT ROWID), `selectFirst(where?)`, `update(where, values)`, `updateAll`, `insertOne(row, {ignoreConflicts?})`, `insertMany` (in one transaction), `delete(where)` (equality AND), `deleteAll`, `upsert(row, conflictColumns)` (`INSERT … ON CONFLICT(cols) DO UPDATE SET k=EXCLUDED.k`), and iteration.
- `scalars.<id>` (`get/set/delete`).
- `raw`, `kysely`, `executeSync`, `transaction(cb)`, `logger`, `schema`.

### 5.2 Migrations: `APP/database/migrations/`

| v | File | Effect |
|---|---|---|
| 1 | `0001-create-scalar-table.ts` | `CREATE TABLE scalar (key TEXT PRIMARY KEY, value ANY NOT NULL) STRICT, WITHOUT ROWID` |
| 2 | `0002-create-outgoing-payment-table.ts` | `outgoing_payment(destination TEXT NN, subnet TEXT NN, total_amount INTEGER NN, sent_amount INTEGER NN) STRICT` (superseded by 12) |
| 3 | `0003-create-incoming-payment-table.ts` | `incoming_payment(id TEXT NN PK, subnet TEXT NN, total_amount INTEGER NN, received_amount INTEGER NN, external_reference TEXT NN) STRICT` (superseded by 12) |
| 4 | `0004-create-settlement-schemes-table.ts` | `settlement_schemes(id TEXT NN PK, config TEXT NN) STRICT`. config = JSON, module-specific |
| 5 | `0005-create-nodes-table.ts` | `nodes(id TEXT NN PK, public_key BLOB NN, url TEXT NN, alias TEXT NN) STRICT` |
| 6 | `0006-create-peers-table.ts` | `peers(node INTEGER PK NN, settlement_scheme_id TEXT NN) STRICT` |
| 7 | `0007-create-acme-tokens-table.ts` | `acme_tokens(token TEXT PK NN, key_authorization TEXT NN, expires TEXT) STRICT` |
| 8 | `0008-create-sessions-table.ts` | `sessions(token TEXT PK NN) STRICT` |
| 9 | `0009-create-btp-tokens-table.ts` | `btp_tokens(token TEXT PK NN) STRICT` |
| 10 | `0010-add-settlement-state-column.ts` | `ALTER TABLE peers ADD COLUMN settlement_scheme_state TEXT NOT NULL DEFAULT "{}"`. Down: `DROP COLUMN` |
| 11 | `0011-create-accounts-table.ts` | `accounts(path TEXT PK NN, debits_posted INTEGER NN DEFAULT 0, credits_posted INTEGER NN DEFAULT 0) STRICT` |
| 12 | `0012-change-payment-tables-for-open-payments.ts` | DROP `outgoing_payment` and `incoming_payment`; recreate (below). Down restores the v2/v3 shapes (data lost) |
| 13 | `0013-create-registrations-table.ts` | `registrations(node INTEGER PK NN, registered_at INTEGER NN, renewed_at INTEGER NN) STRICT` |

Notes:
- Each migration has `up` and `down`.
- Migration 10 uses the double-quoted `"{}"` as a default. SQLite accepts it as a string literal through the legacy identifier fallback. A reimplementation should use `'{}'`.

### 5.3 Final schema (user_version = 13)

All tables are `STRICT`. Only `scalar` is `WITHOUT ROWID`. INTEGER values are 64-bit signed and exposed as BigInt.

**`scalar`** (key/value store)

| column | type | constraints |
|---|---|---|
| key | TEXT | PRIMARY KEY (NOT NULL enforced by check) |
| value | ANY | NOT NULL |

**`settlement_schemes`** (`APP/ledgers/database-tables/settlement-schemes.ts`)

| column | type | constraints | meaning |
|---|---|---|---|
| id | TEXT | PK, NOT NULL | SettlementSchemeId (e.g. `stub`, `xrpl-testnet`) |
| config | TEXT | NOT NULL | JSON, module-specific |

**`nodes`** (`APP/peer-protocol/tables/nodes.ts`)

| column | type | constraints | meaning |
|---|---|---|---|
| (rowid) | INTEGER | implicit | Referenced by `peers.node` and `registrations.node` |
| id | TEXT | PK, NOT NULL | NodeId (`d` + base64url, §8.6) |
| public_key | BLOB | NOT NULL | Ed25519 public key |
| url | TEXT | NOT NULL | Node HTTPS URL |
| alias | TEXT | NOT NULL | Node alias |

**`peers`** (`APP/peer-protocol/tables/peers.ts`)

| column | type | constraints | meaning |
|---|---|---|---|
| node | INTEGER | PK, NOT NULL | = `nodes.rowid` (joined `nodes.rowid = peers.node` in `APP/peer-protocol/persist-node-table.ts`); no FK constraint |
| settlement_scheme_id | TEXT | NOT NULL | |
| settlement_scheme_state | TEXT | NOT NULL, DB default `{}` | JSON state of the settlement module for this peer |

**`registrations`** (`APP/peer-protocol/tables/registrations.ts`)

| column | type | constraints | meaning |
|---|---|---|---|
| node | INTEGER | PK, NOT NULL | = `nodes.rowid` |
| registered_at | INTEGER | NOT NULL | timestamp (ms; see [3 §15](03-peer-network.md)) |
| renewed_at | INTEGER | NOT NULL | timestamp |

**`acme_tokens`** (`APP/acme-certificate-manager/tables/acme-tokens.ts`)

| column | type | constraints | meaning |
|---|---|---|---|
| token | TEXT | PK, NOT NULL | ACME http-01 token |
| key_authorization | TEXT | NOT NULL | Response body |
| expires | TEXT | nullable | ISO-8601 timestamp; **informational only, never enforced** |

**`sessions`** (`APP/authentication/database-tables/sessions.ts`)

| column | type | constraints | meaning |
|---|---|---|---|
| token | TEXT | PK, NOT NULL | 64-char lowercase hex session token, stored in plaintext; no expiry/created-at |

**`btp_tokens`** (`APP/api-keys/database-tables/btp-tokens.ts`)

| column | type | constraints | meaning |
|---|---|---|---|
| token | TEXT | PK, NOT NULL | 22-char base64url BTP auth token (16 random bytes), plaintext |

**`accounts`** (`APP/accounting/database-tables/accounts.ts`)

| column | type | constraints | meaning |
|---|---|---|---|
| path | TEXT | PK, NOT NULL | AccountPath (internal ledger account path) |
| debits_posted | INTEGER | NOT NULL DEFAULT 0 | |
| credits_posted | INTEGER | NOT NULL DEFAULT 0 | |

**`incoming_payment`** (`APP/open-payments/tables/incoming-payment.ts`)

| column | type | constraints | meaning |
|---|---|---|---|
| id | TEXT | PK, NOT NULL | Payment ID (nanoid when created via `/incoming-payments`) |
| ledger | TEXT | NOT NULL | Internal ledger ID |
| total_amount | INTEGER | NOT NULL | In fundamental units of the ledger |
| metadata | TEXT | NOT NULL | JSON |

**`outgoing_payment`** (`APP/open-payments/tables/outgoing-payment.ts`)

| column | type | constraints | meaning |
|---|---|---|---|
| id | TEXT | PK, NOT NULL | Unique Payment ID (client-supplied via RPC `payment.createPayment`) |
| destination | TEXT | NOT NULL | Destination payment URL/pointer |
| ledger | TEXT | NOT NULL | LedgerId |
| total_amount | INTEGER | NOT NULL | |
| metadata | TEXT | NOT NULL | JSON |

Code schema object (`APP/database/schema.ts`):
- `applicationId = 0x1DA53B81`. The comment explains it is a random positive int32 with nibbles 2-4 replaced by `DA5`.
- Table IDs (used by the debug RPC): `accounts, incomingPayment, outgoingPayment, settlementSchemes, nodes, peers, registrations, acmeTokens, sessions, btpTokens`.

### 5.4 Scalars (rows in `scalar`)

| Key | SQLite type | Serialization | Code id |
|---|---|---|---|
| `config.realm` | TEXT | `"test"` / `"live"` (zod enum on read) | configRealm |
| `config.hostname` | TEXT | raw | configHostname |
| `config.http_port` | INTEGER | BigInt on write; `portSchema` (1..65535) on read | configHttpPort |
| `config.https_port` | INTEGER | same | configHttpsPort |
| `config.alias` | TEXT | raw | configAlias |
| `config.tls_web_cert` | TEXT | PEM | configTlsWebCert |
| `config.tls_web_key` | TEXT | PEM | configTlsWebKey |
| `config.dassie_key` | TEXT | PEM `-----BEGIN PRIVATE KEY-----` (PKCS#8 Ed25519, DER = `302E020100300506032B657004220420` ‖ 32-byte seed), 64-char base64 lines | configDassieKey |
| `config.exchange_rate_url` | TEXT | raw | configExchangeRateUrl |
| `config.enable_http_server` | INTEGER | 0/1 ↔ boolean | configEnableHttpServer (**unused**) |
| `acme.account_key` | TEXT | PEM (RSA, from acme-client) | acmeAccountKey |
| `acme.account_url` | TEXT | ACME account URL | acmeAccountUrl |

- Scalar `set` uses `INSERT … ON CONFLICT(key) DO UPDATE SET value=excluded.value`. `get` returns `undefined` when the row is missing (no scalar declares a default).
- Deserialization errors, e.g. an invalid port, **throw** when `DatabaseConfigStore` is loaded.

---

## 6. HTTP / HTTPS server

### 6.1 Listeners, ports, TLS

| Listener | nodejs runtime | nodejs-systemd runtime | Router |
|---|---|---|---|
| HTTP | `http.createServer().listen(httpPort)` (default 80; Node default host = all interfaces, dual stack) | FDs `dassie-http.socket` (0.0.0.0:80) | `HttpRouter` |
| HTTPS | `https.createServer({cert,key}).listen(httpsPort)` (default 443) | FDs `dassie-https.socket` (0.0.0.0:443), `setSecureContext` | `HttpsRouter` (requests) + `HttpsWebSocketRouter` (upgrades) |
| IPC | `net.createServer().listen(ipcSocketPath)` | FDs `dassie-ipc.socket` | lib-rpc (§11) |

TLS:
- Certificate and key come from `config.tls_web_cert` / `config.tls_web_key`. Node's default TLS options are used (no explicit min version, ciphers or ALPN/HTTP2). Only HTTP/1.1 is served.
- Fallback (`APP/http-server/functions/fallback-to-self-signed.ts`): if either value is missing, a self-signed cert is generated through `lib-x509` (§18.9) **every time the HTTPS actor (re)starts**. It is not persisted, so its fingerprint changes on every restart. The SHA-256 fingerprint is logged ("tls certificate successfully generated").
  - Subject = issuer: `CN=localhost, O=Unknown Dassie Operator, OU=Node, C=XX, ST=XX, L=Unknown`
  - RSA 2048, valid for 10 years from `clock.now()`.
- The log line `listening on ${url}` uses the (possibly stale) `url`.

Upgrade handling: `server.on("upgrade")` exists only on HTTPS. The HTTP listener ignores upgrade requests (Node's default behaviour is to destroy the socket).

### 6.2 Request handling semantics (`packages/lib-http-server`)

Request conversion (`environments/nodejs/wrap-request.ts`):
- The request URL is built as `${protocol}://${hostname}${req.url}` with `hostname = "0.0.0.0"`, because no hostname option is passed. **`url.hostname` is therefore always `0.0.0.0`**, and the Host header is ignored (OQ, it affects the redirect, §6.5).
- `protocol` is `https` if the socket is encrypted.
- A body stream is attached unless the method is one of GET, HEAD, OPTIONS, TRACE or DELETE.

Routers (`router.ts`):
- `createRouter()` returns `{get/post/put/delete/patch/head/options|method(m)}.path(p).use(mw).assert(fn).handler(scope, fn)`. Registration is removed when `scope` is disposed, so routes appear and disappear with their actors.
- **Static routes** (no `:param` and no `*`) are keyed by the literal `"<method> <path>"`. They are matched exactly against `url.pathname`, so there is no trailing-slash normalization. Registering the same static route twice throws "Route already exists for …".
- **Dynamic routes** (`:name` matches one segment, `:name*` or `*` matches the remaining segments and must be last) are stored in a per-method trie. They are matched against the pathname after stripping leading and trailing `/`. Precedence:
  1. static key
  2. trie: exact segment
  3. single-segment wildcard
  4. prefix wildcard
- The method is lowercased for matching. **HEAD is not mapped to GET** (OQ: `HEAD /` returns 404).
- No match: `404`, body `Not Found` (`text/plain;charset=UTF-8`).
- Middleware (`use`/`assert`) may return a Failure (short-circuit) or an object that is merged into the context. A handler returns an `HttpResponse` or an `HttpFailure`. Failures render as `status = failure.statusCode`, body = message.
- An uncaught exception in a handler gives `500`, content-negotiated through `Accept`:
  - `application/json`: `{"error":"Internal Server Error"}`
  - `text/html`: `<h1>Internal Server Error</h1>`
  - otherwise plain `Internal Server Error`
- Failure classes and status codes: BadRequest 400, Unauthorized 401, Forbidden 403, NotFound 404, MethodNotAllowed 405, NotAcceptable 406, PayloadTooLarge 413, UnsupportedMediaType 415.
- Responses:
  - `createJsonResponse` → `application/json`, `JSON.stringify`
  - `createPlainResponse` → default `text/plain`
  - `createBinaryResponse` → default `application/octet-stream`
  - `RedirectResponse(location, status=302)` sets `Location`
  - The `Content-Type` option is set only if no handler set it earlier.
- Response headers are copied into a plain object (`convertToNodejsResponseHeaders`). OQ: if a response carries several `Set-Cookie` headers, only the last survives. Today only one cookie is ever set per response.
- Body parsing middlewares:
  - `parseBodyUint8Array` enforces a max of **2 MiB** (`DEFAULT_MAX_BODY_SIZE = 2*1024*1024`) → 413 "Payload Too Large, expected body to be less than 2097152 bytes".
  - `parseJson` uses `secure-json-parse` (protects against prototype poisoning) → 400 "Invalid HTTP request body". It does **not** check `Content-Type`.
  - `parseBodyZod(schema)` → 400 "Invalid HTTP request body" on schema failure.
  - `parseBodyOer(schema)`.
  - `parseQueryParameters(zod)` → 400 "Invalid API request query string".
- Other middleware:
  - `cookie` parses the `Cookie` header with RFC 6265 token and value regexes, first occurrence wins, quoted values are unquoted. (The app parses cookies with the `cookie` npm package instead.)
  - `cors` sets `Access-Control-Allow-Origin: *`. No preflight/OPTIONS handling.
  - `createAcceptHeaderAssertion(mt)` requires `Accept` to **exactly** equal `mt` → 406 "Not Acceptable, expected <mt>".
  - `createContentTypeHeaderAssertion(mt)` requires an exact `Content-Type` match → 415 "Unsupported Media Type, expected <mt>".
- WebSocket router (`createWebSocketRouter`) works the same way. The handler calls `upgrade(cb)`, which uses `ws` `WebSocketServer({noServer:true}).handleUpgrade(...)`, sets `binaryType="arraybuffer"`, and returns a dummy `WebSocketResponse`. If the handler returns a normal response, it is written raw to the socket as HTTP/1.1. **If no route matches, nothing is written and the socket is left open** (OQ: should reply 404 and destroy the socket).
- `setCookie(headers, {name, value, maxAge?, partitioned?})`:
  - **Requires** the name to start with `__Host-`, otherwise it throws.
  - Emits `name=value; Path=/; HttpOnly; Secure; SameSite=Strict[; Max-Age=n][; Partitioned]`.
  - A negative maxAge throws.
  - `clearCookie` sets value `""` with `Max-Age=0`.

### 6.3 Complete HTTP route table (production daemon)

Routes owned by this area are marked ★. The others are listed for completeness; their behaviour is specified in chapters [3](03-peer-network.md) and [4](04-interledger.md).

| Listener | Method | Path | Auth | Request | Response | Registered by / when |
|---|---|---|---|---|---|---|
| HTTP | GET | `/.well-known/acme-challenge/:token` ★ | none | — | 200 `text/plain` key authorization; 404 `Not Found` if unknown; 400 `Bad Request` if empty token | `ServeTokensActor` (always) |
| HTTP | GET | `*` ★ | none | — | If TLS configured: **301** `Location: https://<url.hostname>[:<httpsPort> if ≠443]<pathname>` (query string dropped; hostname is `0.0.0.0`, see OQ); else 404 `Not Found` (TODO: show helpful message) | `RedirectToHttpsActor` (always) |
| HTTP | other | any | — | — | 404 `Not Found` | — |
| HTTPS | POST | `/api/setup` ★ | setup token | JSON `{setupAuthorizationToken, rawDassieKeyHex, loginAuthorizationSignature}` (all strings) | 200 `{}` + Set-Cookie; 401 text `Node is already set up` / `Invalid setup authorization token` / `Invalid login authorization signature`; 400/413 body errors | `RegisterSetupRouteActor` (only while **no** identity) |
| HTTPS | POST | `/api/login` ★ | login secret | JSON `{loginAuthorizationSignature}` | 200 `{}` + Set-Cookie; 401 `Invalid login authorization signature` | `RegisterLoginRouteActor` (only when identity exists) |
| HTTPS | POST | `/api/logout` ★ | none (cookie optional) | any/none | 200 `{}` + clearing Set-Cookie | `RegisterLogoutRouteActor` (only when identity exists) |
| HTTPS (WS) | GET (Upgrade) | `/rpc` ★ | cookie session (or DEV `?token=`) | WebSocket, text frames, superjson envelopes | lib-rpc protocol (§10) | `RegisterTrpcHttpUpgradeActor` (always) |
| HTTPS | GET | `/stats` ★ | none | — | 200 JSON (§14) | `PublicApiServerActor` (identity) |
| HTTPS | GET | `/nodelist/meta` ★ | none | — | 200 JSON `{hash: base64url}` | `PublicApiServerActor` (identity) |
| HTTPS | GET | `*` ★ | none | — | Static asset or `index.html` (§6.4) | `ServeFrontendActor` (unless `NODE_ENV=development`) |
| HTTPS | POST | `/peer` | peer-protocol signature (in body) | `Content-Type` and `Accept` exactly `application/dassie-message`, OER body | see [3 §6](03-peer-network.md) | `APP/peer-protocol/register-peer-http-handler.ts` |
| HTTPS | POST | `/ilp` | see [4 §8](04-interledger.md) | `Content-Type` and `Accept` exactly `application/octet-stream` | | `APP/ilp-http/register-ilp-http-handler.ts` |
| HTTPS | POST | `/ilp/callback` | see ILP-HTTP spec | same | | `APP/ilp-http/register-ilp-http-callback-handler.ts` |
| HTTPS (WS) | GET (Upgrade) | `/btp` | BTP auth with `btp_tokens` | binary BTP | | `APP/btp-server/register-btp-http-upgrade.ts` |
| HTTPS | GET | `/.well-known/pay` | none; CORS `*` | — | wallet JSON `{id, assetCode:"USD", assetScale:9, authServer:"https://not-available", resourceServer}` | `APP/open-payments/handle-wallet-requests.ts` |
| HTTPS | GET | `/incoming-payments` | **none**; CORS `*` | — | `{result:[...]}` | `APP/open-payments/handle-incoming-payments.ts` |
| HTTPS | POST | `/incoming-payments` | **none**; CORS `*` | JSON `{walletAddress, incomingAmount:{value,assetCode,assetScale 0..255}, expiresAt?, metadata?}` | formatted payment | same |
| HTTPS (dev runner only) | POST | `/api/_dev/set-session` | none | `{sessionToken}` | sets cookie to the given token | `packages/app-dev/src/runner/actors/development-set-session.ts` (not in production) |

### 6.4 Frontend serving: `APP/http-server/serve-frontend.ts`

- `frontendPath = <rootPath>/share/public`, `index.html` inside it.
- For `GET *`:
  1. If the pathname matches `^\/((?:assets\/)?[\w-]+\.([a-z]{2,}))$` and the extension is in the MIME map, read the file synchronously and return it as binary with that content type. If the read fails (e.g. missing), fall through.
  2. Otherwise return `index.html` (read from disk on **every** request) as `text/html`, status 200 (SPA fallback).
- MIME map:

  | extension | content type |
  |---|---|
  | `ico` | `image/x-icon` |
  | `png` | `image/png` |
  | `css` | `text/css` |
  | `js` | `text/javascript` |
  | `webmanifest` | `application/manifest+json` |

- OQ: unknown extensions (`svg`, `woff2`, `json`, `map`, …) and nested paths deeper than `assets/` fall back to HTML. No caching headers, ETag or compression. No security headers (CSP, HSTS, X-Frame-Options, X-Content-Type-Options, Referrer-Policy).
- Path traversal is prevented by the regex (only `[\w-]` names).

### 6.5 Redirect

See the route table. OQ: the redirect uses `url.hostname`, which is the hard-coded `0.0.0.0`. It should use the configured `hostname` or the Host header, and it should preserve the query string.

### 6.6 WebSocket RPC upgrade: `APP/rpc-server/rpc-server.ts`

- One lib-rpc server instance per actor run (`createServer({router: appRouter, transformer: superjson})`).
- On `GET /rpc` upgrade:
  1. Parse the `Cookie` header with `cookie.parse`. Read `__Host-Dassie-Session`. `authenticated = sessionsStore.read().has(token)`.
  2. DEV builds only: if `DASSIE_DEV_SECURITY_TOKEN` (64 chars) is set and `?token=` equals it (length 64), then `authenticated = true`. The code comments that even if this were enabled in production it would require the env var.
  3. Upgrade, then `rpcServer.handleConnection({connection: createWebSocketAdapter(ws), context:{sig, isAuthenticated}})`.
- Authentication is evaluated **once at upgrade**. Logging out does not downgrade existing sockets (OQ).
- There is no `Origin` check (OQ). Cross-site WebSocket hijacking is mitigated only by `SameSite=Strict`, which stops the cookie from being sent on cross-site WS handshakes in modern browsers.

---

## 7. ACME certificate manager: `APP/acme-certificate-manager/`

- `AcmeCertificateManagerActor`: `run(ServeTokensActor)`, then `await run(UpdateAcmeCertificateActor)`.
- **ServeTokensActor**: see the route table. It looks up `acme_tokens` by token on each request and ignores `expires`.
- **UpdateAcmeCertificateActor**:
  - Constants: `ACME_EXPIRY_POLLING_INTERVAL = 3_600_000 ms` (1 h) and `MINIMUM_CERTIFICATE_EXPIRY = 2_592_000_000 ms` (30 days).
  - Task `sig.task({handler, interval: 1h})`. The overlap policy defaults to `skip` and there is no jitter. It runs `await task.execute()` immediately, then `task.schedule()`, which reschedules itself after every tick.
  - `checkAcmeCertificateExpiry()`:
    1. Read `{tlsWebCert, hostname}`. If either is missing, return. A self-signed fallback is never stored, so it is never renewed.
    2. Parse with `new X509Certificate(tlsWebCert)` and compute `validTo - now`. If that is more than 30 days, return.
    3. Load `acme.account_key` and `acme.account_url`. If either is missing, `warn("unable to renew certificate: no account key or URL saved")` and return.
    4. `new AcmeClient({directoryUrl, accountKey, accountUrl})`. `info("renewing tls certificate", {hostname})`.
    5. `createOrder({identifiers:[{type:"dns", value: hostname}]})`, then `getAuthorizations`.
    6. For each authorization: find `http-01` (throw if missing), `getChallengeKeyAuthorization`, then `acmeTokens.upsert({token, key_authorization, expires: ISO or null}, ["token"])`. Then `verifyChallenge`, `completeChallenge`, `waitForValidStatus`. On error: `warn("failed to complete ACME challenge", {error})` and **return** (abort). In `finally`, delete the token row.
    7. `createCsr({commonName: hostname})` (new RSA 2048 key), then `finalizeOrder`, then `getCertificate`. `setTlsCertificates(cert, key)`. `info("certificate successfully renewed")`.
  - Errors thrown inside the task are caught by the scheduler and logged as "error in scheduled task".
- The ACME directory is **LE production** (`acme-client` `directory.letsencrypt.production`), or staging in DEV.
- OQ:
  - There is no backoff or rate-limit awareness. A failure is retried hourly, which risks the Let's Encrypt failed-validation limits.
  - No cert/key consistency check.
  - Renewal requires port 80 to be reachable from the internet.
  - In the nodejs runtime the first renewal at startup cannot succeed (§2.2).
  - `X509Certificate` parsing a non-PEM value throws, and the task logs the error.
- RPC used by the CLI wizard (§10): `acme.setAcmeCredentials`, `acme.registerToken` (a plain `insertOne`, which **throws on a duplicate token** and the client sees "Internal server error"), `acme.deregisterToken`.

---

## 8. Authentication: `APP/authentication/`

### 8.1 Constants

- `SESSION_COOKIE_NAME = "__Host-Dassie-Session"` (`APP/constants/cookie-name.ts`).
- `COOKIE_MAX_AGE_SECONDS = 315_360_000` (10 years, `60*60*24*365*10`).
- Seed paths (`APP/constants/seed-paths.ts`):
  - `node`: the node Ed25519 private key.
  - `node/login`: the login proof secret.
  - `dev/session`: the deterministic dev session token. The dev tooling computes it as HMAC(seed, "dev") then "session".
  - `node/settlement-module`: entropy for settlement modules.
- `getPrivateSeedAtPath(nodePrivateKey, path)` (`APP/crypto/utils/seed-paths.ts`):
  - For `node`, return the key itself.
  - For `node/…`, chain HMAC-SHA256 over the remaining segments (key = previous output, message = segment UTF-8).
  - Anything else throws ("Unable to calculate seed values server-side unless their path starts with node/").

### 8.2 Setup token

`SetupAuthorizationTokenSignal`: 32 bytes from `reactor.base.crypto.getRandomBytes`, lowercase hex (64 chars). It lives in memory only and is regenerated on every process start.

### 8.3 `POST /api/setup`

Body (zod): `{setupAuthorizationToken: string, rawDassieKeyHex: string, loginAuthorizationSignature: string}`.

Steps:
1. If an identity already exists → 401 `Node is already set up`. This is defensive, since the route is unregistered after setup anyway, so later calls get 404.
2. If `setupAuthorizationToken !== current token` → 401 `Invalid setup authorization token`.
3. `key = Buffer.from(rawDassieKeyHex, "hex")`. **The length and hex validity are not checked** (OQ: a short or invalid key is stored, and then `NodePrivateKeySignal`'s `parseEd25519Key` throws "Invalid length…").
4. `expected = hex(HMAC-SHA256(key, "login"))`. If it differs → 401 `Invalid login authorization signature`. Ordinary string comparison is used, not constant-time (OQ).
5. `config.setNodeIdentity(serializeEd25519Key(key,"private"))`, which writes the PKCS#8 PEM.
6. `session = hex(random 32 bytes)`. `sessions.addSession(session)`, which inserts into the DB and the in-memory Set.
7. Set-Cookie `__Host-Dassie-Session=<session>; Path=/; HttpOnly; Secure; SameSite=Strict; Max-Age=315360000`. Respond 200 `{}`.

### 8.4 `POST /api/login`

Body: `{loginAuthorizationSignature: string}`.
- `expected = hex(getPrivateSeedAtPath(NodePrivateKeySignal, "node/login"))`. If it does not match → 401 `Invalid login authorization signature`.
- Otherwise create a new session (as above). Respond 200 `{}`.
- There is no rate limiting or lockout (OQ). The secret is 256-bit, so brute force is infeasible.

### 8.5 `POST /api/logout`

- Parse cookies. If `__Host-Dassie-Session` is present, `removeSession(token)` from the DB and memory. Its existence is not checked.
- Always clear the cookie (`Max-Age=0`) and respond 200 `{}`.
- Unauthenticated logout is harmless. CSRF-style logout from another site does not send the cookie (SameSite=Strict).

### 8.6 Node identity derived values (used by config/RPC)

- `NodePrivateKeySignal` = `parseEd25519Key(config.dassieKey, "private")`. It asserts the identity is configured, otherwise it throws `AssertionError("node identity is not configured")`. The parse checks the tag `PRIVATE KEY`, a DER length of 48 and the preamble.
- `NodePublicKeySignal` = `ed25519.getPublicKey(privateKey)` (noble-ed25519 with sha512 from node:crypto).
- `NodeIdSignal` = `calculateNodeId(pub)` = `"d" + base64url(SHA-256(OER(sequence{versionPrefix OID 1.3.6.1.4.1.54392.5.1399.2.1, nodePublicKey octetString}))).slice(0,32).replace(/A+$/,"")` (`APP/ilp-connector/utils/calculate-node-id.ts`). The ID is therefore at most 33 chars, with trailing `A`s stripped.

### 8.7 Sessions store

`SessionsStore` (`APP/authentication/database-stores/sessions.ts`):
- An immer `Set<SessionToken>` loaded from `sessions` at first use.
- Actions: `addSession(token)` (Set add + DB `insertOne`) and `removeSession(token)` (Set delete + DB `delete`).
- There is no expiry, enumeration UI or "log out everywhere" (OQ).

### 8.8 Security notes (also see §16)

- `frontend-security.md`: the root seed is generated in the browser, so the frontend holds more privilege than the node. A node controlling the served JS could steal it.
- Options considered for future work:
  1. An independent, locally installed frontend, or a signed manifest plus a thin shim.
  2. Service-worker verification (arXiv 2105.05551).
  3. A `data:` URL bookmarklet that loads a script by hash.
- CSP is recommended but not implemented.

---

## 9. API keys / BTP tokens: `APP/api-keys/`

- `BtpTokensStore`: an immer `Set<BtpToken>` loaded from `btp_tokens`. Actions: `addToken` (Set add + insert) and `removeToken` (Set delete + delete).
- RPC (`apiKeys.*`, protected):
  - `getCurrentKeys` (query) → `BtpToken[]`.
  - `addBtpToken` (mutation, no input) → generates `base64url(16 random bytes)` without padding, which is 22 chars. Stores and returns it.
  - `removeBtpToken` (mutation, input `z.string().length(22)`) → removes it. Silently succeeds if the token is absent. Returns `undefined`.
- Tokens are consumed by the BTP server (`/btp` WS) for client authentication. See [3 §14](03-peer-network.md).

---

## 10. RPC: `APP/rpc-server/` + `packages/lib-rpc`

### 10.1 Transport and wire protocol (lib-rpc, protocol id `dassie-rpc-01`)

- **Transports.**
  - WebSocket `wss://<host>/rpc`: one envelope per **text** frame. Binary frames make the adapter throw `TypeError("Expected string data")` inside the `ws` listener (OQ: possible uncaught exception).
  - Unix socket: newline-delimited (`\n`) UTF-8 messages, one envelope per line. The message must not contain raw newlines, which superjson/JSON output never does.
- **Serialization.** Every message is `superjson.stringify(envelope)`, i.e. `{"json": <JSON-safe value>, "meta": {"values": {...type annotations...}}}`. BigInt values become strings annotated `"bigint"`; `Date`, `Set`, `Map`, `undefined` and similar are annotated too. The receiver runs `superjson.parse` and then validates against zod.
  - Reimplementation: either implement a superjson-compatible codec, or (since the frontend is also being rewritten) define a new canonical encoding. Values that need type preservation are bigint amounts, `Date` (`acme.registerToken.expires`), `Set` and `Map` (inside store snapshots).
- **Client → server envelopes:**
  - `{protocol:"dassie-rpc-01", type:"request", routeType:"query"|"mutation"|"subscription", id:string, path:string[], input:unknown}`
  - `{protocol:"dassie-rpc-01", type:"cancel", id:string}`
- **Server → client envelopes:**
  - `{protocol, type:"response", id, result:{type:"success", data:unknown} | {type:"error", message:string}}`
  - `{protocol, type:"event", id, data: unknown[]}` (a batch of subscription values)
- **IDs**: generated by the client, decimal strings counting up from `"0"` per client instance.
- **Server processing** (`lib-rpc/src/server/server.ts`):
  - Invalid envelopes are logged ("Invalid envelope") and dropped. No response is possible because the id is unknown.
  - **Router resolution** (`router.ts`) walks `path` through nested routers. Results:
    - Unknown path, or extra segments → `RpcFailure("RPC method not found: a/b")`.
    - The route's type differs from `routeType` → `RpcFailure("Invalid route type: …")`.
    - Input is validated with zod `safeParse`; on failure → `RpcFailure("Invalid input: <messages>")`.
    - Otherwise the route runs: middlewares in order, then the handler.
  - **Error mapping (Observed behaviour):**
    - A router-level `RpcFailure` (not found, wrong type, invalid input) produces an error response whose `message` is `failure.name`, i.e. the literal **`"RpcFailure"`**. The descriptive message is lost (OQ).
    - A **thrown** exception in a handler is logged as "Error during RPC call" and produces an error response `"Internal server error"`. Messages such as "Livenet is not yet supported" therefore never reach the client (OQ).
    - A **middleware** failure (the `protectedRoute` check returning `RpcFailure("Unauthorized")`) is returned by the route as a *value*. The router wraps it in `RpcSuccess`, so the client receives a **success** response whose data is the serialized failure object `{"dassie.failure": true, name: "RpcFailure", message: "Unauthorized"}` (OQ: should be an error response).
    - For subscriptions the server first sends `success(null)` and then calls the returned value as a function. With an unauthorized subscription that value is not a function, so a TypeError is thrown and an extra `error "Internal server error"` response is sent for the same id (OQ).
  - **Subscriptions:**
    - After `success(null)`, the server calls `subscribe(onData)` and keeps the returned disposer under the request id.
    - Each `onData(value)` pushes onto a queue. The first item schedules a flush via `setTimeout(0)`, which sends `event{data:[...queue]}`. When the queue reaches **100** (`MAX_SUBSCRIPTION_BATCH_SIZE`), the producer awaits the pending flush.
    - `cancel{id}` disposes the subscription.
    - Connection close disposes every subscription on that connection.
  - Requests are handled concurrently. Responses may arrive out of order.
- **Client** (`lib-rpc/src/client/client.ts`):
  - A proxy: `client.rpc.<router>.<proc>.query(input) | .mutate(input) | .subscribe(input)`. The last path segment must be one of `query|mutate|subscribe`.
  - Errors reject with `Error(message)`.
  - `subscribe` returns a `Subscription` function: `(onData) => disposer`. The disposer sends `cancel`.
  - WebSocket link (`links/websocket.ts`):
    - Auto-reconnects with exponential backoff: immediate first attempt, min 1 s, max 30 s, factor 2, jitter ±50%.
    - Messages sent while disconnected are queued.
    - On reconnect (the `resets` topic), every active subscription request is re-sent and its response ignored.
    - OQ: in-flight query/mutation callbacks whose responses were lost hang forever. There is no timeout.
  - Node socket link: newline framing as on the server.
  - Disallowed router keys: `call, apply, then, catch, key, path, prototype, constructor, routes, context, input, output, type, inputSchema, middlewares, query, mutation, subscription, use, mutate, subscribe`.

### 10.2 Auth model

- `publicRoute = createRoute().context<{sig: DassieActorContext, isAuthenticated: boolean}>()` (`APP/rpc-server/route-types/public.ts`).
- `protectedRoute = publicRoute.use(({context}) => context.isAuthenticated ? undefined : new RpcFailure("Unauthorized"))` (`protected.ts`).
- `isAuthenticated`:
  - WebSocket: a valid session cookie at upgrade, or the DEV token.
  - IPC: always `true`.
- `sig` is the actor context of the registering actor (`RegisterTrpcHttpUpgradeActor` or `ServeLocalIpcActor`). Subscriptions created through helpers attach listeners to that actor's scope.

### 10.3 Procedure table (`APP/rpc-server/app-router.ts`)

Wire path = `[router, procedure]`. Auth: P = public, A = protected (session cookie, DEV token, or IPC). "Errors" lists thrown errors; the client sees all of them as `"Internal server error"`.

| Path | Type | Auth | Input (zod) | Output | Behaviour / errors | Source |
|---|---|---|---|---|---|---|
| `general.getBasicState` | query | **P** | none | `{state:"uninitialized"}` \| `{state:"authenticated"\|"anonymous", activeSettlementSchemes: SettlementSchemeId[], nodeCount: number, hostname: string\|undefined, nodeId: NodeId, ilpAllocationScheme: "test"\|"g"}` | Comment: only public info may be returned. `nodeCount = NodeTableStore.size` | `APP/rpc-server/routers/general.ts` |
| `general.subscribeBalance` | subscription | A | none | stream of `bigint` (`TotalOwnerBalanceSignal`), initial value immediately | | same |
| `payment.resolvePaymentPointer` | query | A | `{paymentPointer: string}` | wallet `{id, publicName?, assetCode, assetScale 0..255, authServer, resourceServer}` | `$host/path` becomes `https://host/path`, and path `/` becomes `/.well-known/pay`. Server-side `fetch` (OQ: SSRF to arbitrary URLs by authenticated user); `json()` / zod failures throw | `APP/rpc-server/routers/payment.ts`, `APP/utils/resolve-payment-pointer.ts` |
| `payment.createPayment` | mutation | A | `{paymentId: string, paymentPointer: string, amount: string}` | `undefined` | `BigInt(amount)` (throws on non-integer); calls `MakePayment({id, destination, amount})`. TODO: validate paymentId length and uniqueness | same |
| `debug.getLedger` | query | A | none | `LedgerAccount[]` (`{path, debitsPending, debitsPosted, creditsPending, creditsPosted, limit}`) | All accounts (prefix `""`) | `APP/rpc-server/routers/debug.ts` |
| `debug.subscribeToLogs` | subscription | A | none | `{type:"initial", value:{logs: IndexedLogLine[], startTime}}` then `{type:"changes", value: ["addLogLine",[line]] \| ["clear",[]]}` | `subscribeToStore(LogsStore)` | same |
| `debug.subscribeRoutingTable` | subscription | A | none | `RoutingTableSignal` values (initial + changes) | | same |
| `debug.getContextKeys` | query | A | none | `ContextKeyTuple[]`: `[id, name, "actor", parentId]` \| `[id, name, "signal"\|"computed"\|"topic"\|"other"]` | Returns `[]` in production (debug tools DEV-only) | same |
| `debug.getSignalState` | query | A | `number` | `{value: string}` (pretty-format with ANSI highlight + hex typed arrays) | Throws `TypeError("Item is not a signal")` | same |
| `debug.subscribeToTopic` | subscription | A | `number` | stream of `string` (pretty-formatted messages) | Throws if not a topic | same |
| `debug.getDatabaseTables` | query | A | none | `{id: TableId, name: string, columns: string[]}[]` | | same |
| `debug.getDatabaseTableRows` | query | A | enum of table ids (`accounts, incomingPayment, outgoingPayment, settlementSchemes, nodes, peers, registrations, acmeTokens, sessions, btpTokens`) | rows incl. `rowid` | **Exposes session and BTP tokens** to any authenticated user (by design for an admin UI) | same |
| `debug.subscribeToLedgerAccount` | subscription | A | `string` (account path) | first `LedgerAccount`, then `Transfer` (`{state, debitAccount, creditAccount, amount, immediate}`) events from the posted, pending and voided topics where the account is debit or credit | Throws "Account does not exist" | same |
| `acme.setAcmeCredentials` | mutation | A | `{accountUrl: string, accountKey: string}` | `true` | Writes scalars | `APP/acme-certificate-manager/rpc-routers/acme.ts` |
| `acme.registerToken` | mutation | A | `{token: string, keyAuthorization: string, expires?: Date (z.coerce.date)}` | `undefined` | `insertOne`; throws on duplicate | same |
| `acme.deregisterToken` | mutation | A | `{token: string}` | `undefined` | delete by token | same |
| `tls.setNodeTlsConfiguration` | mutation | A | `{certificate: string, privateKey: string}` | `true` | `setTlsCertificates`. **No validation**: an invalid PEM breaks the HTTPS actor (OQ) | `APP/http-server/rpc-routers/tls-admin.ts` |
| `config.getConfig` | query | A | none | `{hostname: string\|undefined, hasNodeIdentity: false, hasTls: boolean}` \| `{hostname, hasNodeIdentity: true, nodeId: NodeId, hasTls}` | | `APP/config/rpc-routers/config-admin.ts` |
| `config.getSetupUrl` | query | A | none | `string` | `SetupUrlSignal` | same |
| `config.setRealm` | mutation | A | `{realm: "test"\|"live"}` | `true` | `live` → throws "Livenet is not yet supported" | same |
| `config.setHostname` | mutation | A | `{hostname: string}` | `true` | No validation; `url` not recomputed (OQ) | same |
| `apiKeys.getCurrentKeys` | query | A | none | `string[]` | | `APP/api-keys/rpc-routers/api-keys.ts` |
| `apiKeys.addBtpToken` | mutation | A | none | `string` (22 chars) | | same |
| `apiKeys.removeBtpToken` | mutation | A | `string` length 22 | `undefined` | | same |
| `registrationClient.subscribeRegistrationStatus` | subscription | A | none | `{nodes:{nodeId, hostname?: string, registered: boolean}[], registrationRatio: number, threshold: 0.8}` | | `APP/registration-client/rpc-router/registration-client.ts` |
| `registrationClient.registerWithNode` | mutation | A | `{nodeId: string}` | `undefined` | Throws if the node is not in the bootstrap lists ("Node with ID … not found in bootstrap lists"), if already registered ("We are already registered with node …"), or if the node is not in the node table | same |
| `network.getNodes` | query | A | none | node table entries (array) | See [3 §9](03-peer-network.md) | `APP/peer-protocol/rpc-routers/network.ts` |
| `network.subscribeToNodeTableStore` | subscription | A | none | store messages (`initial` / `changes`) | | same |
| `ledgers.getList` | query | A | none | `{id: SettlementSchemeId, balance: bigint}[]` | balance via the settlement actor `getBalance.ask()`, or 0n if inactive | `APP/ledgers/rpc-routers/ledgers.ts` |
| `ledgers.checkLedgerDeletePrerequisites` | query | A | `string` (ledger id) | `{isActive: boolean, isUnused: boolean}` | `isUnused` = no peer in state `peered` using that scheme | same |
| `ledgers.addSettlementScheme` | mutation | A | `{id: string, config: {}}` | `undefined` | OQ: `z.object({})` strips all keys, so the config is always `{}` | same |
| `ledgers.removeSettlementScheme` | mutation | A | `{id: string}` | `undefined` | | same |
| `ledgers.stubDeposit` | mutation | A | `string` (amount) | `true` | Throws "Stub settlement scheme is not enabled" | same |

---

## 11. Local IPC server and client

### 11.1 Server

- `APP/local-ipc-server/index.ts` runs `LocalRpcServerActor`, which runs `ServeLocalIpcActor` (`actors/serve-local-ipc.ts`). That actor creates `createServer({router: appRouter, transformer: superjson})` and returns the API `{handleConnection(connection)}`, which logs `debug("received ipc rpc server connection")` and handles it with context `{sig, isAuthenticated: true}`.
- The socket listeners (`command-line/commands/daemon/*/serve-ipc.ts`) wrap each `net.Socket` with `createNodejsSocketAdapter`:
  - utf8 encoding
  - newline framing
  - on `error`: `warn("inbound socket error; closing connection")` and `end()`
  - `closed` resolves on `close`
  - Each connection is passed through `serveLocalIpcActor.api.handleConnection.tell(adapter)`, so connections wait for the actor to be ready.
- Same RPC procedures as WebSocket (§10.3), and **all authenticated**. Socket file permissions are the only access control.
- nodejs runtime: `unlinkSync(path)` first (ignores ENOENT, rethrows other errors). Then `listen(path)`. No mode or ownership is set.
- systemd runtime: the socket comes from `dassie-ipc.socket` (`0660`, `root:dassie-users`).

### 11.2 Client: `APP/local-ipc-client/index.ts`

- `connectIpcClient(context)`: `net.createConnection(process.env.DASSIE_IPC_SOCKET_PATH ?? "/run/dassie.sock")`, wraps it with `createNodejsSocketLink`, and calls `createClient<AppRouter>({transformer: superjson})`. It registers `context.scope.onCleanup(() => socket.end())` and returns `client.rpc`.
- OQ: no `error` listener is attached to the socket. If the daemon is not running (ENOENT/ECONNREFUSED) or the user lacks permission (EACCES), Node raises an unhandled `'error'` event and the CLI crashes with a stack trace instead of a friendly message.

---

## 12. CLI reference (`dassie`, `APP/command-line/`)

Parser: `cmd-ts` `binary(subcommands({name:"dassie", cmds}))`. It provides `--help`, plus `--version` if cmd-ts supplies it (the version is not set explicitly).

### 12.1 `dassie daemon [--runtime <nodejs|nodejs-systemd>]`

- "This command starts the Dassie daemon in the foreground. It is intended to be used by a process manager like systemd."
- Default runtime: `nodejs`. Behaviour as in §2.

### 12.2 `dassie init`

- "This command assists with the initial configuration and setup." No arguments. Interactive TTY wizard (§4.5).
- Requires the daemon to be running and read/write access to the IPC socket.

### 12.3 `dassie verify-install`

- "Verify that Dassie and its dependencies are correctly installed."
- Creates a temporary reactor, resolves the native binding path (PROD: `<DASSIE_ROOT>/lib/better_sqlite3.node`), and opens an **in-memory** DB (`:memory:`) with the full schema. That sets the application id and runs all migrations, but does **not** check the schema. It then runs `SELECT 1 + 1;`.
- Prints "Verifying that SQLite is working..." and "Success! Dassie appears be to installed correctly." Any failure throws (non-zero exit).

### 12.4 `dassie update [-f|--force] [-t|--target-version <ver>]`

"Update Dassie to the latest version or a specific target version."

1. `rootPath` must end with `/current` or `/<__DASSIE_VERSION__>`. Otherwise throw: `DASSIE_ROOT must end with "/current" or "/<ver>" in order for the auto-updater to work (DASSIE_ROOT=…)`.
   - OQ: canary bundles live in `/opt/dassie/canary` while the version is `canary-<date>-<sha>`, and the launcher resolves symlinks, so canary installs always fail here.
2. `superRoot = dirname(rootPath)`, `current = superRoot/current`, `old = superRoot/<__DASSIE_VERSION__>`.
3. Permission check: `access(W_OK)` on rootPath, superRoot, current and old. EACCES → error note "Insufficient permissions / Please run this command as root or with sudo" and return (exit 0).
4. `mirror = DASSIE_MIRROR_URL ?? "https://get.dassie.land"`. If there is no target, `GET ${mirror}/meta/latest`, read the text and use it as the target. The response status is not checked and the text is not trimmed (OQ).
5. If `!force && target === __DASSIE_VERSION__`: note "Already up to date (<v>)" and return.
6. `mkdir <tmp>/dassie-update-<ms>-<pid>`. Download `${mirror}/${target}/dassie-${target}-linux-${process.arch}.tar.gz`. **The status is not checked**, and there is **no checksum or signature verification** (OQ). The installer uses `.tar.xz`, the updater `.tar.gz`, and the build publishes both (`SUPPORTED_COMPRESSIONS = ["gz","xz"]`).
7. `tar -xzf <bundle> -C <tmp>`. (The "Extracting" note is constructed but never shown, a bug.) Then `cp -r <tmp>/dassie → superRoot/<target>`.
8. If `old != new`: `rm -rf old`.
9. `rm current`, then `ln -s new current`. Not atomic (OQ).
10. `systemctl restart dassie`. Success note "Successfully installed <v>".
11. `finally`: `rm -rf <tmp>`.

Scheduled daily by `dassie-update.timer` (§3.1).

### 12.5 Installer (`install.sh`)

See §3.1. It is distributed as `curl --tlsv1.2 -sSf https://sh.dassie.land | sh [-s -- options]`.

Docs `docs/implementation-notes/install-command.md` (design rationale):
- Don't follow redirects, and enforce TLS 1.2+.
- Wrap the script in `main()` so truncation is harmless.
- Self-checksum idea, server-side variants (trade-off with auditability), and ShellCheck.
- Signature verification with keys fetched from multiple sources and a temporary GPG keyring, `SHASUMS256.txt` + `.sig` (not implemented).
- Colour detection heuristic: FORCE_COLOR > NO_COLOR > TTY && TERM≠dumb.

---

## 13. Logger: `APP/logger/` + `packages/lib-logger`

### 13.1 Namespaces (`APP/logger/namespaces.ts`, instances in `instances.ts`)

`das:node:accounting`, `das:node:acme`, `das:node:btp`, `das:node:connector`, `das:node:crypto`, `das:node:daemon`, `das:node:database`, `das:node:exchange`, `das:node:http`, `das:node:ildcp`, `das:node:ilp-http`, `das:node:ipc`, `das:node:payment`, `das:node:peer-protocol`, `das:node:registration-client`, `das:node:settlement`, `das:node:settlement-stub`, `das:node:settlement-xrpl`, `das:node:systemd`.

lib-sqlite defaults: `das:sqlite:database` and `das:sqlite:migrate` (the app passes its own logger to the DB). Captured console output uses the namespace `console`.

### 13.2 Pipeline

- `AttachLogger(reactor)()` (`functions/attach-logger.ts`):
  1. `captureConsole()`: replaces `console.debug/info/warn/error`. `console.debug` is dropped unless `DEBUG` enables `console`. The first string argument becomes the message. `console.clear` emits a `clear` event. **`console.log` is not captured.**
  2. Replaces the global log context `output` with the following:
     - `clear` events → `LogsStore.clear()` (not printed).
     - Other events → `LogsStore.addLogLine(event)`. Then, if `StdoutLogLevel != "none"` and `level ≥ StdoutLogLevel` (debug 0 < info 1 < warn 2 < error 3), write `cliFormatter(event) + "\n"` to stdout.
- `StdoutLogLevel` is a signal initialised from `DASSIE_LOG_LEVEL` (default `info`). There is no RPC to change it at runtime.
- `logger.debug` is a getter that returns a function only if `DEBUG` matches the logger namespace; otherwise it returns `undefined`. Call sites use `logger.debug?.(…)`.
  - DEBUG pattern rules (`createEnableChecker`): split on comma or whitespace; `*` becomes `.*?`; a leading `-` negates; first match wins; full-string anchored. OQ: only the **first** `*` in each pattern is replaced.
- `LogMessage = {type: "debug"|"info"|"warn"|"error", namespace, date (ms epoch), message, parameters: unknown[], caller?: string}`. `caller` is captured only in DEV.
- **LogsStore** (`stores/logs.ts`):
  - State `{logs: IndexedLogLine[], startTime: number}`. Each line adds `index` (previous index + 1, starting at 0) and `relativeTime = date - startTime`.
  - `LOGS_SOFT_LIMIT = 100`, `LOGS_HARD_LIMIT = 120`. When the length exceeds 120, trim to the newest 100.
  - Exposed via `debug.subscribeToLogs`.
- CLI format (`lib-logger/src/formatters/cli-formatter.ts`):
  - `"  " + namespace (colour chosen by hashing the namespace over red, green, yellow, blue, magenta, cyan) + " " + level prefix (debug grey, info blue, warn yellow, error red, bold) + message + pretty-printed parameters`.
  - If the first parameter has an `error` property that is an Error, that error is extracted and printed as a formatted stack after a blank line.
  - **No timestamp** (journald provides one). `clear` becomes the ANSI clear-screen sequence.
- A JSON formatter exists (`createJsonFormatter`: `date` as ISO string), but the app does not use it.
- `assert(logger, cond, msg)`: on failure it logs `error("unmet assertion: msg")` and throws `AssertionError`.

---

## 14. Public API server: `APP/public-api-server/`

Started only once the node identity exists (§2.4).

- `GET /stats` → 200 `application/json`:
  ```json
  {
    "version": "<__DASSIE_VERSION__>",
    "node": { "id": "<NodeId>", "url": "<config.url>", "publicKey": "<base64url Ed25519 pub>" },
    "peers": [ { "nodeId": "<id>", "url": "<linkState.url>|null", "publicKey": "<base64url>|null" } ]
  }
  ```
  - Peers come from `PeersSignal` keys looked up in `NodeTableStore`. Unknown nodes are filtered out.
  - No CORS. No auth: it publicly reveals the version and peer list.
- `GET /nodelist/meta` → `{"hash": base64url(NodeListHashSignal)}`. Used by other nodes polling bootstrap node lists ([3 §11](03-peer-network.md)).

---

## 15. Utils: `APP/utils/`

- `compare-sets.ts`:
  - `compareSetOfKeys(Map, Map)`, `compareKeysToArray(Map, string[])`, `compareSetToArray(Set, string[])`, `compareSets(Set, Set)`: compare sizes, then membership.
  - `compareArrays(a, b)`: compares lengths, then `includes` for each element of b. Order-insensitive but not multiset-exact (OQ).
- `compare-typedarray.ts`: `compareUint8Arrays`, byte-wise equality (not constant-time).
- `pretty-format.ts`: `prettyFormat(value)` = `pretty-format` with `highlight: true` and the `formatTypedArrayPlugin` from lib-format-utils. Used by the debug RPC.
- `resolve-payment-pointer.ts`:
  - `paymentPointerToUrl(pp)`: replace a leading `$` with `https://`; a path of `/` becomes `/.well-known/pay`.
  - `resolvePaymentPointer(pp)`: fetch, `json()`, then `walletSchema.parse`.
  - `spspResultSchema = {destination_account: string, shared_secret: string}`, which is not used here.

---

## 16. Security model (consolidated)

| Aspect | Current behaviour | Notes / OQ |
|---|---|---|
| Admin auth (web) | Session cookie `__Host-Dassie-Session` (HttpOnly, Secure, SameSite=Strict, Path=/, Max-Age 10y) checked at WS `/rpc` upgrade | Sessions never expire server-side; no listing/revocation UI; WS auth not re-checked after logout |
| Admin auth (local) | IPC socket: every connection fully authenticated | Relies on socket file permissions (systemd `0660 root:dassie-users`; nodejs runtime: umask only) |
| Login credential | `hex(HMAC-SHA256(nodeKey,"login"))` sent in the clear inside TLS; non-constant-time compare | No rate limiting |
| Setup | One-time 256-bit random token (in-memory), printed to logs and via `dassie init`; route exists only while uninitialized | The token appears in journald logs (readable by admins). Anyone holding it can claim the node until setup completes |
| Key custody | Root seed generated client-side (BIP39). Node private key sent to the server once at setup and stored as PEM in SQLite in plaintext | `frontend-security.md` describes the trust gap |
| CSRF | POST endpoints `/api/login`, `/api/logout`, `/api/setup` accept JSON without a Content-Type check; protection relies on SameSite=Strict and the secrets in the body | Recommend requiring `Content-Type: application/json` and/or an Origin check |
| CSWSH | No Origin check on `/rpc`; SameSite=Strict mitigates | Recommend an Origin allowlist |
| CSP & security headers | **None** (no CSP, HSTS, X-Frame-Options, nosniff). `index.html` contains an inline `<script>` (dark-mode toggle) | `frontend-security.md` recommends CSP |
| CORS | `*` on `/.well-known/pay` and `/incoming-payments` only | `/incoming-payments` GET/POST are unauthenticated (see [4 §9.3](04-interledger.md)) |
| TLS | ACME (LE) cert or ephemeral self-signed; Node default TLS settings | No HSTS; HTTP→HTTPS redirect broken host (`0.0.0.0`) |
| Debug RPC | Exposes DB rows incl. session and BTP tokens, logs, and reactor internals to authenticated admins | Acceptable for single-admin model |
| SSRF | `payment.resolvePaymentPointer` fetches arbitrary URLs server-side (authenticated only) | |
| Secrets at rest | Node key, TLS key, ACME account key, session tokens, BTP tokens: plaintext in `dassie.sqlite3` | DB file mode not enforced |
| Process | systemd `DynamicUser`, `NoNewPrivileges`, socket activation for privileged ports | Update runs as root daily, without signature verification |
| Body limits | 2 MiB per request body | No connection, rate or concurrency limits |

---

## 17. Constants (with values)

| Constant | Value | File |
|---|---|---|
| `APP_NAME` | `"dassie-node"` | `APP/constants/general.ts` |
| `VALID_REALMS` | `["test","live"]` | same |
| `EMPTY_UINT8ARRAY` | `new Uint8Array(0)` | same |
| `DEV_SECURITY_TOKEN_LENGTH` | `64` | same |
| `SESSION_COOKIE_NAME` | `"__Host-Dassie-Session"` | `APP/constants/cookie-name.ts` |
| `QUERY_PARAMETER_DEVELOPMENT_SESSION` | `"_DASSIE_DEV_SESSION"` (UI query param consumed by the frontend to call `/api/_dev/set-session`) | `APP/constants/ui-query-parameter-names.ts` |
| `SEED_PATH_NODE` / `_NODE_LOGIN` / `_DEV_SESSION` / `_SETTLEMENT_MODULE` | `"node"`, `"node/login"`, `"dev/session"`, `"node/settlement-module"` | `APP/constants/seed-paths.ts` |
| `dassieObjectIdentifierPrefix` | `1.3.6.1.4.1.54392.5.1399` | `APP/constants/object-identifiers.ts` |
| `dassiePeerProtocolId` | `…1399.1` | same |
| `dassiePeerSignedHelloId` | `…1399.1.1` | same |
| `dassiePeerNodeInfoId` | `…1399.1.2` | same |
| `dassieNodeFingerprintId` | `…1399.2` | same |
| `dassieNodeFingerprintV1Id` | `…1399.2.1` | same |
| `DEFAULT_BOOTSTRAP_NODES` | `[{id:"dNKbwOMCw0DTToPM4tyyVASWxM4NP3MKW", url:"https://test1.dassie.xyz", publicKey:"QBe5HLbdYFtIpjFTIu_Lz8lkoUJ3fmAvFHfCb9CVwIM"}, {id:"dyrjUANXiJ5S6JXxwo7jdKKJjdLtK129U", url:"https://test2.dassie.xyz", publicKey:"tmS1PB42DEjYi-Ua4RNguhweId9YjT9Kml3uq3iZSuQ"}]` | `APP/constants/bootstrap-nodes.ts` |
| `COOKIE_MAX_AGE_SECONDS` | `315360000` | `APP/authentication/constants/cookie-lifetime.ts` |
| `ACME_DIRECTORY_URL` | LE production (`https://acme-v02.api.letsencrypt.org/directory`) or, in DEV, staging (`https://acme-staging-v02.api.letsencrypt.org/directory`) | `APP/acme-certificate-manager/constants/acme-service.ts` |
| `ACME_EXPIRY_POLLING_INTERVAL` | `3600000` ms | `update-acme-certificate.ts` |
| `MINIMUM_CERTIFICATE_EXPIRY` | `2592000000` ms (30 d) | same |
| `LETSENCRYPT_TOS_URL` | `https://letsencrypt.org/documents/LE-SA-v1.3-September-21-2022.pdf` | `command-line/commands/init/index.ts` |
| `DASSIE_SQLITE_APPLICATION_ID` | `0x1DA53B81` | `APP/database/schema.ts` |
| DB file name | `dassie.sqlite3` | `APP/database/open-database.ts` |
| Default IPC socket | `/run/dassie.sock` | env config / IPC client |
| Default HTTP/HTTPS ports | `80` / `443` | `APP/config/database-config.ts` |
| Default exchange rate URL | `https://api.coinbase.com/v2/exchange-rates` | same |
| Default realm | `test` | same |
| Frontend MIME map / asset regex | see §6.4 | `serve-frontend.ts` |
| `LOGS_SOFT_LIMIT` / `LOGS_HARD_LIMIT` | `100` / `120` | `APP/logger/stores/logs.ts` |
| `SD_LISTEN_FDS_START` | `3` | `socket-activation.ts` |
| Socket-activation names | `dassie-http.socket`, `dassie-https.socket`, `dassie-ipc.socket` | `nodejs-systemd/serve-*.ts` |
| BTP token length | 16 bytes → 22 base64url chars | `APP/api-keys/rpc-routers/api-keys.ts` |
| Session token length | 32 bytes → 64 hex chars | `login.ts`, `setup.ts` |
| Setup token length | 32 bytes → 64 hex chars | `setup-authorization-token.ts` |
| `DEFAULT_MAX_BODY_SIZE` | `2097152` | `packages/lib-http-server/src/constants.ts` |
| `MAX_SUBSCRIPTION_BATCH_SIZE` | `100` | `packages/lib-rpc/src/server/server.ts` |
| RPC protocol id | `"dassie-rpc-01"` | `packages/lib-rpc/src/common/envelope.ts` |
| WS reconnect backoff | min 1000 ms, max 30000 ms, factor 2, jitter 0.5, immediate first | `lib-rpc/src/common/utils/exponential-backoff.ts` |
| Self-signed cert | RSA 2048, 10 years, serial 20 bytes, KeyId 20 bytes | `APP/http-server/functions/fallback-to-self-signed.ts`, `lib-x509/src/self-signed.ts` |
| Registration threshold | `4/5` | `APP/registration-client/constants/threshold.ts` |
| Mirror | `https://get.dassie.land` (`/meta/latest`, `/<ver>/dassie-<ver>-linux-<arch>.tar.{gz,xz}`) | update command, installer |

---

## 18. Supporting libraries

### 18.1 `lib-reactive`: actor / signal / topic / reactor model (core semantics)

Source: `packages/lib-reactive/src/*`, README `packages/lib-reactive/README.md`. Everything in the app is built on it, so these semantics must be reproduced or mapped deliberately.

**Reactor (DI container + root scope).** `createReactor(rootActorFactory?, base?)` (`reactor.ts`):
- State: a `WeakMap<Factory, instance>`, a `base` object and a root `DisposableScope`.
- `use(factory, {stateless?})`:
  - The first call runs `factory(reactor)` and caches the result (unless `stateless`). The function identity is the key.
  - In DEV, the result is tagged with `FactoryNameSymbol = factory.name`, which is used as the debug name and the actor path.
  - Lifecycle hooks: `InitSymbol` is called after creation, `UseSymbol` on every `use`, `DisposeSymbol` on `delete`.
- `peek(factory)` reads without creating. `set(factory, value)` overrides (mocking). `delete(factory)` disposes and removes.
- `withBase(newBase)` returns a reactor *view* that shares the context state and scope but has the merged base. It is used to inject `{clock, crypto}` and `socketActivationState`.
- `dispose()` disposes the root scope.
- If a root actor is given, it is run immediately. Errors are logged as "error running root actor".
- Factories are lazy singletons per reactor. Several reactors can coexist in one process (used by tests and the multi-node simulation).

**Scope.** `scope.ts`:
- `onCleanup(fn)`: handlers are run in **LIFO** order and awaited sequentially on `dispose()`. A handler registered after disposal runs immediately.
- `offCleanup(fn)`.
- `isDisposed`.
- `abortSignal`: aborted on dispose.
- `confineScope(inner, outer)`: the inner scope is disposed when the outer one is.

**Actor.** `actor.ts`, `actor-context.ts`:
- `createActor(behavior)`: `behavior(sig: ActorContext) → result | Promise<result>`. The result may be an object of methods (the actor API).
- States: Stopped 0, Starting 1, Started 2, Stopping 3.
- `actor.run(parentContext, {pathPrefix?, ignoreRunning?})` starts the loop. It throws `actor is already running: <name>` if the actor is already running, unless `ignoreRunning` is set.
- Loop:
  1. Create a child scope confined to the parent scope, plus a `Cancellable`.
  2. Create a context. Call the behaviour; store `result` synchronously, then await it.
  3. If the value changed, mark observers dirty. Resolve `actor.promise`.
  4. Drop dependencies that were not re-read.
  5. Revalidate, then sleep until woken.
  6. On wake (a dependency is dirty, or `forceRestart`): cancel the cancellable and dispose the scope, which runs all cleanups, including **child actors** registered with `sig.run`, in reverse order. Then re-run the behaviour.
  7. The loop exits when the parent scope is disposed.
- **Behaviour errors**: logged as `"error in actor" {actor, path, error}` and the actor **stops permanently**. There is no restart or supervision. OQ: the state is left at Stopping.
- Actors are also readonly signals: `read()` returns the last resolved result, so other actors and computeds can `readAndTrack` an actor (e.g. waiting for a service actor's value).
- `actor.api.<method>.tell(...args)` is fire-and-forget; errors are logged as "error in actor handler". `.ask(...args)` returns a Promise.
  - Both wait for `actor.promise`, the current or next completed run, and then invoke `result[method](...)`. A missing method throws `Actor does not expose method: X`.
  - In DEV, a call to an actor that is never instantiated logs "Actor called but never instantiated" after 2 s.
- `forceRestart()` marks the actor dirty, cancels it and wakes it. At most one restart is buffered.

**ActorContext (`sig`)** members:
- `reactor`, `base`, `name`, `path` (dot-joined actor names).
- `scope` and the shortcuts `onCleanup`, `offCleanup`, `isDisposed`.
- `abortSignal` (per run), `cancellable`.
- `readAndTrack(signalOrFactory, selector?, comparator?)`: a tracked read. With a selector, an intermediate `ReactiveSelector` is created so that the actor restarts only when the selection changes under the comparator.
- `readKeysAndTrack(signal, keys)`: a tracked shallow pick of keys; restarts only when one of those keys changes (by `===`).
- `read(signal)`: untracked.
- `subscribe(topic)`: restart on the next message.
- `on(topic|signal, listener)` / `once(...)`: auto-unsubscribed at scope cleanup. A signal's `values` topic is used for signals.
- `interval(cb, ms)`, `timeout(cb, ms)`: auto-cleared. The callbacks are wrapped so that errors are logged and calls after disposal are ignored (DEV warns about dangling callbacks).
- `task(descriptor)`: creates a `ScheduledTask` (see below) using `base.clock`.
- `run(factoryOrActor, options?)`: runs a child actor with the current context as parent and path prefix `<path>.`. Returns the first result synchronously (a promise if the behaviour is async), so `await sig.run(X)` waits for X's first run.
- `runMap(mapped)`: runs one actor per entry of a `Mapped` collection, plus newly added entries. Each item's scope is confined to both the map item scope and the current actor.
- `runMapSequential(mapped)`: the same, but awaited one after another.
- `forceRestart()`.
- `withBase(base)`: returns a context with an augmented reactor base.
- `callback(fn)`: wraps the function.

**Signals.** `signal.ts`, `internal/reactive.ts`:
- `createSignal(initial, {comparator?})`: `read()`, `write(v)`, `update(reducer)`.
- A write with an unequal value (default `===`) marks observers `Dirty`.
- `values` is a topic of new values, implemented with a lazily enabled reactive proxy. The proxy is an "effect" scheduled through `queueMicrotask`, so a burst of writes in one tick emits only the latest value.

**Computed.** `computed.ts`:
- `createComputed(parentContext /*usually reactor*/, (sig: ComputationContext) => value, {comparator?, hasSideEffects?})`.
- Lazy push-pull with three cache states: Clean, Check, Dirty.
  - A source change marks direct observers Dirty and indirect observers Check.
  - `read()` recomputes only when needed. In Check state it re-reads its sources to find out whether any actually changed.
  - Dependencies are re-collected on every recompute.
- A computed becomes eager when it is observed by an actor or a topic listener.
- Its scope is confined to the parent context, which in the app is the reactor, so it lives for the process lifetime.
- Its context provides `readAndTrack`, `readKeysAndTrack`, `read`, `reactor` and scope shortcuts.
- `createEffect(ctx, fn)` is a computed with `hasSideEffects`; it re-runs in a microtask after becoming stale.
- Effects are queued in a **process-global** static queue.

**Store.** `store.ts`:
- `createStore(initial).actions({name: (...params) => (state) => newState})` returns a Signal plus `act.<name>(...params)` (apply the reducer, then emit a `changes` message `[name, params]`, then return the new state).
- `actions()` returns a **new** store with merged actions; the app always chains it immediately.
- The app uses immer `produce` reducers; side effects such as DB writes are performed inside the reducer.
- `watchStoreChanges(scope, store, handlers)`.
- Stores exist to replicate state: `subscribeToStore` sends `initial` and then `changes`, and the client replays the actions on a local copy (`useRemoteStore`). The replicated actions must be deterministic and must not depend on server-only state on the client side.

**Topic.** `topic.ts`:
- `createTopic<T>()`: `emit(msg)`, `on(scopeContext, listener)` (auto-off on scope cleanup), `once`, `off`, `next()`, and an async iterator.
- Listener exceptions, including rejected promises, are caught and logged ("error in listener" / "error in async listener"). Emission is synchronous.

**Mapped.** `mapped.ts`:
- `createMapped(ctx, setSignal, (key, scope) => value)`: a reactive map derived from a `Set` signal.
- When a key is added, the value is created with its own scope and emitted on `additions`. When a key is removed, its scope is disposed. `get`, `has`, `size`, iteration.

**ScheduledTask.** `internal/scheduled-task.ts`:
- `{handler({abortSignal}), interval ms, jitter? ms (random 0..jitter added), overlapPolicy?}`.
- Overlap policies:
  - `skip` (default): drop a tick while a previous run is active.
  - `bufferOne`, `bufferAll`.
  - `cancelOther`: abort the previous run.
  - `allowAll`.
- `execute()` runs now. Errors are logged as "error in scheduled task".
- `schedule()` arms the timer, which re-arms itself each tick.
- `cancel()`, `cancelAndAbort()`.
- Cleanup is auto-registered on the scope.

**Cancellation**: `createCancellable()` → `{cancel(reason), isCancelled, cancellation, abortSignal, then}`.

**Base module interfaces:**
- `Clock {now(), setTimeout(cb, ms), clearTimeout(id)}`.
- `Crypto`:
  - `getRandomBytes(n)`
  - `hash("sha256"|"sha512", msg)`
  - `createMac("hmac-sha256"|"hmac-sha512", key)`
  - `createCryptor("aes-128-gcm"|"aes-256-gcm", key)` → `{encrypt → {iv(12), tag(16), ciphertext}, decrypt → plaintext | DECRYPTION_FAILURE}`
  - `generateRsaKeyPair(bits)`, `importRsaKeyPair(pem|der pkcs8)` → `{getPublicKeyPem (spki), getPrivateKeyPem (pkcs8), sign/verify RSA-SHA256}`

**Tools:**
- `delay`, `delayWithAbortSignal`.
- Random helpers: `randomBigInt`, `sampleBoolean`, `randomNumber`, Gaussian and log-normal sampling.
- Mocks: `createMockClock`, `createMockDeterministicCrypto` (xoshiro128++, seeded via murmurhash3), `createTimeDilationClock` (simulation).
- `createAbstract()` throws if a value was not injected.

**Debug tools:** DEV only (`createDebugTools` returns `undefined` in PROD). They provide a context registry of `WeakRef`s with unique ids, an actor-parent map and dangling-callback warnings. They back `debug.getContextKeys`, `getSignalState` and `subscribeToTopic`.

Reimplementation guidance (Rust): model the system as a supervised task tree.
- Each actor is a task with a scope (`CancellationToken` + LIFO cleanup stack).
- Signals and stores are versioned cells with change notification (e.g. `tokio::sync::watch`).
- Restart-on-dependency-change becomes a select over the watched cells' `changed()` futures.
- Stores additionally broadcast `(action, params)` for replication.
- Preserve the start ordering of §2.4, the at-most-once-buffered restart, and the "error stops the actor" semantics, or deliberately replace them with supervision.

### 18.2 `lib-reactive-rpc`: `packages/lib-reactive-rpc/src`

- Server helpers (`server/index.ts`). Each returns a lib-rpc `Subscription`:
  - `subscribeToTopic(sig, topic)`: forwards each message.
  - `subscribeToSignal(sig, signal)`: emits the current value immediately, then each new value (via `signal.values`, de-duplicated per tick).
  - `subscribeToStore(sig, store)`: emits `{type:"initial", value: state}`, then `{type:"changes", value:[action, params]}` per action.
- Client (React; to be replaced by Solid):
  - `createReactiveHooks()` → `Provider`, `useReactor`, `useSignal` (useSyncExternalStore), `useRemoteSignal(rpcSubscriptionHook)` and `useRemoteStore(rpcSubscriptionHook, StoreFactory)`, which replays `initial`/`changes` into a local store instance and throws if a replicated action is unknown locally.
  - `createRpcReact<AppRouter>()` → `{rpc (hooks per route: useQuery/useMutation/useSubscription over @tanstack/react-query), RpcProvider, useWebSocketClient}`. Subscriptions are reference-counted per key, with a 1000 ms disposal delay.

### 18.3 `lib-reactive-io`

- `createRuntime()`: Node gives `{clock: GenericClock (Date.now / setTimeout), crypto: node:crypto-based Crypto}` (`src/node/*`). The browser variant uses WebCrypto (`src/browser/crypto.ts`) for SHA-256/512 digest, HMAC and AES-GCM.
- OQ: the browser RSA support is inconsistent:
  - Keys are generated and imported as `RSA-OAEP` but used with `RSASSA-PKCS1-v1_5` sign/verify.
  - `get*KeyPem` returns raw DER bytes decoded as a string, not PEM.
  - Importing PEM throws.
  - It is not used by the daemon.
- Purpose: abstract I/O for portability and deterministic tests.

### 18.4 `lib-rpc`

See §10.1 for the protocol. Exports:
- server: `createServer`, `createRoute`, `createRouter`, `RpcFailure`, `RpcSuccess`, `createSubscription`, and adapters `createWebSocketAdapter`, `createNodejsSocketAdapter`, `createNodejsMessagePortAdapter`.
- client: `createClient`, `createWebSocketLink`, `createNodejsSocketLink`, `createNodejsMessagePortLink`.

The MessagePort adapter and link are used for worker/dev tooling.

### 18.5 `lib-http-server`

See §6.2. Exports:
- routers: `createRouter`, `createWebSocketRouter`, `createRouteMatcher`
- `createNodejsHttpHandlers({hostname?, onRequest, onUpgrade?, onError?})` → `{handleRequest, handleUpgrade, handleError}`
- failures (see §6.2), responses (JSON/plain/binary/redirect)
- middlewares: `parseBody*`, `parseQueryParameters`, `cookie`, `cors`
- asserts: Accept, Content-Type
- `setCookie` / `clearCookie`, `handleError`, `DEFAULT_MAX_BODY_SIZE`

Routes are scope-bound, so they appear and disappear dynamically with actors.

### 18.6 `lib-sqlite`

See §5.1. Schema DSL:
- `table({name, columns, withoutRowid?})`
- `column().type("TEXT"|"INTEGER"|"REAL"|"BLOB"|"ANY").notNull().primaryKey().default(v).typescriptType<T>()`. Calling `type` after `default` throws.
- `scalar().name("ns.key").type(T).serialize(fn).deserialize(fn).defaultValue(v)`

TS type mapping: TEXT→string, INTEGER→bigint, REAL→number, BLOB→Buffer, ANY→unknown.

### 18.7 `lib-logger`

See §13.
- Global log context stored at `globalThis[Symbol.for("dassie.logger.context")]` (shared across bundles): `{captureCaller, enableChecker, output, getCaller, emit, realConsole}`.
- `createLogger(component)` rejects names that contain spaces.
- Node entry: sets `enableChecker` from `DEBUG` and the default output (`console.log(cliFormatter)`). The default output is overridden by `AttachLogger`.
- Browser entry: a separate console formatter.

### 18.8 `lib-terminal-graphics`

`createFlow({inputStream=stdin, outputStream=stdout, theme})`:
- `show(staticComponent|string)`
- `interact(interactiveComponent)`:
  - Raw-mode key reader. Ctrl+C returns the `Canceled` symbol; check it with `isCanceled`.
  - Ctrl+Z suspends (SIGTSTP) and redraws on SIGCONT. Known bug: the user has to press Ctrl+Z twice.
  - Hides the cursor while active.
- `attach(dynamicComponent, activity)`: a live-updating component driven by a signal and/or refresh interval.
- `emptyLine(n)`
- Output uses minimal TTY redraw (common-prefix diff and cursor moves). A non-TTY output writes sequentially.

Components:
- `header({title})`
- `note({title, body?, style: info|warning|error|success, paddingTop=1, paddingBottom=0})`
- `text({title, explanation?, initialValue?})` → string
- `confirm({title, initialValue=false})` → boolean (Y/N)
- `select`, `progress`, `tasklist`

Theme: icons with ASCII fallbacks; step colours info cyan "i", warning yellow, error red "!", success green "✓".

Used by `init` and `update` (and the build tool).

### 18.9 `lib-x509`

- `generateSelfSignedCertificate({clock, crypto, subject, modulusLength=2048})` → `{certificate: PEM "CERTIFICATE", privateKey: PEM PKCS#8}`. The TBSCertificate is hand-encoded in DER:
  - version v3
  - serial = 20 random bytes
  - sha256WithRSAEncryption
  - issuer = subject: C (PrintableString), then ST, L, O, OU, CN (UTF8String)
  - validity = now → now + 10 × 365 days
  - SPKI from the RSA public key
  - extensions: SubjectKeyIdentifier and AuthorityKeyIdentifier = first 20 bytes of SHA-256(SPKI DER); BasicConstraints critical, CA:TRUE
  - signed RSA-SHA256
  - OQ:
    - The serial is not forced positive (DER INTEGER with the high bit set would be negative).
    - Validity years ≥ 2050 are encoded with UTCTime tag 0x17 but a 4-digit year (should be GeneralizedTime 0x18).
    - DER lengths above 65535 are unsupported.
- `parseEd25519Key(pem, "private"|"public")`:
  - Rejects encrypted keys.
  - Checks the tag (`PRIVATE KEY`/`PUBLIC KEY`), the DER length (48/44) and the fixed preamble (`302E020100300506032B657004220420` / `302A300506032B65700321`).
  - Returns the raw 32 bytes.
- `serializeEd25519Key(raw, type)`: the inverse.
- `parsePem` / `serializePem`: 64-column base64. The parser requires `\n` line endings, so CRLF PEMs fail (OQ).

### 18.10 `lib-type-utils`

- `Failure` abstract class marked with `"dassie.failure": true` (realm-safe), `isFailure`, `findFailure`, `unwrapFailure`.
- `isError`, `isErrorWithCode(err, code)` (errno codes).
- `isObject`, `isThenable`, `assertDefined`, `arrayEquals`.
- `bigIntGcd`, `bigIntMin` / `bigIntMax`, `bufferToUint8Array`, `UnreachableCaseError`.
- `tell(asyncFn)`: runs the function and logs any rejection.

The "failure as return value" pattern runs through the whole app: HTTP handlers return failures, and so do RPC middleware and OER parsing.

### 18.11 `lib-format-utils`

`formatTypedArrayPlugin` for `pretty-format`. It renders typed arrays as hex grouped by element width (1, 2, 4 or 8 bytes) and wraps arrays longer than 16 elements. Used by the debug RPC output.

### 18.12 `lib-itergen-utils`

`byLine` (async split of a string stream into lines), `iterateMultiple` (concatenate iterables), `prefix(str)` (prefix each chunk). Used only by `app-dev` (child-process log handling), **not** by app-dassie.

---

## 19. Docs summary

- `docs/contributing/folder-structure.md`:
  - Package prefixes: `app-` executables, `gui-` frontends, `lib-` libraries, `meta-` dev tooling (pnpm monorepo).
  - Feature folder conventions: `actors/` (often top-level files instead), `computed/`, `constants/`, `database-stores/`, `database-tables/`, `failures/`, `functions/` (reactor-instantiated functions), `signals/`, `stores/`, `topics/`, `rpc-routers/`, `types/`, `utils/`, plus feature-specific folders.
- `docs/implementation-notes/configuration-wizard.md`, `frontend-security.md`, `install-command.md`: summarized in §4.5, §8.8 and §12.5.

---

## 20. Open questions / known gaps (consolidated)

- **RT-1** **Redirect host bug**: the HTTP→HTTPS redirect uses `url.hostname`, which is always `0.0.0.0` because lib-http-server builds request URLs with a fixed hostname. The query string is dropped as well.
- **RT-2** **Setup URL without TLS** (`http://127.0.0.1/setup/<token>`) is unusable. The setup URL also ignores a non-443 `httpsPort`.
- **RT-3** **`url` not recomputed** on `setHostname`. It stays `https://undefined` until restart, which affects `/stats` and log lines, and `ServeHttpsActor` does not restart.
- **RT-4** **No setters** for `httpPort`, `httpsPort`, `alias` or `exchangeRateUrl`. `config.enable_http_server` is unused. `cachePath` is unused.
- **RT-5** **RPC error semantics**:
   - Router failures send the message `"RpcFailure"`.
   - Thrown errors become `"Internal server error"`.
   - An unauthorized call returns a *success* response containing a serialized failure.
   - An unauthorized subscription sends success followed by an error.
- **RT-6** **WS auth is not revoked** on logout. There is no Origin check.
- **RT-7** **No graceful shutdown** (no SIGTERM handler) in production.
- **RT-8** **nodejs runtime startup order**: ACME renewal at startup runs before the HTTP listener exists.
- **RT-9** IPC socket permissions are not set in the nodejs runtime. The IPC client has no socket error handler, so it crashes when the daemon is down.
- **RT-10** `POST /api/setup` does not validate the `rawDassieKeyHex` length. Secret comparisons are not constant-time. There is no rate limiting.
- **RT-11** Sessions never expire, and there is no session management.
- **RT-12** `tls.setNodeTlsConfiguration` does not validate the cert/key pair.
- **RT-13** ACME: no backoff, `acme_tokens.expires` is not enforced, `registerToken` throws on a duplicate, the startup ordering issue above, and only the http-01 challenge is supported (no DNS-01, no other CAs).
- **RT-14** Self-signed fallback cert is regenerated on every HTTPS restart. lib-x509 serial sign and post-2050 time encoding are wrong.
- **RT-15** Frontend serving: limited MIME map (svg/woff2 etc. fall back to HTML), no caching, no security headers (CSP recommended in docs), HEAD not supported.
- **RT-16** WebSocket router leaves unmatched upgrade sockets open. Binary frames on `/rpc` throw inside the listener.
- **RT-17** Only the last `Set-Cookie` header survives in the Node adapter.
- **RT-18** `update` command:
  - no checksum or signature verification
  - HTTP status not checked
  - `meta/latest` text not trimmed
  - symlink swap not atomic
  - canary installs always rejected (DASSIE_ROOT check vs `readlink -f`)
  - "Extracting" note never shown
  - installer `.tar.xz` vs updater `.tar.gz`
- **RT-19** Installer: arm architectures are mapped but not built. Uninstall leaves `/usr/local/bin/dassie`, `dassie-update.service`, the group and the state directory.
- **RT-20** Socket activation does not verify `LISTEN_PID`. Readiness uses `execSync("systemd-notify --ready")` (TODO: native).
- **RT-21** `ledgers.addSettlementScheme` strips all config keys (`z.object({})`).
- **RT-22** `payment.createPayment` does not validate paymentId length or uniqueness (TODOs).
- **RT-23** Node ID and settlement-scheme ID schemas do not enforce a character set (TODOs).
- **RT-24** Actor errors permanently stop the actor; there is no supervision or restart. Invalid env config therefore leaves the daemon half-started instead of exiting.
- **RT-25** Plaintext secrets in SQLite (node key, TLS key, ACME key, session and BTP tokens). The DB file mode is not enforced.
- **RT-26** Debug RPC and `/stats` exposure (by design, but document it). `debug.getContextKeys` is empty in production.
- **RT-27** In-flight RPC queries hang across WebSocket reconnects (no timeout).
- **RT-28** `createEnableChecker` only replaces the first `*`. `compareArrays` is not multiset-exact.

---

## 21. Compatibility surface

This section lists what an implementation would have to keep in order to be a drop-in replacement for the original. [Chapter 7 §2](../overview.md) decides which of these the rewrite actually keeps. In particular, it does **not** keep the database file format.

Drop-in compatibility would require preserving:
- The DB file name, `application_id 0x1DA53B81`, `user_version` 13, and the table, column and scalar names and encodings (§5).
- Scalar key names. `config.dassie_key` PEM PKCS#8 format.
- The Ed25519 identity derivation and node-ID algorithm (§8.6). Login-secret derivation, cookie name and attributes.
- The `/api/setup`, `/api/login` and `/api/logout` contracts, if the frontend contract is kept.
- Socket-activation FD names, `Type=notify`, the default socket path, and the env var names and precedence (§3).
- The ACME http-01 challenge route on the HTTP listener, the renewal window (30 d) and the polling interval (1 h).
- The RPC procedure set and semantics (§10.3), the start order of feature services (§2.4), and the conditional route registration (setup vs login/logout).

May redesign (with conscious decisions on the OQs above):
- RPC encoding (superjson), error mapping, the WebSocket auth lifecycle and security headers.
- Graceful shutdown and the update and installer integrity checks.
