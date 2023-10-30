import { chmod, copyFile, cp, mkdir } from "node:fs/promises"
import { resolve } from "node:path"

import { Architecture } from "../constants/architectures"
import {
  PATH_DIST_STAGING_SHARED,
  PATH_RESOURCES_LAUNCHER,
  PATH_RESOURCES_SYSTEMD,
} from "../constants/paths"
import { DassieVersion } from "../constants/version"
import { getBundlePath, getStagingPath } from "../utils/dynamic-paths"

export const copyFilesIntoBundle = async (
  version: DassieVersion,
  architecture: Architecture,
) => {
  const stagingPath = getStagingPath(architecture)
  const bundlePath = getBundlePath(version, architecture)

  // Node binary
  {
    const pathNode = resolve(stagingPath, "node")
    const pathBundleBin = resolve(bundlePath, "bin")
    const pathBundleBinNode = resolve(pathBundleBin, "node")

    await mkdir(pathBundleBin, { recursive: true })

    await copyFile(resolve(pathNode, "bin/node"), pathBundleBinNode)
  }

  // Better SQLite3 bindings
  {
    const libraryPath = resolve(bundlePath, "lib")

    await mkdir(libraryPath, { recursive: true })

    const bindingSourcePath = resolve(
      stagingPath,
      "better-sqlite3/Release/better_sqlite3.node",
    )
    const bindingOutputPath = resolve(libraryPath, "better_sqlite3.node")

    await copyFile(bindingSourcePath, bindingOutputPath)
  }

  // Backend
  {
    const backendSourcePath = resolve(PATH_DIST_STAGING_SHARED, "backend.mjs")
    const backendOutputPath = resolve(bundlePath, "backend.mjs")

    await copyFile(backendSourcePath, backendOutputPath)
  }

  // Frontend
  {
    const frontendSourcePath = resolve(PATH_DIST_STAGING_SHARED, "frontend")
    const frontendOutputPath = resolve(bundlePath, "share/public")

    await mkdir(frontendOutputPath, { recursive: true })

    await cp(frontendSourcePath, frontendOutputPath, {
      recursive: true,
      preserveTimestamps: true,
    })
  }

  // Launcher
  {
    const launcherOutputPath = resolve(bundlePath, "bin/dassie")

    await cp(PATH_RESOURCES_LAUNCHER, launcherOutputPath)

    // Make the launcher executable
    await chmod(launcherOutputPath, 0o755)
  }

  // systemd units
  {
    const pathShare = resolve(bundlePath, "share")

    await mkdir(pathShare, { recursive: true })

    const pathShareSystemd = resolve(pathShare, "systemd")
    await cp(PATH_RESOURCES_SYSTEMD, pathShareSystemd, { recursive: true })
  }
}
