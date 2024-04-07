import { existsSync, readdirSync } from "node:fs"
import { join } from "node:path"

export default (workspaceDir) => {
  return {
    "package.json": (manifest, dir) => {
      return {
        ...manifest,
        author: "Stefan Thomas <justmoon@members.fsf.org>",
      }
    },
    "tsconfig.json": (tsConfig, { manifest, dir }) => {
      if (!tsConfig) return tsConfig

      if (manifest.name === "@dassie/root") {
        const packages = readdirSync(join(dir, "packages"))

        const packagesWithTsconfig = packages.filter((packageName) => {
          const tsConfigPath = join(
            dir,
            "packages",
            packageName,
            "tsconfig.json",
          )
          return existsSync(tsConfigPath)
        })

        return {
          ...tsConfig,
          references: packagesWithTsconfig.map((packageName) => ({
            path: `./packages/${packageName}/tsconfig.json`,
          })),
        }
      }

      return {
        ...tsConfig,
        compilerOptions: {
          ...tsConfig.compilerOptions,
          outDir: "dist",
        },
      }
    },
  }
}
