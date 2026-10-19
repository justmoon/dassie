# HTTP surface

The node's HTTP and HTTPS listeners: which routes exist and when, the headers and limits every request meets, the owner's setup, login and sessions, and the certificate the HTTPS listener serves, from ACME or otherwise.

**Paths:** `crates/dassie-node/src/http/` (`mod.rs` is the route table, `api.rs`, `ws.rs`, `assets.rs`, `mime.rs`, `response.rs`, `stats.rs`), `crates/dassie-node/src/tls.rs`, `crates/dassie-node/src/acme.rs`, `crates/dassie-node/src/slots.rs`, `crates/dassie-node/src/node/access.rs`, `crates/dassie-node/src/node/certificates.rs`, `crates/dassie-node-core/src/` (`origin.rs`, `session.rs`, `setup.rs`, `rate_limit.rs`, `acme.rs`, `certificate.rs`, `connections.rs`, `urls.rs`)
**Read with:** [RPC](rpc.md), [Payments](payments.md), [Node](node.md), [Identity](identity.md), [Connector](connector.md), [Peer protocol](peer-protocol.md)

The decisions (`origin`, `session`, `setup`, `rate_limit`, `acme`, `certificate`, `connections`, `urls`) live in `dassie-node-core` as pure functions of their inputs, `now` and an injected RNG; `dassie-node` only carries them out.

## Listeners

The node has two listeners, both HTTP/1.1 only (hyper):

