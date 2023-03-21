import { $ } from "zx"

import { createActor } from "@dassie/lib-reactive"

import { checkFileStatus } from "../utils/check-file-status"

export const prepareDataDirectory = () =>
  createActor(async (_sig, dataPath: string) => {
    const dataPathStatus = await checkFileStatus(dataPath)

    if (dataPathStatus === "missing") {
      await $`mkdir -p ${dataPath}`
    }
  })
