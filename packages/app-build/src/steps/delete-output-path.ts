import { rm } from "node:fs/promises"

import { createActor } from "@dassie/lib-reactive"

import { outputPathSignal } from "../computed/output-path"

export const deleteOutputPath = () =>
  createActor(async (sig) => {
    const outputPath = sig.get(outputPathSignal)
    await rm(outputPath, { recursive: true })
  })
