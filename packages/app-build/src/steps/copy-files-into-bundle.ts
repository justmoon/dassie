import { copyFile, cp, mkdir } from "node:fs/promises"
import { resolve } from "node:path"

import { Architecture } from "../constants/architectures"
import {
  PATH_DIST_STAGING_SHARED,
  PATH_RESOURCES_SYSTEMD_UNIT,
} from "../constants/paths"
import { getBundlePath, getStagingPath } from "../utils/dynamic-paths"

export const copyFilesIntoBundle = async (architecture: Architecture) => {
  const stagingPath = getStagingPath(architecture)
  const bundlePath = getBundlePath(architecture)

  {
    const pathNode = resolve(stagingPath, "node")
    const pathBundleBin = resolve(bundlePath, "bin")
    const pathBundleBinNode = resolve(pathBundleBin, "node")

    await mkdir(pathBundleBin, { recursive: true })

    await copyFile(resolve(pathNode, "bin/node"), pathBundleBinNode)
  }

  {
    const libraryPath = resolve(bundlePath, "lib")

    await mkdir(libraryPath, { recursive: true })

    const bindingSourcePath = resolve(
      stagingPath,
      "better-sqlite3/Release/better_sqlite3.node"
    )
    const bindingOutputPath = resolve(libraryPath, "better_sqlite3.node")

    await copyFile(bindingSourcePath, bindingOutputPath)
  }

  {
    const backendSourcePath = resolve(PATH_DIST_STAGING_SHARED, "backend.js")
    const backendOutputPath = resolve(bundlePath, "backend.js")

    await copyFile(backendSourcePath, backendOutputPath)
  }

  {
    const frontendSourcePath = resolve(PATH_DIST_STAGING_SHARED, "frontend")
    const frontendOutputPath = resolve(bundlePath, "share/public")

    await mkdir(frontendOutputPath, { recursive: true })

    await cp(frontendSourcePath, frontendOutputPath, {
      recursive: true,
      preserveTimestamps: true,
    })
  }

  {
    const pathShare = resolve(bundlePath, "share")

    await mkdir(pathShare, { recursive: true })

    const pathShareSystemd = resolve(pathShare, "dassie.service")
    await copyFile(PATH_RESOURCES_SYSTEMD_UNIT, pathShareSystemd)
  }
}
