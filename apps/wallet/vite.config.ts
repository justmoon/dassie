import solid from "@solidjs/vite-plugin";
import { defineConfig } from "vite";
import { workspaceResolve } from "@dassie/ui/vite";

// A static single-page app in dist/, which the node embeds at build time
// (`DASSIE_WALLET_DIST`) and serves under the strict page CSP: no inline
// script or style, nothing from other origins.
//
// The dev environment builds it with `--mode development` into dist-dev/,
// which the dev nodes serve instead (`DASSIE_DEV_WALLET_DIR`). Only that
// build reads a login proof from the URL (spec 7 §6); in a production build
// `__DASSIE_DEV_LOGIN__` is false and the code is left out.
export default defineConfig(({ mode }) => {
  const dev = mode === "development";
  return {
    plugins: [solid()],
    resolve: workspaceResolve(),
    define: { __DASSIE_DEV_LOGIN__: JSON.stringify(dev) },
    build: {
      outDir: dev ? "dist-dev" : "dist",
      // A watch build keeps what is there, so a reload during a rebuild
      // still finds the page.
      emptyOutDir: !dev,
      target: "esnext",
      // Every asset is a file of its own. A `data:` URL would be refused by
      // the CSP's `connect-src 'self'` when the WASM module is fetched.
      assetsInlineLimit: 0,
      // Browsers that run the wallet's WASM all support module preloading,
      // so the polyfill would be dead weight.
      modulePreload: { polyfill: false },
    },
  };
});
