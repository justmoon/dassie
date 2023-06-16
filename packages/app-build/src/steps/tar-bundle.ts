import { mkdir } from "node:fs/promises"
import { basename, dirname, resolve } from "node:path"

import { Architecture } from "../constants/architectures"
import { getTarPath } from "../utils/bundle-name"
import { getBundlePath } from "../utils/dynamic-paths"
import { run } from "../utils/run"

export const tarBundle = async (architecture: Architecture) => {
  const bundlePath = getBundlePath(architecture)
  const archivePath = getTarPath(architecture)

  await mkdir(dirname(archivePath), { recursive: true })

  await run`tar -cf ${archivePath} -C ${resolve(bundlePath, "..")} ${basename(
    bundlePath
  )}`
}
