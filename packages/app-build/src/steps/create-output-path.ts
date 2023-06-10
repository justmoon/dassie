import { mkdir } from "node:fs/promises"

import { PATH_DIST, PATH_DIST_CONTENTS } from "../constants/paths"

export const createOutputPath = async () => {
  await mkdir(PATH_DIST, { recursive: true })
  await mkdir(PATH_DIST_CONTENTS, { recursive: true })
}
