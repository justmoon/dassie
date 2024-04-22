import { readWantedLockfile } from "@pnpm/lockfile-file"
import { createUpdateOptions } from "@pnpm/meta-updater"

import { existsSync, readdirSync } from "node:fs"
import { join, relative, resolve } from "node:path"

export default createUpdateOptions(async (workspaceDir) => {
  const lockfile = await readWantedLockfile(workspaceDir, {
    ignoreIncompatible: false,
  })
  if (lockfile == null) {
    throw new Error("no lockfile found")
  }

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

      const relativePath = relative(workspaceDir, dir)

      const importer = lockfile.importers[relativePath]
      if (!importer) return tsConfig
      const deps = {
        ...importer.dependencies,
        ...importer.devDependencies,
      }

      const references = []

      for (const [depName, spec] of Object.entries(deps)) {
        if (!spec.startsWith("link:") || spec.length === 5) continue
        const relativePath = spec.slice(5)
        const linkedPkgDir = join(dir, relativePath)
        if (!existsSync(join(linkedPkgDir, "tsconfig.json"))) continue
        if (!resolve(linkedPkgDir).startsWith(resolve(workspaceDir))) continue
        references.push({ path: relativePath })
      }

      return {
        ...tsConfig,
        references: references.sort((r1, r2) => r1.path.localeCompare(r2.path)),
        compilerOptions: {
          ...tsConfig.compilerOptions,
          outDir: "dist",
        },
      }
    },
  }
})
