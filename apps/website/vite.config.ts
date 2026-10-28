import { readFileSync } from "node:fs";
import { readdir, rm } from "node:fs/promises";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import solid from "@solidjs/vite-plugin";
import { defineConfig, type Plugin } from "vite";
import { workspaceResolve } from "@dassie/ui/vite";
import { inDocs } from "./src/docs.ts";
import { renderDocs } from "./src/docs-layout.ts";
import { footer, header, sitePaths } from "./src/layout.ts";

// dassie.land (spec/areas/website.md#build): static pages in dist/, which
// its bucket serves under the CSP in headers.json, as strict as the
// wallet's: no inline script or style, nothing from other origins.

/** The release key, as the binaries and the installer trust it: the NixOS
 * guide prints it for checking a release's signature by hand. */
const releaseKey = readFileSync(
  new URL("../../resources/minisign.pub", import.meta.url),
  "utf8",
).split("\n")[1] ?? "";
if (!/^RW[A-Za-z0-9+/]{54}$/.test(releaseKey)) {
  throw new Error("resources/minisign.pub holds no minisign public key");
}

/** Puts the docs layout into every page under /docs/, then the shared
 * header and footer, and the release key, into every page. */
function layout(): Plugin {
  return {
    name: "dassie-website-layout",
    transformIndexHtml: {
      order: "pre",
      handler(html, context) {
        const path = context.path.replace(/index\.html$/, "");
        const page = inDocs(path) ? renderDocs(path, html) : html;
        if (!page.includes("<!-- header -->")) {
          throw new Error(`${context.path} has no header placeholder`);
        }
        return page
          .replace("<!-- header -->", header(path))
          .replace("<!-- footer -->", footer())
          .replaceAll("%RELEASE_KEY%", releaseKey);
      },
    },
  };
}

/** What Pagefind writes that the site doesn't use: its ready-made search
 * interfaces, which set inline styles the CSP refuses. The search page
 * has its own (src/search.ts), on Pagefind's JS API. */
const UNUSED = /^pagefind-(component-ui|modular-ui|ui|highlight)\./;

/**
 * Indexes the built docs for search, with Pagefind, into dist/pagefind/:
 * the pages under /docs/, each by its `data-pagefind-body` (the article of
 * a docs page, src/docs-layout.ts). The index is files the browser
 * fetches from the site's own origin, and searches with WebAssembly,
 * which the CSP lets compile.
 */
function search(): Plugin {
  let outDir = "";
  return {
    name: "dassie-website-search",
    apply: "build",
    configResolved(config) {
      outDir = join(config.root, config.build.outDir);
    },
    async closeBundle() {
      const pagefind = await import("pagefind");
      try {
        const { index, errors } = await pagefind.createIndex();
        if (!index) throw new Error(`pagefind: ${errors.join("; ")}`);
        const added = await index.addDirectory({
          path: outDir,
          glob: "docs/**/*.html",
        });
        if (added.errors.length > 0 || added.page_count === 0) {
          throw new Error(
            `pagefind indexed ${added.page_count} pages: ${
              added.errors.join("; ")
            }`,
          );
        }
        const output = join(outDir, "pagefind");
        const written = await index.writeFiles({ outputPath: output });
        if (written.errors.length > 0) {
          throw new Error(`pagefind: ${written.errors.join("; ")}`);
        }
        for (const file of await readdir(output)) {
          if (UNUSED.test(file)) await rm(join(output, file));
        }
      } finally {
        await pagefind.close();
      }
    },
  };
}

/** Each page's HTML, by a name for its chunk: its path. */
const input = Object.fromEntries(
  sitePaths().map((path) => [
    path === "/" ? "home" : path.slice(1, -1).replaceAll("/", "-"),
    fileURLToPath(new URL(`.${path}index.html`, import.meta.url)),
  ]),
);

export default defineConfig({
  plugins: [solid(), layout(), search()],
  resolve: workspaceResolve(),
  build: {
    outDir: "dist",
    emptyOutDir: true,
    target: "esnext",
    rollupOptions: { input },
    // Every asset is a file of its own: a `data:` URL for the WASM module
    // would be refused by the CSP's `connect-src 'self'`.
    assetsInlineLimit: 0,
    modulePreload: { polyfill: false },
  },
});
