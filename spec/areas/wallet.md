# Wallet

The wallet is the node owner's web app: a Solid 2 single-page app that the node serves from its own binary. The owner sets the node up, logs in, manages ledgers, sends and receives, approves third-party apps and reads the debug pages there. `packages/ui` holds the components it shares with the dev dashboard and the website.

**Paths:** `apps/wallet/`, `packages/ui/`
**Read with:** [RPC](rpc.md), [HTTP](http.md), [Payments](payments.md), [Identity](identity.md), [Dev environment](dev.md)

## Build and serving

- The node embeds the production build (`apps/wallet/dist/`) at build time and serves it on its own origin ([HTTP](http.md#the-wallets-files)). Dev nodes serve a development watch build instead ([Dev environment](dev.md#builds)).
- The wallet has its own small router (`packages/ui/src/router`), since no router for Solid 2's release candidate matches the pinned version.

## Content Security Policy

The node serves the page under a strict policy ([HTTP](http.md#response-headers)): no inline script or style, nothing from another origin, and WebAssembly may compile, for the identity module. The wallet is built to keep to it:

- The theme is applied from the bundle, so the first paint follows the system's theme until the script runs.
- Styles come from classes; positions computed at run time are set through the DOM, never a `style` attribute (Solid's compiled templates included).
- QR codes and the network graph are SVG drawn in the page.
- Every asset is a file of its own: a `data:` URL for the WASM module would be refused by `connect-src 'self'`.

The same policy applies in development, so there is no HMR: a change shows after a reload. `apps/wallet/build_test.ts`, through `packages/ui/testing/csp.ts`, checks the built page.

## Talking to the node

- **RPC.** One client over a WebSocket to the node's own origin, carrying the contract in `proto/dassie/owner/v1` ([RPC](rpc.md)). When the node ends this browser's session (logged out elsewhere, or revoked), the wallet reloads, which shows the login page.
- **HTTP.** Setup, login and logout are the node's only routes outside the contract ([HTTP](http.md#setup), [login and logout](http.md#login-and-logout)). Success sets or clears the session cookie, and the wallet reloads so that [the gate](#the-gate) picks the next screen rather than the wallet tracking it.

## Key derivation

- Keys are derived in the browser with `dassie-identity` compiled to WASM, the same code the node runs ([Identity](identity.md), [rust-and-typescript](../decisions/rust-and-typescript.md)), so the browser and the node cannot disagree on what a passphrase means.
- **The realm is the node's.** The wallet makes and reads phrases in the realm `GetBasicState` reports, as the first segment of the node's ILP address, which before setup is the realm the node sets up in, and setup names it in its request ([HTTP](http.md#setup)). A phrase of the other realm is refused at setup and at login, before anything is derived ([phrase-format](../decisions/phrase-format.md)). A node that reports a realm the wallet does not know gets neither the setup wizard nor the login page.
- **A new phrase** is made by the browser's clock, and only while that clock is within a day of the node's, which the wallet reads from the `Date` header of a response from the node ([HTTP](http.md#response-headers)). It is shown with the date it was made, its 20 words numbered in two columns of ten, as a metal plate lays them out, with the four letters a plate keeps in bold. The owner then types one of the words back, which is matched as a reading matches it.
- **Entering a phrase,** at setup and at login, the owner sees what each word reads as while typing, matched by its first four letters with the reader's own code, with a word not on the list flagged; they can mark a word they cannot read, and the mark stays on that word as words are added or removed before it, or type `?` for it. Each refusal has words of its own (`apps/wallet/src/messages.ts`): a BIP-39 phrase, a wrong count, too damaged, not a phrase, a newer Dassie, the other realm, and a date after this clock, which says how to read a phrase made by a clock that ran ahead.
- **A correction is shown before it is used:** the 20 words with the changed ones marked, which takes the focus so that a screen reader reads it out, and the phrase is taken only once the owner confirms it.
- Setup sends the node key; login sends only the login proof derived from it. The passphrase never leaves the browser and is never stored, in the URL or anywhere else.

## The gate

Which screen the wallet shows follows from `GetBasicState`, asked once per page load, and the path: the setup wizard at `/setup/<token>` while the node is uninitialized, the login page while this browser has no session, the owner's wallet otherwise (`apps/wallet/src/gate.ts`).

For the signed-in owner the wallet follows the node's ledgers live. Without a ledger it shows only the choice of a first ledger, without navigation; because the ledgers are live, adding the first ledger or removing the last takes effect at once.

### Dev auto-login

Only a development build has this code. It reads a login proof from the URL fragment, removes it from the URL and history, and sends it as an ordinary login, which the node checks like any other. The production build has none of it, which `apps/wallet/build_test.ts` checks, since the dev nodes' keys are public ([Dev environment](dev.md#wallet-links)).

## Live data

Every `Subscribe*` procedure delivers a value stream or a snapshot followed by typed deltas ([RPC](rpc.md)). `packages/ui/src/live` turns them into Solid signals and stores, so the client needs none of the node's reducer code.

- A stream stays open while its component is shown and ends when the page is left.
- A stream that ends or fails is subscribed again after a jittered backoff; each attempt starts with a fresh snapshot, so a missed delta can never leave a store wrong. Errors that cannot heal by retrying (unauthenticated, not found, invalid, unimplemented) are final.
- Pages show when a view is not live, rather than showing stale data as current.
- Until a stream carries them ([roadmap](../roadmap.md#several-hubs-per-node)), the node's status and its hubs are asked for again every few seconds while they are shown, one request at a time: a stopgap, since live data comes from streams ([rpc-contract](../decisions/rpc-contract.md)). A failed ask keeps what is known, and the Hubs page says it is not current.
- The node's ledgers, with the external assets they settle through, and its node table are shared by every page while the owner is signed in (`packages/ui/src/node-data.ts`). The external assets come with the ledgers' stream, so a page never shows a ledger and its asset from two moments.

## Pages

The pages are in `apps/wallet/src/pages/`, and their routes in `apps/wallet/src/layout/Shell.tsx`. The wallet holds no rule of the node's: it shows what the node reports and asks the node to act.

- A ledger being removed shows that the node is settling up with its peers on it first, with how many methods are left, as the node reports it; peers that settle with a ledger do not block its removal ([Settlement](settlement.md#schemes-and-ledgers)).
- A peer's details list each of its settlement methods in the order the pair agreed them, with what is owed to it on each and whether the method is being wound down or is short of room, as `ListPeers` reports them ([Network](network.md#settlement-methods)).
- What blocks removing a ledger, the total in the display currency, whether a rate is stale, what a quote costs and how long it holds, and what a grant asks for all come from the node ([Ledger](ledger.md), [Connector](connector.md), [Payments](payments.md)).
- The network page draws the routing table, not the peerings: an edge only where both ends list it, so a peer that is offline or does not list us back has no line. The layout is deterministic: a node starts where its ID puts it and keeps its place as the table changes.
- On a [periphery node](network.md#periphery-nodes), which its uplinks do not list, the page draws the node with a line to each uplink that is a first hop, so not to one it lost that another uplink still reaches, and shows its uplinks and its relay where a core node's registration is: a periphery node registers with no bootstrap node. A core node's card shows its rate feed beside its registration, as `GetStatus` reports it ([Rate feed](rate-feed.md#what-the-owner-sees)); a periphery node publishes none, and its card does not mention one.
- Receive shows each ledger's deposit method as the node reports it: the stub's test money in a build that runs the stub ([stub-in-debug-builds](../decisions/stub-in-debug-builds.md)), or the address the owner sends to, laid out as exchanges lay out theirs (the network, the address as a QR code and text, that no destination tag is needed, the reserve, what not to send), with the faucet where the node has one ([XRP Ledger](xrpl.md#deposits-and-withdrawals)). The faucet is asked by the node, never by the page: the page connects to nothing but the node.
- A ledger that settles in a token, such as `xrpl-testnet-dsusd+usd` or mainnet's `xrpl-rlusd+usd`, has its Receive name both what its account needs: the token, from its issuer, whose address the node reports with the ledger from the start, so the owner knows which token to send before the line exists, and the ledger's own asset, XRP, which pays the fees and the reserve as gas, from the faucet where the node has one, else sent to the same address, as on mainnet ([XRPL](xrpl.md#the-token-scheme)). Where the token is not simply sent, Receive says how to get it: DSUSD has no faucet, and is bought with XRP on the testnet's XRP/DSUSD AMM pool ([XRPL](xrpl.md#dsusd)). The reserve is shown in XRP, as the ledger holds it, never in the ledger's currency, and so is what the account holds of each: from the account, not the books. Which schemes settle in a token, and what they pay gas in, is the wallet's description of the scheme (`apps/wallet/src/pages/schemes.ts`).
- A dollar token's ledger shows its peg guard as the node reports it: the token's last price on its pool, and while the node pauses the scheme, why, off its peg or its price unread, and what the pause stops; Receive shows the same, since what arrives is still booked ([XRPL](xrpl.md#the-peg-guard)). The wallet holds none of the guard's bands or holds.
- External Assets shows what the node holds outside its books, each asset with its address, what it holds as the ledger reports it (XRP with the reserve, each trust line, and whether its issuer froze it, deep or for every holder), and the schemes that settle through it ([external-assets](../decisions/external-assets.md)). It is where the owner funds them, with the faucet where the node has one. The node reports the holdings; the wallet never adds them up from the books, whose `assets/settlement` only follows the holding a ledger settles in ([Settlement](settlement.md#external-assets)).
- Hubs shows every settlement hub the node is in, as `GetHubStatus` reports it: its account, balance, reserve and what deposits and withdrawals moved, the holds toward each peer and from it, the floor and ceiling, whether the node moves funds by itself and until when the owner's last move pauses it, the move on its way and the one given up, and how many validators answer ([Settlement hubs](hubs.md#in-the-node)). While the issuer of a token hub's asset holds the hub's escrow, it says so: since when the hub is frozen, in place of how the account stands unless the node cannot follow the hub, that nothing crosses it until the thaw while what the hub still pays goes, or that its payout waits for the issuer to let go ([Settlement hubs](hubs.md#a-frozen-hub)). It deposits and withdraws, turns the node's own moves on or off, and gives up a move once the node says it is late; each names its hub by its ID. A hub's amounts are in the asset of its scheme's ledger, as the node lists that ledger, never an asset the page assumes. Once a hub ended or stalled, it shows how far the payout the node keeps of it is signed, and once a quorum signed one set, its transactions to copy and submit ([Settlement hubs](hubs.md#payout-bundles)). It draws in Kopje ([visual-identity](../decisions/visual-identity.md)): Ember for action, and Kopje's value cube only to mark the balance.
- Names registers a name for the node, and shows how each of its names stands, and with each registrar what it logged, took and lists, as the node reports them; it renews a name now, and registers one that was not granted again ([Network](network.md#the-registrant)). The node decides which names may be registered: the page checks no syntax of its own. While the node reports names off, in the live realm, the page says names run on the testnet only until registering one has a fee, and offers nothing else ([Network](network.md#names)).
- Settings shows the Dassie version the node reports. The wallet has no version of its own: the node serves the wallet built into it.
- Settings lists the paired devices, each revoked on its own, and shows a pairing code as a QR code. The node decides who may pair: only a browser on its own machine ([RPC](rpc.md#paired-devices)), and the card shows the node's refusal to any other, which names `dassie pair`.
- A BTP token's secret is shown once, when it is made; the node keeps only its hash ([Connector](connector.md#btp-tokens), [secrets-at-rest](../decisions/secrets-at-rest.md)).
- The note on the node's name at the default front door follows the relay `GetStatus` reports: the name reaches the node while it holds the connection to that front door, and the wallet warns once the node finds that front door unreachable, but not while it is still connecting ([Node](node.md#a-chosen-relay)), or when the node keeps another relay or none ([Payments](payments.md#the-nodes-names)).
- The consent page is where the node sends the owner when a third-party app asks for access ([Payments](payments.md#the-owners-consent)). It names Connected Apps, where access is revoked, but links there only once the owner has decided, so that no one leaves a request half-read. When the node flags a limit that no payment to the named receiver can fit, the page warns of it and leaves Approve enabled ([unusable-limits](../decisions/unusable-limits.md)).

## Amounts and currencies

- Every amount carries its currency, a ledger's asset code and scale, as the node reports it ([owner-ledger](../decisions/owner-ledger.md)). Amounts are exact integers in the asset's smallest unit (bigints), never floats.
- Display cuts digits, never rounds, and mutes the digits beyond cents, so a balance never shows more than it is.
- Input refuses a number with both a comma and a point, since `1,500` means different things on different keyboards.

## Debug pages

The debug pages (`packages/ui/src/debug`) read the node only through the RPC client and the node data of their context, so the [dev dashboard](dev.md#dashboard) shows them for any node. What the node puts in their streams, and which columns are secret, is the node's rule ([Node](node.md#logging), [RPC](rpc.md#the-inspector)).

## Shared components (`packages/ui`)

The wallet, the dev dashboard and the website share them: amounts, live data, the debug pages, the log viewer, the network graph, the router, the theme, Kopje's palette for a page that draws in it (`.kopje` in `ui.css`), and the CSP checks. The package imports nothing of any app, which a test enforces, and `@dassie/ui/vite` resolves the workspace with one copy of Solid.

## Decisions

- [rust-and-typescript](../decisions/rust-and-typescript.md): the wallet is Solid 2, and keys are derived in the browser with the node's own code compiled to WASM.
- [phrase-format](../decisions/phrase-format.md): the phrase the wallet makes and reads, its realm, and corrections the owner confirms.
- [rpc-contract](../decisions/rpc-contract.md): the typed RPC contract and its live streams.
- [external-assets](../decisions/external-assets.md): holdings as the ledger reports them, never as the books add them up.
- [owner-ledger](../decisions/owner-ledger.md): every amount carries its currency; the display currency and its total.
- [exchange-rates](../decisions/exchange-rates.md): rate sources, fees and spread, as the Settings page edits them.
- [passive-rebalancing](../decisions/passive-rebalancing.md): the curve's `k`, edited beside the spread.
- [open-payments](../decisions/open-payments.md): Open Payments in full, which the wallet's payment, consent and connected-app pages front ([Payments](payments.md)).
- [secrets-at-rest](../decisions/secrets-at-rest.md): BTP tokens shown once, secret columns never shown.
- [peering-lifecycle](../decisions/peering-lifecycle.md): terminating peerings and bans.
- [node-and-pnpm](../decisions/node-and-pnpm.md): built on Node and pnpm.
- [unusable-limits](../decisions/unusable-limits.md): why the consent page warns of a receive limit no payment can fit, rather than refusing it.
- [default-relay](../decisions/default-relay.md): the default relay, whose connection the note on the default name reports.
