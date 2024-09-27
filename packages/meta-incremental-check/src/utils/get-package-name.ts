import { readFileSync } from "node:fs"
import path from "node:path"

export function getPackageName(packagePath: string) {
  const packageJsonPath = path.resolve(packagePath, "package.json")
  const packageJson = JSON.parse(readFileSync(packageJsonPath, "utf8")) as {
    name: string
  }
  return packageJson.name
}
