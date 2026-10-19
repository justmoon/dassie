# Website

`apps/website` builds `dassie.land`: a few static pages, the architecture page, and the STREAM toy, which sends a STREAM payment over a simulated network in the browser with Dassie's own STREAM code compiled to WebAssembly. Vercel builds and serves it.

**Paths:** `apps/website/`
**Read with:** [Stream](stream.md), [Wallet](wallet.md) (shared components), [Release](release.md) (what the Quick Start installs), [Testing](testing.md)

## Pages

- Each page is a plain HTML file and a Vite build input; a build step puts the shared header and footer into each (`src/layout.ts`). Only the toy and the architecture page run a script, each a Solid 2 island.
- **The Quick Start must match what the tools do:** the installer and `dassie update` ([Release](release.md)) and the `dassie` commands ([Node](node.md#the-dassie-command)).
- The website does not serve the installer: `sh.dassie.land` and `get.dassie.land` belong to the release mirror ([Release](release.md)).

## Build

`node --run build:website` writes `apps/website/dist/`; it needs the WASM modules built first (`node --run build:wasm`). The build keeps to the CSP for the same reasons as the wallet's ([Wallet](wallet.md#content-security-policy)), and resolves the workspace through `@dassie/ui/vite`.

## Content Security Policy and headers

`vercel.json` sets the headers, with a CSP as strict as the wallet's: no inline script or style, no script, style or image from another origin, and WebAssembly may compile. `apps/website/build_test.ts` checks the built pages against it, and `tests/e2e/website_test.ts` serves them with `vercel.json`'s headers so that a refusal fails the test.

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

- **Vercel** builds the site from `apps/website` for every branch, so each branch gets a preview.
- **`vercel-build.sh`:** Vercel's image has neither the pinned Node and pnpm nor Rust, so the script installs them, and `wasm-bindgen`, each pinned by version and its download by SHA-256; rustup installs the toolchain `rust-toolchain.toml` names. `build_test.ts` checks that the pins match the flake's and `Cargo.lock`'s.
- **`vercel-ignore.sh`** skips a deployment only when nothing the site is built from changed since the commit Vercel last deployed on that branch. Anything it cannot compare (a first deployment, a force push, a commit beyond the clone's depth) builds. `build_test.ts` checks that it watches every crate and package the site is built from.

## Decisions

- [D1](../decisions/D01-what-runs-in-rust-and-what-in-typescript.md): the toy runs the real STREAM implementation compiled to WASM.
- [D21](../decisions/D21-tooling-on-node-and-pnpm-not-deno.md): Node and pnpm, one `web` WASM build for the browser and Node.
