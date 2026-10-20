# D35. dassie.land is a bucket behind the mirror's load balancer

**Areas:** [Website](../areas/website.md), [Release](../areas/release.md).

## Facts

- dassie.land is static: Vite writes its pages and assets into a directory, and nothing runs on a server. What a host must add is a few headers on every response (the CSP above all), long caching for assets whose names hold their hash, a redirect from a page's path without its trailing slash, and one from `www`.
- The release mirror already runs on GCP in the project that holds the testnet: a global load balancer with Google-managed certificates in front of the `get.dassie.land` bucket, published to by a workflow through workload identity federation ([Release](../areas/release.md#the-mirror)). The project's other zones, `dassie.xyz` and `dassiepay.net`, are in Cloud DNS with DNSSEC.
- The site needs the flake's Node and pnpm and a Rust toolchain for its WebAssembly. CI has them in the flake's shell. A host that builds on its own image has to install each again, pinned by version and hash, and a test has to keep those pins equal to the flake's.
- CI builds the site on every pull request and serves it with its headers in a browser test, so a page that breaks the CSP fails there.

## Decision

- **Hosting:** a bucket of its own, `dassie-website`, served by the mirror's load balancer: one host rule sends `dassie.land` to it, another redirects `www.dassie.land` to `dassie.land`. Cloud CDN caches by the objects' own `Cache-Control`.
- **Headers:** `apps/website/headers.json` lists the headers every response gets, and is the only list of them. `scripts/website/provision.sh` puts them on the backend bucket; the browser test serves the site with them; and the deploy checks that the live site sends them.
- **Publishing:** `.github/workflows/website.yml` builds the site from `main` in the flake's shell, as CI does, and `scripts/website/deploy.sh` copies it into the bucket: assets first, cached for good, then the pages, which are revalidated on every load, then it deletes what the build no longer has.
- **DNS:** the `dassie.land` zone is in Cloud DNS, with DNSSEC, next to the project's other zones.

## Accepted

- No previews. Only `main` is published, so no pull request's workflow holds credentials that can write the site. The browser test in CI is what a pull request gets instead.
- A change to `headers.json` reaches the site only once someone with the project's rights runs `provision.sh` again. Until then the deploy's check fails, so the difference is seen at the next publish.
- A publish is not atomic. A page never names an asset that is not there yet, but a page loaded just before the old assets are deleted can miss a script it loads later.
- A path without its trailing slash gets the bucket's 301, not a 308. Browsers follow both with a GET, and the site has no forms that post.

## Rejected

- *Stay on Vercel:* a second hosting provider and DNS host for one static site, with its own build image that needs the toolchain installed again, and a second set of pins to keep equal to the flake's.
- *Firebase Hosting:* it keeps headers and redirects in the repository and gives previews, but adds Firebase to the project and its CLI's dependencies to the build, for what one more host rule on the existing load balancer does.
- *Cloud Run with a static server:* full control of the headers, at the cost of an image to build, a registry and a service that runs, for files that a bucket serves.
- *Previews under prefixes of a bucket:* a pull request's workflow would need write access to the bucket, and the pages' absolute asset paths would have to change for each prefix.
