import solid from "@solidjs/vite-plugin";
import { defineConfig } from "vite";
import { workspaceResolve } from "@dassie/ui/vite";

// The dev dashboard (spec/areas/dev.md#dashboard), a static page the dev server
// serves under the same strict CSP as the wallet's. The dev environment builds
// it with `--mode development --watch` into dist-dev/; dist/ is the production
// build CI checks.
export default defineConfig(({ mode }) => {
  const dev = mode === "development";
  return {
    plugins: [solid()],
    resolve: workspaceResolve(),
    build: {
      outDir: dev ? "dist-dev" : "dist",
      // A watch build keeps what is there, so a reload during a rebuild
      // still finds the page.
      emptyOutDir: !dev,
      target: "esnext",
      assetsInlineLimit: 0,
      modulePreload: { polyfill: false },
    },
  };
});
