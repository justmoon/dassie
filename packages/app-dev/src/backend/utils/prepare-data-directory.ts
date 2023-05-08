import { $ } from "zx"

import { checkFileStatus } from "./check-file-status"

export const prepareDataDirectory = async (dataPath: string) => {
  const dataPathStatus = await checkFileStatus(dataPath)

  if (dataPathStatus === "missing") {
    await $`mkdir -p ${dataPath}`
  }
}
