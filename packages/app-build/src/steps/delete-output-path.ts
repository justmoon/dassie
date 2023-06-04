import { rm } from "node:fs/promises"

import { PATH_DIST } from "../constants/paths"

export const deleteOutputPath = async () => {
  await rm(PATH_DIST, { recursive: true })
}
