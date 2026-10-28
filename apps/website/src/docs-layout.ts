/**
 * The docs layout, which the build puts around every page under /docs/
 * (vite.config.ts), as `layout.ts` puts the header and footer: the
 * sidebar from the page list (`docs.ts`), with the page marked, the
 * breadcrumbs, "On this page" from the page's `<h2>`s, and the previous
 * and next pages in sidebar order. It writes each section's index from the
 * page list, and fills the placeholders a page may hold:
 *
 * - `<!-- releases -->`: every release's notes, from the architecture
 *   page's `map.json`, so there is one changelog;
 * - `<!-- newest-release -->`: the newest release, in short;
 * - `<!-- sections -->`: the sections, each with its blurb.
 *
 * A docs page holds its content in `<main class="docs">`; everything here
 * happens at build time, so the pages run no script for it. `docs_test.ts`
 * renders the pages the same way, without a build.
 *
 * @module
 */

import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { MAP } from "./architecture/map.ts";
import {
  type DocPage,
  DOCS,
  inDocs,
  SEARCH,
  type Section,
  SECTIONS,
  SIDEBAR,
} from "./docs.ts";

/** The page with the release notes, which other pages link into. */
export const RELEASES = "/docs/reference/releases/";

/** The source file of the page at `path`. */
export function sourceFile(path: string): string {
  return fileURLToPath(new URL(`..${path}index.html`, import.meta.url));
}

/** `text` with HTML's special characters escaped. */
export function escape(text: string): string {
  return text.replaceAll("&", "&amp;").replaceAll("<", "&lt;").replaceAll(
    ">",
    "&gt;",
  ).replaceAll('"', "&quot;");
}

/** `html` as text: tags dropped, white space collapsed. Entities stay. */
export function text(html: string): string {
  return html.replace(/<[^>]*>/g, "").replace(/\s+/g, " ").trim();
}

/** A page's `<meta name="description">`, as written (HTML-escaped). */
export function description(html: string): string | undefined {
  return html.match(/<meta\s+name="description"\s+content="([^"]*)"/)?.[1]
    .replace(/\s+/g, " ").trim();
}

/** The description of the page at `path`, from its file. */
function describe(path: string): string {
  const found = description(readFileSync(sourceFile(path), "utf8"));
  if (found === undefined) throw new Error(`${path} has no description`);
  return found;
}

/** A release's notes are plain text, with code in backticks. */
function prose(change: string): string {
  return escape(change).split("`").map((part, index) =>
    index % 2 === 1 ? `<code>${part}</code>` : part
  ).join("");
}

/** A day, as `2026-10-24`, in words. */
function day(date: string): string {
  return new Date(`${date}T12:00:00Z`).toLocaleDateString("en", {
    day: "numeric",
    month: "long",
    year: "numeric",
    timeZone: "UTC",
  });
}

/** The anchor of a release's notes on the release notes page. */
export function releaseId(version: string): string {
  return `v${version.replaceAll(".", "-")}`;
}

/** Every release's notes, newest first. */
function releases(): string {
  return MAP.releases.map((release) =>
    `<h2 id="${releaseId(release.version)}">${escape(release.version)}</h2>
      <p class="release-date"><time datetime="${release.date}">${
      day(release.date)
    }</time></p>
      <ul>
        ${
      release.changes.map((change) => `<li>${prose(change)}</li>`).join(
        "\n        ",
      )
    }
      </ul>`
  ).join("\n      ");
}

/** The newest release, in short, for the docs home. */
function newestRelease(): string {
  const newest = MAP.releases[0];
  if (!newest) throw new Error("map.json has no releases");
  return `<section class="newest-release" aria-labelledby="newest-release">
        <h2 id="newest-release">Newest release</h2>
        <p class="newest-release-version">
          <a href="${RELEASES}#${releaseId(newest.version)}">Dassie ${
    escape(newest.version)
  }</a>
          <time datetime="${newest.date}">${day(newest.date)}</time>
        </p>
        <p>${prose(newest.changes[0] ?? "")}</p>
        <p><a href="${RELEASES}">All release notes</a></p>
      </section>`;
}

/** The sections, each with its blurb, for the docs home. */
function sections(): string {
  return `<ul class="docs-sections">
        ${
    SECTIONS.map((section) =>
      `<li><a href="${section.path}">${escape(section.name)}</a>
          <p>${escape(section.blurb)}</p></li>`
    ).join("\n        ")
  }
      </ul>`;
}

/** The source of a section's index: its blurb, then its pages, each with
 * its description. */
function sectionSource(section: Section): string {
  const pages = section.pages.map((page) =>
    `<li>
            <a href="${page.path}">${escape(page.title)}</a>
            <p>${describe(page.path)}</p>
          </li>`
  ).join("\n          ");
  return `<!doctype html>
<html lang="en">
  <head>
    <meta charset="utf-8" />
    <meta name="viewport" content="width=device-width, initial-scale=1" />
    <meta name="color-scheme" content="light dark" />
    <meta name="description" content="${escape(section.blurb)}" />
    <title>${escape(section.name)} · Dassie</title>
    <link rel="icon" href="/brand/dassie-mark-small.svg" type="image/svg+xml" />
    <link rel="stylesheet" href="/src/site.css" />
  </head>
  <body>
    <!-- header -->
    <main class="docs">
      <h1>${escape(section.name)}</h1>
      <p class="lede">${escape(section.blurb)}</p>
      <ul class="section-pages">
          ${pages}
      </ul>
    </main>
    <!-- footer -->
  </body>
</html>
`;
}

/** A heading "On this page" lists. */
export interface Heading {
  id: string;
  /** As HTML text: entities stay escaped. */
  text: string;
}

