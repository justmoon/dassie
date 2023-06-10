import { mkdir } from "node:fs/promises"
import { basename, resolve } from "node:path"

import { Architecture } from "../constants/architectures"
import { PATH_DIST_UPLOAD } from "../constants/paths"
import { DASSIE_VERSION } from "../constants/version"
import { getBundleFilename } from "../utils/bundle-name"
import { getBundlePath } from "../utils/dynamic-paths"
import { run } from "../utils/run"

export const compressBundle = async (architecture: Architecture) => {
  const bundlePath = getBundlePath(architecture)
  const archivePath = resolve(PATH_DIST_UPLOAD, DASSIE_VERSION)

  await mkdir(archivePath, { recursive: true })

  const outputArchiveFile = resolve(
    archivePath,
    `${getBundleFilename(architecture)}`
  )
  await run`tar -cJf ${outputArchiveFile} -C ${resolve(
    bundlePath,
    ".."
  )} ${basename(bundlePath)}`
}
