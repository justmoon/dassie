# 6. User interfaces, packaging and developer tooling

Source citations refer to the original implementation at commit `e962ab78` on `main` (see [the index](README.md#sources)).

Scope: `packages/gui-dassie`, `packages/app-website`, `packages/app-cli`, `packages/app-build`, `packages/app-dev`, `packages/gui-dev`, the root tooling files, and the `meta-*` packages.
All paths are relative to the repository root.
Items marked **Open question / known gap** are TODOs, bugs or unfinished features found in the source. A reimplementation has to decide whether to copy them or fix them.

---

## 0. Cross-cutting summary (read first)

| Surface | Tech today | Where it runs | Talks to |
|---|---|---|---|
| Node web UI ("Dassie Wallet") | React 18 + wouter 3.3.5 + TanStack Query 5 + Radix/shadcn-style components + UnoCSS | Served by the node over HTTPS (prod: static files from `share/public`, dev: Vite middleware) | `wss://<host>/rpc` (dassie-rpc-01 over WebSocket, superjson), REST `POST /api/{login,logout,setup}` and dev-only `POST /api/_dev/set-session` |
| Dev dashboard ("Dassie//dev") | React 18, reuses gui-dassie components | Vite dev server at `https://localhost:10100`, reached through an SNI proxy on `:443` | `wss://dev-rpc.localhost` (UI RPC router) plus each node's `wss://dN.localhost/rpc?token=…` |
| Website (dassie.land) | Astro 5.1.1 + Starlight 0.30 + React island | Static site, `site: https://dassie.land` | Nothing (the STREAM toy simulates in the browser) |
| CLI npm package `dassie` | One-line Node script | npm | Nothing (placeholder) |
| Release build | vite-node script (`cmd-ts`), esbuild, vite, Docker builder | Docker container, `--network none` | Produces `dist/upload/**`, uploaded with `gcloud storage cp` to `gs://get.dassie.land` |
| Installer | POSIX `sh` script | `curl --tlsv1.2 -sSf https://sh.dassie.land \| sh` | `https://get.dassie.land/meta/latest` and the bundles |
| Dev environment | Vite SSR module runner, one forked Node child process per simulated node | Host machine. Needs mkcert, openssl, `*.localhost` resolving to 127.0.0.1, and permission to bind port 443 | Runner RPC over TCP :10102, UI RPC over wss :10101 |

Both GUIs depend on **TypeScript types and store implementations imported straight from `@dassie/app-dassie`**: `AppRouter`, `NodeTableStore`, `LogsStore`, seed-path constants, `SettlementSchemeId`, and others. `useRemoteStore` replays server store *actions* on a client-side copy of the same store implementation (see §1.4). A Rust/Solid rewrite cannot reuse those reducers. It needs either a shared reducer spec or server-sent snapshots/patches.

---

## 1. `packages/gui-dassie`: the node web UI

### 1.1 Package and build
- `packages/gui-dassie/package.json`: private, `@dassie/gui-dassie`. Key deps:
  - React 18.3, wouter 3.3.5 (router), @tanstack/react-query 5, @tanstack/react-virtual (log viewer)
  - Radix primitives (dropdown-menu, label, navigation-menu, scroll-area, slot, tabs, toast)
  - react-hook-form + zod + @hookform/resolvers
  - @scure/bip39, @noble/hashes (HMAC-SHA256)
  - react-force-graph-2d, @react-hook/size, react-number-format, anser (ANSI parsing)
  - lucide-react icons, zustand (toast queue), superjson, nanoid
  - class-variance-authority, clsx, tailwind-merge, unocss
- `vite.config.ts`: `@vitejs/plugin-react` and `unocss/vite` (`configFile: ./uno.config.ts`, which re-exports `@dassie/meta-unocss-config`), with `build.target: esnext`.
- `index.html`:
  - Title "Dassie Wallet", `<html class="h-full">`, `#root`.
  - An inline script adds the `dark` class when `prefers-color-scheme: dark`.
  - Links favicon/apple-touch/manifest from `src/assets/…`.
- `src/index.tsx`:
  - Imports `virtual:uno.css` and `@dassie/meta-unocss-config/base.css`.
  - In DEV it first runs `checkForDevelopmentSessionToken()` (see §1.3), then renders `<Root/>`.
- `src/root.tsx`:
  - Builds the WebSocket URL `wss://<location.host>/rpc` (always wss).
  - Provider order: `DarkModeProvider` → `RpcProvider` (a lib-reactive-rpc client with superjson transformer) → `QueryClientProvider` → `<App/>` + `<Toaster/>`.
- Path alias `@/*` → `./src/*` (`tsconfig.json`).
- Production build: app-build calls `vite build` with root `packages/gui-dassie`, output `dist/staging/shared/frontend`, and `define __DASSIE_VERSION__` (see §5).
- **Open question / known gap**: icons and manifest.
  - `index.html` references `src/assets/favicon.ico`, but `favicon.ico` lives in `src/public/`.
  - The android-chrome icons are also in `src/public/`, while Vite's default `publicDir` is `<root>/public`. So `favicon.ico` and `/android-chrome-*.png` (referenced by `site.webmanifest`) are probably not served.
  - The manifest's `name` and `short_name` are empty strings.
- **Open question / known gap**: `src/index.css` (shadcn tokens in comma-separated HSL form) is **not imported** by the wallet. The wallet uses `meta-unocss-config/base.css`. Only the website's STREAM toy imports `gui-dassie/src/index.css`.

### 1.2 Styling approach
- UnoCSS with `presetUno`, `presetIcons`, a custom `presetShadcn` (`packages/meta-unocss-config/preset.shadcn.ts`: animate-in/out, accordion keyframes and similar), and `transformerDirectives` (`@apply`).
- Theme colors are CSS variables (`--background`, `--foreground`, `--primary`, `--secondary`, `--muted`, `--accent`, `--destructive`, `--popover`, `--card`, `--border`, `--input`, `--ring`, `--radius: 0.5rem`). They are defined in `packages/meta-unocss-config/base.css` for `:root` and `.dark` (zinc-based shadcn palette).
- Dark mode is the `.dark` class on `<html>`. `src/components/context/dark-mode.tsx` provides a `DarkModeProvider` with setting `"system" | "dark" | "light"` (default `"system"`, follows `matchMedia`). **Open question / known gap**: no UI anywhere calls `setDarkMode`, so there is no user toggle.
- Component kit (`src/components/ui/*`) is shadcn/ui ported to UnoCSS:
  - alert, button (variants default, destructive, outline, secondary, ghost, link; sizes default, sm, lg, icon), card, dropdown (unused), form (react-hook-form bindings), input, label, navigation-menu, scroll-area, table, tabs, toast (Radix toast + zustand queue).
  - Helpers: `combine()` = `twMerge(clsx())`, `combineVariants` (cva), `extend(name, element, classes)` for styled wrappers (`src/utils/class-helper.ts`).
- Custom components:
  - `ui-custom/checklist.tsx`: valid/invalid/unknown icons, colored green-500/red-500/slate-400.
  - `vectors/logo.tsx`: inline SVG logo.
  - `amount/amount.tsx`.
- Categorical palette `src/constants/palette.ts`: Adobe Spectrum's 12 colors (`#0fb5ae #4046ca #f68511 #de3d82 #7e84fa #72e06a #147af3 #9a4dff #e8c600 #cb5d00 #008f5d #bce931`). `selectBySeed(COLORS, string)` (from lib-logger) picks a stable color per node ID or log namespace.

### 1.3 Authentication, setup and session (client side)
Files: `src/utils/authentication.ts`, `src/utils/signer.ts`, `src/utils/development.ts`. Server counterparts are in `packages/app-dassie/src/authentication/http-routes/*.ts`.

Key derivation (client):
1. Passphrase = BIP-39 mnemonic. `bip39.mnemonicToSeed(passphrase)` is the standard PBKDF2-HMAC-SHA512 (2048 rounds, salt `"mnemonic"`, empty password), producing a 64-byte seed.
2. `getPrivateSeedAtPath(seed, path)`: split the path on `/` and fold `key = HMAC-SHA256(key, utf8(segment))`.
   - `SEED_PATH_NODE = "node"` gives `nodeKey = HMAC(seed, "node")`, the raw Ed25519 private key.
   - `SEED_PATH_NODE_LOGIN = "node/login"` gives `loginSig = HMAC(HMAC(seed,"node"),"login")`.
   - `SEED_PATH_DEV_SESSION = "dev/session"` gives the deterministic dev session token.
   - `SEED_PATH_SETTLEMENT_MODULE = "node/settlement-module"` is server-side only.
   - Constants come from `packages/app-dassie/src/constants/seed-paths.ts`.
3. The server recomputes `HMAC(nodeKey, "login")` from the stored node key (`app-dassie/src/crypto/utils/seed-paths.ts`). The seed itself never leaves the browser. Only `nodeKey` is sent, and only during setup.

HTTP calls:
- `POST /api/setup` with JSON `{ setupAuthorizationToken, rawDassieKeyHex: hex(nodeKey), loginAuthorizationSignature: hex(loginSig) }`.
  - Server checks: node not already set up, the token matches `SetupAuthorizationTokenSignal` (32 random bytes hex, regenerated each process start), and the login signature matches.
  - On success it stores the key (`setNodeIdentity`), creates a session and sets the cookie.
  - Failure returns 401 `UnauthorizedFailure`.
- `POST /api/login` with `{ loginAuthorizationSignature }`. A match creates a session token (32 random bytes hex) and sets the cookie.
- `POST /api/logout` removes the session from `SessionsStore` and clears the cookie.
- Cookie: `__Host-Dassie-Session`, `Max-Age` 10 years (`app-dassie/src/constants/cookie-name.ts`, `authentication/constants/cookie-lifetime.ts`).
- After any successful auth call the UI does `location.reload()`. After setup it first does `history.replaceState({}, "", "/")`.
- RPC auth: the WebSocket upgrade on `/rpc` reads the cookie. An unauthenticated socket can still call public routes (`general.getBasicState`). Protected routes return `RpcFailure("Unauthorized")` (`app-dassie/src/rpc-server/rpc-server.ts`, `route-types/protected.ts`). In DEV a `?token=<64-hex>` query parameter equal to `DASSIE_DEV_SECURITY_TOKEN` also authenticates.
- Dev auto-login (`src/utils/development.ts`, DEV builds only):
  1. If the URL has `?_DASSIE_DEV_SESSION=<token>`, POST `/api/_dev/set-session {sessionToken}`. The dev runner registers this route (§6.6) and it sets the cookie.
  2. Strip the parameter with `history.replaceState`.
  3. `location.reload()`.
- **Open question / known gap**: the mnemonic is never validated (`bip39.validateMnemonic` is not called) in login or recover. Any string is accepted and simply derives a different identity. Login with a wrong passphrase just shows nothing on failure: the mutation has no error UI.

### 1.4 RPC client and real-time model
- `src/utils/rpc.ts`: `createRpcReact<AppRouter>()` exports `rpc.*` hooks: `.useQuery`, `.useMutation`, `.useSubscription`. Transformer is superjson, so bigint, Map, Set and Date survive.
- Wire protocol `dassie-rpc-01` (`packages/lib-rpc/src/common/envelope.ts`):
  - Client sends `{protocol, type:"request", routeType: query|mutation|subscription, id, path:[...], input}`.
  - Server replies `{type:"response", id, result:{type:"success",data}|{type:"error",message}}`.
  - Subscriptions stream `{type:"event", id, data:[...]}`.
  - The client cancels with `{type:"cancel", id}`.
  - The WebSocket link reconnects with exponential backoff and queues sends while disconnected (`lib-rpc/src/client/links/websocket.ts`).
- Real-time helpers (`packages/lib-reactive-rpc/src/client/reactive/index.tsx`):
  - `useRemoteSignal(rpc.x.subscribe…)`: the server sends the current value first, then every new value. The client keeps the latest value in React state, `undefined` until the first event.
  - `useRemoteStore(rpc.x.subscribe…, StoreFactory)`: the server sends `{type:"initial", value}` and then `{type:"changes", value:[actionName, params]}`. The client instantiates the **same store implementation** and replays actions. Used for `NodeTableStore` and `LogsStore`.
  - `rpc.x.useSubscription(input, {onData})` handles raw event streams.
- Server routers available to the UI (`packages/app-dassie/src/rpc-server/app-router.ts`). P = protected, Pub = public.

| Namespace | Procedure | Kind | Used by UI page |
|---|---|---|---|
| general | getBasicState | query, **Pub** | App gate, nav status (polled every 1 s), Account, Settings, Receive, Network, Delete-ledger, API keys |
| general | subscribeBalance | sub (signal `TotalOwnerBalanceSignal`) | Account (via `useAccount`) |
| payment | resolvePaymentPointer {paymentPointer} | query | Send step 1 |
| payment | createPayment {paymentId, paymentPointer, amount:string} | mutation | Send step 3 |
| ledgers | getList, returns `[{id, balance}]` | query | Ledgers page |
| ledgers | checkLedgerDeletePrerequisites(id), returns `{isActive,isUnused}` | query | Delete ledger |
| ledgers | addSettlementScheme {id, config:{}} | mutation | Create ledger |
| ledgers | removeSettlementScheme {id} | mutation | Delete ledger |
| ledgers | stubDeposit(amountString) | mutation | Receive → Stub tab |
| network | getNodes | query | (unused by UI) |
| network | subscribeToNodeTableStore | sub (store) | Network page, Debug › Nodes |
| registrationClient | subscribeRegistrationStatus | sub (signal) | Network own-node panel, Register page |
| registrationClient | registerWithNode {nodeId} | mutation | Register page |
| apiKeys | getCurrentKeys / addBtpToken / removeBtpToken(token22) | q / m / m | Settings › API keys |
| debug | getLedger, subscribeToLogs (store), subscribeRoutingTable (signal), getContextKeys, getSignalState(id), subscribeToTopic(id), getDatabaseTables, getDatabaseTableRows(tableId), subscribeToLedgerAccount(path) | mixed | Debug pages |
| config, tls, acme | getConfig, getSetupUrl, setRealm, setHostname / setNodeTlsConfiguration / setAcmeCredentials, registerToken, deregisterToken | mixed | **Not used by the UI.** Used by the `dassie init` CLI over the IPC socket |

- `getBasicState` returns either `{state:"uninitialized"}` or `{state:"anonymous"|"authenticated", activeSettlementSchemes, nodeCount, hostname, nodeId, ilpAllocationScheme}`. The comment in the source notes this route is public.
- State management:
  - Server data lives in the TanStack Query cache, keyed by procedure and input.
  - Live data lives in local `useState` via the subscription hooks.
  - Multi-step flows are component-local discriminated-union state machines (Setup, Send).
  - The only global client store is the zustand toast queue (`components/ui/toast/toast-queue.tsx`, limit 5, IDs from an incrementing counter).
  - No URL persistence of wizard steps.

### 1.5 Top-level gating logic (`src/app.tsx`)
The checks run in order:
1. `getBasicState` not loaded yet: render nothing.
2. `state === "uninitialized"`:
   - Path matches `/setup/:token`: render `<Setup token>` (§1.6.1).
   - Otherwise: render `<SetupInfoPage>`, an alert saying "This Dassie node is ready for setup. Please visit the setup URL provided in your server logs or the output of the `dassie init` command."
3. `state === "anonymous"`: render `<LoginPage>` regardless of the URL.
4. `activeSettlementSchemes.length === 0`: render `<CreateLedgerAccount>` full-screen. This is forced onboarding: you must pick a ledger first.
5. Otherwise: render the shell `grid-rows-[auto_1fr]` with `<MainNavigation/>` and a `<Switch>` over:

| Route | Component | File |
|---|---|---|
| `/network` | NetworkPage | `pages/network/network.tsx` |
| `/network/register` | RegisterPage | `pages/network/register/register.tsx` |
| `/network/:nodeId` | NetworkPage (with selection) | same |
| `/ledgers` | LedgersPage | `pages/ledgers/ledgers.tsx` |
| `/ledgers/create` | CreateLedgerAccount | `pages/ledgers/create-ledger-account/create-ledger-account.tsx` |
| `/ledgers/delete/:id` | DeleteLedgerAccountPage | `pages/ledgers/delete-ledger-account/*.tsx` |
| `/send` | Send | `pages/send/send.tsx` |
| `/receive` | ReceivePage | `pages/receive/receive.tsx` |
| `/payments/:paymentId` | PaymentStatus | `pages/payment-status/payment-status.tsx` |
| `/debug/*` (nested) | DebugPage | `pages/debug/debug.tsx` |
| `/settings` | Settings | `pages/settings/settings.tsx` |
| anything else (incl. `/`, `/dashboard`) | Account | `pages/account/account.tsx` |

- **Open question / known gap**:
  - `NetworkStatus` polls `getBasicState` with `refetchInterval: 1000`. It shares the query key with `App`, so the whole app effectively re-polls basic state every second while the nav is mounted.
  - The forced `CreateLedgerAccount` screen (no nav mounted) does not invalidate `getBasicState` after `addSettlementScheme`. The app only advances once react-query refetches, for example on window focus.

### 1.6 Page-by-page inventory

#### 1.6.1 Setup wizard: `/setup/:token` (only while uninitialized)
Files: `pages/setup/setup.tsx`, `subpage-intro`, `subpage-generate`, `subpage-verify`, `subpage-recover`.
Layout: a centered Card. The CardTitle changes per step.

| Step (title) | Shows | Actions |
|---|---|---|
| intro ("Dassie Wallet") | "Welcome to Dassie Wallet. The first step is to create or recover a wallet." | **Generate a new wallet** → `bip39.generateMnemonic(englishWordlist)` (128-bit, 12 words) → generate. Link **Recover an existing wallet** → recover |
| generate ("Generate Wallet") | Warning text and the mnemonic in a large red (`bg-destructive`) box | **Back** → intro. **Ok, I have backed up my passphrase** → verify |
| verify ("Verify Passphrase") | "please enter word #N" (N random, 1-based) and an input | **Back** → generate (same mnemonic). **Verify** is enabled only when the input equals the chosen word. In DEV the literal `dev` is also accepted |
| recover ("Recover Wallet") | Passphrase text input | **Back** → intro. **Open Wallet** → submit |
| opening ("Opening Wallet") | Empty card | Runs `mnemonicToSeed` → `POST /api/setup` → `replaceState("/")` → reload |

**Open question / known gap**: the opening step has no error handling or spinner. A failed setup leaves an empty card.

#### 1.6.2 Login: shown whenever `anonymous`
Files: `pages/login/login.tsx`, `login-form/login-form.tsx`.
- Two-column layout on large screens:
  - Left: dark panel with the logo, "Dassie Wallet", and an F. A. Hayek quote.
  - Right: heading "Reconnect to your Dassie instance", subtitle "Enter your wallet passphrase".
- Form field "Passphrase" (text, placeholder "correct horse staple …", autocomplete off) with a **Reconnect** button that shows a spinner while pending.
- Submit runs `mnemonicToSeed` → `POST /api/login` → reload.
- Footer link "What is Dassie?" → `https://dassie.land`.

#### 1.6.3 Main navigation (`layout/main-navigation/main-navigation.tsx`)
- Radix NavigationMenu.
- Left items:
  - **Dashboard** (`/dashboard`, which falls through to Account)
  - **Network** (`/network`)
  - **Ledgers** (`/ledgers`)
  - **Debug** dropdown: Logs `/debug/logs`, Nodes `/debug/nodes`, Ledger `/debug/ledger`, Routing `/debug/routing`. State and Database are not linked; they are reachable by URL and from the dev dashboard.
- Right items:
  - `NetworkStatus`: a colored dot (gray "unknown", green "connected") plus `nodeCount`, linking to `/debug/nodes`.
  - **Settings** gear (`/settings`; the label is hidden on small screens).

#### 1.6.4 Account / home: `/` and fallback (`pages/account/account.tsx`)
- Card "Payment Pointer" shows `$<hostname>`, or `???` while loading.
- Card "Balance" shows a large `<Amount>` of `general.subscribeBalance` (live), always formatted with `USD_SPECIFICATION`.
- Buttons **Send** → `/send` and **Receive** → `/receive`.
- `Amount` rendering (`components/amount/amount.tsx`):
  - Optional minus, `symbol` + thin space, the grouped integer part (`toLocaleString`), `.`, then `displayPrecision` emphasized digits.
  - It then shows the next **2** digits muted. Example: 9-digit precision `$ 1,234.56` followed by a muted `78`.
- Currency specs (`constants/currency.ts`): USD `{symbol:"$", code:"USD", displayPrecision:2, totalPrecision:9}` and XRP `{symbol:"XRP", code:"XRP", displayPrecision:2, totalPrecision:9}`.
- `SCHEME_NAME_MAP`: stub → "Stub", xrpl → "XRP Ledger", xrpl-testnet → "XRP Ledger Testnet". `SCHEME_CURRENCY_MAP`: stub → USD, xrpl and xrpl-testnet → XRP (`constants/schemes.ts`).
- **Open question / known gap**: the home balance and Send are hard-coded to USD, so there is no multi-currency display.

#### 1.6.5 Send: `/send` (`pages/send/*`)
A state machine in one Card. A `paymentId = nanoid()` (21 characters) is created once on mount.

| Step (title) | UI | Actions / RPC |
|---|---|---|
| enterPaymentPointer ("Send Payment") | Input "Payment Pointer", placeholder `$example.com`. Pre-filled with `$d2.localhost` in DEV. Shows "Loading..." or "Error: …" | Every keystroke calls `payment.resolvePaymentPointer({paymentPointer})`. **Continue** appears only on success. **Cancel** → `/` |
| enterAmount ("Choose Amount") | "To: <pointer>", a `NumericFormat` input with `$ ` prefix, thousands separator `,`, no negatives, default `"5"` | **Review** → `parseDecimalToBigInt(value, 9)` (for example 5 becomes 5_000_000_000). **Back** |
| review ("Review") | Pointer and `<Amount>` in USD | **Confirm** → `payment.createPayment({paymentId, paymentPointer, amount: amount.toString()})`. On success navigates to `/payment/<id>`. **Back** |

`parseDecimalToBigInt` (`utils/currency.ts`) splits on `.`, computes `int*10^p + frac*10^(p-len(frac))`, and does not round.

**Open question / known gap**:
- The success redirect goes to `/payment/<id>` but the route is `/payments/:paymentId`, so it falls through to Account.
- There is no error display for `createPayment`.
- `PaymentStatus` (`/payments/:paymentId`) only renders the raw ID. It is a stub with no status polling.
- The server has `TODO: Validate paymentId length / uniqueness` (`app-dassie/src/rpc-server/routers/payment.ts`).

#### 1.6.6 Receive: `/receive` (`pages/receive/receive.tsx`)
- Card "Receive" with Tabs:
  - **Interledger** (default): "Send money to the following payment pointer:" and `$<hostname>` in a bordered box.
  - One tab per entry in `activeSettlementSchemes` that has a display name. The tab content is a lazily loaded deposit UI from `SCHEME_DEPOSIT_UI_MAP`.
- Stub deposit UI (`settlement-schemes/stub.tsx`):
  - Explanation text and a **Receive $100** button.
  - The button calls `ledgers.stubDeposit("100000000000")` (100 × 10^9) and navigates to `/` immediately.
- **Back** → `/`.
- **Open question / known gap**: `xrpl` and `xrpl-testnet` map to `null`. Their tab trigger shows but has no content: no deposit address or QR code.

#### 1.6.7 Ledgers: `/ledgers` (`pages/ledgers/ledgers.tsx`, `ledger-list-entry.tsx`)
- Heading "Active Ledgers". For each `ledgers.getList()` entry, a bordered card shows:
  - The scheme display name.
  - A destructive icon button (trash) → `/ledgers/delete/<id>`.
  - **Send** and **Receive** buttons.
  - The balance in the scheme currency.
- Outline button **+ Add Ledger** → `/ledgers/create`.
- **Open question / known gap**: the per-ledger Send and Receive buttons have no handlers.

#### 1.6.8 Create ledger: `/ledgers/create` and forced onboarding (`create-ledger-account.tsx`)
- Card "Choose a Ledger Type".
- Alert "Testnet": "You are connected to the Dassie test network. The settlement methods below do **not** use real money."
- A single option card, "Stub Settlement": "simulates settlement delays but does not actually connect to any external ledger". Its **Select →** button calls `ledgers.addSettlementScheme({id:"stub", config:{}})` and then navigates to `/ledgers`.
- **Open question / known gap**: only Stub is offered. XRPL and XRPL-testnet exist server-side and in the dev scenarios but cannot be added from the UI. `config` is always `{}`.

#### 1.6.9 Delete ledger: `/ledgers/delete/:id`
- The page wrapper redirects to `/ledgers` if `id` is not in `basicState.activeSettlementSchemes`.
- It then loads `checkLedgerDeletePrerequisites(id)`. If `!isActive` it redirects to `/ledgers`.
- Card "Removing Ledger <Name>":
  - Destructive alert "Possibility to lose funds…".
  - "Prerequisites" checklist with one item: "Not used by any peers" (valid) or "Settlement scheme is still in use for some peers" (invalid).
  - Form field "Confirmation". It must equal exactly `DELETE <ID-UPPERCASED>` (for example `DELETE STUB`), validated with zod literal and the error "Confirmation must match exactly the expected format".
- **Remove Ledger Account** (destructive) is disabled unless `isUnused`. It calls `ledgers.removeSettlementScheme({id})` and navigates to `/ledgers`.

#### 1.6.10 Network: `/network`, `/network/:nodeId` (`pages/network/*`)
- Data:
  - `useRemoteStore(network.subscribeToNodeTableStore, NodeTableStore)`
  - `useRemoteSignal(debug.subscribeRoutingTable)`
  - `getBasicState`
- Layout: a grid with heading "Network <nodeId?>", a full-width force graph (`h-xl`), and below it the node listing (left) and the detail panel (right).
- Graph (`network-graph/network-graph.tsx`, react-force-graph-2d):
  - Nodes: every node table entry, `{id: nodeId}`.
  - Links:
    - (a) own node → every node whose `peerState.id === "peered"`.
    - (b) for each other node, each neighbor in `linkState.neighbors` that is not us, has `neighbor > nodeId` (dedupe), and lists this node back (mutual).
  - Existing node objects are kept across updates to preserve positions (reducer).
  - Node color = `selectBySeed(COLORS, id)`, link color `#ffffff`, label = id.
  - Clicking a node navigates to `/network/<id>`.
  - Optional `peerTraffic` topic emits particles along the matching link (speed 0.08). Only the dev dashboard uses this.
- Node listing (`node-listing/node-listing.tsx`):
  - Sorted by `sortNodes`: distance from `routingTable.get("<ilpAllocationScheme>.das.<nodeId>")`. Distance is ∞ when there is no route of type "peer". Ties sort by nodeId.
  - Grouped by distance, with a big number column (`C` for clients, `∞`). Headings: 0 "This Node", 1 "Direct Peers", 2 "Other Nodes", ∞ "No Connection", -1 "Client" (peered but absent from the routing table). Distances of 3 or more get no heading.
  - Each row shows nodeId (bold) and `linkState.url` (or "no link state") and links to `/network/<id>`.
- Detail panel:
  - No selection or own node → `OwnNodeDetail` (hidden on mobile): "Details for Your Node", "Tier 1 Registration", and `RegistrationSettings`.
    - `RegistrationSettings` reads `registrationClient.subscribeRegistrationStatus` → `{nodes:[{nodeId, hostname?, registered}], registrationRatio, threshold}`.
    - It shows a badge ("Registered" green-700 or "Not registered" red-800) for `registrationRatio >= threshold`.
    - A button links to `/network/register`, labeled "Update registration" or "Register".
  - Other node → `NodeDetail`: "Details for <id>" and "Distance: <n|???>".
- **Open question / known gap**: `NodeDetail` ignores `nodeTableEntry` (no URL, alias, settlement schemes, or peer state shown). `RegistrationAdvancedSettings` (per-bootstrap-node list) is dead code.

#### 1.6.11 Register: `/network/register` (`pages/network/register/register.tsx`)
- Bare page "Register" with a list of `registrationStatus.nodes`: `<nodeId> - Registered | Not Registered`.
- An unregistered node gets a **Register** button that calls `registrationClient.registerWithNode({nodeId})`.
- Server errors: "not found in bootstrap lists", "already registered", "not found in node table".
- **Open question / known gap**: unstyled, with no pending, success or error feedback.

#### 1.6.12 Settings: `/settings` (`pages/settings/settings.tsx`, `api-keys/api-keys.tsx`)
Three sections in a two-column layout on desktop.
1. **Wallet** ("Manage your account"): shows Node ID and Payment Pointer `$<hostname>`. **Logout** (destructive) → `POST /api/logout` → reload.
2. **Node Settings** ("Edit your node settings"): the button **Edit Node Settings ↗** just links to `/network`.
3. **API Keys** ("Manage BTP access"):
   - `apiKeys.getCurrentKeys()` returns a list of BTP tokens (22-character base64url strings, generated server-side from 16 random bytes).
   - Empty state: "No API keys created yet. Add one below."
   - Each row:
     - The token masked (`•` × (len-4) + last 4) with an eye toggle.
     - A copy button that writes `btp+wss://:<token>@<hostname>/btp` to the clipboard, swaps the icon to ✓ for 1 s, and toasts "Copied BTP URL to clipboard" (or a destructive toast on failure).
     - A delete ✕ that calls `apiKeys.removeBtpToken(token)` and refetches.
   - **Create BTP Token** calls `apiKeys.addBtpToken()` and refetches.

#### 1.6.13 Debug: `/debug/*` (`pages/debug/debug.tsx`, nested wouter routes)
These pages are also embedded in the dev dashboard per node (§7).

| Route | Shows | RPC |
|---|---|---|
| `/debug/logs` | Log viewer (§1.7) over the node's in-memory log buffer. The node keeps only the last 100–120 lines (`LOGS_SOFT_LIMIT=100`, `HARD=120` in `app-dassie/src/logger/stores/logs.ts`) | `debug.subscribeToLogs` (store) |
| `/debug/nodes` | Table: Node (id truncated to `w-32`, url), Distance, Next Hop (first-hop nodes), Neighbors (`linkState.neighbors`), Peer State (`peerState.id`) | node table store, routing table signal, basic state |
| `/debug/ledger` | Table of all ledger accounts sorted by path: Account (links to the detail page), Balance = `creditsPosted − debitsPosted − debitsPending`, Credits (+ pending in faded text), Debits (+ pending). All amounts formatted as USD | `debug.getLedger` |
| `/debug/ledger/:ledgerId/account/*` | Breadcrumb `<ledgerId> » <path>`. Live transfer table: DR/CR (icon), counter-account, amount, running balance. Pending rows are faded, voided rows are faded and struck through. Empty: "Waiting for transfers to occur..." | `debug.subscribeToLedgerAccount("<ledgerId>:<path>")`. The first event is the account (`{path, creditsPosted, …}`), then Transfer events `{creditAccount, debitAccount, amount, state: pending/posted/voided, immediate}` |
| `/debug/routing` | Table: ILP Prefix, Type (`peer`/`fixed`), Details (peer: `firstHopOptions.join(", ") (distance: n)`), sorted by prefix | `debug.subscribeRoutingTable` |
| `/debug/state` | Tabs: **Signals** (list of signal/computed context items, click shows `getSignalState(id).value`, a pretty-printed string, fetched once). **Actors** (tree built from parent IDs with root "Reactor"). **Topics** (click to live-capture events, pretty-printed) | `debug.getContextKeys` returns tuples `[id, name, type, parent?]` with type ∈ actor/signal/computed/topic/other; `debug.getSignalState`; `debug.subscribeToTopic` |
| `/debug/database` | Left list of DB tables (name). Clicking one shows a table with every column (bigint → string) | `debug.getDatabaseTables` returns `[{id,name,columns}]`; `debug.getDatabaseTableRows(id)` |

The ledger account link is built as `path.replace(":", "/account/")`.

**Open question / known gap**:
- Debug amounts are always formatted as USD.
- In Signals, `currentKey ? … : null` hides a key with ID 0.
- Debug introspection depends on `reactor.debug` existing; it returns an empty list otherwise.

### 1.7 Log viewer component (`components/log-viewer/*`), shared with gui-dev and the website
- Input: `ViewableLogLine = {index, relativeTime(ms), type: debug|info|warn|error, namespace, date, message, parameters: unknown[], caller?, node?}`.
- Card with a **Filter** input. The filter splits on whitespace into keywords; a keyword prefixed with `-` negates. A keyword matches if `namespace` or `message` contains it, and every keyword must agree.
- Virtualized with @tanstack/react-virtual (estimated row height 18 px). It sticks to the bottom unless the user scrolls up and re-sticks when scrolled back to the bottom. It returns null when there are 0 lines.
- Line layout:
  - Relative time in seconds with 3 decimals (gray mono).
  - Level, colored: debug `#7e84fa`, info `#147af3`, warn `#e8c600`, error `#de3d82`.
  - Optional node column (`renderNodeColumn` from context).
  - Namespace colored by `selectBySeed`. If `caller` is set it is clickable and calls `openFile(caller)`.
  - The message.
- Message formatting (`log-message.tsx`):
  - printf-style placeholders `%s %j %d %O %o %i %f %c` consume parameters, each rendered as a `DataValue`. `%%` escapes.
  - Literal text is ANSI-parsed with anser using a Dracula-style palette (`ansi-theme.ts`). Decorations: bold, italic, underline, blink (CSS keyframes), dim, hidden, strikethrough.
  - If exactly one trailing plain object with fewer than 10 own props remains, it renders as ` key=value` pairs. Its `error` property is pulled out and rendered as a `PrimaryError` block.
  - Otherwise the remaining parameters are appended.
- `DataValue` colors by type:
  - string bright-yellow, truncated to 30 characters with `…`
  - number and bigint bright-cyan (bigint gets an `n` suffix)
  - boolean, null and undefined bright-red
  - array `Array(n)` and object `Object` bright-magenta
  - error: the stack
  - Clicking a value does `console.log` of it.
- `PrimaryError`: a red box with `Name: message`. Stack frames are parsed with two regexes. Frames under `node:` and `node_modules` are dimmed; other frames are clickable and call `openFile("path:row:col")`. It recurses into `cause` and `AggregateError.errors`.

---

## 2. `packages/gui-dev`: the development dashboard UI

Files: `packages/gui-dev/src/*`. Title "Dassie//dev". Same React, UnoCSS and shadcn stack, mostly reusing gui-dassie components by deep import. It also loads `virtual:unocss-devtools`.

- `src/root.tsx`: `DarkModeProvider` → `RpcProvider` (WebSocket client to **`wss://dev-rpc.localhost`**, superjson with `TypeError` and `AggregateError` registered) → `QueryClientProvider` → App.
- Layout (`src/app.tsx`, `main-navigation.tsx`): left sidebar `w-64` and a content area.
  - Sidebar brand "Dassie" with a lime "//dev".
  - Nav links: **Dashboard** `/`, **Logs** `/logs`, **Scenarios** `/scenarios`. The active link uses the "secondary" button variant.
  - **Nodes** list (`components/nodes-list/nodes-list.tsx`):
    - Header "Nodes" with a **+** button that calls `addRandomNode({})`.
    - A "host" entry spanning 2 columns, then one entry per active node ID (`d1`, `d2`, …) from `subscribeToNodes`.
    - Each entry has a colored dot (seeded color) and links to `/nodes/<id>`. If you are currently on `/nodes/X/debug/<tab>`, the link goes to the same tab on the other node.
    - A wallet icon opens `getWalletUrl(id)` in a new tab.
- Routes:

| Route | Content |
|---|---|
| `/` Dashboard | `h-lg` NodeGraph and the full dev log viewer below it |
| `/logs` | The dev log viewer, all nodes |
| `/scenarios` | Cards for each `getBuiltinScenarios()` entry (name, description); the current one (`subscribeToScenario`) is highlighted. Click → `setScenario(id)` and navigate to `/` |
| `/nodes/host` | Header (seeded color dot, "Host"). Tabs: Logs (dev logs filtered to `node` not starting with `d`) |
| `/nodes/:nodeId/*` | Node detail (below) |

- NodeGraph (`components/node-graph/node-graph.tsx`):
  - Reuses gui-dassie `NetworkGraph`.
  - Nodes = keys of `subscribeToPeeringState` (`Record<nodeId, peerIds[]>`).
  - Links exist only for mutual peering, added in both directions.
  - `subscribeToPeerTraffic` events `{from,to}` are emitted onto a local topic, which makes particles animate along links.
  - Clicking a node → `/nodes/<id>`.
- Node detail (`components/pages/nodes/node-detail.tsx`):
  - Header: colored dot, "Node: <id>", and a **Wallet** button (`getWalletUrl`).
  - Fetches `getSecurityToken()` from the dev server, then creates a **separate RPC client and QueryClient** to `wss://<nodeId>.localhost/rpc?token=<securityToken>`.
  - Wraps the content in gui-dassie's `RpcProvider` so the gui-dassie debug pages talk to that node.
  - Tabs: **Logs** (dev logs filtered to `node === nodeId`), **Nodes**, **Ledger**, **Routing**, **State**, **Database**. The tab is reflected in the URL `/nodes/<id>/debug/<tab>`. `/` redirects to `/debug/logs`. Non-log tabs render gui-dassie `DebugPage`.
  - **Open question / known gap**: `createWebSocketLink(websocket)` is passed a raw `WebSocket` instance rather than options. It works only because `WebSocket.url` exists. The raw socket opened by `new WebSocket(...)` is leaked, and a second socket is opened.
- Wallet URL (`src/utils/wallet-url.ts`):
  - Format: `https://<id>.localhost/?_DASSIE_DEV_SESSION=<hex(HMAC(HMAC(seed,"dev"),"session"))>`.
  - `seed = TEST_NODE_VANITY_SEEDS[index]`.
  - The same token is pre-inserted into each node's `sessions` table (§6.5), so the wallet opens already logged in.
- Dev log provider (`components/development-log-viewer/*`):
  - `openFile` → `rpc.openFile.mutate(path)`, which opens the editor on the host via `launch-editor`.
  - The node column renders a `NodeLink`, a colored link to `~/nodes/<shortId>`.
  - The string formatter linkifies vanity node IDs (`/(d\d{1,4}_[\w-]{25,30})/`, only known vanity IDs) to short `dN` links, with 30-character truncation.
- Static assets in `gui-dev/public/`:
  - `rates.json`: a Coinbase-style `{data:{currency:"USD", rates:{"<CODE>":"<decimal string>", …}}}`. It is served at `https://localhost:10100/rates.json` and used as every dev node's exchange-rate URL.
  - favicons.
  - `moon_map_mercator.jpg`, **unused** (legacy map visualization; see Halton coordinates in §6.4).

---

## 3. `packages/app-website`: dassie.land

- Astro **5.1.1** with `@astrojs/starlight` 0.30.3, `@astrojs/react`, and `unocss/astro` (shared config). `astro.config.mjs`:
  - `site: "https://dassie.land"`, Starlight `title: "Dassie"`.
  - Social links: GitHub `https://github.com/justmoon/dassie` and Discord `https://discord.gg/HqdGNfs9kq`.
  - Sidebar: "Guides" → "Quick Start" (`guides/quick-start`).
  - expressiveCode themes `dracula` and `github-light`.
- Scripts:
  - `dev` / `start` = `astro dev` (root: `pnpm web`)
  - `build` = `astro check && astro build` (root: `pnpm build:web`)
  - `preview`
- Engines `>=20 <=24.11.0`. This is the only package not pinned to the exact Node version (enforced by `.meta-updater/main.mjs`).
- Pages and content:
  - `/`: `src/content/docs/index.mdx`, a splash template.
    - Title "What is Dassie?" with tagline "A peer-to-peer network which can transfer tiny amounts of money quickly and cheaply." and hero image `src/assets/dassie.webp`.
    - Actions: "Get Started" → `/guides/quick-start/` and "See the code" → GitHub.
    - Aside: "Dassie is not a blockchain…".
    - CardGrid: Scalable, Currency-neutral, Ledger-neutral, Under development.
  - `/guides/quick-start/`: `src/content/docs/guides/quick-start.md`.
    - Prerequisites: Linux with systemd (tested on Debian Bullseye), curl, a public IP, a domain name, and TCP ports 80 and 443 open.
    - Install: `curl --tlsv1.2 -sSf https://sh.dassie.land | sh`. Re-login is needed for `dassie-users` group membership.
    - Configure with `dassie init` (Let's Encrypt), then open the printed setup URL. Logs are in `journalctl -xeu dassie`.
    - Updating is automatic; manual update with `sudo dassie update`.
    - Uninstall: `… | sh -s -- uninstall`.
  - `/tools/stream-toy`: `src/pages/tools/stream-toy.astro`, a standalone dark page with the React island `<StreamToy client:load/>`. It imports gui-dassie `index.css` and the meta base.css.
- STREAM Toy (`src/components/stream-toy/*`) is an in-browser STREAM protocol simulator.
  - **Configurator** form defaults: `timeDilationFactor 1`, `maxPacketAmount 1000n`, `latency 100` ms, `jitter 0.2`, `maxPacketsInFlight 10`, `amount 3000n`. **Simulate** starts the run.
  - **Simulator** (`use-simulation.ts`):
    1. `createTestEnvironment` from `@dassie/lib-protocol-stream`, using a time-dilation clock that can be paused and resumed.
    2. Creates a STREAM server and client.
    3. Sends `amount` over one stream with `dangerouslyIgnoreExchangeRate()`.
    4. Captures every ILP Prepare and response, plus logs (shown in LogViewer).
  - The UI shows Back, a Pause/Play toggle, a packet log, and a log viewer.
  - Packet log: "Sender test.client" / "Receiver test.server". `peer.config` packets are filtered out. Each entry is collapsible and shows:
    - Direction arrow, status Prepared / Fulfilled / Rejected (code and human-readable text), and amount.
    - Expanded: ILP Prepare (Amount, Destination, Expiry ISO, Data bytes), ILP Fulfill/Reject (message, triggeredBy, data bytes), and the STREAM request/response decrypted with `pskEnvironment` (amount, sequence, frames: ConnectionClose, ConnectionNewAddress, ConnectionAssetDetails, StreamMoney). F08 rejects show the `receivedAmount` / `maximumAmount` details instead.
- **Install script hosting**: the website does **not** serve `install.sh`. `https://sh.dassie.land` and `https://get.dassie.land` are hosted outside this repo; there is no deployment config. **Open question / known gap**: where `sh.dassie.land` is deployed from, and how the website is deployed. No CI job builds or deploys either.

---

## 4. `packages/app-cli`: the npm `dassie` package

- `packages/app-cli/package.json`: name `dassie`, version `0.0.1`, `bin: index.js`, `files: ["index.js"]`. It is the only non-private package (changesets `access: public`).
- `index.js` is `#!/usr/bin/env node` followed by `console.log("🐻 Dassie is here!")`. That is all.
- **Open question / known gap**: this is a name-reservation placeholder. The real CLI is the `dassie` binary inside the release bundle, which is `app-dassie/src/command-line`. Its subcommands are `init`, `verify-install`, `daemon`, and `update`; the update and verify-install behavior is summarized in §5.6.

---

## 5. `packages/app-build`: release bundles, installer and updater

### 5.1 Entry points and commands
- Root `pnpm build` runs
  ```
  docker build --file=packages/app-build/resources/docker/builder/Dockerfile --build-arg=NODE_VERSION=$(tail -c +2 .node-version) --tag=dassie-builder:latest .
  ```
  followed by
  ```
  docker run --tty --rm --volume=$(pwd)/dist:/app/dist --volume=$(pwd)/.git:/app/.git:ro --network none dassie-builder
  ```
  Extra args are passed through, for example `pnpm build release`.
- Builder Dockerfile (`resources/docker/builder/Dockerfile`):
  1. `FROM node:${NODE_VERSION}`.
  2. Pre-downloads every URL in `resources/binaries/binary-dependencies.txt` into `/opt/binaries`, checks each with `sha256sum -c`, and sets `DASSIE_BUILD_BINARY_DEPENDENCIES_PATH=/opt/binaries`.
  3. Enables corepack, runs `pnpm fetch`, then `pnpm --recursive --filter=!app-website install --frozen-lockfile --offline --ignore-scripts`.
  4. `ENTRYPOINT ["pnpm","--filter=app-build","build"]`, which is `vite-node bin/build.ts --`.
  The build itself runs **offline** (`--network none`).
- CLI (`src/index.ts`, cmd-ts):
  - `build [targets…] [--architectures|-a x64,…]`.
  - Default target is `canary`.
  - Targets:
    - `release`: `version = detailedVersion = LATEST_DASSIE_VERSION = "0.0.1"` (`src/constants/version.ts`), `isMainRelease: true`.
    - `canary`: `version "canary"`, `detailedVersion "canary-YYYY-MM-DD-<git short sha>"` (`git -c safe.directory=* rev-parse --short HEAD`), `isMainRelease: false`.
  - Unknown target throws.
- Architectures: `SUPPORTED_ARCHITECTURES = ["x64"]`. arm64 and armv7l are commented out ("TODO: Re-enable ARM build once better-sqlite3 v10 binaries are available"). Compressions: `gz`, `xz`.

### 5.2 Build flow (`src/flows/build-bundle.ts`)
Progress is rendered with `@dassie/lib-terminal-graphics` (header, note, tasklist).
1. `createOutputPath` (mkdir `dist`, `dist/contents`), then `clearOutputPath` (delete everything in `dist/`).
2. **Backend** (`steps/build-backend.ts`):
   - esbuild bundles `packages/app-dassie/src/command-line/entry.ts` into `dist/staging/shared/backend.mjs`.
   - Settings: `platform node`, `format esm`, `target node20`.
   - Defines: `import.meta.env.MODE="production"`, `PROD=true`, `DEV=false`, `__DASSIE_VERSION__=<detailedVersion>`, `import.meta.vitest=undefined`.
   - A banner shims `__filename`, `__dirname`, and `require` for ESM.
3. **Frontend** (`steps/build-frontend.ts`): sets `NODE_ENV=production`, then runs `vite build` with root `packages/gui-dassie`, `outDir dist/staging/shared/frontend`, `define __DASSIE_VERSION__`, and `chunkSizeWarningLimit 1 MiB`.
4. Per architecture:
   1. `downloadNodeJs`: `https://nodejs.org/dist/v<NODE_VERSION>/node-v<NODE_VERSION>-linux-<arch>.tar.xz`, extracted to `dist/staging/<arch>/node`. `NODE_VERSION` = the **build host's** `process.versions.node`.
   2. `downloadBetterSqlite3`: `https://github.com/WiseLibs/better-sqlite3/releases/download/v<ver>/better-sqlite3-v<ver>-node-v<ABI>-linux-<x64|arm|arm64>.tar.gz`, extracted to `dist/staging/<arch>/better-sqlite3`. `<ver>` is taken from lib-sqlite's installed `better-sqlite3/package.json`; `ABI` = `process.versions.modules`.
   3. `copyFilesIntoBundle` creates the bundle layout (§5.3).
   4. `tarBundle` creates `dist/staging/dassie-<version>-linux-<arch>.tar`, with top-level dir `dassie/`.
   5. `compressBundle` runs `gzip -f … -c` and `xz -T 0 -f … -c`, writing `dist/upload/<version>/dassie-<version>-linux-<arch>.tar.{gz,xz}`.
5. Release only: `generateMetadata` writes `dist/upload/meta/latest` containing the plain version string (for example `0.0.1`). This is the entire **update channel and manifest format**. There is no per-channel manifest, no checksums file and no signature.
- Download integrity (`src/utils/download-file.ts`):
  - Every URL must have a pinned SHA-256 in `binary-dependencies.txt` (format `<url> <sha256>` per line); otherwise it throws "No trusted hash available".
  - Lookup order: first `$DASSIE_BUILD_BINARY_DEPENDENCIES_PATH/<basename>` if its hash matches, then the cache `packages/app-build/.cache/<sha256>`, then a fetch into the cache with the hash verified.
- `bin/generate-binary-dependencies.ts` regenerates the pin lines:
  - Imports the Node.js release signer keys (11 fingerprints, `src/constants/nodejs-signers.ts`) from `hkps://keys.openpgp.org` into a temporary keyring.
  - Fetches and GPG-verifies `SHASUMS256.txt.asc` for the current Node version.
  - Downloads the better-sqlite3 prebuilds and hashes them.
  - Prints `<url> <hash>` lines to stdout.
  - This is the only "signing" in the pipeline: it verifies upstream Node. **Dassie's own artifacts are not signed.**
- **Open question / known gap**: `binary-dependencies.txt` pins **node v22.8.0** and **better-sqlite3 v11.2.1 / ABI 127**. `.node-version` is **v24.11.0** (ABI 137) and lib-sqlite depends on `better-sqlite3 ^11.7.0`. Because the step URLs derive from the running Node, the build fails with "No trusted hash" until the pins are regenerated. The esbuild `target: node20` is also stale.

### 5.3 Release artifact layout
```
dist/
  staging/shared/backend.mjs             # esbuild output
  staging/shared/frontend/…              # vite build of gui-dassie (index.html, assets/*)
  staging/<arch>/node/…                  # extracted Node.js distro
  staging/<arch>/better-sqlite3/…        # extracted prebuild
  staging/dassie-<version>-linux-<arch>.tar
  contents/dassie-<version>-linux-<arch>/dassie/   # bundle root (becomes /opt/dassie/<version>)
      backend.mjs
      bin/node                           # bundled Node.js binary (exact version)
      bin/dassie                         # launcher (resources/launcher/dassie.sh, chmod 755)
      lib/better_sqlite3.node            # native binding (open-database.ts loads <root>/lib/better_sqlite3.node)
      share/public/…                     # frontend, served by ServeFrontendActor
      share/systemd/{dassie.service, dassie-http.socket, dassie-https.socket, dassie-ipc.socket, dassie-update.service, dassie-update.timer}
  upload/<version>/dassie-<version>-linux-<arch>.tar.gz
  upload/<version>/dassie-<version>-linux-<arch>.tar.xz
  upload/meta/latest                     # release builds only
```
- Launcher `bin/dassie` (POSIX sh):
  1. `BIN_FOLDER=$(dirname $(readlink -f $0))`
  2. `DASSIE_ROOT=$(dirname $BIN_FOLDER)` and export it.
  3. `exec $BIN_FOLDER/node $DASSIE_ROOT/backend.mjs "$@"`.
- Frontend serving in production (`app-dassie/src/http-server/serve-frontend.ts`):
  - `GET *` (registered only when `NODE_ENV !== "development"`).
  - Paths matching `/^\/((?:assets\/)?[\w-]+\.([a-z]{2,}))$/` with extension ico, png, css, js or webmanifest are served from `<root>/share/public`.
  - Everything else returns `index.html` (SPA fallback).
  - **Open question / known gap**: svg, webp, woff, json and other extensions fall back to index.html.
- Upload: `pnpm --filter app-build upload` runs `gcloud storage cp dist/upload/* gs://get.dassie.land --recursive`. The bucket is served as `https://get.dassie.land`. This is manual; **there is no CI release job**, although the README says "normally done by our CI". **Open question / known gap.**

### 5.4 systemd units (`resources/systemd/`), installed to `/etc/systemd/system/`
- `dassie.service`:
  - `Type=notify`, `NotifyAccess=all`, `ExecStart=/opt/dassie/current/bin/dassie daemon --runtime=nodejs-systemd`.
  - `WorkingDirectory=/opt/dassie/current`.
  - `DynamicUser=yes`, `StateDirectory=dassie` (→ `/var/lib/dassie`), `CacheDirectory=dassie`.
  - `Restart=always`, `RestartSec=1`, `TimeoutStartSec=infinity`, `NoNewPrivileges=true`.
  - Env: `HOME=/var/lib/dassie`, `PATH=/usr/bin:/usr/local/bin`, `NODE_ENV=production`, `DASSIE_LOG_LEVEL=info`.
  - `Requires` / `After` the three sockets; also `After=network-online.target` and `time-set.target`.
- Socket activation: `dassie-http.socket` `ListenStream=0.0.0.0:80`, `dassie-https.socket` `0.0.0.0:443`, `dassie-ipc.socket` `/run/dassie.sock` (`SocketMode=0660`, `SocketUser=root`, `SocketGroup=dassie-users`). All have `NoDelay=true` (http/https) and `Service=dassie.service`.
- `dassie-update.service`: oneshot, `User=root`, `ExecStart=/opt/dassie/current/bin/dassie update`.
- `dassie-update.timer`: `OnCalendar=*-*-* 03:20:00`, `RandomizedDelaySec=3600`, `Persistent=true`.

### 5.5 Install script (`resources/shell-installer/install.sh`), served at `https://sh.dassie.land`
- The whole body is wrapped in `main()` and called at the end (protects against truncated downloads). Usage text: `curl --tlsv1.2 -sSf https://sh.dassie.land | sh -s -- [options] [command]`.
- Options: `-h|--help`, `-t|--target <version>` (default `latest`), `-m|--mirror <url>` (default `https://get.dassie.land`). Env overrides: `DASSIE_MIRROR_URL`, `DASSIE_VERSION`.
- Commands: `install` (default) and `uninstall`.
- Install steps:
  1. Check the OS: only `linux` (error `E_UNSUPPORTED_OS`).
  2. Map the architecture: `x86_64|x86-64|x64|amd64 → x64`, `aarch64|arm64 → arm64`, `armv7l|armv8l → arm7l`.
  3. `verify_systemd`: `systemctl` present and `is-system-running` ∈ {running, degraded}.
  4. Require curl or wget, and tar.
  5. If the target is `latest`, GET `${mirror}/meta/latest`.
  6. Make a temp dir (`mktemp -d -t dassie-installer-XXXXXXXXXX`, removed by an EXIT trap).
  7. Download `${mirror}/${version}/dassie-${version}-linux-${arch}.tar.xz` and run `tar -xf`.
  8. As root (via `sudo` if needed): `mkdir -p /opt/dassie`, replace `/opt/dassie/<version>`, copy `dassie/` there, point the `/opt/dassie/current` symlink at it, and `ln -sf /opt/dassie/current/bin/dassie /usr/local/bin/dassie`.
  9. Create group `dassie-users` if missing. A non-root user is added to it (`usermod -a -G`), with a notice to log out and back in.
  10. Copy `dassie/share/systemd/*` to `/etc/systemd/system/`, then `systemctl enable dassie.service dassie-update.timer`, `daemon-reload`, and `restart dassie.service dassie-update.timer`.
  11. `dassie verify-install` (`E_SELF_TEST_FAILED` on failure).
  12. Print a success banner and "Next: … `dassie init`".
- Uninstall:
  1. `systemctl stop dassie.service` and the three sockets.
  2. `disable` those plus the timer.
  3. `rm` the unit files (**`dassie-update.service` is not removed**), then `daemon-reload`.
  4. `rm -rf /opt/dassie`.
  The `/usr/local/bin/dassie` symlink, the `dassie-users` group and `/var/lib/dassie` are left behind.
- Downloads: `curl --tlsv1.2 --proto "=https" --retry 3 --retry-delay 5 --fail --show-error` or `wget --https-only --max-redirect=0`.
- Output conventions:
  - `step` prints `▶ ` style headers. `run` echoes `$ cmd args`. `run_root` echoes `# cmd` or `# sudo cmd`.
  - Errors print `! Error (<CODE>)` and exit 1.
  - Color handling: `FORCE_COLOR` (true/yes/y/1/on/enabled or false/…) overrides; `NO_COLOR` disables; otherwise color is on only on a TTY with `TERM` other than `dumb`.
- Error codes: `E_SYNTAX`, `E_UNSUPPORTED_OS`, `E_UNSUPPORTED_ARCH`, `E_SYSTEMD_REQUIRED`, `E_SYSTEMD_NOT_RUNNING`, `E_CURL_OR_WGET_REQUIRED`, `E_TAR_REQUIRED`, `E_VERSION_FETCH_FAILED`, `E_TEMP_DIRECTORY_FAILED`, `E_BUNDLE_DOWNLOAD_FAILED`, `E_BUNDLE_EXTRACTION_FAILED`, `E_BUNDLE_INSTALL_FAILED`, `E_VERSION_SELECT_FAILED`, `E_DASSIE_COMMAND_SYMLINK_FAILED`, `E_GROUP_CREATION_FAILED`, `E_SYSTEMD_SERVICE_INSTALL_FAILED`, `E_SYSTEMD_SERVICE_ENABLE_FAILED`, `E_SYSTEMD_DAEMON_RELOAD_FAILED`, `E_SYSTEMD_SERVICE_START_FAILED`, `E_CLEANUP_FAILED`, `E_SELF_TEST_FAILED`, `E_SYSTEMD_SERVICE_STOP_FAILED`, `E_SYSTEMD_SERVICE_DISABLE_FAILED`, `E_SYSTEMD_SERVICE_REMOVE_FAILED`, `E_BUNDLE_REMOVE_FAILED`, `E_NO_SUDO`, `E_NO_DOWNLOAD_TOOL`, `E_UNEXPECTED_FORCE_COLOR_VALUE`.
- Test harness (`app-build/package.json` scripts):
  - `serve-mirror`: mkcert cert for `dassie-mirror` and `localhost`, then `http-server --port 8443 --tls` serving `dist/upload/`.
  - `test-in-docker:{centos,debian,fedora,ubuntu}`: privileged `jrei/systemd-*` images with `DASSIE_MIRROR_URL=https://localhost:8443` and `CURL_CA_BUNDLE` set to the mkcert root.
  - `shellcheck:installer` and `shellcheck:launcher` via the `koalaman/shellcheck` Docker image.
  - `resources/vagrant/Vagrantfile`: a `generic/fedora37` VM that trusts the mkcert root, sets `DASSIE_MIRROR_URL=https://dassie-mirror:8443`, and maps `dassie-mirror` to the host gateway.
- **Open question / known gap**:
  - The installer maps armv7 to `arm7l` while the build would name it `armv7l`; ARM is disabled anyway.
  - The installer fetches `.tar.xz` while the updater fetches `.tar.gz`, so both must be published.
  - There is no signature or checksum verification of bundles. `docs/implementation-notes/install-command.md` sketches a `SHASUMS256.txt` + `.sig` scheme with a temporary keyring and key fetched from two locations; it is not implemented.

### 5.6 Updater and verify-install (`packages/app-dassie/src/command-line/commands/update/index.ts`)
These are summarized here because they are the consumer side of the release layout; the full CLI reference is [2 §12](02-node-runtime.md).

`dassie update [-f|--force] [-t|--target-version <v>]`:
1. Requires `DASSIE_ROOT` (set by the launcher) to end with `/current` or `/<__DASSIE_VERSION__>`.
2. Checks write access to the root, its parent, the `current` symlink and the old version dir. On `EACCES` it says "run as root".
3. Mirror = `$DASSIE_MIRROR_URL` or `https://get.dassie.land`. The target is `GET <mirror>/meta/latest` unless given explicitly.
4. If the target equals the running version and `--force` was not given, it reports "Already up to date".
5. Downloads `<mirror>/<v>/dassie-<v>-linux-<process.arch>.tar.gz` into `<temp>/dassie-update-<ts>-<pid>`, then `tar -xzf`.
6. Copies `dassie/` to `<parent>/<v>` and removes the old version dir if it differs.
7. Re-points `current` (`rm` + `ln -s`), then `systemctl restart dassie`.

`dassie verify-install` opens an in-memory SQLite database with the bundled native binding and runs `SELECT 1 + 1`.

**Open question / known gap**: `process.arch` on armv7 is `arm`, which does not match the bundle names.

---

## 6. `packages/app-dev`: development environment (`pnpm start` / `pnpm dev`)

### 6.1 Launch sequence
1. Root `start` script:
   ```
   which mkcert >/dev/null && : "${NODE_EXTRA_CA_CERTS:=$(mkcert -CAROOT)/rootCA.pem}"; export NODE_EXTRA_CA_CERTS; node packages/app-dev/bin/start-development-server.js
   ```
2. `bin/start-development-server.js`:
   - Creates a Vite server with `configFile packages/app-dev/vite.backend.config.js`, `hmr:false`, `define __DASSIE_VERSION__="dev"`, and a watcher that ignores dist, .next, .turbo, .cache and .vagrant.
   - Imports `packages/app-dev/src/start.ts` through the SSR `environment.runner`.
   - Calls `prepareReactor({viteServer, viteDevEnvironment, restart, isFirst})` and runs `RootActor`.
   - Prints "Dassie//dev  Starting development server...".
   - On any watched file change inside the module graph of `start.ts` it does a full restart: dispose the reactor and start again ("Restarting development server...").
3. `src/start.ts` `RootActor`, in order:
   1. `RegisterReactiveLoggerActor`
   2. `ApplyDebugLoggingScopes`
   3. `HandleShutdownSignalsActor`
   4. `verifyPrerequisites()`; on failure set exit code 1 and dispose
   5. `PersistScenarioActor`
   6. `await ServeUiRpcActor`
   7. `ServeRunnerRpcActor`
   8. `await DebugUiServerActor`
   9. `ProxyByHostnameActor`
   10. `ListenForKeyboardShortcutsActor`
   11. `HandleFileChangeActor`
   12. `await RunScenarioActor`

### 6.2 Prerequisite checks (`src/functions/verify-prerequisites.ts`)
- Not `win32`. The message suggests WSL, a container or a VM.
- `process.version` must exactly equal `.node-version` (`v24.11.0`). The message recommends a version manager such as fnm.
- `openssl version` and `mkcert --version` must succeed.
- `NODE_EXTRA_CA_CERTS === $(mkcert -CAROOT)/rootCA.pem`.
- `d999.localhost` resolves (IPv4) to `127.0.0.1`.

### 6.3 Ports, hostnames, TLS
Constants are in `src/constants/ports.ts` and `src/constants/hosts.ts`.

| What | Address |
|---|---|
| SNI proxy (entry point, "Debug UI: https://localhost/ <- Start here") | TCP `:443` on all interfaces. On `EACCES` it hints `sudo sysctl -w net.ipv4.ip_unprivileged_port_start=0` |
| Dev dashboard (Vite, root `packages/gui-dev`) | `https://127.0.0.1:10100` (DEBUG_UI_PORT) |
| UI RPC WebSocket (HTTPS + `ws`) | `:10101` (DEBUG_UI_RPC_PORT), hostname `dev-rpc.localhost` |
| Runner RPC (plain TCP, lib-rpc Node socket adapter, no TLS) | `:10102` (DEBUG_RUNNER_RPC_PORT) |
| Node *i* (0-based) HTTPS | `5001 + i`, hostname `d{i+1}.localhost` (URL `https://dN.localhost:500N`) |
| Node *i* Node inspector port | `6001 + i` (`--inspect-port`) |
| Node *i* wallet Vite HMR port | `httpsPort + 4400` |

- SNI proxy (`src/actors/proxy-by-hostname.ts`):
  1. Reads the first TCP packet and parses the TLS ClientHello by hand (record type 22, handshake type 1, skip session id, cipher suites and compression, walk extensions to `server_name` type 0, host_name type 0).
  2. Routes: `dev-rpc.localhost` → 10101; `^d(\d+).localhost$` → `5000 + N`; anything else (including no SNI) → 10100.
  3. Replays the first packet to the upstream and pipes both ways.
  4. `ECONNREFUSED` upstream silently drops the connection; the browser sees `ERR_CONNECTION_CLOSED`.
- Certificates (`src/utils/validate-certificates.ts`) live under `local/tls/` (repo-root `local/` folder, `src/constants/paths.ts`):
  - `localhost/web-localhost{.pem,-key.pem}`
  - `dev-rpc.localhost/web-dev-rpc.localhost{.pem,-key.pem}`
  - `dN.localhost/web-dN.localhost{.pem,-key.pem}`
  - Generation: `openssl genpkey -algorithm RSA -out key`, then `openssl req -new -key key -out cert.csr -days 365 -subj /CN=<name>`, then `mkcert -csr cert.csr -cert-file cert`.
  - Existing files are reused. If the key is missing but the cert exists, the cert is deleted and regenerated.
- Nodes use these certs as their TLS web cert (pre-filled into the DB), so browsers trust them through the mkcert root.

### 6.4 Node identities and the vanity table
- `src/constants/vanity-nodes.ts` (~345 KB, generated) contains 4 exports keyed by index 0…~1000:
  - `TEST_NODE_VANITY_ENTROPY`: 16-byte hex, which becomes a 12-word mnemonic.
  - `TEST_NODE_VANITY_MNEMONICS`.
  - `TEST_NODE_VANITY_SEEDS`: 64-byte BIP-39 seed hex.
  - `TEST_NODE_VANITY_IDS`.
- Node IDs are brute-forced so they start with `d{index+1}_`, for example `d1_UeCE9UMCwvVHSXdI9o0Tpz4rnidWvR`.
  - The UI shortens them to `dN` (`src/utils/shorten-node-id.ts`, regex `d\d{1,4}_[\w-]{25,30}`).
  - The runner converts peer IDs to friendly IDs by cutting at `_`.
- `bin/add-testnet-vanity-addresses.ts`:
  - Reads stdin lines `[0x]<entropyHex>: <nodeId>` from an external brute-forcer (lines starting with `#` are skipped).
  - Validates the pattern `^d([1-9]\d*)_` and re-derives the node ID (`HMAC(seed,"node")` → Ed25519 pubkey → `calculateNodeId`) with an assert.
  - Keeps an entry if it is new or has fewer `-`/`_` characters than the existing one.
  - Drops indexes above 999 except the largest ever (the "record").
  - Rewrites `vanity-nodes.ts`.
- `src/utils/generate-node-config.ts` derives per index:
  - `id = d{i+1}`, `fullId` (vanity node ID), `hostname = dN.localhost`, `httpsPort`, `debugPort`
  - `dataPath = local/data/dN.localhost`, `ipcSocketPath = <dataPath>/dassie.sock`
  - `dassieNodeKey = HMAC(seed,"node")`, `sessionToken = hex(HMAC(HMAC(seed,"dev"),"session"))`
  - TLS file paths
  - `bootstrapNodes` (from env indexes, each `{id, url, publicKey: base64url}`)
  - `registeredNodes`
  - `settlementMethods` (default `["stub"]`)
  - `peers` (default `[]`)
  - `url = https://dN.localhost:500N`
  - `entry = src/runner/launchers/node.ts`
  - `latitude`/`longitude` from a Halton sequence (bases 2 and 3)
- **Open question / known gap**: lat/long and `moon_map_mercator.jpg` are unused leftovers of a former map visualization. `PeerSettings` (pre-configured peers) is supported by prefill but no scenario uses it.

### 6.5 Per-node data directory pre-fill (`src/utils/prefill-database.ts`)
This runs before every node start (`src/functions/start-node.ts`). It deletes and recreates `local/data/dN.localhost/dassie.sqlite3` with `DASSIE_DATABASE_SCHEMA`.
- Scalars:
  - `configRealm="test"`, `configHostname=dN.localhost`, `configHttpsPort=500N`
  - `configEnableHttpServer=false`, `configAlias=dN`
  - `configDassieKey` = the PEM/DER-serialized Ed25519 private key
  - `configTlsWebCert` / `configTlsWebKey` from the files
  - `configExchangeRateUrl=https://localhost:10100/rates.json`
- Tables:
  - `settlementSchemes`: one row per method with `config "{}"`.
  - If this node is a bootstrap node: `nodes` + `registrations` rows (with `registered_at`/`renewed_at` = now) for every registered node.
  - `peers` rows for configured peers.
  - `sessions`: the dev session token.
  - `btpTokens`: the fixed token `beNrVTiQLKReCr9d-YBPjA`.
- The result is that every dev node starts already set up, logged-in-able, with a ledger and a BTP token.

### 6.6 Node child processes (runner)
- `src/functions/run-child-process.ts` forks `src/runner/runner.js` (resolved through Vite) with:
  - `silent:true` and an IPC channel.
  - `execArgv ["--enable-source-maps", "--stack-trace-limit=64", "--trace-deprecation", "--inspect-port=600N"]`.
  - Environment (from `start-node.ts`):
    - `FORCE_COLOR=1`, `DEBUG=<scopes>`, `DEBUG_HIDE_DATE=1`
    - `DASSIE_BOOTSTRAP_NODES=<json>`, `DASSIE_STATE_DIRECTORY=<dataPath>`, `DASSIE_IPC_SOCKET_PATH=<dataPath>/dassie.sock`, `DASSIE_LOG_LEVEL=none`
    - `DASSIE_DEV_ROOT`, `DASSIE_DEV_BASE` (Vite root/base), `DASSIE_DEV_ENTRY=<launcher>`
    - `DASSIE_DEV_RPC_URL=wss://dev-rpc.localhost:10101`, `DASSIE_DEV_NODE_ID=dN`
    - `DASSIE_DEV_SECURITY_TOKEN=<64 hex>`
- Parent↔child IPC protocol:
  - Child sends `{id, method, params}`; parent replies `{id, result}` or `{id, error}`.
  - Methods: `viteInvoke` (forwards Vite module-runner transport calls to `viteDevEnvironment.hot.handleInvoke`) and `ready`.
  - Readiness timeout is 60 s. On shutdown: disconnect IPC, wait 1 s, then `SIGKILL`.
- Child stdout and stderr lines become log lines (`info`/`warn`, namespace `stdout`/`stderr`). Inspector banner lines are suppressed.
- `runner.js` (intentionally minimal): a Vite `ModuleRunner` (`ESModulesEvaluator`, `hmr:false`) imports `/@vite/env`, then `DASSIE_DEV_ENTRY`, then calls `ready`.
- `runner/launchers/node.ts`, `DebugRunnerActor`:
  1. `AttachLogger`.
  2. Connect the RPC client to `:10102`.
  3. `ForwardLogsActor`: every `LogsStore.addLogLine` triggers `notifyLogLine({...line, node: DASSIE_DEV_NODE_ID})`.
  4. `HandleShutdownSignalsActor`, `HandleDisconnectActor` (IPC disconnect disposes the reactor).
  5. `PatchIlpLoggerActor`: hooks the `debug` module used by `ilp-protocol-stream`/`ilp-logger` into LogsStore.
  6. `ForwardPeerTrafficActor`: every `OutgoingPeerMessageTopic` event triggers `notifyPeerTraffic({from: dN, to: friendly(dest)})`.
  7. `ReportPeeringStateActor`: on each `PeersSignal` change it calls `notifyPeerState({nodeId, peers})`.
  8. Run the real `DaemonActor` with `createRuntime()` (lib-reactive-io/node).
  9. `ServeHttpsActor`.
  10. `RegisterDevelopmentSetSessionRouteActor`.
- `ServeHttpsActor` (`runner/actors/serve-https.ts`):
  - One `node:https` server on `httpsPort`, using the DB TLS cert or a self-signed fallback.
  - Requests matching the node's `HttpsRouter` (API, `/rpc` upgrades through `HttpsWebSocketRouter`) go to Dassie.
  - Everything else goes to an Express app with Vite middleware rooted at `packages/gui-dassie` (`appType custom`, HMR on `httpsPort+4400`), a `connect-history-api-fallback` SPA rewrite, and a custom `index.html` transformer. This is how the wallet is served in dev with HMR.
- `POST /api/_dev/set-session {sessionToken}` sets the `__Host-Dassie-Session` cookie (10 years). It is dev-only and registered by the runner.

### 6.7 Dev-server state, RPC routers and behaviors
- Signals and stores:
  - `ScenarioSignal`: default `"two-nodes"`. Persisted to `local/dev/scenario.txt` and restored at boot if valid.
  - `DebugScopesSignal` = `$DEBUG` or `"das:*,ilp*"`.
  - `SecurityTokenSignal` = 32 random bytes hex, new per dev-server start.
  - `ActiveNodesStore` (Set of NodeConfig) and `ActiveNodeIdsComputed` (`dN` list).
  - `AdditionalNodesStore` (Set of indexes).
  - `EnvironmentStore`: `{additionalNodeStartIndex:2, defaultNodeSettings:{}, bootstrapNodes:[0,1], registeredNodes:[0,1]}`, with actions set/reset/apply.
  - `LogsStore`: soft limit 10,000, hard limit 10,100. Lines are `{node, index, relativeTime, type, namespace, date, message, parameters, caller}`.
  - `PeeringStateStore` (`Record<dN, dN[]>`) and `PeerTrafficTopic` (`{from,to}`).
- UI RPC router (`src/rpc-routers/ui-rpc-router.ts`, no auth):

| Procedure | Kind | Behavior |
|---|---|---|
| `getSecurityToken` | query | Returns the 64-hex token used to auth to nodes' `/rpc?token=` |
| `addRandomNode({})` | mutation | Adds index `additionalNodeStartIndex + additionalNodes.size` |
| `getBuiltinScenarios` | query | `[{id,name,description}]` |
| `subscribeToNodes` | sub (signal) | Active `dN` IDs |
| `openFile(path)` | mutation | `launch-editor` on the host |
| `subscribeToLogs` | sub (store) | Dev LogsStore |
| `subscribeToPeerTraffic` | sub (topic) | `{from,to}` |
| `subscribeToPeeringState` | sub (signal) | Peering map |
| `subscribeToScenario` | sub (signal) | Scenario ID |
| `setScenario(id)` | mutation | Validates the ID, writes the signal, clears additional nodes. That triggers `RunScenarioActor` to restart all nodes |

- Runner RPC router (`src/rpc-routers/runner-rpc-router.ts`, called by node children):
  - `notifyPeerTraffic {from,to}`
  - `notifyLogLine {type,namespace,node,date,message,parameters,caller}` (the node ID is shortened)
  - `notifyPeerState {nodeId, peers[]}`
- Transformer: superjson with `allowErrorProps("stack","cause")` and registered `TypeError` / `AggregateError` (`src/utils/transformer.ts`).
- `RunScenarioActor`:
  1. Tracks `ScenarioSignal`.
  2. Clears peering state, resets the environment, and awaits the scenario's `StartScenario`.
  3. Runs `RunAdditionalNodesActor`, which starts each extra index sequentially with 500 ms between nodes.
- `HandleFileChangeActor`:
  - A change within the `start.ts` module graph is handled by the bin script (full restart).
  - A change within the node launcher's graph clears peering state, logs `change <file>`, and calls `runScenarioActor.forceRestart()`, so all nodes restart.
- Keyboard shortcuts (`listen-for-keyboard-shortcuts.ts`; the hint is shown on first start):
  - **space** opens `https://localhost`; **ctrl+space** opens it in a private browser.
  - **r** restarts the dev server.
  - **ctrl+c** quits.
- Shutdown:
  - SIGINT/SIGTERM dispose the reactor. A second ctrl+c force-quits.
  - `DASSIE_DEV_OPEN_HANDLES` set: dump open handles with `wtfnode` after 500 ms.
  - `process.report.reportOnSignal = true`.
- Logging: the host's console is captured into LogsStore as node `"host"`. Lines are echoed to the terminal only if `DASSIE_LOG_TO_CLI=true` or the level is warn/error.
- **Open question / known gap**: `--inspect-port` is passed without `--inspect`, so the inspector is not enabled by default (VS Code auto-attach from `.vscode/launch.json` may cover this).

### 6.8 Scenarios (`src/scenarios/*.ts`)

| ID | Name | Description | Nodes |
|---|---|---|---|
| `two-nodes` (default) | Two Simple Nodes | "Network with two nodes using default configuration and stub settlement" | d1, d2 (stub). Bootstrap and registered = d1, d2. Extra nodes start at index 2 |
| `six-nodes` | Six Simple Nodes | "…six nodes using default configuration and stub settlement" | d1…d6 (stub). Registered = all 6. Bootstrap d1, d2. Extras from index 6 |
| `xrpl-settlement` | XRPL Testnet Settlement | "Network with two nodes using XRPL testnet settlement" | d1, d2 with `xrpl-testnet`. Default node settings are xrpl-testnet, so extras also use XRPL |
| `cross-currency` | Cross-currency Settlement | "Two nodes using different settlement methods (stub and XRPL testnet) and a third node supporting both" | d1 `[stub]`, d2 `[stub, xrpl-testnet]`, d3 `[xrpl-testnet]`. Registered d1–d3. Extras from 3 |

"Simulated ledgers" are the `stub` settlement scheme (in app-dassie). It simulates settlement delays without an external ledger. XRPL scenarios hit the real XRPL testnet.

### 6.9 Dev CLI wrapper
- `pnpm cli dN <args…>` runs `vite-node --config packages/app-dev/vite.backend.config.js packages/app-dev/bin/run-dassie-cli.ts -- dN …`.
- `dN` is validated against `^d[1-9]\d*$`.
- It sets `DASSIE_IPC_SOCKET_PATH=local/data/dN.localhost/dassie.sock` and `__DASSIE_VERSION__="dev"`, then imports `app-dassie/src/command-line/entry`.
- Example: `pnpm cli d1 init`.

---

## 7. Root files: what matters for a reimplementation

- `package.json` (root `@dassie/root`, ESM, private):
  - `packageManager pnpm@9.14.4`, `engines.node "=24.11.0"`. The exact pin is deliberate because releases bundle Node.
  - Scripts:
    - `start`/`dev` (dev env, §6)
    - `build` (Docker release build, §5)
    - `build:web`, `web` (website)
    - `check` (`incremental-check`)
    - `test` (`vitest`)
    - `prettier` (writes `packages/*/{src,test}/**/*.ts`)
    - `lint` (eslint per package, except some meta packages)
    - `typecheck` (`tsc --build`, project references)
    - `metacheck` (`meta-updater --test`)
    - `api` (`extract-apis`)
    - `cli` (§6.9)
    - `unused` (`knip`)
- `.node-version`: `v24.11.0`. It drives CI `setup-node`, the dev prerequisite check, the Docker builder's `NODE_VERSION`, and `engines` (via meta-updater).
- `pnpm-workspace.yaml`: `packages/*`.
- `.meta-updater/main.mjs` normalizes every `package.json`:
  - Requires `name`, `version` and `description`.
  - Non-private packages must list `files`.
  - Forces author, bugs, license (Apache-2.0) and repository (with directory).
  - Sets `engines.node = "=<.node-version>"` (website: `>=20 <=<ver>`).
  - Syncs the root `tsconfig.json` references (packages that have a tsconfig).
- `.changeset/config.json`: changesets with `access public`, `baseBranch main`, and private packages not versioned.
- `vitest.config.ts` (root): `test.includeSource: ["**/*.{js,ts}"]`, which enables in-source `import.meta.vitest` tests. Only one file uses them: `lib-terminal-graphics/src/helpers/string-offsets.ts`.
- **Test strategy**:
  - Unit tests only, all in libraries:
    - lib-oer: 18 files
    - lib-reactive: 10
    - lib-sqlite: 6
    - lib-protocol-stream: 5
    - lib-x509: 3
    - lib-terminal-graphics: 3
    - meta-eslint-plugin: 2
    - lib-http-server: 2
    - lib-rpc: 1
    - lib-protocol-ilp: 1
    - lib-protocol-ildcp: 1
  - **There are no tests** for app-dassie, gui-dassie, gui-dev, app-dev or app-build.
  - Integration testing is manual through the dev environment's scenarios, plus the installer Docker and Vagrant harnesses.
  - The esbuild define `import.meta.vitest = undefined` strips in-source tests from release bundles.
- CI (GitHub Actions `ci.yml`, on push and PR to `main`): checkout → pnpm → setup-node from `.node-version` → `pnpm install` → `pnpm typecheck` → `pnpm test` → `pnpm lint` → `pnpm metacheck`. No build, release, website deploy or knip step. Knip is run manually during maintenance and is deliberately not a CI gate.
- `knip.json`: declares extra entry points.
  - `bin/**`, `src/examples/**`
  - app-dassie `src/command-line/entry.ts`
  - app-dev `uno.config.ts`, `bin/*.ts`, `src/runner/runner.js`, `src/runner/launchers/*.ts`
  - gui-* `uno.config.ts` and the vite entry `src/index.tsx`
  - It ignores the `virtual:uno.css` and `virtual:unocss-devtools` deps, and `sharp` for the website.
- `.gitpod.yml`: init `pnpm install && pnpm build`, command `pnpm start`, port 3000 `open-preview`. **Open question / known gap**: stale. The dev UI is on 443/10100, not 3000, and `pnpm build` needs Docker.
- `.vscode/`:
  - Recommended extensions: UnoCSS, MDX, Astro, ESLint, Prettier.
  - `settings.json`: workspace TypeScript, project-relative imports, no semicolons, cSpell words.
  - `launch.json`: "Debug Dev Server" (`pnpm start` with `autoAttachChildProcesses`) and "Debug Current Test File" (`vitest run ${relativeFile}`).
  - `tasks.json`: "Launch dev server".
- `README.md`: dev prerequisites (version manager respecting `.node-version` such as fnm, `*.localhost` → 127.0.0.1 (dnsmasq on macOS), `mkcert -install`, `NODE_EXTRA_CA_CERTS`, Docker for builds), `corepack enable && pnpm install`, `pnpm start`, `pnpm build [release]`. Intro video link.
- `docs/`:
  - `contributing/folder-structure.md` describes the package prefixes app-, gui-, lib- and meta-, and the per-feature folder conventions: `actors/ computed/ constants/ database-stores/ database-tables/ failures/ functions/ signals/ stores/ topics/ rpc-routers/ types/ utils/`.
  - `implementation-notes/*`:
    - Relevant here: `frontend-security.md` (future signed or independent frontend, service-worker verification, bookmarklet, CSP; all unimplemented), `configuration-wizard.md` (future Cloudflare/origin certs and other ACME CAs), `install-command.md` (curl|sh rationale, signature plan).
    - Others (internal-ledger, network-membership, node-discovery, peer-state, peer-to-peer-communication, token-payment-protocol) are covered in chapters [3](03-peer-network.md) and [5](05-accounting-and-settlement.md).
- `.prettierrc.json` (with `@trivago/prettier-plugin-sort-imports` and the astro plugin), `eslint.config.mjs`, `.editorconfig`, `tsconfig.json` (project references): style tooling only.

---

## 8. `meta-*` packages (one line each)
- `meta-api-extractor`: Microsoft API Extractor base config plus the `extract-apis` script (root `pnpm api`) that documents and checks the public API surface of the lib packages.
- `meta-eslint-config`: the shared flat ESLint config for all packages.
- `meta-eslint-plugin`: custom ESLint rules `@dassie/no-floating-failures`, `no-misused-failures` (for the `Failure` result type), `no-top-level-mutables`, `no-top-level-side-effects`. Has 2 test files.
- `meta-incremental-check`: the `incremental-check` CLI (root `pnpm check`). In a worker thread it runs the TypeScript build to find changed or stale packages, ESLints the changed files, runs related vitest tests, and runs meta-updater, with lib-terminal-graphics progress output.
- `meta-rollup-config`: a shared Rollup config used by the publishable lib packages (lib-oer, lib-rpc, lib-logger, lib-protocol-*, lib-reactive-io, lib-reactive-rpc, lib-type-utils).
- `meta-tsconfig`: tsconfig templates `base.json` (strictest), `vite-node.json`, `vite-react.json` and `vite-isomorphic.json`.
- `meta-unocss-config`: the shared UnoCSS config (presetUno, presetIcons, presetShadcn, directives transformer, shadcn color tokens as `hsl(var(--x))`, radius scale, `flex-center` / `flex-col-center` shortcuts) and `base.css` (reset, light and dark CSS variables, base font stacks).

---

## 9. Consolidated open questions and known gaps (surfaces, packaging, tooling)
- **UI-1** Send redirects to `/payment/:id`, but the route is `/payments/:paymentId`. The payment status page is a stub with no status data, and there is no error UI for failed payments.
- **UI-2** Balances and amounts are hard-coded to USD (9-digit precision) on Account, Send and the debug Ledger. The multi-currency display is unfinished.
- **UI-3** Only the Stub ledger can be added from the UI. XRPL and XRPL-testnet have no add UI and no deposit UI (Receive tab is empty), and the per-ledger Send/Receive buttons are inert.
- **UI-4** Network `NodeDetail` shows only distance. `RegistrationAdvancedSettings` and `components/ui/dropdown.tsx` are unused, and the Register page has no feedback.
- **UI-5** The mnemonic is not validated on login or recover, login failures show no error, and the setup "opening" step has no error state.
- **UI-6** Dark-mode setting has no toggle UI.
- **UI-7** Basic state is polled every second through `NetworkStatus`. The forced create-ledger screen does not invalidate basic state after success.
- **UI-8** Favicon and manifest paths are probably broken (`src/public` vs Vite `public`, empty manifest names). The production static server only serves ico, png, css, js and webmanifest.
- **UI-9** The wallet frontend is served by the node itself. The signed-frontend and CSP ideas are only documented (`docs/implementation-notes/frontend-security.md`).
- **UI-10** The build is pinned to stale binary hashes (node 22.8.0, better-sqlite3 11.2.1 / ABI 127) against Node 24.11.0, and the esbuild target is node20. ARM builds are disabled, and the installer and updater spell the ARM arch differently (`arm7l`, `process.arch`).
- **UI-11** There is no artifact signing or checksum manifest. The update channel is just `meta/latest` (plaintext version). Canary builds publish to `upload/canary/…` but never update `meta/latest`.
- **UI-12** Upload is a manual `gcloud storage cp`, with no CI release job. `sh.dassie.land` and the website hosting are not defined in the repo.
- **UI-13** The installer's uninstall leaves `dassie-update.service`, `/usr/local/bin/dassie`, the `dassie-users` group and `/var/lib/dassie` behind. The installer downloads `.tar.xz` while the updater downloads `.tar.gz`.
- **UI-14** The `app-cli` npm package is a placeholder.
- **UI-15** `.gitpod.yml` is stale (port 3000).
- **UI-16** Dev-env leftovers:
  - Unused Halton geo-coordinates and the `moon_map_mercator.jpg` asset.
  - `PeerSettings` is unused by scenarios.
  - `--inspect-port` is passed without `--inspect`.
  - The gui-dev node-detail leaks a raw WebSocket.
- **UI-17** There are no automated tests for any app or GUI package; all tests are library unit tests.
