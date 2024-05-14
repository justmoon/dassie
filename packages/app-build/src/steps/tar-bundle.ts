import { $ } from "execa"

import { mkdir } from "node:fs/promises"
import path from "node:path"

import { Architecture } from "../constants/architectures"
import { DassieVersion } from "../constants/version"
import { getTarPath } from "../utils/bundle-name"
import { getBundlePath } from "../utils/dynamic-paths"

export const tarBundle = async (
  version: DassieVersion,
  architecture: Architecture,
) => {
  const bundlePath = getBundlePath(version, architecture)
  const archivePath = getTarPath(version, architecture)

  await mkdir(path.dirname(archivePath), { recursive: true })

  await $`tar -cf ${archivePath} -C ${path.resolve(bundlePath, "..")} ${path.basename(
    bundlePath,
  )}`
}
