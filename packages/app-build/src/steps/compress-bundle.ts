import { basename, resolve } from "node:path"

import { Architecture } from "../constants/architectures"
import { PATH_DIST } from "../constants/paths"
import { getBundleFilename } from "../utils/bundle-name"
import { getBundlePath } from "../utils/dynamic-paths"
import { run } from "../utils/run"

export const compressBundle = async (architecture: Architecture) => {
  const bundlePath = getBundlePath(architecture)

  const outputArchiveFile = resolve(
    PATH_DIST,
    `${getBundleFilename(architecture)}`
  )
  await run`tar -cJf ${outputArchiveFile} -C ${resolve(
    bundlePath,
    ".."
  )} ${basename(bundlePath)}`
}
