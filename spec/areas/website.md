# Website

`apps/website` builds `dassie.land`: the home page, the docs, the architecture page, the brand page, and the STREAM toy, which sends a STREAM payment over a simulated network in the browser with Dassie's own STREAM code compiled to WebAssembly. A bucket behind the release mirror's load balancer serves it ([website-hosting](../decisions/website-hosting.md)).

**Paths:** `apps/website/`, `scripts/website/`, `.github/workflows/website.yml`
**Read with:** [Stream](stream.md), [Wallet](wallet.md) (shared components), [Release](release.md) (what the Quick Start installs), [Node](node.md) (what the docs describe), [Testing](testing.md)

## Pages

- Each page is a plain HTML file and a Vite build input; a build step puts the shared header and footer into each (`src/layout.ts`). The header is Kopje's: How it works, Run a node, Docs and Architecture, and Early days as a tag of its own; GitHub and Discord are in the footer. Only the toy and the architecture page run a script, each a Solid 2 island; besides them, the home page runs its Copy button, which stays hidden without script, the docs' search page its search, and the brand page its theme switch.
- **Every page draws in Kopje** ([visual-identity](../decisions/visual-identity.md)). `src/kopje.css` holds what all of them share: the fonts, the palette in light and dusk, and the header and footer. `site.css` starts from it and dresses the components shared with the wallet (`@dassie/ui`) in it; the brand page's `brand.css` starts from it too. The pages follow the system's light or dark theme; the brand page starts light and has its own switch. The Copy button copies only the install command: pasted together, the next line would meet the installer's `sudo` asking for a password.
- **The docs must match what the tools do:** the installer, `dassie update` and the flake for NixOS ([Release](release.md)), the `dassie` commands ([Node](node.md#the-dassie-command)), the wallet's screens ([Wallet](wallet.md)), and the ports, files and environment variables they name. The NixOS guide prints the release key, which the build takes from `resources/minisign.pub`.
- The website does not serve the installer: `sh.dassie.land` and `get.dassie.land` belong to the release mirror ([Release](release.md)).

## Docs

`/docs/` holds the docs: what an owner, an operator or a developer does with Dassie, from a first node to the reference. A page links the area doc or decision that says why, on GitHub; the site renders none of the spec ([docs-section](../decisions/docs-section.md)).

- **Hand-written HTML, in Kopje.** Each page is an HTML file under `apps/website/docs/`, like the site's other pages, with no Markdown and no docs framework. A build step puts the docs layout around each (`src/docs-layout.ts`): the sidebar with the page marked, breadcrumbs, the page's own contents from its `<h2>`s, and the previous and next pages. It writes each section's index, and fills a page's placeholders: the release notes come from `map.json`, so the site has one changelog. The pages run no script for any of it.
- **One page list.** `src/docs.ts` lists the sections and their pages, in sidebar order, and is the only list of them: the build makes a page of each entry, and a page's description is its own `<meta name="description">`.
- **Written for what is built.** The pages describe what the released binaries do. What the roadmap or a design describes but the code does not do is "not there yet", linking the docs' roadmap page. Wherever money is involved they say that Dassie takes test money only.
- **Search** runs in the browser, inside the CSP. The build indexes each docs page's article with Pagefind into `dist/pagefind/`, files on the site's own origin, and deletes Pagefind's ready-made interfaces, which set inline styles. The search page (`src/search.ts`) searches through Pagefind's JS API with elements of its own; every search box submits to it.
- **Moved pages** stay reachable. `MOVED` in `src/docs.ts` lists each page that moved, from its old path to its new one, the guides once under `/guides/` among them; the load balancer redirects the old path, with its slash and without, with a 308 (`provision.sh`), and `check.sh` and the browser test check it.
- **Reference pages list what the code has.** The CLI's subcommands, the `DASSIE_*` variables the released binaries read, the settlement schemes and the NixOS module's options each have an entry on their reference page, an element whose id is its name, and the test reads each list from the code.

`apps/website/docs_test.ts` checks the pages on their sources, without a build: every entry of the page list has its file and every file its entry; each page has the title, description and heading the list names it by and keeps to the CSP; every link to a page of the site or an anchor leads somewhere, and every link into the repository on GitHub names a file that exists (links into `spec/` are `scripts/spec_test.ts`'s); and each reference page above has an entry for everything the code defines. `build_test.ts` checks the built pages: the docs layout on each, and the search index without Pagefind's interfaces. The rule for keeping the docs current is in [AGENTS.md](../../AGENTS.md) ("Keep the docs current").

## Build

`node --run build:website` writes `apps/website/dist/`; it needs the WASM modules built first (`node --run build:wasm`). The flake builds the same, offline, as its `website` package (`nix/ts.nix`): the site CI's tests serve and the workflow publishes. The build keeps to the CSP for the same reasons as the wallet's ([Wallet](wallet.md#content-security-policy)), and resolves the workspace through `@dassie/ui/vite`.

## Content Security Policy and headers

`headers.json` lists the headers every response gets, and is the only list of them: a CSP as strict as the wallet's (no inline script or style, no script, style or image from another origin, and WebAssembly may compile), HSTS, so that the page with the install command is never reached over plain HTTP once a browser has seen it, `nosniff` and a referrer policy. The load balancer adds nothing of its own: a header the site needs is in this list. `apps/website/build_test.ts` checks the built pages against the CSP, and `tests/e2e/website_test.ts` serves them with `headers.json`'s headers so that a refusal fails the test.

## STREAM toy

The sender and the receiver are `dassie-stream`'s real state machines, from the same `web` WASM build that Node loads in the tests, so the toy shows what Dassie's STREAM does, not a model of it ([Stream](stream.md), [rust-and-typescript](../decisions/rust-and-typescript.md)). Its packet log decrypts the STREAM packets inside each ILP packet with the connection's shared secret, through the same build.

### The simulated network

Between them sits a connector, simulated one event at a time in `src/toy/simulation.ts`:

- It enforces a largest packet (F08, with the data the sender reads) and a number of packets in flight (T03), and draws each round trip from a seeded log-normal distribution.
- An answer that would come back at or after the Prepare's expiry never reaches the sender: the connector rejects the Prepare with R00 at the expiry, as a real one does.
- The simulation runs on a clock of its own and never touches the real clock or the page. The same settings and seed take the same course; the view plays it at the speed the settings ask, and tests run it to the end at once.
- A run is bounded in packets, simulated time and log lines.

## Architecture page

`/architecture/` shows how Dassie is built: its crates, apps, packages and contracts, how they connect, journeys through them, how each is tested, and a short changelog per release. Everything on it comes from `apps/website/src/architecture/map.json`, which `map_test.ts` checks against the repository. The rule for keeping it current is in [AGENTS.md](../../AGENTS.md) ("Keep the architecture page current"); it is not repeated here.

## Brand page

`/brand/` is Kopje, Dassie's visual identity: the mark, color, type, the graphic language, voice, and how they come together on the site, the wallet and what operators hand out ([visual-identity](../decisions/visual-identity.md)). It is the reference for how anything of Dassie's looks, so a change to the identity is a change to this page. The header leaves it out: it is for those who design for Dassie, and AGENTS.md and the docs' Project section link it. Its bare path answers once `provision.sh`'s URL map step has run with it; `check.sh` checks only the pages the home page links to. `public/brand/` holds the mark as SVG files, Sunrise and the Silhouette, for anyone who needs them.

- It keeps to the CSP as every page does: its fonts are files of the site's own, with their licenses beside them, and what would be inline styles are classes in `brand.css`.
- It does not load `site.css`, whose class names it shares; both start from `src/kopje.css`, so the identity's fonts, palette, header and footer are written once.
- It is written to be read, by people and by agents designing for Dassie (AGENTS.md, "Follow the visual identity"): its small drawings are inline SVG, so that its dark mode can recolor them, and the heavy ones, grass drawn blade by blade and distant kopjes, are SVG files in `brand/img/`. `theme.ts` switches between light and dark; the page starts light.

## Deployment

- **Where:** the bucket `dassie-website`, behind the load balancer that serves the mirror ([Release](release.md#the-mirror)). It answers `dassie.land`, and redirects `www.dassie.land` there. `scripts/website/provision.sh` makes the bucket, its backend with `headers.json`'s headers, the host rules, the certificates, the account that publishes and the `dassie.land` zone.
- **Publishing:** `.github/workflows/website.yml` builds the flake's `website` package (`nix/ts.nix`) from `main` on the Nix cloud, the package CI's tests serve, so the runner runs none of the build's code, and a second job fetches it from the cloud's cache and publishes it, whenever something the site is built from changes; `build_test.ts` checks that its `paths` name every crate and package the site is built from. Pull requests get no preview.
- **`scripts/website/deploy.sh`** never leaves a page naming an asset that is not there yet: assets first, cached for good (their names hold their hash), then the pages, revalidated on every load, and only then does it delete what the build no longer has.
- **`scripts/website/check.sh`** runs after each publish and fails when the live site's headers differ from `headers.json` (a backend provisioned with other headers), a page the home page links to does not redirect from its path without its slash with the load balancer's 308, a moved page's old path does not redirect to its new one with a 308, assets are not cached for good, or the WebAssembly is not `application/wasm`.

## Decisions

- [rust-and-typescript](../decisions/rust-and-typescript.md): the toy runs the real STREAM implementation compiled to WASM.
- [node-and-pnpm](../decisions/node-and-pnpm.md): Node and pnpm, one `web` WASM build for the browser and Node.
- [website-hosting](../decisions/website-hosting.md): a bucket behind the mirror's load balancer, published from `main`, with no previews.
- [visual-identity](../decisions/visual-identity.md): Dassie's visual identity is Kopje, and the brand page holds it.
- [granite-rocks](../decisions/granite-rocks.md): Kopje's rocks are gray granite blocks, drawn by hand to rules the brand page states, and the footer has a composition of its own on phones.
- [docs-section](../decisions/docs-section.md): the docs are hand-written pages under `/docs/`, searched with Pagefind, and link the spec on GitHub.
