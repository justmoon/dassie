# The docs are hand-written pages under /docs/, searched with Pagefind, and link the spec on GitHub

**Areas:** [Website](../areas/website.md).

## Facts

- dassie.land is hand-written HTML in Kopje, its visual identity ([visual-identity](visual-identity.md)): Vite builds each page, and a build step puts the shared header and footer around it. Every page runs under a CSP as strict as the wallet's: no inline script or style, nothing from another origin, and `connect-src 'self'` ([website-hosting](website-hosting.md)).
- The Quick Start and the NixOS guide were the site's first guides, at `/guides/quick-start/` and `/guides/nixos/`, and links people saved point there.
- Owners, operators and developers need pages they act on: what to run, which screen to open, which port to open. The spec says why Dassie is as it is, for those who change it, in Markdown that cites itself by path and heading and changes in most pull requests.
- What the docs list, a command, an environment variable, a settlement scheme, an option of the NixOS module, is defined in the code. A list kept by hand drifts unless a test reads the code.
- The load balancer in front of the site's bucket already redirects each page's path without its slash, from the site's list of pages ([website-hosting](website-hosting.md)).

## Decision

- **Under `/docs/` on dassie.land.** The docs are part of the site, with its header, its footer and its CSP, in sections under `/docs/`. The guides moved there: `/guides/quick-start/` to `/docs/start/quick-start/` and `/guides/nixos/` to `/docs/node/nixos/`. The load balancer redirects each old path, with its slash and without, with a 308 to the new one, from the site's list of moved pages.
- **Hand-written HTML, in Kopje.** Each page is an HTML file, as the site's other pages are. A build step puts the docs layout around it from one page list: the sidebar, breadcrumbs, the page's own contents, and the previous and next pages. No Markdown and no docs framework.
- **Search with Pagefind.** The build indexes the docs with Pagefind into static files on the site's own origin, and the search page searches them in the browser, with WebAssembly, through Pagefind's JS API and elements of its own. The CSP stays as it is.
- **The spec stays on GitHub.** A docs page says what to do and links the area doc or decision that says why, at `https://github.com/justmoon/dassie/blob/main/spec/...`. The site renders none of the spec.

## Accepted

- HTML is longer to write than Markdown, and a writer keeps to the site's classes by hand. `docs_test.ts` checks each page's title, description, links and CSP without a build, so a mistake fails in the test rather than on the site.
- A new page is two edits: its file, and its entry in the page list. The test fails when either is missing.
- Pagefind is a build dependency with a native binary per platform, which the lockfile pins. Its ready-made search interfaces set inline styles, so the build deletes them and the site keeps a search page of its own.
- Search needs script. Without it, the search page says so and links the docs home; the sidebar and the section pages still reach every page.
- A reader who wants the reasons leaves the site for GitHub, and sees the spec as it is on `main`, which can be ahead of the release they run.
- A moved page needs `provision.sh`'s URL map step run again before its old path redirects; `check.sh` fails until it has.

## Rejected

- *A docs framework (VitePress, Starlight, Docusaurus, mdBook):* each brings a theme to override into Kopje, a client script and inline scripts or styles of its own that the CSP refuses, and dependencies the offline Nix build has to fetch, for pages the site's own build already makes.
- *Markdown, converted by the build:* Kopje's parts of a page, the early-days note, "not there yet", "why" and the reference entries with their ids, would each need an extension of the converter, and the pages would be one step further from what the test and the browser read.
- *A subdomain, `docs.dassie.land`:* a second origin with its own host rule, certificate and CSP, a header that links across origins, and a search index apart from the site's, for no gain over a path.
- *Keeping `/guides/` beside `/docs/`:* two places for pages of one kind. The redirects keep the old links working instead.
- *A hosted search (Algolia DocSearch):* every query would go to another origin, which `connect-src 'self'` refuses, and readers' searches would leave the site.
- *Rendering the spec on the site:* a second copy of a text that changes in most pull requests, published only when the site is, with its Markdown converted under the CSP. GitHub already renders it, with its history, and the spec's own test checks every citation of it from the pages.
