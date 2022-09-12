import { $ } from "zx"

import type { EffectContext } from "@dassie/lib-reactive"

import { checkFileStatus } from "../utils/check-file-status"

export const prepareDataDirectory = async (
  _sig: EffectContext,
  dataPath: string
) => {
  const dataPathStatus = await checkFileStatus(dataPath)

  if (dataPathStatus === "missing") {
    await $`mkdir -p ${dataPath}`
  }
}
