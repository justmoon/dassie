/**
 * What every page of dassie.land shares: its header with the navigation,
 * and its footer. `vite.config.ts` puts them into each page at build time,
 * so the pages stay plain HTML with no script.
 *
 * @module
 */

/** A page of the site. */
export interface Page {
  path: string;
  title: string;
  /** False for a page the navigation leaves out, reached by its address. */
  nav: boolean;
}

/** The pages, in the order the navigation lists them. */
export const PAGES: readonly Page[] = [
  { path: "/", title: "Home", nav: true },
  { path: "/guides/quick-start/", title: "Quick Start", nav: true },
  { path: "/guides/nixos/", title: "NixOS", nav: true },
  { path: "/tools/stream-toy/", title: "STREAM Toy", nav: true },
  { path: "/architecture/", title: "Architecture", nav: true },
  // The visual identity, for those who design for Dassie: AGENTS.md links it.
  { path: "/brand/", title: "Brand", nav: false },
];

export const GITHUB = "https://github.com/justmoon/dassie";
export const DISCORD = "https://discord.gg/HqdGNfs9kq";

/** The header, with the page at `path` marked as the current one. */
export function header(path: string): string {
  const links = PAGES.filter((page) => page.nav && page.path !== "/").map((
    page,
  ) =>
    `<a href="${page.path}"${
      page.path === path ? ' aria-current="page"' : ""
    }>${page.title}</a>`
  ).join("\n        ");
  return `<header class="site-header">
      <a class="brand" href="/"${path === "/" ? ' aria-current="page"' : ""}>
        <img src="/favicon.svg" alt="" width="28" height="28" />
        Dassie
      </a>
      <nav aria-label="Site">
        ${links}
        <a href="${GITHUB}">GitHub</a>
        <a href="${DISCORD}">Discord</a>
      </nav>
    </header>`;
}

/** The footer. */
export function footer(): string {
  return `<footer class="site-footer">
      <p>
        Dassie is open source. <a href="${GITHUB}">See the code</a> or
        <a href="${DISCORD}">talk to us on Discord</a>.
      </p>
    </footer>`;
}
