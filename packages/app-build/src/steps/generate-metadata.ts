import { mkdir, writeFile } from "node:fs/promises"
import path from "node:path"

import { PATH_DIST_UPLOAD } from "../constants/paths"
import type { DassieVersion } from "../constants/version"

export const generateMetadata = async (latestVersion: DassieVersion) => {
  const metadataPath = path.resolve(PATH_DIST_UPLOAD, "meta")
  const latestFilePath = path.resolve(metadataPath, "latest")

  await mkdir(metadataPath, { recursive: true })
  await writeFile(latestFilePath, latestVersion, "utf8")
}
