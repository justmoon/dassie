import { build } from "vite"

import assert from "node:assert"
import { resolve } from "node:path"

import viteConfigExport from "@dassie/app-node/src/frontend/vite.config"

import {
  PATH_DIST_STAGING_SHARED,
  PATH_PACKAGE_APP_NODE,
} from "../constants/paths"

export const buildFrontend = async (detailedVersion: string) => {
  const sourceDirectory = resolve(PATH_PACKAGE_APP_NODE, "src/frontend")
  const outputDirectory = resolve(PATH_DIST_STAGING_SHARED, "frontend")

  const viteConfig = await viteConfigExport

  assert(typeof viteConfig === "object", "Expected viteConfig to be an object")

  // vite-node sets NODE_ENV to "development" by default so we need to override
  // it, otherwise react/jsx-runtime will use the development version.
  process.env["NODE_ENV"] = "production"

  await build({
    ...viteConfig,
    mode: "production",
    root: sourceDirectory,
    define: {
      ...viteConfig.define,
      __DASSIE_VERSION__: JSON.stringify(detailedVersion),
    },
    build: {
      ...viteConfig.build,
      outDir: outputDirectory,
      chunkSizeWarningLimit: 1024 * 1024,
      minify: false,
    },
  })
}
