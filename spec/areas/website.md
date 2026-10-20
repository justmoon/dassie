# Website

`apps/website` builds `dassie.land`: a few static pages, the architecture page, and the STREAM toy, which sends a STREAM payment over a simulated network in the browser with Dassie's own STREAM code compiled to WebAssembly. A bucket behind the release mirror's load balancer serves it ([D35](../decisions/D35-dassie-land-is-a-bucket-behind-the-mirrors-load-balancer.md)).

**Paths:** `apps/website/`, `scripts/website/`, `.github/workflows/website.yml`
**Read with:** [Stream](stream.md), [Wallet](wallet.md) (shared components), [Release](release.md) (what the Quick Start installs), [Testing](testing.md)

## Pages

- Each page is a plain HTML file and a Vite build input; a build step puts the shared header and footer into each (`src/layout.ts`). Only the toy and the architecture page run a script, each a Solid 2 island.
- **The Quick Start must match what the tools do:** the installer and `dassie update` ([Release](release.md)) and the `dassie` commands ([Node](node.md#the-dassie-command)).
- The website does not serve the installer: `sh.dassie.land` and `get.dassie.land` belong to the release mirror ([Release](release.md)).

## Build

`node --run build:website` writes `apps/website/dist/`; it needs the WASM modules built first (`node --run build:wasm`). The build keeps to the CSP for the same reasons as the wallet's ([Wallet](wallet.md#content-security-policy)), and resolves the workspace through `@dassie/ui/vite`.

## Content Security Policy and headers

`headers.json` lists the headers every response gets, and is the only list of them: a CSP as strict as the wallet's (no inline script or style, no script, style or image from another origin, and WebAssembly may compile), HSTS, so that the page with the install command is never reached over plain HTTP once a browser has seen it, `nosniff` and a referrer policy. The load balancer adds nothing of its own: a header the site needs is in this list. `apps/website/build_test.ts` checks the built pages against the CSP, and `tests/e2e/website_test.ts` serves them with `headers.json`'s headers so that a refusal fails the test.

## STREAM toy

The sender and the receiver are `dassie-stream`'s real state machines, from the same `web` WASM build that Node loads in the tests, so the toy shows what Dassie's STREAM does, not a model of it ([Stream](stream.md), [D1](../decisions/D01-what-runs-in-rust-and-what-in-typescript.md)). Its packet log decrypts the STREAM packets inside each ILP packet with the connection's shared secret, through the same build.

### The simulated network

Between them sits a connector, simulated one event at a time in `src/toy/simulation.ts`:

- It enforces a largest packet (F08, with the data the sender reads) and a number of packets in flight (T03), and draws each round trip from a seeded log-normal distribution.
- An answer that would come back at or after the Prepare's expiry never reaches the sender: the connector rejects the Prepare with R00 at the expiry, as a real one does.
- The simulation runs on a clock of its own and never touches the real clock or the page. The same settings and seed take the same course; the view plays it at the speed the settings ask, and tests run it to the end at once.
- A run is bounded in packets, simulated time and log lines.

## Architecture page

`/architecture/` shows how Dassie is built: its crates, apps, packages and contracts, how they connect, journeys through them, how each is tested, and a short changelog per release. Everything on it comes from `apps/website/src/architecture/map.json`, which `map_test.ts` checks against the repository. The rule for keeping it current is in [AGENTS.md](../../AGENTS.md) ("Keep the architecture page current"); it is not repeated here.

## Deployment

- **Where:** the bucket `dassie-website`, behind the load balancer that serves the mirror ([Release](release.md#the-mirror)). It answers `dassie.land`, and redirects `www.dassie.land` there. `scripts/website/provision.sh` makes the bucket, its backend with `headers.json`'s headers, the host rules, the certificates, the account that publishes and the `dassie.land` zone.
- **Publishing:** `.github/workflows/website.yml` builds the site from `main` in the flake's shell, as CI does, in a job that holds no credential, and a second job publishes it, whenever something the site is built from changes; `build_test.ts` checks that its `paths` name every crate and package the site is built from. Pull requests get no preview.
- **`scripts/website/deploy.sh`** never leaves a page naming an asset that is not there yet: assets first, cached for good (their names hold their hash), then the pages, revalidated on every load, and only then does it delete what the build no longer has.
- **`scripts/website/check.sh`** runs after each publish and fails when the live site's headers differ from `headers.json` (a backend provisioned with other headers), a page's path without its slash does not redirect, assets are not cached for good, or the WebAssembly is not `application/wasm`.

## Decisions

- [D1](../decisions/D01-what-runs-in-rust-and-what-in-typescript.md): the toy runs the real STREAM implementation compiled to WASM.
- [D21](../decisions/D21-tooling-on-node-and-pnpm-not-deno.md): Node and pnpm, one `web` WASM build for the browser and Node.
- [D35](../decisions/D35-dassie-land-is-a-bucket-behind-the-mirrors-load-balancer.md): a bucket behind the mirror's load balancer, published from `main`, with no previews.
