/**
 * What every page of dassie.land shares: its header with the navigation,
 * and its footer. `vite.config.ts` puts them into each page at build time,
 * so the pages stay plain HTML with no script.
 *
 * @module
 */

import { DOCS, docsPaths } from "./docs.ts";

/** A page of the site outside the docs, whose pages `src/docs.ts`
 * lists. */
export interface Page {
  path: string;
  title: string;
}

/** The pages outside /docs/. */
export const PAGES: readonly Page[] = [
  { path: "/", title: "Home" },
  { path: "/tools/stream-toy/", title: "STREAM Toy" },
  { path: "/architecture/", title: "Architecture" },
  // The visual identity, for those who design for Dassie: AGENTS.md and
  // the docs' Project section link it, the header does not.
  { path: "/brand/", title: "Brand" },
];

/** The header's links, in order, as Kopje draws the site
 * (brand/index.html, "Applications"). Early days is a tag of its own, at
 * the end. GitHub and Discord are in the footer. */
export const NAV: readonly Page[] = [
  { path: "/docs/concepts/", title: "How it works" },
  { path: "/docs/node/", title: "Run a node" },
  { path: DOCS, title: "Docs" },
  { path: "/architecture/", title: "Architecture" },
];

/** The page that says how early Dassie is: the header's tag. */
export const EARLY_DAYS: Page = {
  path: "/docs/start/early-days/",
  title: "Early days",
};

/** Every page the site has, outside the docs and in them. */
export function sitePaths(): string[] {
  return [...PAGES.map((page) => page.path), ...docsPaths()];
}

export const GITHUB = "https://github.com/justmoon/dassie";
export const DISCORD = "https://discord.gg/HqdGNfs9kq";

/** How a header link relates to the page at `path`: the page itself, the
 * part of the site the page is in, or neither. */
function current(link: string, path: string): string {
  if (link === path) return ' aria-current="page"';
  const within = (prefix: string) => path.startsWith(prefix);
  const docsPart = NAV.find((page) =>
    page.path !== DOCS && page.path.startsWith(DOCS) && within(page.path)
  );
  if (docsPart) return link === docsPart.path ? ' aria-current="true"' : "";
  return link === DOCS && within(DOCS) && path !== EARLY_DAYS.path
    ? ' aria-current="true"'
    : "";
}

/** The header, with the page at `path` marked as the current one, and
 * the part of the docs it is in. */
export function header(path: string): string {
  const links = NAV.map((page) =>
    `<a href="${page.path}"${current(page.path, path)}>${page.title}</a>`
  ).join("\n        ");
  return `<header class="site-header">
      <a class="brand" href="/"${path === "/" ? ' aria-current="page"' : ""}>
        <img src="/brand/dassie-mark.svg" alt="" width="34" height="33" />
        <span>Dassie</span>
      </a>
      <nav aria-label="Site">
        ${links}
        <a class="tag" href="${EARLY_DAYS.path}"${
    EARLY_DAYS.path === path ? ' aria-current="page"' : ""
  }>${EARLY_DAYS.title}</a>
      </nav>
    </header>`;
}

/** The footer: a kopje against the sun, then the night. A phone gets a
 * composition of its own rather than the wide one shrunk. */
export function footer(): string {
  return `<div class="foot-scene">
      <picture>
        <source
          media="(max-width: 600px)"
          srcset="/brand/img/footer-kopje-narrow.svg"
          width="480"
          height="197"
        />
        <img
          src="/brand/img/footer-kopje.svg"
          alt="A kopje of granite blocks against the sun, in veld grass; one dassie climbs its slab, one keeps lookout on top inside the sun, and one sleeps on the next rock"
          width="1200"
          height="197"
          loading="lazy"
        />
      </picture>
    </div>
    <footer class="site-footer">
      <p>
        Dassie is open source. <a href="${GITHUB}">See the code</a> or
        <a href="${DISCORD}">talk to us on Discord</a>.
      </p>
    </footer>`;
}
