# Open Payments

How the node receives and sends payments. It serves [Open Payments v1.4.0](https://github.com/interledger/open-payments-specifications) (wallet addresses, a [GNAP](https://www.rfc-editor.org/rfc/rfc9635) authorization server and a resource server) to other wallets and third-party clients, acts as an Open Payments client of other wallets, and gives its owner Receive and Send over RPC with the same logic. Money moves over STREAM through each ledger's receiver and payer.

**Paths:** `crates/dassie-payments/` (the protocol's pure parts: resources, GNAP rules, HTTP message signatures, intervals, URLs), `crates/dassie-node/src/open_payments/`, `crates/dassie-node/src/http/open_payments/`, `crates/dassie-node/src/engine/core/` (`incoming.rs`, `payers.rs`, `probes.rs`), `crates/dassie-node/src/rpc/` (`payments.rs`, `grants.rs`, `send.rs`), `crates/dassie-node/src/store/` (`payments.rs`, `outgoing.rs`, `gnap.rs`)
**Read with:** [STREAM](stream.md), [Connector](connector.md), [Ledger](ledger.md), [HTTP](http.md), [RPC](rpc.md), [Wallet](wallet.md)

The node implements what the specs define as they define it: resources, GNAP flows, [HTTP message signatures](https://www.rfc-editor.org/rfc/rfc9421) on every call the specs protect, error shapes, pagination, and [ISO 8601 repeating intervals](https://en.wikipedia.org/wiki/ISO_8601#Repeating_intervals) for grant limits, with windows computed as the reference implementation, Rafiki, computes them, so clients see the same windows anywhere. The reference client `@interledger/open-payments` runs against the node in the interop tests ([testing.md](testing.md#interop)) and pins that. This doc states only what the specs leave to the node, and why.

Not supported: SPSP ([open-payments](../decisions/open-payments.md)), card authorization (the node has no cards), and STREAM receipts, which are not part of Open Payments. Payment pointers are accepted only as input syntax.

## Wallet addresses

- **One wallet address per ledger,** since a wallet address has one asset: `https://<name>/pay/<slug>`, where the slug is the ledger ID up to its `+` and the name is one of [the node's names](#the-nodes-names).
- **`$name` names the default ledger:** the one the owner chose, or else the lowest active one. Grants and resources record every wallet address as its `/pay/<slug>` form, so changing the default never moves a grant or a payment to another ledger.
- **The key in `jwks.json`** signs the node's own requests to other wallets. It derives from the node key at `node/open-payments-client` ([identity.md](identity.md)), so restoring from the passphrase restores the key other wallets know the node by.
- Open Payments is served only once the node is set up, under its names.

## The node's names

Open Payments clients reach a wallet by DNS and HTTPS, and the node writes absolute URLs into its answers (wallet address `id`, `authServer`, `resourceServer`, payment and GNAP URLs), checks request signatures against a URL, and tells its own wallets from others' by origin. So it must know which names are its own.

- **A node with a hostname** answers under it alone, whatever `Host` says, as a core node always has.
- **A node without one** answers to a list of names (`Names` in `open_payments/mod.rs`): first its name at the default front door, `<label>.dassiepay.net`, which it computes from its [node label](identity.md#node-label), then the names its owner adds, another front door's or a domain of the owner's (`env.rs`). A [front door](http.md#front-doors) passes requests on under those names.
- **Each request is answered under the name it came through,** its `Host`, which a front door passes on through a [relayed stream](http.md#relayed-streams): the node writes that name into the URLs of its answer and checks the request's signature against it. So a client that fetched `$pay.example.com` finds a wallet address whose `id` is at that origin, as [the node as a client](#the-node-as-a-client) requires of others.
- **A name not on the list is refused** with 421, in the specs' error form, so that whoever passes requests on cannot make the node sign for a name its owner never chose, or hand it a request signed for another server.
- **What outlives a request holds under every name.** Grants and resources keep the wallet address URL they were made under, which is the node's by construction, and are read and written under the name of the request at hand. The node takes a URL at any of its names as its own: a wallet address, a quote, or a payment request in the owner's Send.
- **A pending grant keeps the name it was asked for under.** The interaction hash covers the grant endpoint the client called (RFC 9635 §4.2.3), and the node computes it when the owner decides, outside any request.
- **The wallet says when its name may reach no one.** A node without a hostname is named at the default front door, which reaches it only while the node keeps a connection to it, as it does by default ([Node](node.md#a-chosen-relay), [default-relay](../decisions/default-relay.md)). The wallet says so, and warns its owner while the node is not connected to it, or keeps another relay or none, since then no payment reaches the node at that name.
- **The first name is the main one:** the node's name to its owner, and as a client of other wallets ([below](#the-node-as-a-client)). Changing it changes how other wallets know the node, as changing a core node's hostname does.

## Signed responses

The node signs every Open Payments answer it gives under a name with its node key ([RFC 9421](https://www.rfc-editor.org/rfc/rfc9421), `httpsig.rs`), so that a Dassie-aware client can tell an answer came from the node, whoever passed it on. Whoever answers for a name is trusted by clients that do not check, so this is what keeps a front door from answering for the node.

- **What it covers:** `@status`, the `content-digest` of the body (added to every answer that has one), and the request's `@method`, `@target-uri` and `dassie-nonce`. The nonce is the client's, so an old answer cannot be replayed as a new one. The signature is tagged `dassie`, names `alg` Ed25519, and its `keyid` is the node's public key.
- **Errors are signed too,** a refusal as much as an answer. Only what is answered under no name goes unsigned: CORS preflights, and the refusals of an unknown name or of a node not set up.
- **A client checks** (`check_node_response`) that the key hashes to the node it meant to reach, and that the signature verifies. The node is the one whose label is the first label of the host in the URL it fetched, which is the URL the signature covers, so one name's label never vouches for another URL; or it is one the client knows by other means, such as a TXT record on an owner's domain, whose lookup is a [design](../designs/periphery.md#7-open-payments-without-a-public-address). A client that sent a nonce refuses an answer whose signature leaves it out, and a body whose digest the signature does not cover.
- **Responses are verified strictly,** as clients' request signatures are, rather than by ZIP-215: an answer is checked by one client, not agreed on by a network.

## The `dassie` field

Wallet address documents carry an optional `dassie` object with the node's ID, which the specs' schema lets other clients ignore (it allows more properties). It tells a Dassie-aware client which node should sign the answers. The node reads other wallets' documents leniently: a `dassie` property in any other shape, which another server may use for something of its own, counts as absent, so it never makes a wallet unpayable. The relay a node chooses ([Node](node.md#a-chosen-relay)) is to join it, so that a client finds the node through it; that is a [design](../designs/periphery.md#5-reaching-a-periphery-node).

## HTTP routes

The routes are in `crates/dassie-node/src/http/open_payments/`. Unlike the owner's endpoints ([http.md](http.md#same-origin-rule)) they are for anyone:

- **No `Origin` check, CORS for every origin, without credentials.** A client proves itself by its GNAP token and its signature, never by a cookie.
- **The URL a signature covers is at the name the request is answered under** ([The node's names](#the-nodes-names)), with the request's path and query, so a request signed for another server never verifies here.
- **Rate limits per client address,** with a tighter one on top for whatever creates state (grants, incoming payments, quotes, outgoing payments), since anyone may create them.
- **Requests are read strictly** (unknown fields refused where the schemas forbid them, every string and object bounded, URLs parsed by one strict parser, `uri.rs`, so the node and a client never read one URL differently); **responses are written exactly as the schemas describe them,** since the reference client validates them.

## Grants

### The owner's consent

**Which grants need the owner:** any outgoing-payment access, any `list-all`, `read-all` on quotes, and a subject request (proof of a wallet address). These move the owner's money or show more than the client's own resources. Such a grant must ask to interact, and a client known only by its key ("directed identity") is refused, since the owner must see who asks.

**Which do not:** incoming-payment and quote grants without those actions are approved at once. Creating an incoming payment only asks to be paid, and a quote moves nothing. **Reading any incoming payment needs no owner,** because a sender's wallet reads a payment it did not create to learn how to pay it, and the payment shows only what paying needs.

The flow is GNAP's redirect interaction. The interaction URL carries a random key per grant, so only the client and the owner know the way to the consent page in the wallet ([wallet.md](wallet.md)). Pending grants lapse, and their number is bounded, so a flood of requests cannot grow the table; how strangers are kept from filling it is in [Clients never crowd out the owner](#clients-never-crowd-out-the-owner).

**The page says each side of a payment as far as it is known, and never estimates.** A grant caps what is debited or what is delivered, so one side is firm, and a debit cap includes every fee and exchange on the way. The other side depends on the path, which only a quote measures, so the page says that it depends. A receiver the grant names is read while the owner is asked, to show its currency and what it still asks for. One that cannot be read is shown as such rather than refused, since the owner may still deny the request. The reads are bounded in time together, so a receiver's node that never answers cannot hold the page. The page shows the reason in the node's own words, never text the other node sent. **A receive limit in another asset than the receiver it names is flagged, not refused**, since the check needs the other node, and the owner sees the flag before deciding ([unusable-limits](../decisions/unusable-limits.md)). The node reads when the interval's windows start and end, because only its parser clamps months as the windows do; an open side reads as no end date.

### What a grant allows

- **A grant is bound to its client's key:** every later request of the grant must be signed with it, so a stolen token is of no use without the key.
- **A limit caps the amount debited or the amount received, never both,** as in the specs and Rafiki.
- **On an item that creates outgoing payments, a debit limit is in its wallet address's asset,** code and scale, or the request is refused with the asset it must be in, since no payment from that wallet could fit it ([unusable-limits](../decisions/unusable-limits.md)).
- **Each access item keeps to its own limits,** and only one item per wallet address may allow `create`, since otherwise which limits apply would be unclear. On an item that only reads or lists, a `receiver` limit narrows what it reaches.
- An item without an identifier reaches every wallet address of the node.
- **Limits are reserved when a payment is created,** in the interval's current window, and what the payment did not send is released when it ends. Windows are at least a minute, since each window spent in is stored.
- Access and continuation tokens are stored only as hashes ([secrets-at-rest](../decisions/secrets-at-rest.md)).

## Clients never crowd out the owner

Anyone can be a client. So the owner's incoming payments and quotes are bounded apart from clients' (`IncomingPolicy` in `engine/core/incoming.rs`, and `MAX_LIVE_OWNER_QUOTES` in `store/outgoing.rs`), and a flood of clients' requests never stops the owner from asking to be paid or from paying. When clients' share is full, the oldest of their incoming payments that received nothing is closed to make room: a flood pushes out only other unpaid requests. Incoming payments that received nothing are deleted some days after they close, so they do not pile up.

Every other pool strangers can reach keeps a share they cannot take, and no one of them holds more than a small share of it ([owners-share](../decisions/owners-share.md)). A stranger is counted by its client and by its network, an IPv4 /24 or an IPv6 /56 (`counted_network` in `node/mod.rs`); loopback counts in the totals only where private peers are allowed, so development and tests run as before.

- **Grants** (`grant_fits` in `open_payments/auth.rs`). The owner's side is the clients the owner approved before, known by a grant the owner approved and by their key. Strangers leave them a share of the grants kept and of the places where grants wait for the owner, and grants approved at once leave room for every grant that may wait. One stranger holds at most a small share of each, and so do the strangers of one network. A client the owner approved has no share of its own or of its network's, only the pools, as before: the grants the owner approved last for months, an app may ask for one at every payment, and a network is shared by many. A client the owner never approved is a stranger like any other, so on its first request it competes with them, within these shares.
- **Rate probes** (`engine/core/probes.rs`). Clients' quotes need no owner, and their probes hold the slots the owner's Send measures with. A few slots are the owner's alone, and one stranger, or the strangers of one network, hold at most a small share of the rest. A probe for a client the owner approved, under a grant the owner approved or by a client with one, has only the clients' slots as its bound.
- **Places in an incoming payment** ([Receiving](#receiving)). Anyone who can read a payment request gets a connection of its own, so connections that each pay one unit could take every place. A new payer takes the place of an idle one that paid only dust instead.

## Receiving

Each active ledger has a **receiver**, a local STREAM server endpoint at `<own>.stream-<slug>` that credits `L:equity/owner` in the owner's group ([ledger.md](ledger.md), [connector.md](connector.md#addresses-and-endpoints)). Its server secret derives from the node key at `node/stream-server/<slug>` ([identity.md](identity.md#seed-paths)), so the credentials it issued stay valid across restarts and the node keeps no table of them.

- **A packet is fulfilled only for an open incoming payment on that ledger,** and only up to what the payment still expects, or over it by at most the payment's tolerance. What the receiver took counts once the packet is posted, whatever the upstream does with the Fulfill ([own-fulfills](../decisions/own-fulfills.md)). The payment's received amount is written in the same transaction as the journal row, so a payment never shows money the books lack, nor the other way round.
- **One STREAM connection per payment.** Each read of an incoming payment hands out fresh credentials: the STREAM connection tag is the payment's ID and a random connection key. So each payment that pays it has a connection of its own, and the receiver tells each connection what *it* received, not what the incoming payment did. A sender never takes another's delivery for its own: of two payments to one incoming payment, the one that delivered less fails rather than completes.
- **Admission.** Before a packet reaches a connection, the receiver reads its tag and lets the connection take what the incoming payment still expects over all its connections, and over it by at most the payment's tolerance. That is why the receiver's streams accept money from their first packet ([stream.md](stream.md#money-limit)): the incoming payment, not STREAM's stream limit, bounds the connection.
- **A bounded number of places.** A connection takes a place in its incoming payment once its first money is posted, and keeps what it received there. Once every place is taken, a new connection is refused, unless one that paid less than dust (`dust` in `engine/core/incoming.rs`, as little as the XRP Ledger can move) has paid nothing for STREAM's idle timeout: the new one takes the place of the one idle longest once its own money is posted ([owners-share](../decisions/owners-share.md)). A place changes hands only in the step that posts the new connection's money, and the receiver answers every packet within that step, so no money is on its way to the connection replaced; what it received stays in the payment's total. If its sender comes back, it is a new connection and is told it received nothing, wrong by less than dust.
- **Paid in full within a tolerance.** An incoming payment completes once what it received lands within its tolerance of its amount, under or over: a ten-thousandth of its asset's unit, never more than a thousandth of the amount ([paid-within-a-tolerance](../decisions/paid-within-a-tolerance.md)). A packet further over than the tolerance is refused with F99, whose reply tells its sender the room left, and Dassie's sender sends that instead ([stream.md](stream.md#money-limit)). `receivedAmount` is what the payment really received, so it can exceed `incomingAmount` by up to the tolerance: the specs call `incomingAmount` the most that should be paid, and `receivedAmount` what was paid.
- **A connection with nothing left is closed** when it offers money: with `NoError` if the incoming payment received all it asked for, possibly from another payment, and with `ApplicationError` if it was completed or closed short of that, or asked for no amount, so that its sender can tell the two apart ([full-or-stopped-short](../decisions/full-or-stopped-short.md)).
- **After it completes,** the node remembers for a while whether it was full and what each connection received, on disk, so a sender that lost a Fulfill to a crash and reconnects, even to a receiver that restarted meanwhile, learns that its money arrived rather than being refused. The retention is at least the longest a node's own payer tries (`FINISHED_RETENTION_MS`, asserted against `OUTGOING_DEADLINE_MS` in `payers.rs`).

## Outgoing payments

Each active ledger has a **payer**, a local endpoint at `<own>.pay-<slug>` that takes no packets and debits `L:equity/owner/local/pay`, a sub-account in the owner's group. So the group's limit is the outgoing payments' limit too. Each payment that is sending has a STREAM client connection of its own on the payer.

**Quotes.** A quote measures the route with a STREAM rate probe ([stream.md](stream.md)) from the payer, even between two accounts in one asset, since intermediaries charge fees; to the node's own receivers the rate is one for one. A quote pays from the ledger the route starts on, because the node's own endpoints never convert ([owner-ledger](../decisions/owner-ledger.md)). Probe packets hold the owner's funds while in flight, as a payment's would.

**Creating a payment:**

- **Credentials are used once:** no two outgoing payments may pay to the same destination. Two payments on one set of credentials would share a connection and each be told what both delivered. A wallet that hands out the same credentials on every read must be read again for each payment.
- **The funds must cover it:** what the owner's group has free on the ledger must cover this debit and what the unfinished payments on that ledger still have to send.
- Grant limits are reserved ([What a grant allows](#what-a-grant-allows)); the connector does not check them.

**Sending:**

- **Each packet must deliver at least the payment's rate,** its receive amount over its debit amount: a worse path fails it.
- **It never debits more than its debit amount, across crashes too.** Its sent amount is what was posted, written with the journal row. A packet in flight at a crash was never posted, so a restart sends only the rest, on a new connection with the same credentials.
- **It completes** once it has sent its debit amount, or, when the amount to deliver is fixed, once the receiver says it has all it asked for, or all but less than one unit sent is worth (which no packet can be sized to deliver, since hops round down), or all but the tolerance of its receive amount and takes no more, as a receiver counts that as paid in full ([paid-within-a-tolerance](../decisions/paid-within-a-tolerance.md)). A receiver may say so although the payment heard of less, when a hop lost a Fulfill it paid for ([own-fulfills](../decisions/own-fulfills.md)).
- **It ends only with none of its packets in flight,** so what it says it sent is final. A connection that closes early is reopened for the rest, a bounded number of times, and a payment fails at its deadline (`payers.rs`). A connection the receiver closes with `NoError` or `ApplicationError` is not reopened, since a new one would be closed too, and the payment ends at once, saying which the receiver said: with `NoError` that it takes no more, as when another payment filled the incoming payment, with `ApplicationError` that it stopped the payment before it was complete, as when its incoming payment was completed early ([full-or-stopped-short](../decisions/full-or-stopped-short.md)). A payment can only go by what the receiver says. It expects the receiver to take its receive amount, so a receiver that reports no room before it took that is asked why at once ([stream.md](stream.md#sending)), even when another payment took from the incoming payment before this one first heard its maximum.
- **To the node's own receiver** a payment goes through the connector like any other, through a bounded queue drained at the end of the engine's step, so nothing recurses.

## The node as a client

The node pays and creates incoming payments on other wallets as an Open Payments client, as the `$name` wallet address of its main name and its key (`open_payments/client.rs`).

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
- Shared secrets, token hashes and the GNAP interaction state are secret columns, which the database inspector never reads ([rpc.md](rpc.md#the-inspector)). A pending grant's name is part of its interaction state, which its row keeps up to 4096 characters: the finish URI and the subject are the client's, each up to a URI's bound, so a request whose interaction could not be kept, with the reference the owner's decision adds, is refused with 400 before it is stored. The client's public name is left out when it alone would not fit.
- In memory only: clients' key sets, the node's tokens at other wallets, the rate limiters, probes, and the payers' packets in flight.

## Decisions

- [open-payments](../decisions/open-payments.md): Open Payments in full as its specs define it, no SPSP, and GNAP tokens even to create incoming payments.
- [owner-ledger](../decisions/owner-ledger.md): the owner's group, which the receiver and payer book into, and why the owner's endpoints never convert.
- [exchange-rates](../decisions/exchange-rates.md): rates and fees, which make quotes probe.
- [secrets-at-rest](../decisions/secrets-at-rest.md): tokens stored as hashes.
- [own-books](../decisions/own-books.md): a payment's sent amount is what the node's own books posted.
- [own-fulfills](../decisions/own-fulfills.md): why a receiver's word can complete a payment that heard of less.
- [unusable-limits](../decisions/unusable-limits.md): why a debit limit in another asset is refused when the grant is asked for, and a receive limit in another asset than its receiver is flagged on the consent page.
- [owners-share](../decisions/owners-share.md): why strangers leave the owner a share of each pool, who counts as the owner's side, and why an idle connection that paid only dust gives up its place.
- [full-or-stopped-short](../decisions/full-or-stopped-short.md): why a receiver closes with a code that says whether its incoming payment was full, and why not with a message.
- [paid-within-a-tolerance](../decisions/paid-within-a-tolerance.md): why an incoming payment is paid in full within a tolerance of its amount, and how large it is.
