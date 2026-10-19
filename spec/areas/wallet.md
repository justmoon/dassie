# Wallet

The wallet is the node owner's web app: a Solid 2 single-page app that the node serves from its own binary. The owner sets the node up, logs in, manages ledgers, sends and receives, approves third-party apps and reads the debug pages there. `packages/ui` holds the components it shares with the dev dashboard and the website.

**Paths:** `apps/wallet/`, `packages/ui/`
**Read with:** [RPC](rpc.md), [HTTP](http.md), [Payments](payments.md), [Identity](identity.md), [Dev environment](dev.md)

## Stack and build

- Solid 2 (`solid-js@2.0.0-rc.x` with `@solidjs/web`), built by Vite through `@solidjs/vite-plugin`, on Node and pnpm. There is no React, wouter, TanStack Query or Radix. The wallet has its own small router (see [The shell](#the-shell)), since no router of Solid 2's release candidate matches the pinned version.
- `vite build` writes a static page to `apps/wallet/dist/` (`node --run build:wallet`). The node embeds it at build time from `DASSIE_WALLET_DIST` and serves it ([Node](node.md), [HTTP](http.md)).
- The dev environment builds it with `--mode development --watch` into `dist-dev/`, which dev nodes serve instead of their embedded copy (`DASSIE_DEV_WALLET_DIR`, [Dev environment](dev.md#nodes)). A watch build does not empty its directory, so a reload during a rebuild still finds the page.
- Every asset is a file of its own (`assetsInlineLimit: 0`): a `data:` URL for the WASM module would be refused by the CSP's `connect-src 'self'`. There is no module-preload polyfill, and the target is `esnext`.
- The page title is "Dassie Wallet". The setup and login screens, which load the identity WASM, and every page but Dashboard, Receive and the ledger pages are loaded on first use.

## Content Security Policy

The node serves the page under a strict policy, whose header is in [HTTP](http.md#response-headers). The wallet keeps to it:

- No inline script. The theme is applied from the bundle, so the first paint follows the system's theme until the script runs.
- No inline style: no `<style>`, no `style=` attribute in `index.html` or in Solid's compiled templates. Styles come from classes; positions computed at run time are set through the DOM.
- Nothing loaded from another origin. QR codes and the network graph are SVG drawn in the page; images are the page's own or `data:` URLs.
- WebAssembly may compile, for the identity module.

The same policy applies in development: there is no HMR proxy in the node, and a change shows after a reload.

## Talking to the node

- **RPC.** One `RpcClient` (`packages/rpc`) over `wss://<location.host>/rpc`, carrying the contract in `proto/dassie/ui/v1` ([RPC](rpc.md)). When the node ends this browser's session (logged out elsewhere, or revoked), the client reports it and the wallet reloads, which shows the login page.
- **HTTP.** Setup, login and logout are the node's three routes outside the contract: `POST /api/setup`, `/api/login` and `/api/logout`, each with a JSON body, `Content-Type: application/json` and `credentials: "same-origin"`. Their bodies and checks are in [HTTP](http.md#setup) ([login and logout](http.md#login-and-logout)). Success sets or clears the session cookie, and the wallet reloads so the gate picks the next screen. A failure carries the HTTP status (0 when the node could not be reached), the node's plain-text message, and for 429 the `Retry-After` seconds.

## Key derivation

- Keys are derived in the browser with `dassie-identity` compiled to WASM (`@dassie/identity-wasm/web`), the same code the node runs ([Identity](identity.md)).
- **New passphrase:** 12 BIP-39 words (128 bits of entropy) from the platform's CSPRNG.
- **Validation (UI-5):** a passphrase is checked (known words and checksum) and normalized before anything is derived from it. An invalid one gets "That is not a valid passphrase." and nothing is sent.
- **What is sent:** setup sends the node key and the login proof derived from it, as hex; login sends only the proof. The passphrase never leaves the browser and is never stored, in the URL or anywhere else.
- If the WASM module fails to load: "The wallet could not load its key derivation module. Reload the page to try again."

## The gate

Which screen the wallet shows follows from `getBasicState`, asked once per page load, and the path.

| Node state | Screen |
|---|---|
| not answered yet | spinner "Connecting to your node…" |
| the call failed | "Cannot reach your node", with the error and **Try again** (reload) |
| `UNINITIALIZED`, path `/setup/<64 hex digits>` (trailing slash allowed; token lowercased) | the [setup wizard](#setup-wizard) |
| `UNINITIALIZED`, any other path | "This Dassie node is ready for setup. Please visit its setup URL: on the server, `sudo dassie setup-url` prints it." |
| `ANONYMOUS` | the [login page](#login), whatever the path |
| `AUTHENTICATED` | the owner's wallet, below |
| anything else | "Unexpected node state" ("Is the wallet older than the node?") |

For the signed-in owner, a setup path is replaced by `/`. The wallet then follows the node's ledgers live:

- the ledger stream failed for good: "Cannot load your ledgers", with the reason;
- no ledger list yet: spinner;
- no ledger: the [forced choice of a first ledger](#choose-a-ledger-type), full screen, without navigation;
- otherwise: [the shell](#the-shell).

Because the ledgers are live, adding the first ledger or removing the last takes effect at once.

### Setup wizard

A local state machine in one card. Nothing about the steps is kept in the URL, so a reload starts over.

| Step (title) | Shows | Actions |
|---|---|---|
| intro ("Dassie Wallet") | "Welcome to Dassie Wallet. The first step is to create or recover a wallet." | **Generate a new wallet** → generate. **Recover an existing wallet** → recover |
| generate ("Generate Wallet") | A warning that the passphrase is the wallet, and the 12 words | **Back** → intro. **Ok, I have backed up my passphrase** → verify |
| verify ("Verify Passphrase") | "Please enter word #N", N chosen uniformly at random from the CSPRNG | **Back** → generate (same words). **Verify** is enabled only when the input, trimmed and lowercased, equals word N |
| recover ("Recover Wallet") | A passphrase text area, placeholder "correct horse staple …" | **Back** → intro. **Open Wallet** validates the phrase, then → opening |
| opening ("Opening Wallet") | Spinner "Setting up your node…" | Derives, then `POST /api/setup`. Success: `history.replaceState` to `/`, reload. Failure: the node's message (or "Setup failed (HTTP n).", or that the node could not be reached), and **Back** to verify or recover |

The identity module starts loading when the wizard opens.

### Login

- Card "Reconnect to your Dassie instance", subtitle "Enter your wallet passphrase".
- Field "Passphrase" (placeholder "correct horse staple …", no autocomplete, autocapitalize or spellcheck) and **Reconnect**, which reads "Reconnecting…" and stays disabled while the request runs and the page reloads.
- Errors:

  | Case | Message |
  |---|---|
  | invalid passphrase | "That is not a valid passphrase." |
  | node unreachable | "The node could not be reached. Check your connection and try again." |
  | 401 | "That passphrase does not belong to this node." |
  | 429 | "Too many attempts. Try again in N seconds." (or "later" without `Retry-After`) |
  | other | "Could not sign in: <message>" or "Could not sign in (HTTP n). Please try again." |

- Footer link "What is Dassie?" → `https://dassie.land`.

### Dev auto-login

Only a build with `--mode development` has this code (`__DASSIE_DEV_LOGIN__`). Before rendering, it reads a URL fragment `#dassie-dev-login=<64 hex>`, removes it from the URL and history, and sends the proof as an ordinary `POST /api/login`; a failure is logged to the console. The node checks it like any other login. The production build has none of this, which a build test checks ([Dev environment](dev.md#wallet-links)).

## The shell

- A skip link to the content, the navigation, a connection notice, and the page inside an error boundary (a failed page shows the error and a way to try again).
- **Connection notice:** "Reconnecting to your node…" once the RPC connection has been waiting for 2 s; it goes when the connection is open again.
- **Router** (`packages/ui/router`): a pattern is a path of segments, each a literal, `:name` for one segment, or `*name` as the last segment for the rest of the path. A trailing slash does not matter, parameters are URL-decoded, and the first route that matches wins. `useSearchParam` reads the query.

### Navigation

- Brand "Dassie Wallet" → `/`. A **Menu** toggle on small screens.
- Links: **Dashboard** (`/dashboard`), **Activity** (`/payments`), **Connected Apps** (`/grants`), **Network**, **Ledgers**, and a **Debug** menu: Logs, Nodes, Ledger, Routing, Packets, State, Database (Escape closes it).
- On the right: the network status, the theme toggle, **Settings**.
- Moving to a new page closes both menus.
- **Network status** links to `/debug/nodes` and shows a dot and "N nodes", the number of entries in the node table. The dot is `failed` if the node-table stream failed, `reconnecting` while the RPC connection waits, `connected` while the stream is live, `connecting` otherwise.

### Routes

| Route | Page |
|---|---|
| `/`, `/dashboard`, and any path not below | [Dashboard](#dashboard) |
| `/receive` | [Receive](#receive) |
| `/send` | [Send](#send) |
| `/payments` | [Activity](#activity) |
| `/payments/:id` | [Payment status](#payment-status) |
| `/ledgers` | [Ledgers](#ledgers) |
| `/ledgers/create` | [Choose a ledger type](#choose-a-ledger-type) |
| `/ledgers/delete/:scheme` | [Remove a ledger](#remove-a-ledger) |
| `/network`, `/network/:node` | [Network](#network) |
| `/network/register` | [Register](#register) |
| `/settings` | [Settings](#settings) |
| `/consent/:id` | [Consent](#consent) |
| `/grants` | [Connected apps](#connected-apps) |
| `/debug`, `/debug/*rest` | [Debug pages](#debug-pages) |

## Live data

Every `Subscribe*` procedure delivers either a value stream or a snapshot followed by typed deltas ([RPC](rpc.md)). The client turns them into Solid signals and stores; it needs none of the node's reducer code.

- `createLiveValue`: a value streamed whole, as a signal.
- `createLiveTable`: a snapshot and changes by key, as a store. An item with `reset` empties the collection; the items after it upsert and remove.
- `createLiveBuffer`: the newest lines of a stream, at most a given number, in a ring.
- A stream stays open while its component is shown and ends when the page is left.
- **Resubscribing.** A stream that ends or fails is subscribed again after a backoff: 500 ms doubling to at most 30 s, each delay multiplied by a random factor between 0.5 and 1.5. A stream that was live for 10 s or more starts the backoff over. `UNIMPLEMENTED`, `INVALID_ARGUMENT`, `NOT_FOUND` and `UNAUTHENTICATED` are final: the stream is not retried. Each attempt is a new generation, whose first item is a fresh snapshot.
- A view's status is `connecting`, `live`, `retrying` (with the reason and attempt) or `failed`; pages show a note whenever it is not `live`.
- **The node's data** while the owner is signed in, shared by most pages: `SubscribeLedgers` (the ledgers with their balances, the exchange rates, the display currency and the total) and `SubscribeNodes` (the node table, by node ID). A ledger the node just returned from `AddSettlementScheme` shows at once, before the stream has it, for at most 5 s.

## Pages

### Dashboard

- **Payment Pointer:** the payment pointer of the default wallet address (`ListWalletAddresses`), else `$<hostname>`, else `???`; with the note "Other wallets pay this address through Open Payments."
- **Balance:** each ledger's scheme name and the owner's balance on it, in the ledger's own currency, live; "(<amount> in flight)" when the owner has money pending on it. Below, "In all: <total>" in the display currency, while the node reports a total ([Amounts and currencies](#amounts-and-currencies)).
- **Send** and **Receive** buttons.

### Receive

Tabs, the first selected unless `?tab=<scheme>` names another:

- **Open Payments:**
  - The wallet addresses, one per ledger (`ListWalletAddresses`): the ledger's name, its asset code, a "default" badge, the payment pointer and the URL, each with a copy button. A non-default address has **Make <$host> this ledger**, which calls `SetDefaultLedger`.
  - **Request a Payment:** a ledger (shown only with more than one; default: the default address's ledger), an amount in its asset (empty takes any amount), and "What it is for" (at most 256 characters). **Create Request** calls `CreateIncomingPayment { ledger, incomingAmount?, description }` and shows the payment's URL to share, with a copy button, and, folded, its STREAM credentials: the destination address and the shared secret (masked until revealed), each with a copy button.
- **One tab per ledger**, by scheme:
  - stub: "Receive $100" calls `StubDeposit` with 100 × 10^scale, toasts "Received $ 100.00" and goes to the dashboard;
  - other schemes: "No deposit method for this ledger yet." An XRP Ledger deposit address is on the [Ledgers](#ledgers) page.
- **Back** → `/`.

### Ledgers

Title "Active Ledgers". For each ledger, live:

- the scheme's name, and a remove button → `/ledgers/delete/<scheme>`;
- the owner's balance, and "On the underlying ledger: <amount>";
- the connection to the underlying ledger, when it is not in step: "Connecting to the ledger…", "Catching up with the ledger…", "The account does not exist yet: send it XRP, or wait for the testnet faucet.", or "Something is wrong with the ledger: <problem>.";
- a deposit address, when the scheme has one: a QR code (SVG, 160 px), the address with a copy button, and, when the ledger holds back a reserve, "The ledger holds back <amount> as a reserve: never spent, and not in the balance.";
- **Receive** → `/receive?tab=<scheme>`.

Then **+ Add Ledger** → `/ledgers/create`. When the node keeps a rate for any currency but USD, an **Exchange Rates** card lists each as "1 USD = <rate> <code>", with its age ("just now", "N minutes ago") or "stale: payments that need it are refused", and links to Settings to change the sources.

### Choose a ledger type

- `ListSettlementSchemes` lists the schemes the node can run. Each shows "<name> Settlement" and a description: the wallet's own for stub ("simulates settlement delays but does not actually connect to any external ledger") and `xrpl-testnet`, otherwise "Settles in <asset> on <ledger>."
- An active scheme shows an "Active" badge. Any other has **Select →**, enabled only for schemes of the test realm and while nothing else is being added. It calls `AddSettlementScheme { id, config: "" }` and goes to `/ledgers`.
- On a node whose ILP allocation scheme is `test`, an alert "Testnet" says the methods do not use real money.
- **Onboarding** is the same list, full screen with no navigation, "Signed in to node <id>", and a **Log out** button.

Scheme names: `stub` "Stub", `xrpl` "XRP Ledger", `xrpl-testnet` "XRP Ledger Testnet"; others by the node's name or the ID.

### Remove a ledger

- `/ledgers/delete/:scheme` goes back to `/ledgers` when no active ledger has that scheme.
- Title "Removing Ledger <name>", and a danger alert "Possibility to lose funds": removing stops the node using the ledger; the books keep what they hold and it comes back when the ledger is added again, but the node does not settle on it meanwhile.
- **Prerequisites**, from the ledger's live removal blockers ([Ledger](ledger.md)): always the peers line ("Not used by any peers", or "Settlement scheme is still in use for N peers"), then one line per other blocker: BTP tokens that use the ledger ("revoke them first"), ended peerings still to be settled with it, outgoing payments from it still being sent, or "Something else keeps it in use".
- **Confirmation:** the owner types `DELETE <SCHEME IN UPPER CASE>` (`DELETE STUB`, `DELETE XRPL-TESTNET`). After leaving the field with anything else: "Confirmation must match exactly the expected format".
- **Remove Ledger Account** is enabled only when the confirmation matches and nothing blocks. It calls `RemoveSettlementScheme { id }`, toasts and goes to `/ledgers`.

### Network

`/network`, or `/network/<node ID>` with that node selected.

- **The graph is the routing table:** this node, every node a route reaches, and between them the edges routing runs over, those both ends list. A peering alone draws nothing, so a peer that is offline or not listed back has no line. This node is always drawn.
  - The layout is force-directed and deterministic: a node starts where its ID puts it and keeps its place as the table changes. For readers who prefer reduced motion it settles at once.
  - Nodes are coloured by ID (one of 12 colours), this node larger; labels are the alias, "this node", or the ID cut to 8 characters. Clicking a node, or Enter on it, selects it.
- **The listing**, sorted by distance and then ID, grouped:

  | Mark | Heading | Nodes |
  |---|---|---|
  | 0 | This Node | |
  | 1 | Direct Peers | peers a route reaches |
  | – | Peers Not Routed | peers without a route that the node reports online |
  | – | Offline Peers | peers without a route, not online |
  | 2+ | Other Nodes | |
  | ∞ | No Connection | no route, not a peer |

  A peer here is a node whose peering is `peered` or `terminating`; online means heard from within the last 60 s ([Network](network.md#advertised-peers)). Each row shows the alias or the ID (cut to 20), and its URL or "no link state". Empty groups are left out.
- **This node's details** (nothing or this node selected): "Tier 1 Registration" from `SubscribeRegistration`, a badge "Registered" or "Not registered", "Listed by N of M bootstrap nodes", and a link "Register" or "Update registration" → `/network/register`.
- **Another node's details:** its ID, URL, distance (or "no route"), settlement schemes, neighbours and peering state (`unknown`, `none`, `requesting`, `peered`, `terminating`); "Banned by you." if banned; and, for a peering: scheme, since, online or offline, last heard, advertised, what is owed to it (in the scheme's ledger currency), and why it is ending.
- **Actions**, each confirmed in a dialog:
  - **Terminate peering** (peered and not already terminating): "No new payments go through this peer. Once those in flight are done, the peering ends and what is owed is settled. Your node does not peer with it again for a day." Calls `TerminatePeering`.
  - **Ban node**: "Your node ends any peering with it, and never peers with it again until you lift the ban." Calls `BanNode`. A banned node has **Unban node** (`UnbanNode`) instead.

  The rules behind them are in [Network](network.md).

### Register

From `SubscribeRegistration`: "Your node registers with the network's bootstrap nodes by itself, and renews before a registration lapses. You can also renew now." Each bootstrap node shows its ID, "Registered" or "Not registered" (whether its list names this node), when this node registered and when the list was fetched, and **Renew now**, which calls `RegisterWithBootstrap { node }` and toasts "Registration sent", or shows the error under that node. Without bootstrap nodes: "This node has no bootstrap nodes: it runs alone until `DASSIE_BOOTSTRAP_NODES` names some."

### Settings

Two columns on wide screens.

- **Wallet:** the node ID, the payment pointer `$<hostname>`, and **Logout** (`POST /api/logout`, then reload; a failure is shown).
- **Appearance:** the theme, "As the system", "Light" or "Dark" ([Theme](#theme)).
- **Node Settings:** the alias (at most 64 bytes, "Shown to other nodes"; `GetConfig`, `SetAlias`), and the hostname, read-only ("change it with `dassie init`").
- **Peers:** the connector fee ("% of what passes", charged only on what passes from one peer to another) and the FX spread ("% of what is converted"). Each is a percentage with at most four decimals, from 0 to 10%; empty means 0. They are sent in parts per million (`GetPeerSettings`, `SetPeerSettings`). The rules are in [Connector](connector.md).
- **Exchange Rates:**
  - "Show totals in": "No total" or one of USD, XRP, EUR, GBP, JPY, CHF, CAD, AUD (`SetDisplayCurrency`, applied at once).
  - The rate sources (`GetRateSources`, `SetRateSources`), edited and then saved: each Coinbase (with an optional URL: empty for Coinbase's own, else an HTTPS server of its format), Kraken or Bitstamp. At most 8; nothing can be added before the stored list is read. With none: "Without a source, rates go stale, and payments across currencies stop." The rules are in [Exchange rates](connector.md#exchange-rates).
- **Sessions** (`ListSessions`): each browser signed in, by user agent, with "This browser" on the current one, since when and when last used. **Revoke** (confirmed: "The browser is logged out at once.") calls `RevokeSession`; revoking the current one reloads. **Log out all other sessions** calls `RevokeAllSessions { keepCurrent: true }` and toasts how many.
- **API Keys**: [BTP tokens](#btp-tokens).

#### BTP tokens

- `ListBtpTokens`: each token's label (or "unnamed"), ledger, when it was made, how many clients are connected, what it received net, and its cap: "Cap: <amount> per <window>" with a meter of the share used and "<amount> left", or "No limit". Empty: "No API keys created yet. Add one below."
- **Create BTP Token:** a label (at most 64), a ledger (default the first), and an optional spending cap: an amount in the ledger's asset, above zero, per hour, day or week (empty: no limit; "Paying your own node does not count"). It calls `CreateBtpToken { label, ledger, cap? }`; a second submit while one is under way is ignored, since a second secret would hide the first.
- The new token's secret and its URL `btp+wss://:<token>@<host>/btp` are shown once, in a modal dialog: "Copy this token now. It will not be shown again." Both are masked until revealed; **Copy BTP URL** toasts "Copied BTP URL to clipboard" (or a failure). The node keeps only the token's hash ([Connector](connector.md#btp-tokens)).
- Per token: **Set a cap** / **Edit cap** (starting from the token's cap; a window that is not one of the three shows as day), **Remove cap**, each through `SetBtpTokenCap { id, cap? }`; and **Revoke** (confirmed: "Its clients are disconnected, and it is refused from now on."), through `RevokeBtpToken`.
- A cap whose window is not an hour, day or week reads "per N h" or "per N s". What a cap counts is in [Connector](connector.md).

### Activity

- **Incoming Payments** (`SubscribeIncomingPayments { limit: 100 }`), newest first, ties by ID. Each shows:
  - its description (the `description` of its metadata JSON) or "Incoming payment";
  - its state: `completed`, `open` before its expiry, `expired` after (checked again every 5 s);
  - what it received, "of" what it asks for, if it asks;
  - who requested it ("You", a client's wallet address as host and path, or "A client known by its key") and when.

  An open one has **Complete** (confirmed: "It takes no more money, whatever it was asked for."), which calls `CompleteIncomingPayment`. Empty: "No incoming payments yet. Request one under Receive."
- **Outgoing Payments** (`SubscribeOutgoingPayments { limit: 100 }`), newest first: "To <destination>" linking to its [status page](#payment-status), its state, what it sent of its debit amount, who sent it and when, and its failure, if any. The destination is the payment pointer the owner named, else the host of the incoming payment it pays.

### Send

Three steps in one page. `?to=<recipient>` fills the first and looks it up.

1. **Whom to Pay.** One field: "Payment pointer, wallet address or payment request". Input that looks like one (`$host…` or `https://host…`) is looked up after a 400 ms pause in typing, or on **Continue**; only the latest lookup counts. `ResolveRecipient { recipient }` returns the recipient: its name (public name and payment pointer or address), "(one of yours)" when it is on this node, its currency, and for a payment request what it still asks for ("any amount") and its description. Errors show under the field.
2. **How Much.**
   - "To <recipient>", and "For: <description>" for a request.
   - **Pay from**, shown with more than one ledger. Default: the ledger chosen, else one in the recipient's asset code and scale, else the default wallet address's, else the first. The balance of the paying ledger is shown. With no ledger: "You have no ledger to pay from."
   - **The amount is** "What they receive" (default) or "What you send". The amount is in the recipient's asset or in the paying ledger's, as labelled; switching clears it. A request's remaining amount is filled in.
   - An empty amount is allowed only for a request, which then pays what it still accepts. Zero: "Send more than nothing."
   - **Note for them** (at most 256), only for a wallet address.
   - **Review** calls `QuoteSend { recipient, ledger, amount, note }`: the request's URL or the wallet address; the paying ledger for "What you send", else the ledger chosen or none; `receiveAmount`, `debitAmount` or none.
3. **Review:** to whom, from which ledger, "You send" (the quote's debit), "They receive", the note, and how long the quote is good for ("Good for N more seconds/minutes", ticking each second; "This quote has lapsed."). **Send** calls `SendPayment { quoteId }` and goes to the payment's page. After the quote lapses, or after an error, **Get a New Quote** replaces it.

**Back** returns a step. What the node does with a lookup, a quote and the note is in [Payments](payments.md#the-owners-send).

### Payment status

`/payments/:id` follows one outgoing payment live (`SubscribeOutgoingPayment { id }`): "To <destination>", a badge "Sending…", "Sent" or "Failed", what it sent of its debit amount with a progress bar, the failure, what they receive, the recipient, who sent it, when it started and finished, and **Send Again** (`/send?to=<recipient URL>`) when the recipient is known. A link leads to all activity.

### Consent

The node sends the owner here, to `/consent/{id}?key={key}`, when a third-party app asks for access that needs consent ([Payments](payments.md#the-owners-consent)).

- `GetGrantRequest { id, key }` loads the request. If it is gone: "This request is no longer waiting. It was decided already, or lapsed after 10 minutes (<reason>). Start again from the app."
- The page "An App Asks for Access" shows who asks, the client's public name with its wallet address in parentheses, or the address's host and path, and what for: one line per access item, plus "Learn that <wallet address> is yours" for a subject request. Then "Only approve apps you trust. You can revoke access at any time."
- **Deny** or **Approve** calls `DecideGrant { id, key, approve }` and sends the browser to the returned finish URL, if any. Otherwise the page says "Approved" (the app may now do what it asked; it can be revoked under Connected Apps) or "Denied", and that the page can be closed.

**An access item in words:**

- What: `outgoing-payment` with `create` is "Send payments", `incoming-payment` "Request payments", `quote` "Get quotes for payments"; an item without `create` is "See <type>s" ("See outgoing payments").
- Limits, for an item that creates: " of up to <debit amount>", " delivering up to <receive amount>", both on grants stored before the node refused that, or " of any amount"; then the interval: "a day", "every 2 weeks", "every 1 month and 10 days", or "each period" for an interval between two dates.
- ", to one payment request only" when the limits name a receiver (for an item that only reads, that is all its limits say).
- " from your <CURRENCY> wallet" (outgoing payments) or " in your <CURRENCY> wallet", from the ledger's currency.
- ", and see all of them, not only its own" with `read-all` or `list-all`.

### Connected apps

`/grants` (`SubscribeGrants`): each approved grant's client name, since when, and each access item in words, with what it spent this period against its limit ("<spent> of <limit> spent this period", or "delivered" for a receive limit). **Revoke** (confirmed: "It can do nothing more for you. Payments it already made stay made.") calls `RevokeGrant`. Empty: "No app has access. When an app asks, you approve it here first."

### Debug pages

They read the node only through the RPC client and the node data of their context, so the [dev dashboard](dev.md#dashboard) shows them for any node. Each has its own tab bar. Amounts show in their ledger's currency.

| Route | Shows | RPC |
|---|---|---|
| `/debug/logs` (and `/debug`) | The [log viewer](#log-viewer) over the node's log. The page keeps the newest 10,000 lines it received. | `SubscribeLogs` |
| `/debug/nodes` | Node (ID, links to the network page; URL), alias, distance (∞ without a route), next hops, neighbours, peering state, and why the node is in the table: self, bootstrap, member, registered, peer, banned | node data |
| `/debug/ledger` | Every account by path: balance (credits posted − debits posted − debits pending), credits and debits posted with "+N pending". **Refresh** asks again. | `ListAccounts` |
| `/debug/ledger/:ledger/account/*path` | Breadcrumb, then the account's transfers live, newest first: DR or CR, the counter-account (linked), amount, the posted balance after it, when. Pending rows are faded, voided rows struck through. Empty: "Waiting for transfers to occur..." | `SubscribeTransfers { account, history: 200 }`; keeps 1,000 |
| `/debug/routing` | Local routes (prefix, endpoint, ledger), then peer routes (prefix, first hops, distance) | `SubscribeRoutes` |
| `/debug/packets` | Packets, newest first: from, to, destination address, amount in, amount out, state (pending, fulfilled, rejected with code). Keeps 500; "N packets were missed." when the stream lagged. | `SubscribePackets`; each fresh subscription first pages `ListPackets` (1000 at a time, at most 8 pages) for what it may have missed |
| `/debug/state` | Services: name, state (running, restarting, stopped), started, restarts, last failure. Cells: name and version, opened to show their JSON. **Refresh** asks again. | `ListServices` |
| `/debug/database`, `/debug/database/:table` | The tables with their row counts ("≤" when not exact); a table's rows, 50 a page, with Previous and Next. Integers and text as they are, blobs as hex, `NULL`, and "[redacted]" for secret columns, whose headers say "(secret)"; a cut value ends in "…". | `ListDatabaseTables`, `ListDatabaseRows { table, cursor, limit: 50 }` |

What the node puts in these streams, the log ring and which columns are secret, is the node's rule: [Node](node.md#logging), [RPC](rpc.md#the-inspector).

## Amounts and currencies

- Every amount carries its currency: a ledger's asset code and scale, as the node reports it. Amounts are exact integers in the asset's smallest unit (bigints), never floats.
- **Display:** an optional minus, the symbol (`$` USD, `€` EUR, `£` GBP, `XRP` XRP, otherwise the code), a space, the integer part grouped as the locale groups it, then `min(2, scale)` digits after the point, then up to 2 more digits muted. Digits are cut, never rounded. `$ 1,234.56` with a muted `78` for USD at scale 9.
- **Input:** digits with an optional decimal point; a comma counts as a decimal point, and a number with both is refused, since `1,500` means different things on different keyboards. Errors: "Enter an amount.", "Enter digits, with a decimal point or comma.", "The amount cannot be negative.", "At most N digits after the decimal point.", "That amount is too large." (above 2^63 − 1).
- **The total** in the display currency is the node's to compute ([Connector](connector.md#the-owners-total)); the wallet shows it whenever the node reports one.

## Theme

Light, dark, or as the system has it. The choice is kept in `localStorage` under `dassie.theme` (`dark` or `light`; "system" removes the key) and applied as `data-theme` on the root element, from the bundle before rendering. Storage that refuses is ignored and the choice lasts the page. The toggle in the navigation cycles system → dark → light → system; Settings sets it directly.

## Shared components (`packages/ui`)

The wallet, the dev dashboard and the website share them. The package imports nothing of any app: only itself, Solid, `@dassie/rpc` and its `proto` module, and `uqr` (a QR encoder without dependencies of its own), which a test enforces.

| Export | What |
|---|---|
| `amount`, `currency`, `format` | The `Amount` component, formatting and parsing ([above](#amounts-and-currencies)); times, dates, hex, short IDs, JSON with bigints |
| `debug` | The [debug pages](#debug-pages) |
| `live`, `live/reduce`, `live/subscription` | [Live data](#live-data) |
| `logs` | The [log viewer](#log-viewer) |
| `network`, `network/nodes` | The network graph and its layout; the listing's sorting and groups; the routing graph |
| `node-data` | The node's live ledgers and node table |
| `router`, `rpc`, `toast`, `ui`, `theme`, `theme-toggle`, `qr` | The router, the RPC context, toasts (at most 5, each shown 5 s), cards, alerts, badges, checklists, tabs, confirm dialogs (the browser's `<dialog>`), copy buttons (✓ for 1 s), masked secrets, fields, the theme, QR codes as SVG |
| `ui.css` | The shared stylesheet, imported first by each app's own; 12 seeded colours (`seed-0` … `seed-11`), picked per ID by an FNV-1a hash |
| `testing/csp`, `vite` | The build checks for the CSP, and the Vite aliases that resolve the workspace and keep one copy of Solid |

### Log viewer

- A **Filter** field: whitespace-separated keywords, each of which must match; a keyword starting with `-` must not. A keyword matches a line whose source, target, message or a `name=value` field contains it, ignoring case.
- A **Level** select (debug, info, warn, error): lines below it are hidden. The number of lines shown.
- Only the rows in view are rendered (20 px each, 10 rows of overscan). The view follows the newest line unless the reader scrolled up, and follows again once scrolled back to the bottom.
- A row: the time (`HH:MM:SS.mmm`), the source (the dev dashboard only, linked to its page), the level, the target in its seeded colour, the message and the fields; the fields also as the row's tooltip.
- In the dev dashboard, dev nodes' vanity IDs (`d12_…`) in a message become links named `d12`.

## Tests

- `apps/wallet/build_test.ts`: the built page has no inline script or style, no `style` attribute in templates, nothing from elsewhere; and the production build contains no `dassie-dev-login`.
- Unit tests for the gate, the dev-login fragment, the ledger removal checklist, the connection text, caps, rates, Send's helpers, incoming payments and grants in words; in `packages/ui`, the amount formatting and parsing, live reducers and subscriptions, the router, the theme, the log filter, the network listing and layout, the QR encoder and the package boundary.
- `tests/e2e/wallet_test.ts` drives every page in a browser against two nodes ([Testing](testing.md)).

## Decisions

- [D1](../decisions/D01-what-runs-in-rust-and-what-in-typescript.md): the wallet is Solid 2, and keys are derived in the browser with the node's own code compiled to WASM.
- [D5](../decisions/D05-ui-and-cli-rpc-contract.md): the typed RPC contract and its live streams replace the original's RPC and polling.
- [D6](../decisions/D06-owner-ledger-and-multi-currency-accounts.md): every amount carries its currency; the display currency and its total.
- [D7](../decisions/D07-exchange-rates-fees-and-spread.md): rate sources, fees and spread, as the Settings page edits them.
- [D9](../decisions/D09-open-payments-fully-and-no-spsp.md): wallet addresses, payment requests, the consent page, connected apps and the owner's Send.
- [D10](../decisions/D10-secrets-at-rest.md): BTP tokens shown once, secret columns never shown.
- [D12](../decisions/D12-peering-lifecycle-and-liveness.md): terminating peerings and bans.
- [D21](../decisions/D21-tooling-on-node-and-pnpm-not-deno.md): built on Node and pnpm.

## Original

- [6 §1](../original/06-user-interfaces-and-tooling.md): the original wallet (`gui-dassie`), page by page, with its gaps UI-1 to UI-8.
- [6 §1.7](../original/06-user-interfaces-and-tooling.md): the original log viewer.
- [2 §8, §10](../original/02-node-runtime.md): the original's setup, login and RPC procedures.
