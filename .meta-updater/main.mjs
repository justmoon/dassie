import { readWantedLockfile } from "@pnpm/lockfile-file"
import { createUpdateOptions } from "@pnpm/meta-updater"

import { existsSync, readFileSync, readdirSync } from "node:fs"
import path from "node:path"

export default createUpdateOptions(async (workspaceDir) => {
  const lockfile = await readWantedLockfile(workspaceDir, {
    ignoreIncompatible: false,
  })
  if (lockfile == null) {
    throw new Error("no lockfile found")
  }

  const NODE_VERSION = readFileSync(
    path.join(workspaceDir, ".node-version"),
    "utf8",
  )
    .trim()
    .slice(1)

  return {
    "package.json": (manifest, { dir }) => {
      const relativePath = path.relative(workspaceDir, dir)

      for (const key of ["name", "version", "description"]) {
        if (!manifest[key]) {
          throw new Error(
            `Package ${relativePath} must have a "${key}" field in its package.json`,
          )
        }
      }
      if (!manifest.private && !manifest.files) {
        throw new Error(
          "Any package that isn't private must specify the `files` to be published",
        )
      }

      return {
        ...manifest,
        author: {
          name: "Stefan Thomas",
          email: "justmoon@members.fsf.org",
          url: "https://justmoon.com/",
        },
        bugs: {
          url: "https://github.com/justmoon/dassie/issues",
        },
        license: "Apache-2.0",
        repository: {
          type: "git",
          url: "git+https://github.com/justmoon/dassie.git",
          ...(relativePath ? { directory: relativePath } : {}),
        },
        engines: {
          node:
            manifest.name === "@dassie/app-website" ?
              `>=20 <=${NODE_VERSION}`
            : `=${NODE_VERSION}`,
        },
      }
    },
    "tsconfig.json": (tsConfig, { manifest, dir }) => {
      if (!tsConfig) return tsConfig

      if (manifest.name === "@dassie/root") {
        const packages = readdirSync(path.join(dir, "packages"))

        const packagesWithTsconfig = packages.filter((packageName) => {
          const tsConfigPath = path.join(
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

      const relativePath = path.relative(workspaceDir, dir)

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
        const linkedPkgDir = path.join(dir, relativePath)
        if (!existsSync(path.join(linkedPkgDir, "tsconfig.json"))) continue
        if (!path.resolve(linkedPkgDir).startsWith(path.resolve(workspaceDir)))
          continue
        references.push({ path: relativePath })
      }

      return {
        ...tsConfig,
        references: references.sort((r1, r2) => r1.path.localeCompare(r2.path)),
        compilerOptions: {
          ...tsConfig.compilerOptions,
          paths: {
            "@/*": ["./src/*"],
          },
          outDir: "dist",
        },
      }
    },
  }
})
