import { build } from "esbuild"

import { resolve } from "node:path"

import {
  PATH_DIST_STAGING_SHARED,
  PATH_PACKAGE_APP_NODE,
} from "../constants/paths"

const BANNER = `
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

export const buildBackend = async (detailedVersion: string) => {
  await build({
    entryPoints: [resolve(PATH_PACKAGE_APP_NODE, "src/command-line/entry.ts")],
    outfile: resolve(PATH_DIST_STAGING_SHARED, "backend.mjs"),
    bundle: true,
    platform: "node",
    format: "esm",
    target: "node20",
    define: {
      "import.meta.env.MODE": '"production"',
      "import.meta.env.PROD": "true",
      "import.meta.env.DEV": "false",
      __DASSIE_VERSION__: JSON.stringify(detailedVersion),
      // We have to split the string here to avoid vitest from seeing it
      ["import.meta." + "vitest"]: "undefined",
    },
    banner: {
      js: BANNER,
    },
    logLevel: "info",
  })
}
