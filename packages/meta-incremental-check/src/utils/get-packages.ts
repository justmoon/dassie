import { access, readdir } from "node:fs/promises"
import path from "node:path"

import { WORKSPACE_ROOT_PATH } from "../constants/workspace-path"

export async function* getPackages() {
  const packagesPath = path.resolve(WORKSPACE_ROOT_PATH, "packages")
  const files = await readdir(packagesPath, { withFileTypes: true })
  for (const file of files) {
    if (!file.isDirectory()) continue
    const packageJsonPath = path.resolve(
      packagesPath,
      file.name,
      "package.json",
    )
    if (
      await access(packageJsonPath).then(
        () => false,
        () => true,
      )
    )
      continue

    yield file.name
  }
}
