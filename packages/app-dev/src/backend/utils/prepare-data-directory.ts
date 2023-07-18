import { mkdir } from "node:fs/promises"

import { checkFileStatus } from "./check-file-status"

export const prepareDataDirectory = async (dataPath: string) => {
  const dataPathStatus = await checkFileStatus(dataPath)

  if (dataPathStatus === "missing") {
    await mkdir(dataPath, { recursive: true })
  }
}
