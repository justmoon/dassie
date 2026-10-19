# Open Payments

How the node receives and sends payments: it serves [Open Payments](https://github.com/interledger/open-payments-specifications) v1.4.0 (wallet addresses, a GNAP authorization server and a resource server) to other wallets and third-party clients, acts as an Open Payments client of other wallets, and gives its owner Receive and Send over RPC with the same logic. Money moves over STREAM through each ledger's receiver and payer.

**Paths:** `crates/dassie-payments/`, `crates/dassie-node/src/open_payments/`, `crates/dassie-node/src/http/open_payments/`, `crates/dassie-node/src/rpc/payments.rs`, `crates/dassie-node/src/rpc/grants.rs`, `crates/dassie-node/src/rpc/send.rs`, `crates/dassie-node/src/engine/core/incoming.rs`, `crates/dassie-node/src/engine/core/payers.rs`, `crates/dassie-node/src/engine/core/probes.rs`, `crates/dassie-node/src/store/payments.rs`, `crates/dassie-node/src/store/outgoing.rs`, `crates/dassie-node/src/store/gnap.rs`
**Read with:** [STREAM](stream.md), [Connector](connector.md), [Ledger](ledger.md), [HTTP](http.md), [RPC](rpc.md), [Wallet](wallet.md)

`dassie-payments` is sans-IO and `no_std`: the JSON of the resources, GNAP's rules, HTTP message signatures (RFC 9421) with `Content-Digest` (RFC 9530) over structured fields (RFC 8941), Ed25519 JWKs, payment pointers and URLs, RFC 3339 times and ISO 8601 repeating intervals. Time comes in as an argument. `dassie-node` serves and calls Open Payments with it.

## Scope

- **All three servers,** as the specs define them: the wallet address server with its `jwks.json`; the authorization server (grants, continuation, interaction, token management); the resource server (incoming payments, quotes, outgoing payments, grant spend).
- **Authorization:** every authorization-server and resource-server call needs an HTTP message signature by the client's key, and every resource-server call a GNAP access token, as the specs require. Only reading a wallet address, its key set, and the public view of an incoming payment need neither.
- **No SPSP.** Wallets that speak only SPSP cannot pay the node.
- **Not supported:** card authorization (an access item that asks for it is refused), since the node has no cards. STREAM receipts are not part of Open Payments and are not issued.
- Open Payments is served once the node is set up and has a hostname, which every URL names. Before that every Open Payments path answers 404 `not_found` `Open Payments is not set up`.

## Wallet addresses

**One wallet address per ledger,** since a wallet address has one asset.

- Each active ledger is at `https://<host>/pay/<slug>`, where the slug is the ledger ID up to its `+` (`stub`, `xrpl-testnet`). `<host>` includes the HTTPS port when it is not 443.
- `$host`, that is `https://<host>/.well-known/pay`, names the **default ledger**: the one the owner chose (`SetDefaultLedger`), or else the lowest active one. The default ledger's address is also at its `/pay/<slug>`.
- The node records every wallet address in its canonical form, `/pay/<slug>`, in grants and resources, so `$host` and `/pay/<slug>` mean the same, and `$host` stops naming a ledger when the default changes.

**`GET /.well-known/pay`, `GET /pay/<slug>`:**

```json
{
  "id": "<the URL asked for>",
  "publicName": "<the node's alias, if it has one>",
  "assetCode": "<the ledger's asset>",
  "assetScale": 9,
  "authServer": "https://<host>/gnap",
  "resourceServer": "https://<host>"
}
```

`Cache-Control: max-age=60`. 404 `not_found` `no such wallet address` for a path that names no active ledger; 503 `unavailable` while the engine is not running.

**`GET <wallet address>/jwks.json`:** the node's own key, with which it signs its requests to other wallets. It is Ed25519, derived from the node key at the seed path `node/open-payments-client` ([identity.md](identity.md)), so the same passphrase gives the same key. Its `kid` is `dassie-` and the first 8 bytes of the SHA-256 of the public key, in hex. `Cache-Control: max-age=300`.

## HTTP routes

They come before the wallet's fallback to `index.html` ([http.md](http.md#route-table)).

| Method | Path | Answer |
|---|---|---|
| `GET`, `HEAD` | `/.well-known/pay`, `/pay/<slug>` | the wallet address |
| `GET`, `HEAD` | `<either>/jwks.json` | its key set |
| `POST` | `/gnap` | 200, a [grant](#grant-requests) |
| `POST` | `/gnap/continue/{id}` | 200, the [continuation](#continuing) |
| `DELETE` | `/gnap/continue/{id}` | 204, the grant and its tokens revoked |
| `POST` | `/gnap/token/{id}` | 200 `{"access_token": …}`, the token [rotated](#access-tokens) |
| `DELETE` | `/gnap/token/{id}` | 204, the token revoked |
| `GET` | `/gnap/interact/{id}/{key}` | 302 to the wallet's consent page ([The owner's consent](#the-owners-consent)) |
| `POST` | `/incoming-payments` | 201, a new [incoming payment](#incoming-payments) |
| `GET` | `/incoming-payments?wallet-address=…` | a page of them |
| `GET`, `HEAD` | `/incoming-payments/{id}` | one |
| `POST` | `/incoming-payments/{id}/complete` | 200, completed |
| `POST` | `/quotes` | 201, a new [quote](#quotes) |
| `GET`, `HEAD` | `/quotes/{id}` | one |
| `POST` | `/outgoing-payments` | 201, a new [outgoing payment](#outgoing-payments) |
| `GET` | `/outgoing-payments?wallet-address=…` | a page of them |
| `GET`, `HEAD` | `/outgoing-payments/{id}` | one |
| `GET`, `HEAD` | `/outgoing-payment-grant` | what the token's grant spent |
| `OPTIONS` | any of these | 204, the CORS preflight |

Anything else under these prefixes is 404 `not_found`. Resource IDs are UUIDs (16 random bytes); `{id}` reads in either case.

They differ from the rest of the HTTP surface:

- **No `Origin` check.** Clients authenticate by GNAP token and signature, never by cookie.
- **CORS.** Every answer carries `Access-Control-Allow-Origin: *` and `Access-Control-Expose-Headers: WWW-Authenticate`, without credentials. A preflight `OPTIONS` gets 204 with `Access-Control-Allow-Methods: GET, HEAD, POST, DELETE, OPTIONS`, `Access-Control-Allow-Headers: Accept, Authorization, Content-Type, Content-Digest, Content-Length, Signature, Signature-Input` and `Access-Control-Max-Age: 600`.
- **Bodies** are at most 16 KiB (413), sent within 10 s (408), and a `POST` with a body must say `application/json` (415). Requests are read strictly: unknown fields are refused where the specs say `additionalProperties: false`, and every string and object has a bound.
- **Rate limits,** per client address (an IPv4 address or an IPv6 /56), in token buckets; past them: 429 `too_many_requests` `slow down`.

  | Bucket | Burst | Refill | Addresses tracked |
  |---|---|---|---|
  | Every request (not preflights) | 120 | one per 250 ms | 1024 |
  | Grant requests, new incoming payments, quotes and outgoing payments, on top | 20 | one per 3 s | 1024 |

- **The URL a signature covers** is the node's own origin with the request's path and query, whatever `Host` says, so a request signed for another server never verifies here.
- Responses are written exactly as the specs' schemas describe them, since the reference client validates them.

### Errors

Errors take the specs' shape, `{"error": {"code": "…", "description": "…"}}`. A 401 from the resource server carries `WWW-Authenticate: GNAP as_uri=https://<host>/gnap`.

| Status | Codes |
|---|---|
| 400 | `invalid_request` (malformed, out of range, not allowed by the rules below); GNAP's `too_fast`, `invalid_continuation`, `invalid_rotation` |
| 401 | `unauthorized` (resource server: no valid token or signature); GNAP's `invalid_client`, `invalid_continuation`, `request_denied` |
| 403 | `forbidden` (the token's grant does not reach it); `invalid_request` for a payment its grant refuses |
| 404 | `not_found`; GNAP's `invalid_rotation` |
| 408, 413, 415 | `invalid_request`, for the body |
| 429 | `too_many_requests` |
| 500 | `internal_error`; the details go to the log |
| 502 | `unreachable`: another wallet or the route to a receiver failed |
| 503 | `unavailable` while the engine is not running; GNAP's `request_denied` when too many grants are kept or pending |

`GET /gnap/interact/…` is read by a person, so its 404 is plain text.

## HTTP message signatures

Open Payments' profile of RFC 9421 (`httpsig.rs`):

- **Algorithm:** Ed25519 only. An `alg` parameter, if present, must be `ed25519`.
- **Covered components,** at least: `@method`, `@target-uri`, `authorization` when the request has one, and `content-digest` when it has a body. The signature's label is free; the first `Signature-Input` member that covers them is the one checked.
- **Parameters:** `keyid` names the key; `created` is required and must be within 300 s of the node's clock; `expires`, if present, must not have passed.
- **`Content-Digest`:** at least one of `sha-512` and `sha-256`, and every one present must match the body; others are ignored.
- A covered `content-length` that the transport dropped, as chunked bodies do, is the length of the body received; the digest binds the body itself.
- **Bounds:** a structured field is at most 8 KiB, a dictionary at most 16 members, an inner list at most 32 items, at most 16 parameters each, integers at most 999,999,999,999,999, decimals refused; at most 16 header lines of one name are joined.
- **When the node signs** as a client, it covers `@method`, `@target-uri`, `authorization` when it sends a token, and with a body `content-digest` (SHA-512), `content-length` and `content-type`, with label `sig1`, `keyid` and `created`.

## The authorization server

### Grant requests

`POST /gnap` takes a GNAP grant request (RFC 9635) as Open Payments profiles it: `access_token.access`, `client`, and optionally `interact` and `subject`. It is signed by the client's key.

**The rules** (else 400 `invalid_request`):

- It asks for access, or a subject, or both. `access` is not empty, has at most 3 items, and no two are the same.
- Each access item:

  | Type | Actions allowed | `identifier` | `limits` |
  |---|---|---|---|
  | `incoming-payment` | `create`, `complete`, `read`, `read-all`, `list`, `list-all` | optional | none |
  | `outgoing-payment` | `create`, `read`, `read-all`, `list`, `list-all` | required | optional |
  | `quote` | `create`, `read`, `read-all` | none | none |

  Actions are not empty and not repeated. An identifier is a URL, and must be one of this node's wallet addresses; it is rewritten to its canonical form. An item asking for card authorization is refused.
- **Limits** (`limits-outgoing`): `receiver`, if given, is an incoming payment's URL (its path has `/incoming-payments/<id>`, no query or fragment). A limit caps the amount debited or the amount received, never both: a request that names both `debitAmount` and `receiveAmount` is refused, as in Open Payments and its reference implementation, Rafiki. Amounts are more than 0. `interval` is an ISO 8601 repeating interval ([Intervals](#intervals)) whose windows are at least a minute, since each window spent in is kept.
- **Each access item keeps to its own limits.** A grant may have several outgoing-payment items for one wallet address, with different actions. Only one item per wallet address may allow `create` (compared after canonicalization): a request with two is refused, as which limits apply would be unclear.
- `subject` has exactly one `sub_ids` entry, with format `uri` and a URL as its ID.
- `interact.start` includes `redirect`. `interact.finish`, if given, has method `redirect`, a URL without fragment as its URI, and a nonce of 1 to 256 bytes.

**Which grants need the owner:** a grant asking for a subject (proof of a wallet address), any outgoing-payment item, any `list-all`, or `read-all` on quotes. Such a grant must ask to interact (else `invalid_request`), and a client known only by its key is refused (401 `invalid_client`), since the owner must see who asks. Incoming-payment and quote grants without those actions are approved at once. **Reading any incoming payment needs no owner:** a sender's wallet reads a payment it did not create to learn how to pay it, and all it shows is what paying needs.

**The client's key:**

- **By wallet address** (`client: {"walletAddress": …}`, or the deprecated bare string): the URL must parse (else `invalid_client`). The key is the one with the signature's `keyid` in the wallet address's `jwks.json`, which must be `https` without query or fragment. Key sets are fetched over the guarded HTTPS client ([The node as a client](#the-node-as-a-client)), at most 64 KiB within 10 s, and kept 5 minutes; at most 256 are kept, the oldest dropped first. A key set that cannot be had, or lacks the key, is 401 `invalid_client`. The grant records the client as its wallet address.
- **By key alone** (`client: {"jwk": …}`, "directed identity"): the JWK must be Ed25519. The grant records the client as `jwk:<x>`.
- The signature must name that key and verify (401 `invalid_client`). The grant keeps the key: every later request of the grant must be signed with it, so a stolen token is of no use without the key.

**A grant that needs no owner** is stored approved with its first access token, and answered at once:

```json
{
  "access_token": { "value": "…", "manage": "https://<host>/gnap/token/<manage id>", "expires_in": 3600, "access": [ … ] },
  "continue": { "access_token": { "value": "…" }, "uri": "https://<host>/gnap/continue/<grant id>" }
}
```

Continuing such a grant is 400 `invalid_continuation` (`the grant is final`). At most 16,384 grants are kept; past that a request is 503 `request_denied` until old ones are pruned.

### Access tokens

- Access and continuation tokens are 32 random bytes in base64url (43 characters). The node stores only their SHA-256 hashes ([D10](../decisions/D10-secrets-at-rest.md)).
- A client presents a token as `Authorization: GNAP <token>`, at most 256 printable ASCII characters.
- An access token works for 1 hour (`expires_in: 3600`), while its grant is approved.
- **Rotating** (`POST /gnap/token/{manage id}`, with the token and the grant's signature): a new token and a new manage URL; the old one stops working. An unknown token is 404 `invalid_rotation`; one whose grant is no longer approved, 400 `invalid_rotation`. An expired token can still be rotated as long as it is kept: up to 90 days after it expired for a grant the owner approved; others' tokens go when they expire.
- **Revoking** (`DELETE /gnap/token/{manage id}`, with the token and the grant's signature): 204. An unknown token is gone already: revoking is idempotent.
- **Revoking the grant** (`DELETE /gnap/continue/{id}`, with the continuation token and the grant's signature): 204, the grant and all its tokens. A wrong continuation is 401 `invalid_continuation`.

**Pruning,** at every grant request: tokens past their expiry (owner-approved ones after 90 more days); pending and denied grants past their expiry (a denied one stays until then, so its client learns why); revoked grants; approved grants decided more than an hour ago that have no token left; and what grants that are gone spent.

### The owner's consent

A grant that needs the owner waits, pending, until the owner decides it in the wallet.

1. **The request** is stored pending, and the client is told where to send the owner and how to continue:

   ```json
   {
     "interact": { "redirect": "https://<host>/gnap/interact/<grant id>/<key>", "finish": "<server nonce>" },
     "continue": { "access_token": { "value": "…" }, "uri": "https://<host>/gnap/continue/<grant id>", "wait": 5 }
   }
   ```

   The key and the server nonce are 24 random bytes each, in base64url; the key is per grant, so only the client and the owner know the way. The interaction state (key, nonces, reference, the client's finish URI) is a secret column, never shown. At most 256 grants are pending at once (503 `request_denied`), and a pending grant lapses after 10 minutes.
2. **The redirect.** `GET /gnap/interact/{id}/{key}` answers 302 to the wallet's consent page, `https://<host>/consent/{id}?key={key}`. For a grant that is no longer pending, has lapsed, or with a wrong key (compared in constant time) it answers 404 in plain text: `This request for your consent is no longer waiting: it was decided, or it lapsed after 10 minutes. Ask the app to start again.`
3. **The consent page** ([wallet.md](wallet.md)) reads the grant with `GetGrantRequest` (its ID and key). It shows who asks: the client's wallet address and its public name, read from the client's wallet address when the grant was requested (at most 128 characters; blank counts as none). It shows the access and its limits in plain words; payments without limits read as "of any amount". For a subject request it shows the wallet address to prove.
4. **The decision** (`DecideGrant`, approve or deny) draws an interaction reference (24 random bytes, base64url) and sends the owner back to the client's finish URI, with `?` or `&` and either `hash=<interaction hash>&interact_ref=<reference>` or `result=grant_rejected`. A client that named no finish URI gets no redirect.
5. **The interaction hash** (RFC 9635 §4.2.3): SHA-256 over the client's nonce, the server's nonce, the interaction reference and the grant endpoint URL (`https://<host>/gnap`), joined by `\n`, as unpadded base64url. When the node checks another server's hash, padded standard base64 is accepted too.

#### Continuing

`POST /gnap/continue/{id}`, with the continuation token, the grant's signature and optionally `{"interact_ref": …}`:

| The grant is | Answer |
|---|---|
| pending, lapsed | 401 `invalid_continuation` |
| pending, continued (or requested) less than 5 s ago | 400 `too_fast` |
| pending | 200, `continue` again with `wait: 5` |
| denied or revoked | 401 `request_denied` |
| approved, continued already | 401 `invalid_continuation`: rotate its token instead |
| approved | 200 with its access token (if it asked for access), and `continue` without `wait`, to revoke it with |

When the client named a finish URI, `interact_ref` is required and must match (constant time); without one it may be left out, but must match if given. A wrong reference is 401 `invalid_continuation`. Of two continuations at once, one issues the token. For a subject request, the answer's `subject` lists the asked-for wallet address only if it is one of this node's (format `uri`).

The owner's grants page lists the approved grants (`ListGrants`, `SubscribeGrants`: newest first, at most 256), each access item with its limits, its current window and what it spent there. `RevokeGrant` revokes a grant and its tokens.

### What a token reaches

- `read` and `list` reach only the resources the grant's client created; `read-all` and `list-all` reach every resource of the wallet address, and include the plain actions. `create` reaches the whole wallet address. `complete`, which has no `-all` form, reaches only what the client created.
- An item without an identifier reaches every wallet address of the node.
- **Each item keeps to its own limits.** On an outgoing-payment item that only reads or lists, a `receiver` limit narrows it to the payments to that receiver, and its amounts and interval limit nothing (they must still be valid). A list shows what any item that lists reaches.
- The limits of the item that allows `create` are what payments created with the grant may spend.

## Incoming payments

**Creating** (`POST /incoming-payments`):

- The token must allow `incoming-payment` `create` for the wallet address (403 `forbidden`); the creation rate limit applies.
- `walletAddress` must be one of the node's (400). `incomingAmount`, if given, is in the wallet address's asset and more than 0. `expiresAt`, if given, is an RFC 3339 date-time. `metadata` is a JSON object of at most 4 KiB.
- **Expiry:** a day from now if none is named. It must be in the future, and at most 7 days away for a client's payment (30 days for the owner's).
- **Bounds:** the owner's incoming payments and clients' are bounded apart, so that clients, whom anyone can be, never crowd out the owner: at most 1024 of the owner's and 4096 of clients' are open at once, over all ledgers. Expired ones are let go first. When clients' share is still full, the oldest of their payments that received nothing is closed to make room; when there is none, 429 `too_many_requests` `too many incoming payments are open`.
- The answer is 201 with the payment and the credentials it was made with (its `methods`), the creator's to pay it with.

**The resource:**

```json
{
  "id": "https://<host>/incoming-payments/<uuid>",
  "walletAddress": "https://<host>/pay/<slug>",
  "completed": false,
  "incomingAmount": { "value": "…", "assetCode": "…", "assetScale": 9 },
  "receivedAmount": { "value": "…", "assetCode": "…", "assetScale": 9 },
  "expiresAt": "…", "metadata": { }, "createdAt": "…",
  "methods": [ { "type": "ilp", "ilpAddress": "…", "sharedSecret": "<base64url>" } ]
}
```

Amounts are decimal strings of the asset's smallest unit. `completed` means it received all it asked for, or its creator completed it.

**Reading** (`GET /incoming-payments/{id}`):

- **Without `Authorization`:** anyone sees `{"receivedAmount": …, "authServer": "https://<host>/gnap"}`, which tells a sender where to get a grant.
- **With a token:** `read` reaches the client's own payments, `read-all` any. The answer is the whole resource. While the payment takes money, its `methods` carry **credentials of their own**: each read gets a new STREAM connection ([Receiving](#receiving)). A closed payment has none.

**Completing** (`POST /incoming-payments/{id}/complete`): `complete` reaches the client's own. Completing a completed payment changes nothing; an expired one is 400.

**Listing** (`GET /incoming-payments?wallet-address=…`, and the same for outgoing payments):

- `wallet-address` is required and must be one of the node's; `first=n` or `last=n` (1 to 100, not both; 20 forward by default); `cursor` a resource ID from an earlier page. A repeated parameter or an unknown cursor is 400.
- Newest first, only what the grant's listing items reach (403 when none lists). `pagination` gives `startCursor`, `endCursor`, `hasNextPage` (older) and `hasPreviousPage` (newer).

**Forgetting:** an incoming payment that received nothing is deleted 7 days after it closed or expired, at most 1000 at a time, whenever one is created; anyone may create them, so they must not pile up. What a closed payment's connections received is deleted then too.

## Quotes

**Creating** (`POST /quotes`, body `{walletAddress, receiver, method: "ilp", debitAmount?, receiveAmount?}`):

1. The token must allow `quote` `create` for the wallet address; `method` must be `ilp`; the creation rate limit applies.
2. The [receiver](#receivers) is read and must be payable.
3. The [rate](#rates-and-probes) of the route to it is measured.
4. The amounts are fixed:

   | Named | Debit | Receive |
   |---|---|---|
   | `debitAmount`, in the ledger's asset | as named | what it buys at the rate, rounded down |
   | `receiveAmount`, in the receiver's asset | what it costs at the rate, rounded up, so it always covers it | as named |
   | neither | the cost of what the receiver still accepts | what it still accepts; a receiver that takes any amount needs one named |
   | both | refused | |

   Both must be more than 0 and at most the ledger's largest amount, and what is received at most what the receiver still accepts.
5. The quote is stored and answered 201: `{id: "https://<host>/quotes/<uuid>", walletAddress, receiver, receiveAmount, debitAmount, method: "ilp", expiresAt, createdAt}`.

- **A quote lasts 5 minutes,** or less if its receiver expires sooner, and pays one outgoing payment.
- **Live quotes:** at most 4096 of clients' and, apart, 256 of the owner's, so clients cannot crowd the owner out (429 `too many quotes are live`).
- **Reading** (`GET /quotes/{id}`): `read` reaches the client's own; `read-all`, which needs the owner, any.
- **It pays from the ledger the route starts on,** since the node's own endpoints never convert ([ledger.md](ledger.md)): a quote whose wallet address is on another ledger is 400 `pay from <ledger>: the route to the receiver starts there`.

### Rates and probes

- Without a route to the receiver's ILP address: 502 `unreachable` `there is no route to the receiver`.
- To one of the node's own receivers the rate is one for one: they convert and charge nothing.
- Otherwise a **probe** measures it: a STREAM connection from the ledger's payer that sends only packets the receiver cannot fulfill, as STREAM measures rates ([stream.md](stream.md)). Intermediaries charge fees, so even a same-asset route is probed.
  - At most 8 probes run at once (429 past that), each for at most 10 s.
  - Probe packets hold the owner's funds while in flight, as the payment's would.
  - The quote's rate is the measured rate less STREAM's slippage, 1%. A rate that cannot be measured, or one that delivers nothing, is 502 `unreachable`: there is no quote.

## Outgoing payments

**Creating** (`POST /outgoing-payments`): `{walletAddress, quoteId, metadata?}`, or `{walletAddress, incomingPayment, debitAmount, metadata?}`.

- The token must allow `outgoing-payment` `create` for the wallet address; the creation rate limit applies. `metadata` is at most 4 KiB.
- **From a quote:** it must exist and be the same client's and the same ledger's (403 `the quote is another's`), not expired, and not paid already.
- **From an incoming payment:** the receiver is read and the rate measured now; the debit is fixed.
- **The node's checks** (400 unless said):
  - the ledger is active;
  - the amounts are more than 0 and in range; the receiver's ILP address, at most 1023 bytes, parses; URLs are at most 2048 bytes; the receiver's asset code is 1 to 32 bytes;
  - at most 4096 outgoing payments are unfinished (429);
  - **credentials are used once:** no other outgoing payment pays to the same destination (`another payment uses the receiver's credentials: read the receiver again`). Each payment has a STREAM connection of its own, which the receiver tells by its credentials; two payments sharing them would each be told what both delivered. A wallet that gives the same credentials to every read must be read again for each payment;
  - **the funds:** what the owner's group on the ledger has free (credits posted, less debits posted and pending) must cover this debit and what the unfinished payments on that ledger still have to send (`the funds do not cover the payment`).
- **Grant limits** are reserved when the payment is created, in the window of the grant's interval it is created in (403 `invalid_request` when refused):
  - a `receiver` limit must name the payment's receiver;
  - a `debitAmount` limit must be in the ledger's asset, a `receiveAmount` limit in the receiver's;
  - some window of the interval must hold now (without an interval, all time is one window);
  - what the window spent, with this payment, must stay within the limit.

  The answer then carries `grantSpentDebitAmount` and `grantSpentReceiveAmount`: the window's totals with this payment.
- The answer is 201 with `{id, walletAddress, quoteId?, failed, receiver, receiveAmount, debitAmount, sentAmount, metadata?, createdAt}`, and the payment starts sending at once.

**Reading and listing:** `read`/`list` reach the client's own payments, `read-all`/`list-all` any, each within its item's `receiver` limit.

**`GET /outgoing-payment-grant`:** what the token's grant spent in the current window of its interval, `{spentDebitAmount, spentReceiveAmount}`, each `null` when nothing. It reads the item that allows `create`, else the first outgoing-payment item.

### Sending

Each ledger has its own [payer](#stream-endpoints), a local endpoint that debits `L:equity/owner/local/pay`, a sub-account in the owner's group ([ledger.md](ledger.md)), so the group's limit is the outgoing payments' limit too. Grant limits are checked when a payment is created, not by the connector.

- **One STREAM connection per payment** that is sending; at most 16 send at once, and the rest wait their turn.
- **Each packet must deliver at least the payment's rate,** its receive amount over its debit amount: a worse path fails it.
- **It never debits more than its debit amount,** across crashes too: its sent amount is what was posted, written with the journal row. A packet in flight at a crash was never posted, so a restart sends only the rest, on a new connection with the same credentials.
- **It completes** once it has sent its debit amount. One that fixes the amount to send also completes once it delivered what its quote said and its receiver takes no more (a request it filled). One that fixes the amount to deliver also completes once its receiver says it has all it asked for, or all of it but less than a unit sent is worth, which no packet can be sized to deliver as the hops round down. A receiver may say so although the payment heard of less, when a hop lost a Fulfill it paid for ([D23](../decisions/D23-each-node-books-its-own-fulfills.md)): a completed incoming payment tells a sender what its connection delivered and that no more fits.
- **A connection that closes early** (an idle timeout while liquidity is short, a receiver that restarted) is opened again for the rest, up to 8 times, waiting 1 s and doubling each time.
- **It fails** when the receiver keeps refusing it, past those 8 times, or 10 minutes after it was created.
- **It ends only with none of its packets in flight,** so what it says it sent is final. What it did not send is released from its grant's window.
- **To the node's own receiver** a payment goes through the connector like any other, through a bounded queue (1024 packets, 256 a step) drained at the end of the step, so nothing recurses.
- Its state is `sending`, `completed` or `failed`, with why it failed for people.

## STREAM endpoints

Open Payments' money moves over STREAM ([stream.md](stream.md)). Each active ledger has two local endpoints in the connector ([connector.md](connector.md#addresses-and-endpoints)), added when its scheme activates and removed when it goes:

| Endpoint | ILP address | Books | STREAM side |
|---|---|---|---|
| Receiver | `<own>.stream-<slug>` | credits `L:equity/owner` | a STREAM server for every incoming payment on the ledger |
| Payer | `<own>.pay-<slug>` | debits `L:equity/owner/local/pay` | the client of each outgoing payment's connection and of each probe |

`<own>` is the node's ILP address, `<slug>` the ledger ID up to its `+`. Both accounts are in the owner's group ([ledger.md](ledger.md)).

**The receiver** is a STREAM server ([stream.md](stream.md#server)):

- Its server secret derives from the node key at `node/stream-server/<slug>` ([identity.md](identity.md#seed-paths)), so the credentials it issued stay valid across restarts, and the node keeps no table of them.
- It holds at most 256 connections. A full table forgets closed connections first, then the one heard from least recently; that sender's next packet opens it again, and [admission](#receiving) tells it what it received.
- Its connections use STREAM's default policy, except that a stream the sender opens takes money from its first packet (`remote_stream_receive_max` is unbounded): each incoming payment limits its connection through admission instead ([stream.md](stream.md#money-limit)).
- Data a sender puts on a stream is read and dropped. The receiver issues no STREAM receipts.

**The payer** takes no packets: a Prepare routed to it is rejected at once with F02 `a payer takes no packets`. Each outgoing payment that is sending, and each probe, has a STREAM client connection of its own on the payer, in the ledger's asset, with STREAM's default policy (packets expire after 30 s, 1 to 100 in flight, the connection closes after 60 s idle; [stream.md](stream.md#policy)):

- a payment's connection opens one stream, whose send maximum is the payment's debit amount less what it sent, at the payment's rate ([Sending](#sending));
- a probe's connection only measures the exchange rate and carries no money ([Rates and probes](#rates-and-probes)).

## Receiving

What the ledger's [receiver](#stream-endpoints) takes for incoming payments:

- **A packet is fulfilled only for an open incoming payment on that ledger,** and only up to what that payment still expects (without an `incomingAmount`, up to the ledger's largest total). What the receiver took counts as soon as the packet is posted, whatever the upstream does with the Fulfill ([D23](../decisions/D23-each-node-books-its-own-fulfills.md)). The payment's new received amount is written in the same transaction as the journal row, so a payment never shows money the books lack, nor the other way round.
- **A STREAM connection per payment.** Each read of an incoming payment's methods gets credentials of its own: the STREAM token's tag is the payment's ID (16 bytes) and a random connection key (16 bytes). Each payment that pays it therefore has a connection of its own, and the receiver tells each connection what it received, not what the payment did, so a sender never takes another's delivery for its own: of two payments to one incoming payment, the one that delivered less fails rather than completes. What each connection received is written with the payment's.
- The credentials kept with the payment are one connection like any other: the owner's wallet shows them, to pay with STREAM alone. Credentials issued before connection keys carry the ID alone (a 16-byte tag), and are one connection.
- **Admission.** Before a packet reaches a connection, new or known, the receiver reads its tag:

  | The tag names | Admission |
  |---|---|
  | an open incoming payment on this ledger, not expired | accepted: the connection may receive what the payment still expects over all its connections, and is told what it received itself. A new connection past the payment's 32 is refused (F99) |
  | a completed payment still remembered, on this ledger | accepted with nothing more to receive, and told what it received |
  | anything else: a tag that does not parse, an unknown payment, one on another ledger, one expired | refused (F99) |

- **After it completes,** the node remembers what each connection of the last 4096 completed payments received, so a sender still sending is told what its connection delivered and that no more fits, rather than refused without a word.

## Receivers

Paying an incoming payment, here or on another node, needs to read it (`spending.rs`):

- Its URL is at most 2048 bytes and is an incoming payment's (`/incoming-payments/<id>`, no query or fragment).
- One at this node's origin is read from the database, with fresh credentials. One elsewhere is read as a client ([The node as a client](#the-node-as-a-client)); if that wallet cannot be reached, 502 `unreachable`.
- It must be payable: not completed, not expired, with an `ilp` method whose address parses (at most 1023 bytes) and whose shared secret is 32 bytes of base64url. What it still accepts is its `incomingAmount` less its `receivedAmount`, or anything.

## The node as a client

The node pays and creates incoming payments on other wallets as an Open Payments client, as its own `$host` wallet address and key.

- **Reading an incoming payment elsewhere,** to pay it: `GET` it without a token for its `authServer`; get a token from that authorization server with a non-interactive grant for `incoming-payment` `read-all`; `GET` it again, signed, with the token. The payment's `id` must be the URL read.
- **Creating an incoming payment elsewhere,** for the owner's Send: a token for `incoming-payment` `create`, then `POST <resourceServer>/incoming-payments`, signed, expecting 201. The payment that comes back must be at that wallet: its `walletAddress` at the wallet address's origin (a wallet may name the account by another of its addresses), and its `id` an incoming payment's URL at the resource server's origin.
- **Only what it needs:** `read-all` to pay an incoming payment, `create` to make one, each its own token.
- **Tokens are cached** by authorization server, resource origin and action, so a payment that names another wallet's authorization server is never sent that wallet's token. At most 64 are kept (the soonest to expire goes first); a token is not used in the last 30 s before it expires, and is kept at most an hour (a minute when the server gives no `expires_in`). A 401 forgets the token and asks once more.
- **Resolving a wallet address on another node:** it is paid only if it names itself: its `id` is `https` without query or fragment, at the origin it was read from; its `authServer` and `resourceServer` are HTTPS URLs; its asset code is 1 to 32 bytes; its public name at most 256 bytes.
- **Origins** compare without case and without the scheme's default port.
- **Every request** goes through the node's guarded HTTPS client ([peer-protocol.md](peer-protocol.md)): public addresses only (private ones only where dev environments allow them), no redirects, answers of at most 64 KiB, within 10 s.

## The owner's side

Over RPC ([rpc.md](rpc.md#procedure-groups)), the owner needs no grant. Every method is `FAILED_PRECONDITION` until the node has a hostname.

- `ListWalletAddresses`: one per active ledger, each as a URL and a payment pointer (`$host`, or `$host/pay/<slug>`), saying whether `$host` names it. `SetDefaultLedger`: the ledger `$host` names (`NOT_FOUND` unless active; empty for the lowest active one).
- `CreateIncomingPayment`: on the default ledger unless one is named; the most it accepts, or any; an expiry (0 for a day from now, at most 30 days); a description of at most 256 characters, which goes into its metadata as `description`. This is how the owner asks for a payment, or lets a STREAM client pay the node. `RESOURCE_EXHAUSTED` when the owner's 1024 are open.
- `CompleteIncomingPayment`: `NOT_FOUND` for an unknown one, `FAILED_PRECONDITION` for an expired one.
- `ListIncomingPayments`, `ListOutgoingPayments` and their `Subscribe*` streams: the newest, up to 1000 (100 by default), owner's and clients' alike.

### The owner's Send

The owner's Send quotes and pays as the owner, with the logic third-party clients go through.

- **The recipient** (`ResolveRecipient`, and the input of `QuoteSend`) is a payment pointer or wallet address URL, here or on another node, or a **payment request**: an incoming payment's URL, as Receive hands out.
  - A wallet address of this node is read from its ledgers; `$host` stays `$host`, as the owner wrote it. Another is [resolved](#the-node-as-a-client).
  - A payment request is read and must be payable now. It speaks only for its own wallet: one here for one of this node's wallet addresses, one elsewhere for a wallet whose resource server is where the request is; never one elsewhere for this node's. Otherwise it could name anyone, the owner too, and be paid as them.
  - Text from other wallets (public names, descriptions) loses control and direction-changing characters, so it cannot read as something else beside the payment pointer; a description is cut to 256 characters.
  - **Lookups:** at most 4 run at once, since each may fetch from other wallets as the node; the rest wait up to 30 s, then `RESOURCE_EXHAUSTED`.
- **`QuoteSend`** fixes either the amount the recipient gets or the amount sent, never both (each more than 0 and in range):
  - **To a wallet address,** an amount is needed. Quoting first creates the incoming payment there, for 30 minutes (the quote's 5 and the payment's 10, with room to spare), with the owner's note (at most 256 characters) as its description. It expects what the recipient is to get when that is fixed; when the amount to send is fixed, it goes out whole and the incoming payment takes any amount.
  - **To a payment request,** it is paid as it is: what it still accepts, or less. It has its own description, so a note is refused.
  - The ledger is the one the route to the recipient starts on; naming another is `INVALID_ARGUMENT`.
  - If quoting fails after the node created an incoming payment here, that payment is completed; one created elsewhere lapses with its expiry.
  - The quote and the payment keep the wallet address paid, and whether they fix the amount to send or to deliver, for the owner's pages.
- **`SendPayment`** pays a quote of the owner's (`INVALID_ARGUMENT` once it expired or was paid; `FAILED_PRECONDITION` when the funds do not cover it). **`SubscribeOutgoingPayment`** follows one payment, whole, each time it changes, for the payment status page.

## Payment pointers

Payment pointers stay only as input syntax:

- `$host/path` means `https://host/path`;
- a bare `$host` (or `$host/`) means `https://host/.well-known/pay`.

A wallet address URL is `https`, with a host, without user information, query or fragment. URLs are parsed strictly (`uri.rs`): printable ASCII only, user information refused, at most 2048 bytes and 16 query pairs, so two parties never read one URL differently. The node shows its own wallet addresses as payment pointers too.

## Intervals

Grant limits use ISO 8601 repeating intervals, `R[n]/<start>/<duration>`, `R[n]/<start>/<end>` or `R[n]/<duration>/<end>`, with windows as the reference implementation (Rafiki, over luxon) computes them, so clients see the same ones anywhere:

- `Rn` is the first interval and `n` repetitions after it (before it, for `<duration>/<end>`); `R` and `R-1` repeat for ever.
- Each repetition starts where the one before ended and adds the duration from there: a month from January 31 ends on February 28, and the next window then starts on March 28. A window contains its start, not its end.
- Durations with years or months step one window at a time, at most 1200 steps (a century of months); fixed-length durations are divided out.
- Every window must be at least a minute.

Times are RFC 3339 date-times, held as milliseconds since the Unix epoch, up to the end of year 9999.

## Persistence

What survives a restart, written with the journal rows that move the money where there are any ([node.md](node.md) has the database as a whole):

- **Incoming payments:** ledger, wallet address, creating client (none for the owner), incoming and received amounts, expiry, completion, metadata, and the STREAM credentials kept with it; and what each of an open payment's connections received. At a start, the open ones are read back (at most 65,536), within the owner's and clients' bounds.
- **Quotes:** the client, the receiver and its credentials, debit and receive amounts, expiry; for the owner's, the wallet address paid and whether it fixes the amount sent.
- **Outgoing payments:** everything a quote has, plus the grant and window it was reserved in, its sent amount (what was posted), state, failure, deadline and finish time. At a start, those still sending are read back (at most 4096) and continue.
- **Grant spend:** for each grant and window, what was debited and delivered, written with the payment that spends it.
- **GNAP grants:** state (`pending`, `approved`, `denied`, `revoked`), client, client key, access, whether it needs the owner, the continuation token's hash, and the interaction state; **access tokens** as SHA-256 hashes, with their grant, manage ID and expiry.
- **Secret columns,** never shown by the database inspector: the shared secrets of incoming payments, quotes and outgoing payments, token and continuation hashes, and the interaction state ([rpc.md](rpc.md#the-inspector)).
- In memory only: clients' key sets, the node's tokens at other wallets, the rate limiters, probes and the payers' packets in flight.

## Bounds

| Bound | Value |
|---|---|
| Access items per grant | 3 |
| Grants kept / pending | 16,384 / 256 |
| Pending grant lapses | 10 minutes |
| Access token lifetime | 1 hour |
| Continuation wait | 5 s |
| Open incoming payments | 1024 the owner's, 4096 clients' |
| Incoming payment expiry | default 1 day; at most 7 days for clients, 30 for the owner |
| Connections per receiver (one per ledger) | 256 |
| Connections per incoming payment | 32 |
| Completed payments remembered for their senders | 4096 |
| Unpaid incoming payments forgotten | 7 days after they close |
| Metadata | 4 KiB |
| Live quotes | 4096 clients', 256 the owner's |
| Quote lifetime | 5 minutes |
| Probes | 8 at once, 10 s each |
| Unfinished outgoing payments | 4096 |
| Sending at once | 16 |
| Outgoing payment deadline | 10 minutes |
| Reconnects per payment | 8, from 1 s doubling |
| The owner's lookups | 4 at once, waiting up to 30 s |
| The Send's incoming payment | 30 minutes |
| Client key sets | 256, 5 minutes each, 64 KiB |
| The node's tokens at other wallets | 64, at most an hour |
| Requests to other wallets | 10 s, 64 KiB |
| Signature clock skew | 300 s |
| Page size | 1 to 100, default 20 |

## Tests

`dassie-payments` follows the sans-IO split ([D16](../decisions/D16-engineering-style-pragmatic-tigerstyle-and-deterministic-simulation.md)): golden vectors from the standards' own examples (RFC 9421, RFC 9530, RFC 8941, the interaction hash, repeating intervals) and properties that its parsers never panic. The engine's money side (receivers, payers, quotes, grant spend) runs in the node simulation; the HTTP and GNAP services are tested over real HTTPS, end to end and against the reference client `@interledger/open-payments` ([testing.md](testing.md)).

## Decisions

- [D9](../decisions/D09-open-payments-fully-and-no-spsp.md): Open Payments in full, no SPSP; why tokens are needed to create incoming payments, and why GNAP and outgoing payments are not deferred.
- [D6](../decisions/D06-owner-ledger-and-multi-currency-accounts.md): the owner's group, which the receiver and payer book into, and why the owner's endpoints never convert.
- [D7](../decisions/D07-exchange-rates-fees-and-spread.md): rates and fees, which make quotes probe.
- [D10](../decisions/D10-secrets-at-rest.md): tokens stored as hashes.
- [D23](../decisions/D23-each-node-books-its-own-fulfills.md): why a receiver's word can complete a payment that heard of less.
- [D19](../decisions/D19-each-node-keeps-its-own-books.md): each node's books are its own; a payment's sent amount is what its own books posted.

## Original

- [4 §9](../original/04-interledger.md): the original's Open-Payments-style wallet address and unauthenticated incoming payments with hard-coded amounts, and its unfinished `makePayment` over the upstream `ilp-protocol-stream`.
- [2 §15](../original/02-node-runtime.md): its payment pointer resolution.
