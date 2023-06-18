import { mkdir, writeFile } from "node:fs/promises"
import { resolve } from "node:path"

import { PATH_DIST_UPLOAD } from "../constants/paths"
import { DASSIE_VERSION } from "../constants/version"

export const generateMetadata = async () => {
  const metadataPath = resolve(PATH_DIST_UPLOAD, "meta")
  const latestFilePath = resolve(metadataPath, "latest")

  await mkdir(metadataPath, { recursive: true })
  await writeFile(latestFilePath, DASSIE_VERSION, "utf8")
}