- **HTTP** serves ACME challenges and redirects everything else to HTTPS.
- **HTTPS** serves everything else, with TLS from rustls (see [Certificates](#certificates)).

The ports, and how the sockets come (bound by the daemon or passed by systemd), are in [node.md](node.md). URLs the node prints or redirects to use the port the HTTPS listener actually has, which under systemd may differ from the configured one.

| Limit | Value |
|---|---|
| Connections per listener | 1024; more are closed on arrival |
| Connections per client on one listener | 64. A client is an IPv4 address or an IPv6 /56. Loopback is not counted per client where private peers are allowed (dev environments and tests), only in the total |
| TLS handshake | 10 s |
| Request headers | 10 s, at most 64 KiB buffered |
| Request body (`/api/*`, Open Payments) | 16 KiB, sent within 10 s |
| Drain at shutdown | the listener stops accepting, requests in progress finish, WebSockets close with 1001; at most 10 s |

A failed `accept` (out of file descriptors, say) is logged and retried after 100 ms.

## Route table

**HTTP listener:**

| Method | Path | Answer |
|---|---|---|
| any | WebSocket upgrade | 404 |
| `GET` | `/.well-known/acme-challenge/{token}` | 200 `text/plain` with the key authorization; 404 for an unknown token; 400 `Bad Request` for an empty one ([ACME](#acme)). Other methods: 404 |
| `GET`, `HEAD` | anything else | 301 `Moved Permanently` to `https://<host>[:<https port>]<path>?<query>`. The host is the configured hostname, or before there is one the request's `Host` (its host part, validated). Path and query are kept. 400 `Bad Request` when neither gives a usable host |
| other | anything else | 404 |

**HTTPS listener,** matched in this order:

| # | Method | Path | Answer |
|---|---|---|---|
| 1 | upgrade | `/btp` | the BTP server ([connector.md](connector.md#the-btp-server)) |
| 2 | upgrade | `/rpc` | the RPC WebSocket ([rpc.md](rpc.md)). A request to `/rpc` that is not an upgrade gets 426 `Upgrade Required` |
| 3 | upgrade | anything else | 404 |
| 4 | `POST` | `/api/setup` | [setup](#setup), only while the node has no identity |
| 5 | `POST` | `/api/login`, `/api/logout` | [login and logout](#login-and-logout), only once it has one |
| 6 | `POST` | `/peer` | the peer protocol ([peer-protocol.md](peer-protocol.md)) |
| 7 | any | Open Payments paths: `/.well-known/pay[/…]`, `/pay/…`, `/gnap`, `/gnap/…`, `/incoming-payments[/…]`, `/quotes[/…]`, `/outgoing-payments[/…]`, `/outgoing-payment-grant` | [payments.md](payments.md#http-routes) |
| 8 | any | any other `/api/…` | 404, so `/api/setup` after setup and `/api/login` before it do not exist |
| 9 | `GET`, `HEAD` | `/stats` | [`GET /stats`](#get-stats) |
| 10 | `GET`, `HEAD` | anything else | the wallet's files ([The wallet's files](#the-wallets-files)) |
| 11 | other | anything else | 405 `Method Not Allowed`, `Allow: GET, HEAD` |

Paths match exactly; there is no trailing-slash normalization. ILP-over-HTTP (`/ilp`) is not served yet ([roadmap](../roadmap.md)).

Errors of the node's own routes are plain text (`text/plain; charset=utf-8`), as the original's were. Open Payments routes answer in their own error shape ([payments.md](payments.md#errors)).

## Response headers

Every HTTPS response but a 101 carries:

| Header | Value |
|---|---|
| `Content-Security-Policy` | `default-src 'self'; script-src 'self' 'wasm-unsafe-eval'; style-src 'self'; img-src 'self' data:; connect-src 'self'; object-src 'none'; base-uri 'none'; form-action 'self'; frame-ancestors 'none'` |
| `X-Content-Type-Options` | `nosniff` |
| `Referrer-Policy` | `no-referrer` |
| `X-Frame-Options` | `DENY` |
| `Cross-Origin-Opener-Policy` | `same-origin` |
| `Strict-Transport-Security` | `max-age=31536000`, only while a certificate from ACME or from the owner is installed. Never with the self-signed one: the owner would have no way past the browser's warning once it saw the header |

The CSP allows no inline script or style and nothing from other origins. WebAssembly may compile, for the identity module the wallet runs. Open Payments routes add CORS headers on top ([payments.md](payments.md#http-routes)).

## Same-origin rule

The owner's endpoints, the `/rpc` upgrade and the three `/api/*` POSTs, accept a request only from a page the node served itself (`origin::check_same_origin`):

1. `Host` must parse as `name[:port]`, `ipv4[:port]` or `[ipv6][:port]`. Names are lowercased and must be valid host names. A port is 1 to 5 digits, without a sign or leading zero, and not 0, so each port has one spelling. Otherwise: 403 `missing or invalid Host header`.
2. The `Host`'s host must be one the node answers to: the configured hostname, `localhost`, or an IP literal. Otherwise: 403 `this node does not answer to that host name`. This stops DNS rebinding, where an attacker's name resolves to the node and its `Origin` and `Host` agree.
3. `Origin` must be `https://` plus an authority as above. Otherwise: 403 `missing or invalid Origin header`.
4. `Origin`'s host and port must equal `Host`'s, a missing port counting as 443. Otherwise: 403 `cross-origin requests are not allowed`.

Open Payments, `/peer`, `/btp`, `/stats` and the wallet's files do not apply it: their clients are not browsers acting for the owner, and authenticate otherwise or not at all.

## The wallet's files

The wallet (`apps/wallet`, [wallet.md](wallet.md)) is embedded in the binary at build time.

- Every file has its media type (`mime.rs`): `html`, `js`/`mjs`, `css`, `json`/`map`, `webmanifest`, `svg`, `txt`, `wasm` (`application/wasm`), `png`, `ico`, `woff2`, `woff`; anything else is `application/octet-stream`.
- Every file has an ETag from its content. `If-None-Match` with that tag, or `*`, gets 304.
- Vite's content-hashed files under `/assets/` are cached for a year (`public, max-age=31536000, immutable`); everything else, `index.html` included, is `no-cache`, so a new build shows at once.
- A file that gets smaller gzipped is sent gzipped to a client that accepts it. Responses carry `Vary: Accept-Encoding`.
- `HEAD` gets the headers without the body.
- A path that names no file gets `index.html`, for the wallet's own routing (`/setup/{token}`, `/consent/{id}` and the rest), except under `/assets/`, where it is 404.

In development, `DASSIE_DEV_WALLET_DIR` names a directory that `vite build --watch` writes, and the node serves the wallet from it under the same CSP ([node.md](node.md), [dev.md](dev.md)). Nothing from it is cached, nothing outside it is read, a file is at most 64 MiB, and while the directory has no `index.html` the answer is 503 `The wallet is being built; reload in a moment.`

## `GET /stats`

Who the node is, for anyone. No authentication, no CORS.

```json
{
  "version": "<node version>",
  "node": { "id": "<node ID>", "url": "<its link state's URL>|null", "publicKey": "<base64url Ed25519>" },
  "peers": [ { "nodeId": "<id>", "url": "<url>|null", "publicKey": "<base64url>|null" } ]
}
```

- `peers` lists only the peers the node advertises, which its signed link state publishes anyway ([network.md](network.md)).
- 404 before setup; 503 `Service Unavailable` while the engine is not running.
- The answer is kept 5 s. One request at a time makes a new one while the others wait for it, so unauthenticated requests queue at most one call to the engine. `Cache-Control: no-store`.

## Setup

Setup happens once: whoever opens the setup URL claims the node with their passphrase.

**The setup token.** 32 random bytes, written as 64 lowercase hex digits. It is drawn the first time the daemon starts without a node key and stored in the database until setup is done, when it is deleted with the key's write; so a restart, such as the nightly update, keeps the URL the owner was given.

- The setup URL is `https://<hostname>[:<https port>]/setup/<token>`, with `localhost` before a hostname is configured (it reaches the node from its own machine, or through an SSH tunnel). The HTTPS listener always has a certificate, so the URL always works. The port is left out when it is 443.
- `dassie setup-url` prints it, through the RPC method `GetSetupUrl` over the IPC socket, which is `FAILED_PRECONDITION` once setup is done.
- The daemon never logs the URL, since the journal is readable by groups that do not administer the node. It logs that `sudo dassie setup-url` prints it. Log fields named `setup_url` are redacted.

**`POST /api/setup`,** body (the original's field names):

```json
{ "setupAuthorizationToken": "<64 hex>", "rawDassieKeyHex": "<64 hex>", "loginAuthorizationSignature": "<64 hex>" }
```

`rawDassieKeyHex` is the 32-byte node key seed the browser derived from the passphrase; `loginAuthorizationSignature` is the login proof derived from that key ([identity.md](identity.md)). The browser sends only these.

The checks, in order:

| Check | Refusal |
|---|---|
| [Same-origin rule](#same-origin-rule) | 403, as there |
| `Content-Type` is `application/json`, any parameters, any case | 415 `Expected application/json` |
| Body at most 16 KiB | 413 `Payload Too Large` |
| Body sent within 10 s | 408 `Request Timeout` |
| Body is JSON of that shape | 400 `Invalid HTTP request body` |
| [Rate limit](#rate-limit-on-setup-and-login) | 429 `Too many attempts, try again later`, `Retry-After` in whole seconds, at least 1 |
| Not set up yet | 401 `Node is already set up` |
| Token is exactly 64 hex digits and equals the setup token, compared in constant time | 401 `Invalid setup authorization token` |
| Key is exactly 32 bytes of hex | 400 `Invalid node key` |
| Proof is exactly 32 bytes of hex and is the key's login proof, compared in constant time | 401 `Invalid login authorization signature` |
| The key is stored only if none is (two setups at once: one wins) | 401 `Node is already set up` |

A database failure is 500 `Internal Server Error`. On success the node stores the key, starts what depends on its identity, creates a [session](#sessions) and answers 200 `{}` with the session cookie.

## Login and logout

**`POST /api/login`,** body `{"loginAuthorizationSignature": "<64 hex>"}`: the same checks as setup up to the rate limit, then the proof must be the node key's login proof, compared in constant time (401 `Invalid login authorization signature`). On success: a new session, 200 `{}` with its cookie.

**`POST /api/logout`:** the [same-origin rule](#same-origin-rule), then the session the cookie names, if any, ends and its sockets close; whether it existed is not checked. The answer is always 200 `{}` with a cookie that clears it. Logging out from another site does not send the cookie (`SameSite=Strict`), and the Origin check refuses it anyway.

### Rate limit on setup and login

Setup and login attempts share one table of token buckets (`rate_limit.rs`):

| Setting | Value |
|---|---|
| Who | each client: an IPv4 address (IPv4-mapped IPv6 counts as IPv4), or the /56 of an IPv6 address |
| Burst | 5 attempts |
| Refill | one attempt every 12 s |
| Clients tracked | 256; when full, the client seen least recently loses its slot |

There is no bucket shared by all clients, so a handful of addresses cannot lock the owner out. The login secret is 256 bits, so the limit is not against guessing but against making the node work as fast as a client can send.

## Sessions

A session is what a browser holds after setup or login.

- **The token** is 32 random bytes; the cookie carries it as 64 hex digits. The node keeps only its SHA-256, which is the session's ID ([D10](../decisions/D10-secrets-at-rest.md)), with when it was created, when it was last used, and the browser's `User-Agent` (control characters removed, at most 256 bytes), for the owner's list of sessions.
- **The cookie:** `__Host-Dassie-Session=<hex>; Path=/; HttpOnly; Secure; SameSite=Strict; Max-Age=<s>`, with `Max-Age` running to the session's absolute end. Clearing it sends an empty value with `Max-Age=0`. The node reads the first `__Host-Dassie-Session` pair in any `Cookie` header, with surrounding quotes removed; a value that is not 64 hex digits is no session.
- **Lifetime:**

  | Rule | Value |
  |---|---|
  | Ends when unused for | 30 days |
  | Ends after, however used | 365 days from creation |
  | Last use recorded | at most once an hour |
  | Sessions kept | 64. A new session always stays; of the others the 63 most recently used stay (ties by creation, then ID), and the rest are deleted |
  | Expired sessions deleted | when checked, and by a pruning task every hour |

- A session also ends when the owner revokes it: `RevokeSession`, or `RevokeAllSessions`, optionally keeping the caller's own ([rpc.md](rpc.md#procedure-groups)). `ListSessions` shows each with its user agent, times, when it will end unless used, and whether it is the caller's.
- Every way a session ends (logout, revocation, expiry, pruning, eviction for a newer one) closes its RPC sockets with code 4001; sockets are checked against the database on every call ([rpc.md](rpc.md#websocket)).

## Certificates

The HTTPS listener always has a certificate. It reads the current one from a slot at every handshake, so a renewal or an installed certificate takes effect at once, without restarting the listener. It serves that one certificate whatever name the client asks for.

- **TLS:** rustls with ring as the only crypto provider, TLS 1.2 and 1.3 with rustls's safe defaults, and ALPN `http/1.1` only.
- **Sources:** `self-signed` (made by the node), `acme` (issued through ACME and renewed by the node), and `manual` (installed by the owner, never renewed). The chain and key are stored as PEM, each at most 64 KiB, with the source.
- **The self-signed certificate:** ECDSA P-256; subject `CN=Dassie node, O=Dassie`; names: the hostname if there is one, then `localhost`, `127.0.0.1` and `::1`; valid from an hour before it is made, for 10 years; a positive 20-byte serial derived from the key. It is made once and stored, not made again at every start.
- **What to serve,** decided at start and whenever the hostname changes (`certificate::decide`):

  | Stored | Kept when | Otherwise |
  |---|---|---|
  | none | — | make a self-signed one |
  | self-signed | it names exactly the names above, and has more than 30 days left | make a new self-signed one |
  | ACME | it covers the hostname | make a self-signed one, served until the renewal gets one for the new name |
  | manual | always: it is the owner's business | — |

  A certificate covers a hostname that is one of its names exactly, or matches a wildcard `*.rest` with exactly one non-empty label in front of `rest`, ignoring case.
- **Installing one from elsewhere** (`SetCertificate`, [rpc.md](rpc.md#procedure-groups)): the chain must parse, the key must be the leaf's, and the leaf must be valid now; otherwise `INVALID_ARGUMENT` and nothing changes. It becomes `manual`.

## ACME

The daemon runs every ACME order (RFC 8555, http-01 only), the first as well as renewals. `dassie init` asks for the first one over RPC: `GetAcmeTerms` gives the directory and its terms of service, and `ObtainCertificate` streams the progress back (`CreatingAccount`, `Ordering`, `PreparingChallenge`, `Validating`, `Finalizing`, `Downloading`, then `Installed` with the certificate).

**The directory** is the configured one, Let's Encrypt's production directory by default (`https://acme-v02.api.letsencrypt.org/directory`). A directory URL is `https://`, at most 2048 bytes, without whitespace or control characters. An order may name another directory; if it succeeds, that directory becomes the configured one.

**The account:** one per directory, stored with its contact email; its credentials are a secret column. An order for a directory without an account creates one with the owner's email, but only once the owner agreed to the CA's terms (`FAILED_PRECONDITION` `the CA's terms of service must be agreed to`).

**An order:**

1. The hostname must be set (`FAILED_PRECONDITION` `set the hostname before ordering a certificate`).
2. Order the hostname, and put each http-01 key authorization in the challenge table.
3. Tell the server the challenge is ready, and wait for the order: at most 120 s to validate and 120 s to issue.
4. Send a CSR for a new ECDSA P-256 key, and fetch the chain.
5. Check and install the certificate under the configuration's writer lock, so a hostname change lands either before (the certificate is discarded: `the hostname changed while ordering; order again`) or after (it is replaced by a self-signed one until a new order).

Every request to the ACME server has 10 s to connect and 30 s to answer, a directory document is read up to 64 KiB, and a whole order takes at most 600 s. A stalled server can therefore not hold the order lock for ever.

**One order at a time.** A request that finds another order running (a renewal woken by the hostname change just before, say) waits for it, and takes its certificate if it fits: from ACME, from the directory asked for, covering the hostname, and with more than 30 days left. A renewal that finds the owner's order running skips its turn.

**Challenge tokens** live in memory only, since the daemon that serves them also runs the order:

| Rule | Value |
|---|---|
| Entries | at most 16 unexpired; an order for one name has one |
| Token | non-empty base64url (`A–Z a–z 0–9 - _`), at most 128 bytes |
| Key authorization | printable ASCII, at most 256 bytes |
| Expiry | the server's, or 10 minutes if it gives none; enforced on lookup |
| Duplicate token | replaces the entry |
| Order ends | its tokens leave the table, however it ends, cancelled included |

**Renewal** (`acme::Renewal`):

| Setting | Value |
|---|---|
| Check | every hour, and at once when the hostname changes or an account is created (unless backing off) |
| Renew when | a hostname is set, an account exists for the configured directory, and the certificate is missing, self-signed, or from ACME and either does not cover the hostname or has 30 days or less left. Never for a manual certificate |
| After a failure | wait 1 h, doubling with each further failure, at most 24 h; or the server's `Retry-After` (honoured up to 7 days) if that is later |
| After a success | failures reset; next check in an hour |

A renewal that failed because the hostname changed during it is not a failure: it checks again at once. `GetStatus` reports whether there is an account, when the next check is, and the last error.

## Security model

What protects the owner's side of the node, in one place:

| Aspect | Behaviour |
|---|---|
| Owner, in a browser | the session cookie, checked at the `/rpc` upgrade and again on every call ([rpc.md](rpc.md#authentication)) |
| Owner, on the machine | the IPC socket: whoever can open it is the owner; its file mode is the only access control ([rpc.md](rpc.md#ipc-socket), [node.md](node.md)) |
| Login | a 256-bit login proof, compared in constant time, rate-limited per client |
| Setup | the one-time setup token in the URL; the route exists only until setup is done; the URL is not logged |
| Key custody | the passphrase stays in the browser; only the node key seed and the login proof reach the node ([identity.md](identity.md)) |
| Cross-site requests | the same-origin rule on `/api/*` and `/rpc`, JSON required on `/api/*`, and `SameSite=Strict` |
| Page integrity | the strict CSP, `frame-ancestors 'none'`, `X-Frame-Options: DENY`, no inline script |
| TLS | an ACME or owner certificate, else the stored self-signed one; HSTS only with the former |
| Secrets at rest | session tokens as SHA-256 hashes, BTP tokens too ([connector.md](connector.md#btp-tokens)); data directory 0700 and database 0600 ([node.md](node.md), [D10](../decisions/D10-secrets-at-rest.md)) |
| Bounds | connections per listener and per client, header and body sizes and times, setup and login attempts |

The frontend-signing ideas of the original's design notes (an independently installed frontend, service-worker verification, a hash-pinned bookmarklet) are not built ([roadmap](../roadmap.md)).

## Decisions

- [D5](../decisions/D05-ui-and-cli-rpc-contract.md): the RPC contract the `/rpc` WebSocket carries, and why the Origin check and per-call session checks came with it.
- [D9](../decisions/D09-open-payments-fully-and-no-spsp.md): why Open Payments routes are open to every origin.
- [D10](../decisions/D10-secrets-at-rest.md): session tokens stored only as hashes.
- [D16](../decisions/D16-engineering-style-pragmatic-tigerstyle-and-deterministic-simulation.md): the node's decisions as pure functions in `dassie-node-core`; every table and timer bounded.

## Original

- [2 §6](../original/02-node-runtime.md): the original's listeners, request handling, route table and frontend serving, with its redirect and MIME bugs.
- [2 §7](../original/02-node-runtime.md): its ACME manager, run by the CLI with tokens over RPC.
- [2 §8](../original/02-node-runtime.md): setup, login, logout and its never-expiring sessions; the `__Host-` cookie and the one-time setup token are kept.
- [2 §14, §16, §17](../original/02-node-runtime.md): `/stats`, the original's security model and its constants.
