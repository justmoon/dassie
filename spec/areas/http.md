# HTTP surface

The node's HTTP and HTTPS listeners, how the owner's side of them is protected (setup, login, sessions, the same-origin rule), and the certificate the HTTPS listener serves, from ACME or otherwise; and front doors, which pass Open Payments on to nodes without a public address.

**Paths:** `crates/dassie-node/src/http/` (`mod.rs` is the route table), `crates/dassie-node/src/front_door/`, `resources/front-door/`, `crates/dassie-node/src/tls.rs`, `crates/dassie-node/src/acme.rs`, `crates/dassie-node/src/slots.rs`, `crates/dassie-node/src/node/access.rs`, `crates/dassie-node/src/node/certificates.rs`, `crates/dassie-node-core/src/` (`origin.rs`, `session.rs`, `setup.rs`, `rate_limit.rs`, `acme.rs`, `certificate.rs`, `connections.rs`, `urls.rs`)
**Read with:** [RPC](rpc.md), [Payments](payments.md), [Node](node.md), [Identity](identity.md), [Connector](connector.md), [Peer protocol](peer-protocol.md)

The decisions (whether an origin is allowed, a session valid, a setup attempt right, a certificate kept, a renewal due) are pure functions in `dassie-node-core`, of their inputs, `now` and an injected RNG; `dassie-node` only carries them out ([D16](../decisions/D16-engineering-style-pragmatic-tigerstyle-and-deterministic-simulation.md)).

## Listeners

