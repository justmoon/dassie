import { build } from "vite"

import { resolve } from "node:path"

import {
  PATH_DIST_STAGING_SHARED,
  PATH_PACKAGE_APP_NODE,
} from "../constants/paths"

export const buildFrontend = async () => {
  const sourceDirectory = resolve(PATH_PACKAGE_APP_NODE, "src/frontend")
  const outputDirectory = resolve(PATH_DIST_STAGING_SHARED, "frontend")

  await build({
    root: sourceDirectory,
    base: "/",
    build: {
      outDir: outputDirectory,
      chunkSizeWarningLimit: 1024 * 1024,
    },
  })
}
