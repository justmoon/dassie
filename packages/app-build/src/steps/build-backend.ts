import { build } from "esbuild"

import { resolve } from "node:path"

import { createActor } from "@dassie/lib-reactive"

import { outputPathSignal } from "../computed/output-path"
import { sourcePathSignal } from "../computed/source-path"

export const buildBackend = () =>
  createActor(async (sig) => {
    const sourcePath = sig.get(sourcePathSignal)
    const outputPath = sig.get(outputPathSignal)

    await build({
      entryPoints: [resolve(sourcePath, "src/backend/index.ts")],
      outfile: resolve(outputPath, "backend.js"),
      bundle: true,
      platform: "node",
      format: "esm",
    })
  })
