import { copyFile, mkdir } from "node:fs/promises"
import { resolve } from "node:path"

import { PATH_DIST_UPLOAD, PATH_RESOURCES_INSTALLER } from "../constants/paths"

export const copyInstallScript = async () => {
  const installScriptSource = PATH_RESOURCES_INSTALLER
  const installScriptTarget = resolve(PATH_DIST_UPLOAD, "install.sh")

  await mkdir(PATH_DIST_UPLOAD, { recursive: true })

  await copyFile(installScriptSource, installScriptTarget)
}
