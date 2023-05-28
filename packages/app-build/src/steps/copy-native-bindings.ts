import { copyFile, mkdir } from "node:fs/promises"
import { createRequire } from "node:module"
import { dirname, resolve } from "node:path"

import { createActor } from "@dassie/lib-reactive"

import { outputPathSignal } from "../computed/output-path"
import { sourcePathSignal } from "../computed/source-path"

export const copyNativeBindings = () =>
  createActor(async (sig) => {
    const sourcePath = sig.get(sourcePathSignal)
    const outputPath = sig.get(outputPathSignal)
    const require = createRequire(sourcePath)
    const sqlite3Path = require.resolve("better-sqlite3")
    const bindingSourcePath = resolve(
      dirname(sqlite3Path),
      "../build/Release/better_sqlite3.node"
    )

    const outputLibraryPath = resolve(outputPath, "lib")

    await mkdir(outputLibraryPath, { recursive: true })

    const bindingOutputPath = resolve(outputPath, "lib/better_sqlite3.node")

    await copyFile(bindingSourcePath, bindingOutputPath)
  })
