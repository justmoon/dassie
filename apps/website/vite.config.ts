import { fileURLToPath } from "node:url";
import solid from "@solidjs/vite-plugin";
import { defineConfig, type Plugin } from "vite";
import { workspaceResolve } from "@dassie/ui/vite";
import { footer, header, PAGES } from "./src/layout.ts";

// dassie.land (spec/areas/website.md#build): static pages in dist/, which
// Vercel serves under the CSP in vercel.json, as strict as the wallet's: no
// inline script or style, nothing from other origins. Only the STREAM toy runs
// a script.

/** Puts the shared header and footer into every page. */
function layout(): Plugin {
  return {
    name: "dassie-website-layout",
    transformIndexHtml: {
      order: "pre",
      handler(html, context) {
        const path = context.path.replace(/index\.html$/, "");
        if (!html.includes("<!-- header -->")) {
          throw new Error(`${context.path} has no header placeholder`);
        }
        return html
          .replace("<!-- header -->", header(path))
          .replace("<!-- footer -->", footer());
      },
    },
  };
}

/** Each page's HTML, by a name for its chunk. */
const input = Object.fromEntries(
  PAGES.map((page) => [
    page.path === "/" ? "home" : page.path.split("/").at(-2)!,
    fileURLToPath(new URL(`.${page.path}index.html`, import.meta.url)),
  ]),
);

export default defineConfig({
  plugins: [solid(), layout()],
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
