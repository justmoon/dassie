import { mkdir } from "node:fs/promises"

import { createActor } from "@dassie/lib-reactive"

import { outputPathSignal } from "../computed/output-path"

export const createOutputPath = () =>
  createActor(async (sig) => {
    const outputPath = sig.get(outputPathSignal)
    await mkdir(outputPath, { recursive: true })
  })
