import { copyFile, mkdir } from "node:fs/promises"
import { resolve } from "node:path"

import { PATH_DIST_BUNDLE, PATH_RESOURCES } from "../constants/paths"

export const addSystemdServiceToBundle = async () => {
  const sharePath = resolve(PATH_DIST_BUNDLE, "share")

  await mkdir(sharePath, { recursive: true })

  const systemdUnitSourcePath = resolve(
    PATH_RESOURCES,
    "systemd/dassie.service"
  )
  const systemdUnitDestinationPath = resolve(sharePath, "dassie.service")
  await copyFile(systemdUnitSourcePath, systemdUnitDestinationPath)
}
