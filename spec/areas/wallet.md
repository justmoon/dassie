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

- **RPC.** One client over a WebSocket to the node's own origin, carrying the contract in `proto/dassie/ui/v1` ([RPC](rpc.md)). When the node ends this browser's session (logged out elsewhere, or revoked), the wallet reloads, which shows the login page.
- **HTTP.** Setup, login and logout are the node's only routes outside the contract ([HTTP](http.md#setup), [login and logout](http.md#login-and-logout)). Success sets or clears the session cookie, and the wallet reloads so that [the gate](#the-gate) picks the next screen rather than the wallet tracking it.

## Key derivation

- Keys are derived in the browser with `dassie-identity` compiled to WASM, the same code the node runs ([Identity](identity.md), [D1](../decisions/D01-what-runs-in-rust-and-what-in-typescript.md)), so the browser and the node cannot disagree on what a passphrase means.
- A passphrase is checked (known words and checksum) and normalized before anything is derived from it.
- Setup sends the node key and the login proof derived from it; login sends only the proof. The passphrase never leaves the browser and is never stored, in the URL or anywhere else.

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
- The node's ledgers and node table are shared by every page while the owner is signed in (`packages/ui/src/node-data.ts`).

## Pages

The pages are in `apps/wallet/src/pages/`, and their routes in `apps/wallet/src/layout/Shell.tsx`. The wallet holds no rule of the node's: it shows what the node reports and asks the node to act.

- What blocks removing a ledger, the total in the display currency, whether a rate is stale, what a quote costs and how long it holds, and what a grant asks for all come from the node ([Ledger](ledger.md), [Connector](connector.md), [Payments](payments.md)).
- The network page draws the routing table, not the peerings: an edge only where both ends list it, so a peer that is offline or does not list us back has no line. The layout is deterministic: a node starts where its ID puts it and keeps its place as the table changes.
- A BTP token's secret is shown once, when it is made; the node keeps only its hash ([Connector](connector.md#btp-tokens), [D10](../decisions/D10-secrets-at-rest.md)).
- The consent page is where the node sends the owner when a third-party app asks for access ([Payments](payments.md#the-owners-consent)). It names Connected Apps, where access is revoked, but links there only once the owner has decided, so that no one leaves a request half-read.

## Amounts and currencies

- Every amount carries its currency, a ledger's asset code and scale, as the node reports it ([D6](../decisions/D06-owner-ledger-and-multi-currency-accounts.md)). Amounts are exact integers in the asset's smallest unit (bigints), never floats.
- Display cuts digits, never rounds, and mutes the digits beyond cents, so a balance never shows more than it is.
- Input refuses a number with both a comma and a point, since `1,500` means different things on different keyboards.

## Debug pages

The debug pages (`packages/ui/src/debug`) read the node only through the RPC client and the node data of their context, so the [dev dashboard](dev.md#dashboard) shows them for any node. What the node puts in their streams, and which columns are secret, is the node's rule ([Node](node.md#logging), [RPC](rpc.md#the-inspector)).

## Shared components (`packages/ui`)

The wallet, the dev dashboard and the website share them: amounts, live data, the debug pages, the log viewer, the network graph, the router, the theme and the CSP checks. The package imports nothing of any app, which a test enforces, and `@dassie/ui/vite` resolves the workspace with one copy of Solid.

## Decisions

- [D1](../decisions/D01-what-runs-in-rust-and-what-in-typescript.md): the wallet is Solid 2, and keys are derived in the browser with the node's own code compiled to WASM.
- [D5](../decisions/D05-ui-and-cli-rpc-contract.md): the typed RPC contract and its live streams.
- [D6](../decisions/D06-owner-ledger-and-multi-currency-accounts.md): every amount carries its currency; the display currency and its total.
- [D7](../decisions/D07-exchange-rates-fees-and-spread.md): rate sources, fees and spread, as the Settings page edits them.
- [D9](../decisions/D09-open-payments-fully-and-no-spsp.md): Open Payments in full, which the wallet's payment, consent and connected-app pages front ([Payments](payments.md)).
- [D10](../decisions/D10-secrets-at-rest.md): BTP tokens shown once, secret columns never shown.
- [D12](../decisions/D12-peering-lifecycle-and-liveness.md): terminating peerings and bans.
- [D21](../decisions/D21-tooling-on-node-and-pnpm-not-deno.md): built on Node and pnpm.
