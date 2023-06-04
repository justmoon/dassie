import { copyFile, mkdir } from "node:fs/promises"
import { createRequire } from "node:module"
import { dirname, resolve } from "node:path"

import { PATH_DIST_BUNDLE, PATH_PACKAGE_APP_NODE } from "../constants/paths"

export const copyNativeBindings = async () => {
  const require = createRequire(PATH_PACKAGE_APP_NODE)
  const sqlite3Path = require.resolve("better-sqlite3")
  const bindingSourcePath = resolve(
    dirname(sqlite3Path),
    "../build/Release/better_sqlite3.node"
  )

  const outputLibraryPath = resolve(PATH_DIST_BUNDLE, "lib")

  await mkdir(outputLibraryPath, { recursive: true })

  const bindingOutputPath = resolve(PATH_DIST_BUNDLE, "lib/better_sqlite3.node")

  await copyFile(bindingSourcePath, bindingOutputPath)
}
