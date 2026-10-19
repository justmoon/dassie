# Website

`apps/website` builds `dassie.land`: a few static pages, the architecture page, and the STREAM toy, which sends a STREAM payment over a simulated network in the browser with Dassie's own STREAM code compiled to WebAssembly. Vercel builds and serves it.

**Paths:** `apps/website/`
**Read with:** [Stream](stream.md), [Wallet](wallet.md) (shared components), [Release](release.md) (what the Quick Start installs), [Testing](testing.md)

## Pages

| Path | Page | Script |
|---|---|---|
| `/` | Home: "What is Dassie?" | none |
| `/guides/quick-start/` | Quick Start | none |
| `/tools/stream-toy/` | [STREAM toy](#stream-toy) | the toy, a Solid 2 island |
| `/architecture/` | [Architecture](#architecture-page) | the map, a Solid 2 island |

- Each page is a plain HTML file (`index.html`, `guides/quick-start/index.html`, `tools/stream-toy/index.html`, `architecture/index.html`) and a Vite build input. All share `src/site.css`, which imports `@dassie/ui/ui.css`, and the icon `/favicon.svg`.
- **Header and footer.** A build step (`vite.config.ts`, from `src/layout.ts`) replaces `<!-- header -->` and `<!-- footer -->` in each page; a page without the header placeholder fails the build. The header holds the brand (icon and "Dassie", linking to `/`) and links to Quick Start, STREAM Toy, Architecture, GitHub (`https://github.com/justmoon/dassie`) and Discord (`https://discord.gg/HqdGNfs9kq`). The current page's link has `aria-current="page"`, exactly once per page. The footer: "Dassie is open source. See the code or talk to us on Discord."
- **Home:** the title "What is Dassie?", the tagline "A peer-to-peer network which can transfer tiny amounts of money quickly and cheaply.", buttons **Get Started** (Quick Start), **Try STREAM** (the toy) and **See the code** (GitHub), and the dassie picture (`src/assets/dassie.webp`). A note: "Dassie is not a blockchain. Payments are routed like packets through the network and are not stored. The only state the nodes share is the link state they route by." Four cards: Scalable, Currency-neutral, Ledger-neutral (nodes settle on existing ledgers, today the XRP Ledger testnet; based on Interledger), Under development (not ready for real money).
- **Quick Start** walks an owner through a node's life, in this order: prerequisites (Linux with systemd on x86-64 or arm64, a public IP address with a domain name pointing to it, ports 80 and 443 open); installing with `curl --proto =https --tlsv1.2 -sSf https://sh.dassie.land | sh` and checking with `dassie verify-install`; `sudo dassie init` and the setup URL (`sudo dassie setup-url` prints it again); `dassie status` and `journalctl -u dassie`; updates, daily or at once with `sudo dassie update`; and uninstalling with `sh -s -- uninstall`. The page must match what the tools do: the installer and updater are specified in [Release](release.md#installsh) ([`dassie update`](release.md#dassie-update)), and the commands in [Node](node.md#the-dassie-command).
- The website does not serve `install.sh`; `sh.dassie.land` and `get.dassie.land` are the release mirror's ([Release](release.md)).

## Build

- `node --run build:website` (in the site: `vite build --configLoader native`) writes `apps/website/dist/`. It needs the WASM modules built first (`node --run build:wasm`). `dev` watches and `preview` serves the build.
- Every asset is a file of its own (`assetsInlineLimit: 0`), since a `data:` URL for the WASM module would be refused by `connect-src 'self'`. Target `esnext`, no module-preload polyfill.
- The workspace is resolved through `@dassie/ui/vite`, with one copy of Solid ([Wallet](wallet.md#shared-components-packagesui)).

## Content Security Policy and headers

`vercel.json` sets on every path:

```
Content-Security-Policy: default-src 'self'; script-src 'self' 'wasm-unsafe-eval'; style-src 'self';
  img-src 'self'; connect-src 'self'; object-src 'none'; base-uri 'none'; form-action 'self';
  frame-ancestors 'none'
X-Content-Type-Options: nosniff
Referrer-Policy: strict-origin-when-cross-origin
```

and `Cache-Control: public, max-age=31536000, immutable` on `/assets/*`. URLs end in a slash (`trailingSlash: true`).

So the pages keep to the same rules as the wallet: no inline script, no inline style or `style` attribute (in the HTML or in Solid's compiled templates), no script, style or image from another origin (links to elsewhere are fine), and WebAssembly may compile. Colours on the architecture map are classes, and the toy's progress bar is sized through the DOM.

## STREAM toy

The sender and the receiver are `dassie-stream`'s real state machines, from its WASM build (`@dassie/identity-wasm/web/stream`, the same `web` build Node loads in tests). Between them sits a connector, simulated one event at a time on a clock of its own ([Stream](stream.md) for the protocol).

### Settings

| Field | Default | Range | Meaning |
|---|---|---|---|
| Largest packet | 1000 | 1 … 2^64 − 1 | The connector refuses larger packets with F08 |
| Latency (ms) | 100 | 0 … 60,000 | The median round trip from the connector to the receiver |
| Jitter | 0.2 | 0 … 2 | The σ of the round trip's log-normal distribution |
| Packets in flight | 10 | 1 … 1000, whole | The connector refuses packets beyond this many with T03 |
| Amount | 3000 | 1 … 2^64 − 1 | What the sender sends, in the asset's smallest unit |
| Time dilation | 1 | 0.01 … 100 | How fast the run plays: 1 is real time, 0.1 ten times slower |
| Seed | 1 | 0 … 2^32 − 1 | Seeds the round trips |

The defaults are the original's, with a seed. A field out of range or not a number gets a message and the run does not start. **Reset** restores the defaults; **Simulate** starts.

### The simulated network

- The receiver is a STREAM server at `test.server` and the sender a connection from `test.client`, both in XRP at scale 9. The server's secret is random per run; the sender opens one stream with a send maximum of the amount, ignores the exchange rate, and closes the connection once the receiver has acknowledged the whole amount.
- The connector (`test.connector`) takes each Prepare either end sends:
  - beyond its packets in flight: rejected at once with T03 "too many packets in flight";
  - above its largest packet: rejected at once with F08 "amount too large", with the data an F08 carries (the amount received and the maximum), which the sender reads;
  - otherwise forwarded. A round trip is drawn from a log-normal distribution whose median is the latency and whose σ is the jitter. The Prepare reaches the other end after half of it, which answers then; the answer is back after the whole of it.
- An answer that would come back at or after the Prepare's expiry never reaches the sender: the connector rejects the Prepare with R00 "expired" at the expiry, as a real one does. A Prepare that would arrive after its expiry is not delivered; a Fulfill that comes back too late is the connector's loss, and is logged as a warning.
- **Randomness:** a 32-bit seeded generator (mulberry32) with Box–Muller normals. The same settings and seed take the same course.
- **Bounds:** a run stops after 2000 packets or after 10 minutes on its own clock, with the reason logged; the log keeps at most 10,000 lines ("log full" marks the end).
- The simulation never touches the real clock or the page. The view advances it as fast as the time dilation asks; tests run it to the end at once.

### The view

- **Back** to the settings, **Pause**/**Play**, **Step** (one event), **Finish** (run to the end at once), and **Run again** once it has ended.
- Status: "Running", "Paused", "Done" or "Stopped: <reason>"; the time on the run's clock; "N of M delivered"; the number of packets; a progress bar.
- **Packet log:** each Prepare as it is sent ("Sender test.client" or "Receiver test.server"), its state (Prepared, Fulfilled, or Rejected with the code and its name), and its amount. Opened: the Prepare's amount, destination, expiry and data; the Fulfill's or Reject's fields (message, triggered-by, data); and the STREAM packets inside both, decrypted with the connection's shared secret through `decodeStreamPacket` of the same WASM build: the packet's amount and sequence, and each frame by its RFC 29 name. An F08's data shows the received and maximum amounts instead.
- **Log:** the shared [log viewer](wallet.md#log-viewer), over the sender's, the receiver's and the connector's lines.

## Architecture page

`/architecture/` shows how Dassie is built: its crates, apps, packages and contracts as boxes in layers, coloured by subsystem, with the links between them; journeys through them; how each part is tested; and a short changelog per release. A box opens a panel with what it is and does, and links to its source on GitHub.

- Everything on the page comes from one file, `apps/website/src/architecture/map.json`: `intro`, `subsystems`, `viewBox`, `bands` (the layers), `notes`, `components` (each box: `name`, `path`, `label`, `sub`, position `x`, `y`, `w`, optional `h`, `desc`, `does`), `edges` (`f` builds on `t`, kind `uses`, `talks`, `wasm` or `runs`, and `h` for hidden until selected), `flows` (journeys of steps over boxes), `testsIntro`, `tests`, and `releases` (`version`, `date`, `changes`), newest first.
- `map_test.ts` checks that every subsystem has a colour in `site.css`; every directory in `crates/`, `apps/`, `packages/`, `tests/` and every contract version in `proto/dassie/` has a box, and every box's path exists; boxes sit inside a layer, apart, inside the map; every link and journey step names boxes; and the newest release is the version in `crates/dassie-cli/Cargo.toml`, with dates in order.
- `map.json` changes in the same PR as what it describes. The rule for what to update, and how to write it, is in [AGENTS.md](../../AGENTS.md) ("Keep the architecture page current"); it is not repeated here.

## Deployment

- **Vercel** builds the site from `apps/website` for every branch, so each branch gets a preview. `vercel.json`: no framework, `installCommand` installs `gcc`, `gzip` and `tar` with `dnf`, `buildCommand` is `sh vercel-build.sh`, `ignoreCommand` is `sh vercel-ignore.sh`, output `dist`.
- **`vercel-build.sh`** (x86-64 Linux only, as Vercel builds): Vercel's image has neither the pinned Node and pnpm nor Rust, so it installs, into a temporary directory:
  - Node, pnpm (its standalone build), rustup and the `wasm-bindgen` CLI, each pinned by version and its download by SHA-256, fetched over HTTPS only (TLS 1.2 or later);
  - through rustup, the toolchain `rust-toolchain.toml` names, which rustup checks against the channel's manifest, and the `wasm32-unknown-unknown` target.

  Then `pnpm install --frozen-lockfile`, `node --run build:wasm` and `node --run build:website`.
- **`vercel-ignore.sh`** skips a deployment (exits 0) only when nothing the site is built from changed since `VERCEL_GIT_PREVIOUS_SHA`, the commit Vercel last deployed on that branch. It watches the site, the packages it imports (`packages/identity-wasm`, `packages/rpc`, `packages/ui`), the crates of its WASM modules and those they depend on in the workspace (`dassie-identity`, `dassie-ilp`, `dassie-node-core`, `dassie-oer`, `dassie-stream`), and the lockfiles and pins (`Cargo.toml`, `Cargo.lock`, `rust-toolchain.toml`, `package.json`, `pnpm-workspace.yaml`, `pnpm-lock.yaml`, `.npmrc`, `scripts/build-wasm.ts`). Anything it cannot compare builds: a first deployment, an empty or unknown previous commit (a force push, or one beyond the clone's depth).

## Tests

- `apps/website/build_test.ts`: the built pages keep to the CSP and each has the header and footer with one current link; `vercel-build.sh` pins the flake's Node and pnpm and `Cargo.lock`'s `wasm-bindgen`, each with a SHA-256; `vercel-ignore.sh` watches every crate and package the site is built from (from the workspace's dependencies and the site's imports), skips when the previous commit is `HEAD`, and builds without one or with one the clone lacks.
- Unit tests for the toy's settings, simulation and descriptions, run in Node against the same `web` build of the WASM module that the browser loads; and `map_test.ts`.
- `tests/e2e/website_test.ts` serves the built site with the headers and trailing slashes of `vercel.json`, so its CSP applies, and drives it in headless Chrome: the pages link to each other, the architecture map shows a journey and a box, the toy delivers the default payment and its packets decrypt, and settings out of range are refused. Errors, exceptions and CSP refusals fail it ([Testing](testing.md)).

## Decisions

- [D1](../decisions/D01-what-runs-in-rust-and-what-in-typescript.md): the toy runs the real STREAM implementation compiled to WASM.
- [D21](../decisions/D21-tooling-on-node-and-pnpm-not-deno.md): Node and pnpm, one `web` WASM build for the browser and Node.

## Original

- [6 §3](../original/06-user-interfaces-and-tooling.md): the original site (Astro and Starlight), its content and the STREAM toy.
