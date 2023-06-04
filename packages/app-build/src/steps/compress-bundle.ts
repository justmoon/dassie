import { basename, resolve } from "node:path"

import { PATH_DIST, PATH_DIST_BUNDLE } from "../constants/paths"
import { run } from "../utils/run"

export const compressBundle = async () => {
  const outputArchiveFile = resolve(PATH_DIST, "bundle.tar.xz")
  await run`tar -cJf ${outputArchiveFile} -C ${resolve(
    PATH_DIST_BUNDLE,
    ".."
  )} ${basename(PATH_DIST_BUNDLE)}`
}
