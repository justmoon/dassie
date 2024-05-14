import { build } from "vite"

import path from "node:path"

import {
  PATH_DIST_STAGING_SHARED,
  PATH_PACKAGE_APP_NODE,
} from "../constants/paths"

export const buildFrontend = async (detailedVersion: string) => {
  const sourceDirectory = path.resolve(PATH_PACKAGE_APP_NODE, "src/frontend")
  const outputDirectory = path.resolve(PATH_DIST_STAGING_SHARED, "frontend")

  // vite-node sets NODE_ENV to "development" by default so we need to override
  // it, otherwise react/jsx-runtime will use the development version.
  process.env["NODE_ENV"] = "production"

  console.info()
  await build({
    mode: "production",
    root: sourceDirectory,
    define: {
      __DASSIE_VERSION__: JSON.stringify(detailedVersion),
    },
    build: {
      outDir: outputDirectory,
      chunkSizeWarningLimit: 1024 * 1024,
    },
  })
}
