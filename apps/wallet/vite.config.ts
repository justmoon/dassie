import { fileURLToPath } from "node:url";
import solid from "@solidjs/vite-plugin";
import { type Alias, defineConfig } from "vite";

/**
 * Vite resolves the wallet's imports itself, not through Deno, so it is told
 * where Deno found each workspace module the wallet imports. Asking Deno
 * (`import.meta.resolve`) keeps the wallet out of the other packages'
 * directories: it only knows their names and exports.
 *
 * The build task loads this file natively (`--configLoader native`), so
 * Deno runs it in place and resolves through the workspace. Vite's default
 * loader bundles it into a temporary module first, where
 * `import.meta.resolve` no longer yields a file URL.
 */
function workspaceModule(specifier: string): Alias {
  const escaped = specifier.replace(/[.*+?^${}()|[\]\\/]/g, "\\$&");
  return {
    find: new RegExp(`^${escaped}$`),
    replacement: fileURLToPath(import.meta.resolve(specifier)),
  };
}

// A static single-page app in dist/, which the node embeds at build time
// (`DASSIE_WALLET_DIST`) and serves under the strict page CSP: no inline
// script or style, nothing from other origins.
export default defineConfig({
  plugins: [solid()],
  resolve: {
    alias: [
      workspaceModule("@dassie/rpc"),
      workspaceModule("@dassie/rpc/proto"),
      workspaceModule("@dassie/identity-wasm/web"),
    ],
  },
  build: {
    outDir: "dist",
    emptyOutDir: true,
    target: "esnext",
    // Every asset is a file of its own. A `data:` URL would be refused by
    // the CSP's `connect-src 'self'` when the WASM module is fetched.
    assetsInlineLimit: 0,
    // Browsers that run the wallet's WASM all support module preloading, so
    // the polyfill would be dead weight.
    modulePreload: { polyfill: false },
  },
});