/** The `<h2>`s of `content`, each of which must have an id. */
export function headings(path: string, content: string): Heading[] {
  return [...content.matchAll(/<h2\b([^>]*)>([\s\S]*?)<\/h2>/g)].map(
    ([whole, attributes, inner]) => {
      const id = attributes.match(/\bid="([^"]+)"/)?.[1];
      if (!id) throw new Error(`${path}: an <h2> without an id: ${whole}`);
      return { id, text: text(inner) };
    },
  );
}

function tocList(list: Heading[]): string {
  return `<ol>
          ${
    list.map((heading) =>
      `<li><a href="#${heading.id}">${heading.text}</a></li>`
    )
      .join("\n          ")
  }
        </ol>`;
}

function sidebar(path: string): string {
  const mark = (link: string) => link === path ? ' aria-current="page"' : "";
  const groups = SECTIONS.map((section) => {
    const open = section.path === path ||
      section.pages.some((page) => page.path === path);
    const pages = [
      { path: section.path, title: "Overview" },
      ...section.pages,
    ].map((page) =>
      `<li><a href="${page.path}"${mark(page.path)}>${
        escape(page.title)
      }</a></li>`
    ).join("\n            ");
    return `<details class="docs-nav-section"${open ? " open" : ""}>
          <summary>${escape(section.name)}</summary>
          <ul>
            ${pages}
          </ul>
        </details>`;
  }).join("\n        ");
  return `<nav class="docs-nav" id="docs-nav" aria-label="Docs">
        <form class="docs-search" role="search" action="${SEARCH}" method="get">
          <label class="visually-hidden" for="docs-nav-q">Search the docs</label>
          <input id="docs-nav-q" type="search" name="q" placeholder="Search the docs" />
        </form>
        <a class="docs-nav-home" href="${DOCS}"${mark(DOCS)}>Docs home</a>
        ${groups}
      </nav>`;
}

function breadcrumbs(trail: { path?: string; title: string }[]): string {
  if (trail.length === 0) return "";
  const items = trail.map((crumb, index) =>
    index === trail.length - 1
      ? `<li><span aria-current="page">${escape(crumb.title)}</span></li>`
      : `<li><a href="${crumb.path}">${escape(crumb.title)}</a></li>`
  ).join("");
  return `<nav class="breadcrumbs" aria-label="Breadcrumbs"><ol>${items}</ol></nav>`;
}

function pager(path: string): string {
  const at = SIDEBAR.findIndex((page) => page.path === path);
  const link = (page: DocPage | undefined, rel: "prev" | "next") =>
    page
      ? `<a class="${rel}" rel="${rel}" href="${page.path}"><span>${
        rel === "prev" ? "Previous" : "Next"
      }</span>${escape(page.title)}</a>`
      : "";
  return `<nav class="docs-pager" aria-label="Previous and next">
        ${link(SIDEBAR[at - 1], "prev")}
        ${link(SIDEBAR[at + 1], "next")}
      </nav>`;
}

/** What a page under /docs/ is. */
type Kind =
  | { kind: "home" }
  | { kind: "search" }
  | { kind: "section"; section: Section }
  | { kind: "page"; section: Section; page: DocPage };

function kindOf(path: string): Kind {
  if (path === DOCS) return { kind: "home" };
  if (path === SEARCH) return { kind: "search" };
  for (const section of SECTIONS) {
    if (section.path === path) return { kind: "section", section };
    const page = section.pages.find((page) => page.path === path);
    if (page && inDocs(path)) return { kind: "page", section, page };
  }
  throw new Error(`${path} is not in the docs' page list (src/docs.ts)`);
}

/** The page at `path` under /docs/, from its source `html`, in the docs
 * layout. A section's index needs no source: it is written from the page
 * list. The header and footer placeholders stay for the site's layout. */
export function renderDocs(path: string, html: string): string {
  const kind = kindOf(path);
  const source = kind.kind === "section" ? sectionSource(kind.section) : html;
  const main = source.match(/<main class="docs">([\s\S]*)<\/main>/);
  if (!main) throw new Error(`${path} has no <main class="docs">`);
  let content = main[1]
    .replace("<!-- releases -->", releases)
    .replace("<!-- newest-release -->", newestRelease)
    .replace("<!-- sections -->", sections);

  const list = kind.kind === "page" ? headings(path, content) : [];
  const toc = list.length >= 2;
  if (toc) {
    content = content.replace(
      /<h2\b/,
      (h2) =>
        `<details class="docs-toc-inline" data-pagefind-ignore>
        <summary>On this page</summary>
        ${tocList(list)}
      </details>
      ${h2}`,
    );
  }

  const docs = { path: DOCS, title: "Docs" };
  const trail = kind.kind === "page"
    ? [
      docs,
      { path: kind.section.path, title: kind.section.name },
      { title: kind.page.title },
    ]
    : kind.kind === "section"
    ? [docs, { title: kind.section.name }]
    : kind.kind === "search"
    ? [docs, { title: "Search" }]
    : [];
  const indexed = kind.kind === "page"
    ? ` data-pagefind-body data-pagefind-meta="section:${
      escape(kind.section.name)
    }"`
    : "";

  const layout = `<div class="docs-layout${toc ? " has-toc" : ""}">
      <main class="docs-main" id="main">
        <div class="docs-top">
          ${breadcrumbs(trail)}
          <a class="docs-menu-link" href="#docs-nav">Menu</a>
        </div>
        <article class="docs-article"${indexed}>${content}</article>
        ${kind.kind === "page" ? pager(path) : ""}
      </main>
      ${sidebar(path)}
      ${
    toc
      ? `<nav class="docs-toc" aria-label="On this page">
        <p class="docs-toc-title">On this page</p>
        ${tocList(list)}
      </nav>`
      : ""
  }
    </div>`;
  return source.replace(main[0], () => layout);
}
