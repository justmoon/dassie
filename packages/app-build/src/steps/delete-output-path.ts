import { access, rm } from "node:fs/promises"

import { isErrorWithCode } from "@dassie/lib-type-utils"

import { PATH_DIST } from "../constants/paths"

export const deleteOutputPath = async () => {
  try {
    await access(PATH_DIST)

    await rm(PATH_DIST, { recursive: true })
  } catch (error: unknown) {
    if (isErrorWithCode(error, "ENOENT")) return

    throw error
  }
}
