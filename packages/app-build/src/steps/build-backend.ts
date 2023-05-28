import { build } from "esbuild"

import { resolve } from "node:path"

import { createActor } from "@dassie/lib-reactive"

import { outputPathSignal } from "../computed/output-path"
import { sourcePathSignal } from "../computed/source-path"

const ESM_REQUIRE_SHIM = `
await (async () => {
  const { dirname } = await import("path");
  const { fileURLToPath } = await import("url");

  /**
   * Shim entry-point related paths.
   */
  if (typeof globalThis.__filename === "undefined") {
    globalThis.__filename = fileURLToPath(import.meta.url);
  }
  if (typeof globalThis.__dirname === "undefined") {
    globalThis.__dirname = dirname(globalThis.__filename);
  }
  /**
   * Shim require if needed.
   */
  if (typeof globalThis.require === "undefined") {
    const { default: module } = await import("module");
    globalThis.require = module.createRequire(import.meta.url);
  }
})();
`

export const buildBackend = () =>
  createActor(async (sig) => {
    const sourcePath = sig.get(sourcePathSignal)
    const outputPath = sig.get(outputPathSignal)

    await build({
      entryPoints: [resolve(sourcePath, "src/backend/entry.ts")],
      outdir: outputPath,
      bundle: true,
      platform: "node",
      format: "esm",
      target: "node20",
      define: {
        "import.meta.env.MODE": '"production"',
        "import.meta.env.PROD": "true",
        "import.meta.env.DEV": "false",
      },
      banner: {
        js: ESM_REQUIRE_SHIM,
      },
    })
  })
