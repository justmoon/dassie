# Open Payments

How the node receives and sends payments. It serves [Open Payments v1.4.0](https://github.com/interledger/open-payments-specifications) (wallet addresses, a [GNAP](https://www.rfc-editor.org/rfc/rfc9635) authorization server and a resource server) to other wallets and third-party clients, acts as an Open Payments client of other wallets, and gives its owner Receive and Send over RPC with the same logic. Money moves over STREAM through each ledger's receiver and payer.

**Paths:** `crates/dassie-payments/` (the protocol's pure parts: resources, GNAP rules, HTTP message signatures, intervals, URLs), `crates/dassie-node/src/open_payments/`, `crates/dassie-node/src/http/open_payments/`, `crates/dassie-node/src/engine/core/` (`incoming.rs`, `payers.rs`, `probes.rs`), `crates/dassie-node/src/rpc/` (`payments.rs`, `grants.rs`, `send.rs`), `crates/dassie-node/src/store/` (`payments.rs`, `outgoing.rs`, `gnap.rs`)
**Read with:** [STREAM](stream.md), [Connector](connector.md), [Ledger](ledger.md), [HTTP](http.md), [RPC](rpc.md), [Wallet](wallet.md)

The node implements what the specs define as they define it: resources, GNAP flows, [HTTP message signatures](https://www.rfc-editor.org/rfc/rfc9421) on every call the specs protect, error shapes, pagination, and [ISO 8601 repeating intervals](https://en.wikipedia.org/wiki/ISO_8601#Repeating_intervals) for grant limits, with windows computed as the reference implementation, Rafiki, computes them, so clients see the same windows anywhere. The reference client `@interledger/open-payments` runs against the node in the interop tests ([testing.md](testing.md#interop)) and pins that. This doc states only what the specs leave to the node, and why.

Not supported: SPSP ([D9](../decisions/D09-open-payments-fully-and-no-spsp.md)), card authorization (the node has no cards), and STREAM receipts, which are not part of Open Payments. Payment pointers are accepted only as input syntax.

## Wallet addresses

- **One wallet address per ledger,** since a wallet address has one asset: `https://<host>/pay/<slug>`, where the slug is the ledger ID up to its `+`.
- **`$host` names the default ledger:** the one the owner chose, or else the lowest active one. Grants and resources record every wallet address as its `/pay/<slug>` form, so changing the default never moves a grant or a payment to another ledger.
- **The key in `jwks.json`** signs the node's own requests to other wallets. It derives from the node key at `node/open-payments-client` ([identity.md](identity.md)), so restoring from the passphrase restores the key other wallets know the node by.
- Open Payments is served only once the node is set up and has a hostname, which every URL names.

## HTTP routes

The routes are in `crates/dassie-node/src/http/open_payments/`. Unlike the owner's endpoints ([http.md](http.md#same-origin-rule)) they are for anyone:

- **No `Origin` check, CORS for every origin, without credentials.** A client proves itself by its GNAP token and its signature, never by a cookie.
- **The URL a signature covers is the node's own origin** with the request's path and query, whatever `Host` says, so a request signed for another server never verifies here.
- **Rate limits per client address,** with a tighter one on top for whatever creates state (grants, incoming payments, quotes, outgoing payments), since anyone may create them.
- **Requests are read strictly** (unknown fields refused where the schemas forbid them, every string and object bounded, URLs parsed by one strict parser, `uri.rs`, so the node and a client never read one URL differently); **responses are written exactly as the schemas describe them,** since the reference client validates them.

## Grants

### The owner's consent

**Which grants need the owner:** any outgoing-payment access, any `list-all`, `read-all` on quotes, and a subject request (proof of a wallet address). These move the owner's money or show more than the client's own resources. Such a grant must ask to interact, and a client known only by its key ("directed identity") is refused, since the owner must see who asks.

**Which do not:** incoming-payment and quote grants without those actions are approved at once. Creating an incoming payment only asks to be paid, and a quote moves nothing. **Reading any incoming payment needs no owner,** because a sender's wallet reads a payment it did not create to learn how to pay it, and the payment shows only what paying needs.

The flow is GNAP's redirect interaction. The interaction URL carries a random key per grant, so only the client and the owner know the way to the consent page in the wallet ([wallet.md](wallet.md)). Pending grants lapse, and their number is bounded, so a flood of requests cannot grow the table.

### What a grant allows

- **A grant is bound to its client's key:** every later request of the grant must be signed with it, so a stolen token is of no use without the key.
- **A limit caps the amount debited or the amount received, never both,** as in the specs and Rafiki.
- **Each access item keeps to its own limits,** and only one item per wallet address may allow `create`, since otherwise which limits apply would be unclear. On an item that only reads or lists, a `receiver` limit narrows what it reaches.
- An item without an identifier reaches every wallet address of the node.
- **Limits are reserved when a payment is created,** in the interval's current window, and what the payment did not send is released when it ends. Windows are at least a minute, since each window spent in is stored.
- Access and continuation tokens are stored only as hashes ([D10](../decisions/D10-secrets-at-rest.md)).

## Clients never crowd out the owner

Anyone can be a client. So the owner's incoming payments and quotes are bounded apart from clients' (`IncomingPolicy` in `engine/core/incoming.rs`, and `MAX_LIVE_OWNER_QUOTES` in `store/outgoing.rs`), and a flood of clients' requests never stops the owner from asking to be paid or from paying. When clients' share is full, the oldest of their incoming payments that received nothing is closed to make room: a flood pushes out only other unpaid requests. Incoming payments that received nothing are deleted some days after they close, so they do not pile up.

## Receiving

Each active ledger has a **receiver**, a local STREAM server endpoint at `<own>.stream-<slug>` that credits `L:equity/owner` in the owner's group ([ledger.md](ledger.md), [connector.md](connector.md#addresses-and-endpoints)). Its server secret derives from the node key at `node/stream-server/<slug>` ([identity.md](identity.md#seed-paths)), so the credentials it issued stay valid across restarts and the node keeps no table of them.

- **A packet is fulfilled only for an open incoming payment on that ledger,** and only up to what the payment still expects. What the receiver took counts once the packet is posted, whatever the upstream does with the Fulfill ([D23](../decisions/D23-each-node-books-its-own-fulfills.md)). The payment's received amount is written in the same transaction as the journal row, so a payment never shows money the books lack, nor the other way round.
- **One STREAM connection per payment.** Each read of an incoming payment hands out fresh credentials: the STREAM connection tag is the payment's ID and a random connection key. So each payment that pays it has a connection of its own, and the receiver tells each connection what *it* received, not what the incoming payment did. A sender never takes another's delivery for its own: of two payments to one incoming payment, the one that delivered less fails rather than completes.
- **Admission.** Before a packet reaches a connection, the receiver reads its tag and lets the connection take at most what the incoming payment still expects over all its connections. That is why the receiver's streams accept money from their first packet ([stream.md](stream.md#money-limit)): the incoming payment, not STREAM's stream limit, bounds the connection.
- **After it completes,** the node remembers for a while what each connection received, on disk, so a sender that lost a Fulfill to a crash and reconnects, even to a receiver that restarted meanwhile, learns that its money arrived rather than being refused. The retention is at least the longest a node's own payer tries (`FINISHED_RETENTION_MS`, asserted against `OUTGOING_DEADLINE_MS` in `payers.rs`).

## Outgoing payments

Each active ledger has a **payer**, a local endpoint at `<own>.pay-<slug>` that takes no packets and debits `L:equity/owner/local/pay`, a sub-account in the owner's group. So the group's limit is the outgoing payments' limit too. Each payment that is sending has a STREAM client connection of its own on the payer.

**Quotes.** A quote measures the route with a STREAM rate probe ([stream.md](stream.md)) from the payer, even between two accounts in one asset, since intermediaries charge fees; to the node's own receivers the rate is one for one. A quote pays from the ledger the route starts on, because the node's own endpoints never convert ([D6](../decisions/D06-owner-ledger-and-multi-currency-accounts.md)). Probe packets hold the owner's funds while in flight, as a payment's would.

**Creating a payment:**

- **Credentials are used once:** no two outgoing payments may pay to the same destination. Two payments on one set of credentials would share a connection and each be told what both delivered. A wallet that hands out the same credentials on every read must be read again for each payment.
- **The funds must cover it:** what the owner's group has free on the ledger must cover this debit and what the unfinished payments on that ledger still have to send.
- Grant limits are reserved ([What a grant allows](#what-a-grant-allows)); the connector does not check them.

**Sending:**

- **Each packet must deliver at least the payment's rate,** its receive amount over its debit amount: a worse path fails it.
- **It never debits more than its debit amount, across crashes too.** Its sent amount is what was posted, written with the journal row. A packet in flight at a crash was never posted, so a restart sends only the rest, on a new connection with the same credentials.
- **It completes** once it has sent its debit amount, or, when the amount to deliver is fixed, once the receiver says it has all it asked for, or all but less than one unit sent is worth (which no packet can be sized to deliver, since hops round down). A receiver may say so although the payment heard of less, when a hop lost a Fulfill it paid for ([D23](../decisions/D23-each-node-books-its-own-fulfills.md)).
- **It ends only with none of its packets in flight,** so what it says it sent is final. A connection that closes early is reopened for the rest, a bounded number of times, and a payment fails at its deadline (`payers.rs`). A connection the receiver closes without an error is not reopened: the receiver takes no more, as when another payment filled the incoming payment, so the payment ends at once.
- **To the node's own receiver** a payment goes through the connector like any other, through a bounded queue drained at the end of the engine's step, so nothing recurses.

## The node as a client

The node pays and creates incoming payments on other wallets as an Open Payments client, as its own `$host` wallet address and key (`open_payments/client.rs`).

- **Only what it needs:** a non-interactive `incoming-payment` `read-all` grant to pay an incoming payment, `create` to make one, each its own token. Tokens are cached by authorization server, resource origin and action, so a payment that names another wallet's authorization server is never sent that wallet's token.
- **What comes back must belong where it was asked:** a wallet address elsewhere is paid only if it names itself (its `id` at the origin it was read from), and an incoming payment created elsewhere must be at that wallet (its `walletAddress` at the wallet address's origin, its `id` at the resource server's origin).
- **Every request** goes through the node's guarded HTTPS client ([peer-protocol.md](peer-protocol.md)): public addresses only, no redirects, bounded answers and times.

## The owner's side

The owner reaches all of this over RPC ([rpc.md](rpc.md)), without a grant, through the same code third-party clients go through: Receive creates an incoming payment on a ledger the owner picks, and the owner's pages list incoming and outgoing payments, owner's and clients' alike.

### The owner's Send

- **The recipient** is a payment pointer, a wallet address URL, or a **payment request** (an incoming payment's URL, as Receive hands out), here or on another node.
- **A payment request speaks only for its own wallet:** one here for one of this node's wallet addresses, one elsewhere only for a wallet whose resource server is where the request is, never one elsewhere for this node's. Otherwise a request could name anyone, the owner too, and be paid as them.
- **Text from other wallets** (public names, descriptions) loses control and direction-changing characters, so it cannot read as something else beside the payment pointer.
- **Lookups are bounded,** since each fetches from other wallets as the node.
- **To a wallet address,** quoting first creates an incoming payment there, which outlasts the quote and the payment's deadline. When the amount to send is fixed, the incoming payment takes any amount. If quoting fails, an incoming payment created here is completed; one created elsewhere lapses with its expiry.
- **A payment request is paid as it is,** what it still accepts or less; it carries its own description, so the owner's note is refused.

## Persistence

- **What moves money is written with the journal rows that move it:** an incoming payment's received amount and what each of its connections received, an outgoing payment's sent amount, and a grant's spend per window. So the resources never disagree with the books.
- Open incoming payments and outgoing payments still sending are read back at start; the payments continue.
- Shared secrets, token hashes and the GNAP interaction state are secret columns, which the database inspector never reads ([rpc.md](rpc.md#the-inspector)).
- In memory only: clients' key sets, the node's tokens at other wallets, the rate limiters, probes, and the payers' packets in flight.

## Decisions

- [D9](../decisions/D09-open-payments-fully-and-no-spsp.md): Open Payments in full as its specs define it, no SPSP, and GNAP tokens even to create incoming payments.
- [D6](../decisions/D06-owner-ledger-and-multi-currency-accounts.md): the owner's group, which the receiver and payer book into, and why the owner's endpoints never convert.
- [D7](../decisions/D07-exchange-rates-fees-and-spread.md): rates and fees, which make quotes probe.
- [D10](../decisions/D10-secrets-at-rest.md): tokens stored as hashes.
- [D19](../decisions/D19-each-node-keeps-its-own-books.md): a payment's sent amount is what the node's own books posted.
- [D23](../decisions/D23-each-node-books-its-own-fulfills.md): why a receiver's word can complete a payment that heard of less.
