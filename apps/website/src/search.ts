/**
 * The docs' search page (/docs/search/): searches the index the build
 * wrote with Pagefind (vite.config.ts) through Pagefind's JS API, and
 * lists what it finds, with elements of its own rather than Pagefind's
 * ready-made interface, which sets inline styles the CSP refuses. The
 * index and its WebAssembly come from the site's own origin.
 *
 * It searches for `?q=`, which every search box submits, and again as
 * the reader types.
 *
 * @module
 */

/** The parts of Pagefind's API used here. */
interface Pagefind {
  init(): Promise<void>;
  debouncedSearch(
    query: string,
    options?: object,
    wait?: number,
  ): Promise<{ results: Result[] } | null>;
}

interface Result {
  data(): Promise<{
    url: string;
    excerpt: string;
    meta: { title?: string; section?: string };
  }>;
}

/** The most results listed. */
const LIMIT = 20;

/** Where the build puts Pagefind's script. A variable, so that the
 * bundler leaves the import for the browser. */
const PAGEFIND = "/pagefind/pagefind.js";

const input = document.querySelector<HTMLInputElement>("#search-q");
const status = document.querySelector<HTMLElement>(".search-status");
const list = document.querySelector<HTMLOListElement>(".search-results");

/** Pagefind's excerpt is HTML with the matches in `<mark>`: kept as text,
 * with only the marks as elements. */
function excerpt(html: string): DocumentFragment {
  const parsed = new DOMParser().parseFromString(html, "text/html").body;
  const fragment = document.createDocumentFragment();
  for (const node of parsed.childNodes) {
    if (node.nodeName === "MARK") {
      const mark = document.createElement("mark");
      mark.textContent = node.textContent;
      fragment.append(mark);
    } else {
      fragment.append(node.textContent ?? "");
    }
  }
  return fragment;
}

async function main(
  input: HTMLInputElement,
  status: HTMLElement,
  list: HTMLOListElement,
): Promise<void> {
  const pagefind = await import(/* @vite-ignore */ PAGEFIND) as Pagefind;
  await pagefind.init();

  async function search(query: string): Promise<void> {
    if (query.trim() === "") {
      status.textContent = "";
      list.replaceChildren();
      return;
    }
    const found = await pagefind.debouncedSearch(query, {}, 200);
    // A newer search took this one's place.
    if (found === null) return;
    const shown = await Promise.all(
      found.results.slice(0, LIMIT).map((result) => result.data()),
    );
    if (input.value !== query) return;
    status.textContent = shown.length === 0
      ? `Nothing found for “${query}”.`
      : found.results.length > LIMIT
      ? `The first ${LIMIT} of ${found.results.length} pages for “${query}”.`
      : `${shown.length} ${
        shown.length === 1 ? "page" : "pages"
      } for “${query}”.`;
    list.replaceChildren(
      ...shown.map((page) => {
        const item = document.createElement("li");
        item.dataset.testid = "search-result";
        const link = document.createElement("a");
        link.href = page.url;
        link.textContent = page.meta.title ?? page.url;
        item.append(link);
        if (page.meta.section) {
          const section = document.createElement("span");
          section.className = "search-section";
          section.textContent = page.meta.section;
          item.append(section);
        }
        const text = document.createElement("p");
        text.append(excerpt(page.excerpt));
        item.append(text);
        return item;
      }),
    );
  }

  const query = new URLSearchParams(location.search).get("q") ?? "";
  input.value = query;
  input.addEventListener("input", () => {
    const url = new URL(location.href);
    url.searchParams.set("q", input.value);
    history.replaceState(null, "", url);
    void search(input.value);
  });
  await search(query);
}

if (input && status && list) {
  main(input, status, list).catch((error: unknown) => {
    status.textContent = "Search could not start. Try reloading the page.";
    console.error(error);
  });
}
