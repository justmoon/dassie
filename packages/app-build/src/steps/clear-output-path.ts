import { readdir, rm } from "node:fs/promises"
import path from "node:path"

import { PATH_DIST } from "../constants/paths"

export const clearOutputPath = async () => {
  const files = await readdir(PATH_DIST)
  const deletePromises = files.map((file) =>
    rm(path.join(PATH_DIST, file), { recursive: true, force: true }),
  )
  await Promise.all(deletePromises)
}