- **HTTP** serves ACME challenges and redirects everything else to HTTPS, keeping path and query.
- **HTTPS** serves everything else: the BTP server, the RPC WebSocket, setup and login, the peer protocol for nodes that do not take QUIC ([Peer protocol](peer-protocol.md#transport)), Open Payments, `/stats` and the wallet.
- Connections are bounded per listener and per client, where a client is an IPv4 address or an IPv6 /56, so one host cannot take every place by rotating addresses. Loopback is counted only in the total where private peers are allowed (dev environments and tests). Handshakes, headers and bodies are bounded in size and time too (`http/mod.rs`, `dassie-node-core/src/connections.rs`).
- At shutdown a listener stops accepting, lets requests in progress finish for a bounded time, and closes WebSockets with 1001.

### Route table

`http/mod.rs` holds it. Routes exist only in the state they belong to: `/api/setup` only while the node has no identity, `/api/login` and `/api/logout` only once it has one; otherwise they are 404. ILP-over-HTTP (`/ilp`) is not served yet ([roadmap](../roadmap.md)).

### Relayed streams

A node reached through a relay ([Peer protocol](peer-protocol.md#relayed-streams)) serves HTTP/1.1 on a relayed stream with purpose `payments`, once its TLS is done (`serve_relayed` in `http/mod.rs`):

- **Open Payments only.** Every other path is 404, upgrades included: the wallet, `/api/*`, `/rpc`, `/peer`, BTP and `/stats` are served only on the node's own listeners ([D26](../decisions/D26-core-and-periphery.md)). A page served through a relay could keep the owner's passphrase, and nothing on a relayed stream shows that a browser's origin is the node's.
- **The same rules as over HTTPS** ([Payments](payments.md#http-routes)): the request's `Host` names the name it is answered under, and one that is not among [the node's names](payments.md#the-nodes-names) is refused with 421; the answer is [signed with the node key](payments.md#signed-responses), so a client can check it whoever passed it on; and CORS, request signatures, bounded bodies and the security headers apply, without HSTS. The rate limits per client address count the relay's address, the only one the node sees; the relay bounds streams per client.

### The wallet's files

The wallet ([wallet.md](wallet.md)) is embedded in the binary and served for any other `GET` (`http/assets.rs`). A path that names no file gets `index.html`, for the wallet's own routes, except under `/assets/`, where a missing file stays 404. Vite's content-hashed files under `/assets/` are cached as immutable, and everything else, `index.html` included, is revalidated, so a new build shows at once. A dev node serves a watch build from a directory instead ([dev.md](dev.md)), under the same policy.

## Front doors

A front door lets ordinary Open Payments clients reach a node without a public address, at `<label>.<domain>`, where the label is the node's [label](identity.md#node-label) ([D26](../decisions/D26-core-and-periphery.md), [the design](../designs/periphery.md#7-open-payments-without-a-public-address)). `dassiepay.net` is the default domain, whose names nodes answer to unless told otherwise ([Payments](payments.md#the-nodes-names)); anyone can run another. It is a mode of the `dassie` binary (`dassie front-door`, `front_door/`), and no node: no ledger, no peer protocol, no wallet, no database. `resources/front-door/` has its systemd unit and what an operator sets up, apart from the node's units, which the installer never installs with it.

- **Its own key.** A front door proves an Ed25519 key of its own, as a node proves its node key. A node names it as its relay by that key, its ID and a URL for its QUIC endpoint (`DASSIE_RELAY`, [Node](node.md#a-chosen-relay)), and keeps a connection to it. The key signs nothing else, and changing it means telling every node again.
- **Relay and client in one.** It opens relayed streams with purpose `payments` itself, on the connection the node keeps to it (`Quic::open_held`), naming its own ID as the client, and runs TLS inside them pinned to the node's key, as any client through any relay ([Peer protocol](peer-protocol.md#relayed-streams)). It answers no peer requests, carries no one else's relay requests, and serves no relayed stream. Reaching nodes that chose another relay, by records and home nodes, is not built ([roadmap](../roadmap.md#periphery)).
- **Names.** Each request is routed by its `Host`: exactly one label under the domain, on the default port, which must be a node's label. Any other name under the domain is 404; a name outside it is refused with 421. Both are in the specs' error form, as every answer the front door gives itself.
- **What it passes on:** Open Payments' routes alone, by the node's own predicate (`is_open_payments_path`), with the node's bound on bodies and bounded heads; everything else is 404 and never reaches a node, upgrades included. It passes `Host` unchanged, since the node answers under it and checks signatures against it, and only end-to-end headers: no hop-by-hop headers and nothing a proxy adds (`Forwarded`, `X-Forwarded-*`), which the node would not trust anyway.
- **What it returns:** the node's status, headers and body, so the node's signature still verifies ([Payments](payments.md#signed-responses)), less the headers that concern the stream it came over, cookies, `Alt-Svc` and `Refresh`. Only JSON comes back, with exactly one `Content-Type`, which the front door writes anew as plain `application/json`, or no body: a node's answer of any other type, with two types (a browser takes the last), a redirect, or over a bound, is 502. On every answer, its own and those it passes on, the front door sets `nosniff` and a Content Security Policy under which nothing runs or loads, replacing the node's; the node's signature covers neither. So no page and no cookie is ever served on a front door's names, whatever a node sends: that is why its names need no place on the Public Suffix List, and why a node cannot toss cookies onto the shared domain or put a phishing page on it.
- **Its own answers.** A node that holds no connection to it, or has no stream to spare, is 503; one that does not answer in time is 504; a stream that breaks is 502. The consent route, `/gnap/interact/…`, is for an owner app ([the design](../designs/periphery.md#6-owner-apps)), which is not built: the front door answers it with a fixed error saying so, and never passes it on. The consent page of the design is not built ([roadmap](../roadmap.md#periphery)).
- **Connections to nodes.** It keeps a few relayed streams per node, each a keep-alive HTTP/1.1 connection, within the share a relay gives one client address of a destination ([Peer protocol](peer-protocol.md#relayed-streams)), and a bounded number in all; when all are taken, the stream idle longest, of any node, is closed for a new one. A request that finds its node's streams all in use waits a moment for one before it is 503. A stream left idle is closed before the node's own header timeout would close it. Before each request on a stream it waits until the connection is ready: hyper's HTTP/1 client cancels a request sent before then.
- **Limits,** as a node's HTTPS listener has: connections in all and per client, a rate limit on requests per client address, at the rate the node's Open Payments allows one client, and bounded handshakes, heads and bodies (`front_door/https.rs`). Nodes connected at once are bounded in all and per address too (`front_door/relay.rs`).
- **A node's budget for its front door.** The node sees only the front door's address, and counts every request the front door passes on against that one client, at the rate it allows any client. So the front door is what tells clients apart, and it gives each client a fraction of that rate for each node; a few clients at their allowance still use up a node's whole budget for its front door, and the node's other clients through it wait. A front door that the node can trust to name the client is not built ([roadmap](../roadmap.md#periphery)).
- **Its certificate** for `*.<domain>` comes from files, which it reads again periodically, so that a renewal (certbot's, by DNS-01, since a wildcard needs it) takes effect without a restart. A certificate that does not load, or does not name the wildcard, is never served; the one before stays.
- **What it sees.** It terminates TLS, so it reads the requests and answers, including the STREAM secret of an incoming payment; a client that does not check the node's signature trusts it ([Payments](payments.md#signed-responses), [D26](../decisions/D26-core-and-periphery.md)). A relay between it and the node would see only ciphertext, but a front door is its own relay.

## Security model

What protects the owner's side of the node:

- **The owner in a browser** holds a [session](#sessions) cookie, which the RPC WebSocket checks at the upgrade and again on every call ([rpc.md](rpc.md#authentication)).
- **The owner on the machine** uses the IPC socket, whose file mode is the only access control ([node.md](node.md)).
- **The passphrase never leaves the browser:** setup sends the node key seed and the login proof derived from it, login only the proof ([identity.md](identity.md)).
- **Cross-site requests** meet the [same-origin rule](#same-origin-rule) on `/rpc` and `/api/*`. `POST /api/setup` and `POST /api/login` also require `Content-Type: application/json`, which no cross-site form can send and no cross-origin script can send without a CORS preflight, which the node never grants.
- **Setup and login attempts are rate-limited per client,** with no bucket shared by all clients, so a handful of addresses cannot lock the owner out. The login proof is 256 bits, so the limit is not against guessing but against making the node work as fast as a client can send.
- **Secrets at rest:** sessions are stored as hashes, and so are BTP tokens ([D10](../decisions/D10-secrets-at-rest.md)).

Making the wallet trustworthy without trusting the node that serves it (signed or independent frontends) is not built ([roadmap](../roadmap.md)).

### Same-origin rule

The owner's endpoints, the `/rpc` upgrade and the `/api/*` POSTs, accept a request only from a page the node served itself (`origin.rs`): `Origin` must be `https://` with the same host and port as `Host`. `SameSite=Strict` alone is not enough: it keeps the cookie off other *sites'* requests, but a site is a whole registrable domain, so a page on a sibling host would still send it; and browsers do not apply the same-origin policy to the WebSocket handshake at all.

**`Host` must also be a name the node answers to:** its configured hostname, `localhost`, or an IP literal. Otherwise DNS rebinding gets through: an attacker's name that resolves to the node makes the attacker's page same-origin with itself, `Origin` and `Host` agreeing. Hosts and ports are parsed to one canonical spelling each, so the comparison cannot be fooled by spelling.

Open Payments, `/peer`, `/btp`, `/stats` and the wallet's files do not apply the rule: their clients are not browsers acting for the owner, and authenticate otherwise or not at all. Open Payments allows every origin ([payments.md](payments.md#http-routes)).

## Response headers

Every HTTPS response but a WebSocket upgrade carries a strict Content Security Policy (`CONTENT_SECURITY_POLICY` in `http/response.rs`): nothing inline and nothing from other origins, with `wasm-unsafe-eval` only so the identity module the wallet runs can compile; framing is denied.

**HSTS is sent only while a certificate from ACME or from the owner is installed,** never with the self-signed one: once a browser saw the header, the owner would have no way past its warning.

## `GET /stats`

Who the node is, for anyone, without authentication: its version, ID, URL and key, and its peers. It lists only the peers the node advertises, which its signed link state publishes anyway ([network.md](network.md)). The answer is cached briefly and made by one request at a time, so unauthenticated requests queue at most one call to the engine.

## Setup

Setup happens once: whoever opens the setup URL claims the node with their passphrase. Until then the setup token is all that stands between a fresh node on the internet and anyone who reaches it.

- **The token** is 32 random bytes, compared in constant time. It is stored until setup completes and deleted in the same write that stores the node key, so a restart (the nightly update, say) keeps the URL the owner was given, and of two setups at once only one wins.
- **The URL is never logged,** since the journal is readable by groups that do not administer the node. `dassie setup-url` prints it, over the IPC socket. It names `localhost` until a hostname is configured, which reaches the node from its own machine or through an SSH tunnel; the HTTPS listener always has a certificate, so it always works.
- On success the node starts what depends on its identity and opens a session.

## Login and logout

- **Login** sends the login proof, compared in constant time with the node key's, and opens a session.
- **Logout** ends the session the cookie names, closing its sockets, and always answers with a cookie that clears it, whether or not the session existed.

## Sessions

- **The token** is random; the cookie is `__Host-Dassie-Session`, `HttpOnly`, `Secure`, `SameSite=Strict`. The node keeps only the token's SHA-256, as the session's ID ([D10](../decisions/D10-secrets-at-rest.md)), so the database alone gives no one a session.
- **A session ends** when unused for a while, after an absolute lifetime however used, and when the owner revokes it. The number kept is bounded, and a new session always stays, so a script that logs in over and over cannot grow the table, nor lock the owner out (`SessionPolicy` in `session.rs`).
- **Every way a session ends closes its RPC sockets** ([rpc.md](rpc.md#a-sessions-end-closes-its-sockets)).

## Certificates

The HTTPS listener always has a certificate: a self-signed one until ACME or the owner provides another. It reads the current one at every handshake, so a renewal or an installed certificate takes effect at once, without restarting the listener. TLS is rustls with ring as the only crypto provider (`tls.rs`); post-quantum key exchange is not built ([roadmap](../roadmap.md)).

What to serve is decided at start and whenever the hostname changes (`certificate::decide`):

- **Self-signed** is made once and stored, so a browser exception the owner made survives restarts; it is made again only when its names no longer fit or it nears expiry.
- **From ACME** it is kept while it covers the hostname; otherwise a self-signed one is served until renewal gets one for the new name.
- **From the owner** (`SetCertificate`) it is always kept and never renewed: it is the owner's business. It must parse, match its key and be valid now, or nothing changes.

## ACME

The daemon runs every ACME order ([RFC 8555](https://www.rfc-editor.org/rfc/rfc8555), http-01 only), the first as well as renewals, since the process that serves the challenge tokens must hold them; they live in memory only. `dassie init` asks for the first order over RPC and streams its progress back.

- **An account is created only once the owner agreed** to the CA's terms of service.
- **One order at a time.** A request that finds another order running waits for it and takes its certificate if it fits; a renewal that finds the owner's order running skips its turn.
- **A hostname change during an order** cannot install a certificate for the old name: the certificate is installed under the configuration's writer lock, so the change lands either before (the certificate is discarded) or after (it is replaced, as above).
- **Every request and every order is time-bounded,** so a stalled CA cannot hold the order lock for ever.
- **Renewal** (`acme::Renewal`) checks periodically and at once when the hostname changes, renews a missing, self-signed or soon-expiring ACME certificate, never the owner's, and backs off after failures, honouring the CA's `Retry-After`.

## Decisions

- [D5](../decisions/D05-ui-and-cli-rpc-contract.md): the RPC contract the `/rpc` WebSocket carries, and why the Origin check and per-call session checks came with it.
- [D9](../decisions/D09-open-payments-fully-and-no-spsp.md): why Open Payments routes are open to every origin.
- [D10](../decisions/D10-secrets-at-rest.md): session tokens stored only as hashes.
- [D16](../decisions/D16-engineering-style-pragmatic-tigerstyle-and-deterministic-simulation.md): the node's decisions as pure functions in `dassie-node-core`; every table and timer bounded.
